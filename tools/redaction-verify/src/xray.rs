//! The x-ray copy of a file: the same pages with nothing left that could hide drawn content, so
//! that a render of it shows every ink mark that sits under a redaction region.
//!
//! The copy is written from the objects of the normalised file (see `Ctx::plain_copy`), with
//! these changes:
//!
//! - Every content stream that a page, a form, an annotation appearance, a pattern or a Type 3
//!   glyph can draw is rewritten operator by operator (see [`rewrite_content`]):
//!   - the text render modes 3 and 7 (invisible) become 0, and the modes that add to the clip
//!     (4 to 6) become their modes without the clip (0 to 2);
//!   - the clip operators `W` and `W*` are dropped;
//!   - every colour operator (`g`, `rg`, `k`, `sc`, `scn`, `cs` and the stroke forms, in any colour
//!     space, with a pattern or not) becomes black, so that white on white and a colour that is
//!     the colour of the page cannot hide a mark;
//!   - marked content (`BDC`, `BMC`, `EMC`, `MP`, `DP`) is dropped;
//!   - inline images are dropped (images are looked at sample by sample, see `regions`);
//!   - a fill or a stroke that the walk found to carry nothing about a region (the box itself, a
//!     page background that holds the region, at most two rules that cross it) is taken away,
//!     so that what it covered shows;
//!   - a shading painted with `sh` is dropped unless it can reach a region, and then its colours
//!     are made black.
//! - Every graphics state: constant alpha 1, no soft mask, no blend mode, no transfer function.
//! - Every image object is replaced by an image mask that paints nothing.
//! - Optional content: the `/OCProperties` and every `/OC` entry go, so that every layer is on.
//! - Pages: no `/CropBox` and no `/UserUnit` (the render is of the media box), no `/Group`.
//! - Annotations: the Invisible, Hidden and NoView flags are cleared, the opacity is 1.
//!
//! Whatever the rewrite meets and cannot neutralise with certainty (an operator that is not in the
//! profile, an operand that is not clean, an inline image whose end is in doubt, a stream that
//! cannot be decoded, a shading of a type that cannot be made black) is a problem and the render
//! is not done: the check ends with "could not check".

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

use crate::content::{for_each_op, Op, Operand};
use crate::pdfobj::{Doc, Obj};
use crate::profile;
use crate::walk::{Marks, StreamId};

/// Keys that are dropped from every dictionary: optional content, the boxes that make a viewer
/// draw less than the media box, transparency groups.
const DROPPED_KEYS: [&str; 6] = [
    "OC",
    "OCProperties",
    "CropBox",
    "UserUnit",
    "Group",
    "SMaskInData",
];

/// Keys of a graphics state parameter dictionary that make a viewer draw less, or differently.
const GRAPHICS_STATE_DROPPED: [&str; 9] = [
    "SMask", "BM", "TR", "TR2", "OP", "op", "OPM", "TK", "AIS",
];

/// The annotation flags Invisible (1), Hidden (2), NoView (32) and ToggleNoView (256).
const HIDING_FLAGS: i64 = 1 | 2 | 32 | 256;

/// What the walk learned about the content of the pages, and the pages to write.
pub struct Plan<'a> {
    /// The decoded, joined content of each page that is rendered, by the object number of the
    /// page. Pages that are not here are written with no content.
    pub pages: &'a BTreeMap<u32, Vec<u8>>,
    pub marks: &'a Marks,
}

/// Why the x-ray copy cannot be made, in words.
pub type Problems = Vec<String>;

/// Writes the x-ray copy of the objects of `doc`.
pub fn build(doc: &Doc, plan: &Plan) -> Result<Vec<u8>, Problems> {
    let mut b = Builder::new(doc, plan);
    b.run();
    if b.problems.is_empty() {
        Ok(b.finish())
    } else {
        Err(b.problems)
    }
}

struct Builder<'a> {
    doc: &'a Doc<'a>,
    plan: &'a Plan<'a>,
    problems: Problems,
    out: Vec<u8>,
    offsets: BTreeMap<u32, usize>,
    /// Objects that are graphics state parameter dictionaries, and dictionaries of them.
    extg: HashSet<u32>,
    extg_maps: HashSet<u32>,
    /// The annotations of the pages, and the objects that are pages.
    annots: HashSet<u32>,
    pages: HashSet<u32>,
    /// The streams that are the contents of a page.
    page_streams: HashSet<u32>,
    content: HashSet<u32>,
    next: u32,
    root: u32,
    /// For each page that is rendered: the annotations that are added to it (see
    /// [`Builder::add_state_annotations`]), and the objects that they are.
    extra_annots: HashMap<u32, Vec<u32>>,
    extra_objects: Vec<(u32, Obj)>,
}

impl<'a> Builder<'a> {
    fn new(doc: &'a Doc<'a>, plan: &'a Plan<'a>) -> Builder<'a> {
        let root = doc
            .trailers()
            .iter()
            .rev()
            .find_map(|t| match t.get("Root") {
                Some(Obj::Ref(n)) => Some(*n),
                _ => None,
            })
            .unwrap_or(0);
        Builder {
            doc,
            plan,
            problems: Vec::new(),
            out: Vec::new(),
            offsets: BTreeMap::new(),
            extg: HashSet::new(),
            extg_maps: HashSet::new(),
            annots: HashSet::new(),
            pages: HashSet::new(),
            page_streams: HashSet::new(),
            content: crate::roles::content_streams(doc),
            next: doc.nums().max().unwrap_or(0) + 1,
            root,
            extra_annots: HashMap::new(),
            extra_objects: Vec::new(),
        }
    }

    fn problem(&mut self, msg: String) {
        if self.problems.len() < 20 && !self.problems.contains(&msg) {
            self.problems.push(msg);
        }
    }

    fn run(&mut self) {
        let doc = self.doc;
        if self.root == 0 || doc.latest(self.root).is_none() {
            self.problem("the file has no catalog, so no x-ray copy could be made".to_owned());
            return;
        }
        // What the pages and the graphics states are.
        for (page, _) in doc.pages() {
            self.pages.insert(page.num);
            match doc.dget(&page.value, "Annots") {
                Obj::Array(a) => {
                    for x in a {
                        if let Obj::Ref(n) = x {
                            self.annots.insert(*n);
                        }
                    }
                }
                _ => {}
            }
            collect_refs(&page.value, "Contents", doc, &mut self.page_streams);
        }
        for e in doc.entries() {
            self.find_graphics_states(&e.value, 0);
        }
        self.out.extend_from_slice(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n");
        self.add_state_annotations();
        // A page that is rendered gets a new content stream (numbered after all the others).
        let mut page_content: HashMap<u32, u32> = HashMap::new();
        for page in self.pages.clone() {
            if self.plan.pages.contains_key(&page) {
                page_content.insert(page, self.next);
                self.next += 1;
            }
        }
        let nums: Vec<u32> = doc.nums().collect();
        for n in nums {
            let Some(e) = doc.latest(n) else { continue };
            // A stream that only a page's contents list names is replaced below.
            if self.page_streams.contains(&n) && !self.content_elsewhere(n) {
                continue;
            }
            let Some(range) = e.stream.clone() else {
                let kind = if self.extg.contains(&n) {
                    Kind::GraphicsState
                } else if self.extg_maps.contains(&n) {
                    Kind::GraphicsStates
                } else {
                    Kind::Plain
                };
                let mut v = self.fix(&e.value, kind);
                self.fix_object(n, &mut v, &page_content);
                self.write_object(n, &v);
                continue;
            };
            let sub = doc.dget(&e.value, "Subtype").name().unwrap_or("");
            if sub == "Image" {
                self.write_blank_image(n);
                continue;
            }
            let mut dict = match self.fix(&e.value, Kind::Plain) {
                Obj::Dict(d) => d,
                _ => Vec::new(),
            };
            if self.content.contains(&n) && !self.page_streams.contains(&n) {
                let Some(d) = doc.stream_data(e) else { continue };
                if let Some(p) = &d.problem {
                    self.problem(format!("the content stream {n} cannot be decoded: {p}"));
                    continue;
                }
                if let Some(s) = &d.stopped_at {
                    self.problem(format!(
                        "the content stream {n} has the filter {s}, which is not decoded"
                    ));
                    continue;
                }
                match rewrite_content(&d.data, StreamId::Obj(n), self.plan.marks) {
                    Ok(data) => {
                        dict.retain(|(k, _)| !matches!(k.as_str(), "Filter" | "DecodeParms" | "DP"));
                        self.write_stream(n, dict, &data);
                    }
                    Err(why) => self.problem(format!("the content stream {n}: {why}")),
                }
                continue;
            }
            let raw = doc.buf[range].to_vec();
            self.write_stream(n, dict, &raw);
        }
        for (n, v) in std::mem::take(&mut self.extra_objects) {
            self.write_object(n, &v);
        }
        // The new content of the pages that are rendered.
        for (page, new) in page_content {
            let data = &self.plan.pages[&page];
            match rewrite_content(data, StreamId::Page(page), self.plan.marks) {
                Ok(out) => self.write_stream(new, Vec::new(), &out),
                Err(why) => self.problem(format!("the content of the page object {page}: {why}")),
            }
        }
    }

    /// A renderer draws one appearance of an annotation (the normal one, or the state that `/AS`
    /// names in it), and another reader draws another; the rest (the down and rollover
    /// appearances, the other states, the icons of a button) are drawn on a click or a hover. The
    /// x-ray copy draws them all: each of them is made the appearance of an annotation of its
    /// own (a stamp, at the rectangle of the annotation) that every renderer draws.
    fn add_state_annotations(&mut self) {
        let doc = self.doc;
        for (page, _) in doc.pages() {
            if !self.plan.pages.contains_key(&page.num) {
                continue;
            }
            let Obj::Array(annots) = doc.dget(&page.value, "Annots") else {
                continue;
            };
            let mut added = Vec::new();
            for a in annots {
                let ad = doc.resolve(a);
                let Some(rect) = ad
                    .get("Rect")
                    .and_then(|r| profile::exact_numbers(r, doc, 4))
                else {
                    continue;
                };
                let mut streams: Vec<u32> = Vec::new();
                if let Some(ap) = ad.get("AP").map(|x| doc.resolve(x)) {
                    for state in ["N", "R", "D"] {
                        if let Some(s) = ap.get(state) {
                            crate::regions::appearance_streams(doc, s, &mut streams, 0);
                        }
                    }
                }
                if let Some(mk) = ad.get("MK").map(|x| doc.resolve(x)) {
                    for icon in ["I", "RI", "IX"] {
                        if let Some(s) = mk.get(icon) {
                            crate::regions::appearance_streams(doc, s, &mut streams, 0);
                        }
                    }
                }
                for s in streams {
                    let n = self.next;
                    self.next += 1;
                    let dict = Obj::Dict(vec![
                        ("Type".to_owned(), Obj::Name("Annot".to_owned())),
                        ("Subtype".to_owned(), Obj::Name("Stamp".to_owned())),
                        (
                            "Rect".to_owned(),
                            Obj::Array(rect.iter().map(|v| Obj::Real(*v)).collect()),
                        ),
                        ("F".to_owned(), Obj::Int(4)),
                        (
                            "AP".to_owned(),
                            Obj::Dict(vec![("N".to_owned(), Obj::Ref(s))]),
                        ),
                    ]);
                    self.extra_objects.push((n, dict));
                    added.push(n);
                }
            }
            if !added.is_empty() {
                self.extra_annots.insert(page.num, added);
            }
        }
    }

    /// Whether a stream that is a page's contents is also something else that is drawn (a form
    /// that a page lists as its content as well).
    fn content_elsewhere(&self, n: u32) -> bool {
        self.doc
            .latest(n)
            .is_some_and(|e| e.value.get("Subtype").and_then(Obj::name) == Some("Form"))
    }

    /// Finds the graphics state dictionaries by the `/ExtGState` entry of resources.
    fn find_graphics_states(&mut self, v: &Obj, depth: usize) {
        if depth > 12 {
            return;
        }
        match v {
            Obj::Dict(d) => {
                for (k, x) in d {
                    if k == "ExtGState" {
                        match x {
                            Obj::Ref(n) => {
                                self.extg_maps.insert(*n);
                                if let Some(e) = self.doc.latest(*n) {
                                    for (_, c) in e.value.entries() {
                                        if let Obj::Ref(m) = c {
                                            self.extg.insert(*m);
                                        }
                                    }
                                }
                            }
                            Obj::Dict(children) => {
                                for (_, c) in children {
                                    if let Obj::Ref(m) = c {
                                        self.extg.insert(*m);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    self.find_graphics_states(x, depth + 1);
                }
            }
            Obj::Array(a) => a
                .iter()
                .for_each(|x| self.find_graphics_states(x, depth + 1)),
            _ => {}
        }
    }

    /// The value with the changes of the x-ray copy. `kind` says what the dictionary is, when the
    /// place it was found in tells.
    fn fix(&mut self, v: &Obj, kind: Kind) -> Obj {
        match v {
            Obj::Dict(d) => {
                let mut out: Vec<(String, Obj)> = Vec::with_capacity(d.len());
                let is_shading = matches!(
                    d.iter().find(|(k, _)| k == "ShadingType").map(|(_, v)| v),
                    Some(Obj::Int(2 | 3))
                );
                for (k, x) in d {
                    if DROPPED_KEYS.contains(&k.as_str()) {
                        continue;
                    }
                    if kind == Kind::GraphicsState
                        && (GRAPHICS_STATE_DROPPED.contains(&k.as_str())
                            || matches!(k.as_str(), "ca" | "CA"))
                    {
                        continue;
                    }
                    if is_shading && matches!(k.as_str(), "ColorSpace" | "Function" | "Background")
                    {
                        continue;
                    }
                    let child = match (kind, k.as_str()) {
                        (_, "ExtGState") => Kind::GraphicsStates,
                        (Kind::GraphicsStates, _) => Kind::GraphicsState,
                        _ => Kind::Plain,
                    };
                    out.push((k.clone(), self.fix(x, child)));
                }
                if kind == Kind::GraphicsState {
                    out.push(("ca".to_owned(), Obj::Int(1)));
                    out.push(("CA".to_owned(), Obj::Int(1)));
                }
                if is_shading {
                    out.push(("ColorSpace".to_owned(), Obj::Name("DeviceGray".to_owned())));
                    out.push((
                        "Function".to_owned(),
                        Obj::Dict(vec![
                            ("FunctionType".to_owned(), Obj::Int(2)),
                            (
                                "Domain".to_owned(),
                                Obj::Array(vec![Obj::Int(0), Obj::Int(1)]),
                            ),
                            ("C0".to_owned(), Obj::Array(vec![Obj::Int(0)])),
                            ("C1".to_owned(), Obj::Array(vec![Obj::Int(0)])),
                            ("N".to_owned(), Obj::Int(1)),
                        ]),
                    ));
                }
                Obj::Dict(out)
            }
            Obj::Array(a) => Obj::Array(a.iter().map(|x| self.fix(x, Kind::Plain)).collect()),
            other => other.clone(),
        }
    }

    /// Changes that depend on what the object is: a page, an annotation.
    fn fix_object(&mut self, n: u32, v: &mut Obj, page_content: &HashMap<u32, u32>) {
        let Obj::Dict(d) = v else { return };
        if self.pages.contains(&n) {
            d.retain(|(k, _)| k != "Contents");
            if let Some(new) = page_content.get(&n) {
                d.push(("Contents".to_owned(), Obj::Ref(*new)));
            }
            if let Some(more) = self.extra_annots.get(&n) {
                let mut all: Vec<Obj> = match self
                    .doc
                    .latest(n)
                    .map(|e| self.doc.dget(&e.value, "Annots"))
                {
                    Some(Obj::Array(a)) => a.clone(),
                    _ => Vec::new(),
                };
                all.extend(more.iter().map(|m| Obj::Ref(*m)));
                d.retain(|(k, _)| k != "Annots");
                d.push(("Annots".to_owned(), Obj::Array(all)));
            }
        }
        if self.annots.contains(&n) {
            let flags = d
                .iter()
                .find(|(k, _)| k == "F")
                .and_then(|(_, v)| v.int())
                .unwrap_or(0);
            d.retain(|(k, _)| k != "F" && k != "CA");
            d.push(("F".to_owned(), Obj::Int(flags & !HIDING_FLAGS)));
        }
    }

    /// An image that paints nothing: a stencil mask of one sample that is not painted.
    fn write_blank_image(&mut self, n: u32) {
        let dict = vec![
            ("Type".to_owned(), Obj::Name("XObject".to_owned())),
            ("Subtype".to_owned(), Obj::Name("Image".to_owned())),
            ("Width".to_owned(), Obj::Int(1)),
            ("Height".to_owned(), Obj::Int(1)),
            ("ImageMask".to_owned(), Obj::Bool(true)),
            ("BitsPerComponent".to_owned(), Obj::Int(1)),
        ];
        self.write_stream(n, dict, &[0xFF]);
    }

    fn start_object(&mut self, n: u32) {
        self.offsets.insert(n, self.out.len());
        let _ = write!(OutWriter(&mut self.out), "{n} 0 obj\n");
    }

    fn write_object(&mut self, n: u32, v: &Obj) {
        self.start_object(n);
        write_obj(&mut self.out, v);
        self.out.extend_from_slice(b"\nendobj\n");
    }

    fn write_stream(&mut self, n: u32, mut dict: Vec<(String, Obj)>, data: &[u8]) {
        dict.retain(|(k, _)| k != "Length");
        dict.push(("Length".to_owned(), Obj::Int(data.len() as i64)));
        self.start_object(n);
        write_obj(&mut self.out, &Obj::Dict(dict));
        self.out.extend_from_slice(b"\nstream\n");
        self.out.extend_from_slice(data);
        self.out.extend_from_slice(b"\nendstream\nendobj\n");
    }

    fn finish(mut self) -> Vec<u8> {
        let size = self.offsets.keys().max().copied().unwrap_or(0) + 1;
        let xref = self.out.len();
        let _ = write!(OutWriter(&mut self.out), "xref\n0 {size}\n");
        self.out.extend_from_slice(b"0000000000 65535 f \n");
        for n in 1..size {
            match self.offsets.get(&n) {
                Some(o) => {
                    let _ = write!(OutWriter(&mut self.out), "{o:010} 00000 n \n");
                }
                None => self.out.extend_from_slice(b"0000000000 65535 f \n"),
            }
        }
        let _ = write!(
            OutWriter(&mut self.out),
            "trailer\n<< /Size {size} /Root {} 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            self.root
        );
        self.out
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Plain,
    /// A dictionary of graphics state parameter dictionaries.
    GraphicsStates,
    /// A graphics state parameter dictionary.
    GraphicsState,
}

/// The references under `key` of a dictionary (a reference, an array of them, a reference to an
/// array).
fn collect_refs(dict: &Obj, key: &str, doc: &Doc, out: &mut HashSet<u32>) {
    fn add(o: &Obj, doc: &Doc, out: &mut HashSet<u32>, depth: usize) {
        match o {
            Obj::Ref(n) => {
                out.insert(*n);
                if depth < 2 {
                    if let Some(e) = doc.latest(*n) {
                        if e.stream.is_none() {
                            add(&e.value, doc, out, depth + 1);
                        }
                    }
                }
            }
            Obj::Array(a) => a.iter().for_each(|x| add(x, doc, out, depth)),
            _ => {}
        }
    }
    if let Some(v) = dict.get(key) {
        add(v, doc, out, 0);
    }
}

struct OutWriter<'a>(&'a mut Vec<u8>);

impl std::fmt::Write for OutWriter<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

/// A name as it is written: bytes that are not regular characters are `#xx`.
fn write_name(out: &mut Vec<u8>, name: &[u8]) {
    out.push(b'/');
    for &b in name {
        if b <= b' ' || b >= 0x7F || b == b'#' || crate::strings::is_delim(b) {
            let _ = write!(OutWriter(out), "#{b:02X}");
        } else {
            out.push(b);
        }
    }
}

fn latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

fn write_obj(out: &mut Vec<u8>, v: &Obj) {
    match v {
        Obj::Null => out.extend_from_slice(b"null"),
        Obj::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Obj::Int(i) => {
            let _ = write!(OutWriter(out), "{i}");
        }
        Obj::Real(r) => {
            let _ = write!(OutWriter(out), "{r}");
        }
        Obj::Name(n) => write_name(out, &latin1(n)),
        Obj::Str(s) => write_hex(out, s),
        Obj::Array(a) => {
            out.push(b'[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                write_obj(out, x);
            }
            out.push(b']');
        }
        Obj::Dict(d) => {
            out.extend_from_slice(b"<<");
            for (k, x) in d {
                write_name(out, &latin1(k));
                out.push(b' ');
                write_obj(out, x);
                out.push(b' ');
            }
            out.extend_from_slice(b">>");
        }
        Obj::Ref(n) => {
            let _ = write!(OutWriter(out), "{n} 0 R");
        }
    }
}

fn write_hex(out: &mut Vec<u8>, s: &[u8]) {
    out.push(b'<');
    for b in s {
        let _ = write!(OutWriter(out), "{b:02X}");
    }
    out.push(b'>');
}

fn write_operand(out: &mut Vec<u8>, a: &Operand) -> Result<(), String> {
    match a {
        Operand::Num(v) if v.is_finite() => {
            let _ = write!(OutWriter(out), "{v}");
        }
        Operand::Name(n) => write_name(out, n),
        Operand::Str(s) => write_hex(out, s),
        Operand::Array(items) => {
            out.push(b'[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                write_operand(out, x)?;
            }
            out.push(b']');
        }
        _ => return Err("an operand is not a number, a name, a string or an array".to_owned()),
    }
    Ok(())
}

/// The content of a stream as the x-ray copy has it (see the module).
pub fn rewrite_content(data: &[u8], id: StreamId, marks: &Marks) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::with_capacity(data.len() + data.len() / 8);
    let mut err: Option<String> = None;
    for_each_op(data, &mut |op: &Op| {
        if err.is_some() {
            return;
        }
        if let Err(why) = rewrite_op(op, id, marks, &mut out) {
            err = Some(format!(
                "the operator {} cannot be neutralised: {why}",
                String::from_utf8_lossy(op.name)
            ));
        }
    });
    match err {
        Some(e) => Err(e),
        None => Ok(out),
    }
}

fn emit(out: &mut Vec<u8>, args: &[Operand], name: &[u8]) -> Result<(), String> {
    for a in args {
        write_operand(out, a)?;
        out.push(b' ');
    }
    out.extend_from_slice(name);
    out.push(b'\n');
    Ok(())
}

fn rewrite_op(op: &Op, id: StreamId, marks: &Marks, out: &mut Vec<u8>) -> Result<(), String> {
    if !profile::is_known(op.name) {
        return Err("it is not one of the operators of the specification".to_owned());
    }
    if let Some(why) = profile::operand_problem(op) {
        return Err(why);
    }
    if op.bad {
        return Err("an operand is not a clean PDF token".to_owned());
    }
    if op.inline_unsure {
        return Err("an inline image has an end that is not certain".to_owned());
    }
    match op.name {
        // Marked content, compatibility sections, the inline images, the clips.
        b"BDC" | b"BMC" | b"EMC" | b"MP" | b"DP" | b"BX" | b"EX" | b"BI" | b"W" | b"W*" => Ok(()),
        // Every colour is black.
        b"g" | b"rg" | b"k" | b"sc" | b"scn" | b"cs" => {
            out.extend_from_slice(b"0 g\n");
            Ok(())
        }
        b"G" | b"RG" | b"K" | b"SC" | b"SCN" | b"CS" => {
            out.extend_from_slice(b"0 G\n");
            Ok(())
        }
        b"Tr" => {
            let mode = match op.args.first() {
                Some(Operand::Num(v)) => *v as i64,
                _ => return Err("the render mode is not a number".to_owned()),
            };
            let mode = match mode {
                3 | 4 | 7 => 0,
                5 => 1,
                6 => 2,
                m => m,
            };
            let _ = write!(OutWriter(out), "{mode} Tr\n");
            Ok(())
        }
        b"sh" => {
            if marks.sh.get(&(id, op.offset)).copied().unwrap_or(false) {
                emit(out, &op.args, op.name)
            } else {
                Ok(())
            }
        }
        // A fill or a stroke that carries nothing about the region is taken away.
        b"f" | b"F" | b"f*" | b"S" | b"s" if marks.is_free(id, op.offset) => {
            out.extend_from_slice(b"n\n");
            Ok(())
        }
        _ => emit(out, &op.args, op.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(c: &str) -> String {
        String::from_utf8(
            rewrite_content(c.as_bytes(), StreamId::Page(1), &Marks::default()).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn invisible_text_is_drawn_and_clipping_text_is_not_clipping() {
        let r = rewrite("BT 3 Tr (a) Tj 7 Tr (b) Tj 4 Tr 5 Tr 6 Tr 1 Tr ET");
        assert!(r.contains("0 Tr\n<61> Tj"), "{r}");
        assert!(r.contains("0 Tr\n<62> Tj"), "{r}");
        assert!(!r.contains("3 Tr") && !r.contains("7 Tr") && !r.contains("4 Tr"), "{r}");
        assert!(r.contains("1 Tr\n2 Tr\n1 Tr"), "{r}");
    }

    #[test]
    fn clips_marked_content_and_inline_images_go_and_colours_are_black() {
        let r = rewrite(
            "q 10 10 50 50 re W n /OC /MC0 BDC 1 1 1 rg 0.5 g /Cs1 cs 0.2 0.3 0.4 sc /P1 scn \
             0 0 0 1 K 10 10 20 20 re f EMC BI /W 1 /H 1 /BPC 8 /CS /G ID \x00 EI Q",
        );
        assert!(!r.contains(" W") && !r.contains("BDC") && !r.contains("EMC"), "{r}");
        assert!(!r.contains("rg") && !r.contains("sc") && !r.contains(" cs"), "{r}");
        assert!(!r.contains("BI"), "{r}");
        assert!(r.contains("0 g\n") && r.contains("0 G\n"), "{r}");
        assert!(r.contains("10 10 20 20 re"), "{r}");
    }

    #[test]
    fn what_the_rewrite_cannot_neutralise_is_refused() {
        for bad in ["1 2 3 zork", "1 2 cm", "/F1 /F2 Tf", "BI /W 1 /H 1 ID \x00"] {
            assert!(
                rewrite_content(bad.as_bytes(), StreamId::Page(1), &Marks::default()).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_free_fill_becomes_nothing_and_a_shading_goes_unless_it_reaches() {
        let mut m = Marks::default();
        let src = "0 g 72 400 50 50 re f /Sh1 sh";
        let r = String::from_utf8(
            rewrite_content(src.as_bytes(), StreamId::Page(1), &m).unwrap(),
        )
        .unwrap();
        assert!(r.contains(" f\n") && !r.contains("sh"), "{r}");
        let at_f = src.find(" f").unwrap() + 1;
        let at_sh = src.find("/Sh1").unwrap();
        m.paint.insert((StreamId::Page(1), at_f), (true, false));
        m.sh.insert((StreamId::Page(1), at_sh), true);
        let r = String::from_utf8(
            rewrite_content(src.as_bytes(), StreamId::Page(1), &m).unwrap(),
        )
        .unwrap();
        assert!(r.contains("\nn\n") && r.contains("sh"), "{r}");
    }
}
