//! The layers that read bytes and tool output: raw, qdf, carve, decode, text, images and fonts.
//! The structure layer has its own file.

use std::cell::OnceCell;
use std::collections::{BTreeSet, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::Path;

use tempfile::NamedTempFile;

use crate::carve::{self, Carved, Visitor};
use crate::pdfobj::{Doc, Entry, Obj};
use crate::roles::{self, Kind};
use crate::search::{find_all, BufHit, NeedleSet};
use crate::textdec::{self, Resolver};
use crate::tools;
use crate::{Hit, Layer, NeedleKind, Options, Report, Result, Spec, Subject};

pub struct Ctx<'a> {
    pub path: &'a Path,
    pub data: &'a [u8],
    pub spec: &'a Spec,
    pub opts: &'a Options,
    pub set: &'a NeedleSet,
    /// The user password, in a file that qpdf reads.
    pub password: Option<NamedTempFile>,
    /// The copy of the file that qpdf normalised, or why there is none.
    pub qdf: OnceCell<std::result::Result<Vec<u8>, String>>,
    /// The revisions of an encrypted file, decrypted.
    pub revisions: OnceCell<Revisions>,
}

/// The revisions of an encrypted file, each decrypted by qpdf as a whole (strings outside streams
/// too). `copies` is `None` for a file that is not encrypted: the byte layers read it as it is.
pub struct Revisions {
    pub copies: Option<Vec<(String, Vec<u8>)>>,
    /// What stopped some revision from being read.
    pub problems: Vec<String>,
}

impl Ctx<'_> {
    pub fn on(&self, layer: Layer) -> bool {
        self.opts.layers.contains(&layer)
    }

    /// Runs qpdf with the password of the file, if there is one.
    pub fn qpdf(&self, args: &[&OsStr]) -> Result<tools::Output> {
        let mut all: Vec<OsString> = Vec::new();
        if let Some(f) = &self.password {
            let mut a = OsString::from("--password-file=");
            a.push(f.path());
            all.push(a);
        }
        all.extend(args.iter().map(|a| (*a).to_owned()));
        let refs: Vec<&OsStr> = all.iter().map(OsString::as_os_str).collect();
        tools::qpdf(&refs)
    }

    /// The arguments that give a Poppler tool the password.
    pub fn poppler_password(&self) -> Vec<OsString> {
        match &self.opts.password {
            Some(p) => vec![OsString::from("-upw"), OsString::from(p)],
            None => Vec::new(),
        }
    }

    /// `qpdf --qdf` of the file: object streams opened, streams uncompressed, encryption removed.
    pub fn qdf_copy(&self) -> &std::result::Result<Vec<u8>, String> {
        self.qdf.get_or_init(|| {
            let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
            let out = dir.path().join("normalised.pdf");
            let r = self
                .qpdf(&[
                    OsStr::new("--qdf"),
                    OsStr::new("--object-streams=disable"),
                    OsStr::new("--stream-data=uncompress"),
                    self.path.as_os_str(),
                    out.as_os_str(),
                ])
                .map_err(|e| e.0)?;
            // 0 is success and 3 is success with warnings.
            match std::fs::read(&out) {
                Ok(b) if r.status == 0 || r.status == 3 => Ok(b),
                _ => Err(format!(
                    "qpdf could not normalise the file (exit {}): {}",
                    r.status,
                    first_line(&r.stderr)
                )),
            }
        })
    }
}

pub fn push(rep: &mut Report, layer: Layer, h: &BufHit, location: String) {
    rep.hits.push(Hit {
        layer,
        subject: Subject::Needle(h.needle),
        offset: Some(h.offset),
        location,
        form: Some(h.form),
    });
}

pub fn run(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let has_text = !ctx.set.is_empty();
    let has_images = ctx
        .spec
        .needles
        .iter()
        .any(|n| n.kind == NeedleKind::ImageSha256)
        || !ctx.spec.regions.is_empty();
    let has_rules = !ctx.spec.forbidden_keys.is_empty() || ctx.spec.single_revision;
    // A layer that has nothing to look for did not run, and is not named as run.
    let applies = |l: Layer| match l {
        Layer::Raw | Layer::Qdf | Layer::Carve | Layer::Decode | Layer::Text | Layer::Fonts => {
            has_text
        }
        Layer::Structure => has_text || has_rules,
        Layer::Images => has_images,
    };
    rep.layers_run = Layer::ALL
        .into_iter()
        .filter(|l| ctx.on(*l) && applies(*l))
        .collect();
    if rep.layers_run.is_empty() {
        rep.problems.push(
            "nothing was checked: none of the selected layers has anything to look for in this spec"
                .into(),
        );
        return Ok(());
    }
    let ran = |l: Layer| rep.layers_run.contains(&l);
    let (do_raw, do_qdf, do_text, do_structure, do_images) = (
        ran(Layer::Raw),
        ran(Layer::Qdf),
        ran(Layer::Text),
        ran(Layer::Structure),
        ran(Layer::Images),
    );
    let do_pass =
        ran(Layer::Carve) || ran(Layer::Decode) || ran(Layer::Images) || ran(Layer::Fonts);
    if do_raw {
        raw(ctx, rep);
    }
    if do_qdf {
        qdf(ctx, rep);
    }
    if do_pass {
        carve_pass(ctx, rep);
    }
    if do_text {
        text(ctx, rep)?;
    }
    if do_images {
        pdfimages(ctx, rep)?;
        if !ctx.spec.regions.is_empty() {
            match ctx.qdf_copy() {
                Ok(bytes) => crate::regions::check(ctx, bytes, rep)?,
                Err(why) => rep.problems.push(format!(
                    "images: the regions were not checked, because {why}"
                )),
            }
        }
    }
    if do_structure {
        crate::structure::run(ctx, rep)?;
    }
    rep.hits
        .sort_by_key(|h| (h.layer, h.subject_key(), h.offset));
    Ok(())
}

impl Hit {
    fn subject_key(&self) -> String {
        match &self.subject {
            Subject::Needle(i) => format!("n{i:06}"),
            Subject::Rule(r) => format!("r{r}"),
        }
    }
}

/// The file as written. Streams that have a filter are left to the carve layer: they are
/// compressed, and a scan for strings in them would only find noise.
fn raw(ctx: &Ctx, rep: &mut Report) {
    let masks: Vec<(usize, usize)> = carve::find_streams(ctx.data)
        .iter()
        .filter(|s| s.has_filter())
        .map(|s| (s.body.start, s.body.end))
        .collect();
    for h in ctx.set.search(ctx.data, &masks) {
        push(rep, Layer::Raw, &h, "file bytes".into());
    }
    for h in ctx.set.search_split(ctx.data) {
        push(rep, Layer::Raw, &h, "file bytes, text object".into());
    }
}

/// A copy normalised by qpdf: no object streams, every stream decompressed. qpdf also decrypts
/// a file whose user password is empty (or given), which the other byte layers cannot.
fn qdf(ctx: &Ctx, rep: &mut Report) {
    let bytes = match ctx.qdf_copy() {
        Ok(b) => b,
        Err(why) => {
            rep.problems.push(format!("qdf: {why}"));
            return;
        }
    };
    for h in ctx.set.search(bytes, &[]) {
        push(rep, Layer::Qdf, &h, "qpdf-normalised copy".into());
    }
    for h in ctx.set.search_split(bytes) {
        push(
            rep,
            Layer::Qdf,
            &h,
            "qpdf-normalised copy, text object".into(),
        );
    }
    // An encrypted file keeps its old revisions encrypted, and the copy above is the last one.
    // Each revision is decrypted as a whole, so the strings outside streams (an old /Info, an old
    // annotation text, a field value) are searched too.
    if let Some(copies) = &revisions(ctx, rep).copies {
        for (label, bytes) in copies {
            for h in ctx.set.search(bytes, &[]) {
                push(rep, Layer::Qdf, &h, format!("{label}qpdf-normalised"));
            }
            for h in ctx.set.search_split(bytes) {
                push(
                    rep,
                    Layer::Qdf,
                    &h,
                    format!("{label}qpdf-normalised, text object"),
                );
            }
        }
    }
}

fn first_line(s: &str) -> &str {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or("")
}

// ---------------------------------------------------------------------------------------------
// Encrypted files. The bytes of an encrypted stream are noise, so the carve and decode layers read
// copies that qpdf decrypts: one for the file, and one for each earlier revision (the file cut
// after each `%%EOF`). Objects that nothing refers to stay in the copies.

/// The decrypted copies, made once. `copies` is `None` when the file is not encrypted.
fn revisions<'c>(ctx: &'c Ctx, rep: &mut Report) -> &'c Revisions {
    let r = ctx.revisions.get_or_init(|| {
        let mut r = Report::default();
        let copies = decrypted_copies(ctx, &mut r);
        Revisions {
            copies,
            problems: r.problems,
        }
    });
    for p in &r.problems {
        rep.problem(p.clone());
    }
    r
}

/// `None` when the file is not encrypted (it is carved as it is), else the decrypted copies.
fn decrypted_copies(ctx: &Ctx, rep: &mut Report) -> Option<Vec<(String, Vec<u8>)>> {
    let enc = match ctx.qpdf(&[OsStr::new("--show-encryption"), ctx.path.as_os_str()]) {
        Ok(o) => o,
        Err(e) => {
            rep.problems.push(format!("revisions: {e}"));
            return Some(Vec::new());
        }
    };
    let out = String::from_utf8_lossy(&enc.stdout).into_owned();
    if enc.status != 0 && enc.status != 3 {
        rep.problems.push(format!(
            "revisions: qpdf could not open the file to see whether it is encrypted (exit {}): {}",
            enc.status,
            first_line(&enc.stderr)
        ));
        return Some(Vec::new());
    }
    if out.contains("File is not encrypted") {
        return None;
    }
    let dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => {
            rep.problems.push(format!("revisions: {e}"));
            return Some(Vec::new());
        }
    };
    let mut ends: Vec<usize> = find_all(ctx.data, b"%%EOF")
        .into_iter()
        .map(|p| p + 5)
        .collect();
    match ends.last_mut() {
        // Only white space after the last %%EOF: the file is that revision.
        Some(last)
            if ctx.data[*last..]
                .iter()
                .all(|b| crate::strings::is_space(*b)) =>
        {
            *last = ctx.data.len();
        }
        _ => ends.push(ctx.data.len()),
    }
    ends.dedup();
    let mut copies = Vec::new();
    let total = ends.len();
    for (i, end) in ends.iter().enumerate() {
        let (label, what) = if i + 1 == total {
            ("decrypted copy: ".to_owned(), "last revision".to_owned())
        } else {
            (
                format!("decrypted copy of revision {}: ", i + 1),
                format!("revision {}", i + 1),
            )
        };
        let input = dir.path().join(format!("rev{i}.pdf"));
        let output = dir.path().join(format!("rev{i}-out.pdf"));
        if std::fs::write(&input, &ctx.data[..*end]).is_err() {
            rep.problems
                .push("revisions: cannot write a temporary file".into());
            continue;
        }
        let r = ctx.qpdf(&[
            OsStr::new("--qdf"),
            OsStr::new("--object-streams=disable"),
            OsStr::new("--preserve-unreferenced"),
            input.as_os_str(),
            output.as_os_str(),
        ]);
        match r {
            Ok(r) if (r.status == 0 || r.status == 3) && output.exists() => {
                if let Ok(b) = std::fs::read(&output) {
                    copies.push((label, b));
                }
            }
            Ok(r) => rep.problems.push(format!(
                "revisions: qpdf could not decrypt the file for the {} (exit {}): {}. It was not checked.",
                what,
                r.status,
                first_line(&r.stderr)
            )),
            Err(e) => rep.problems.push(format!("revisions: {e}")),
        }
    }
    // A stream that qpdf does not reach cannot be decrypted without its object number. An object
    // stream is reached when some object of the cross-reference table is stored in it.
    let mut reached: BTreeSet<u32> = BTreeSet::new();
    for (_, b) in &copies {
        for p in find_all(b, b"%% Original object ID: ") {
            let rest = &b[p + 23..];
            let end = rest
                .iter()
                .position(|c| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            if let Some(n) = std::str::from_utf8(&rest[..end])
                .ok()
                .and_then(|s| s.parse().ok())
            {
                reached.insert(n);
            }
        }
    }
    let streams = carve::find_streams(ctx.data);
    let is_objstm =
        |s: &carve::Stream| carve::name_value(&s.dict, "/Type").is_some_and(|t| t == "ObjStm");
    let mut objstm_reached: BTreeSet<u32> = BTreeSet::new();
    let mut xref_listed = true;
    if streams.iter().any(is_objstm) {
        for i in 0..ends.len() {
            let input = dir.path().join(format!("rev{i}.pdf"));
            match ctx.qpdf(&[OsStr::new("--show-xref"), input.as_os_str()]) {
                Ok(r) if r.status == 0 || r.status == 3 => {
                    for line in String::from_utf8_lossy(&r.stdout).lines() {
                        if let Some((_, rest)) = line.split_once("; stream = ") {
                            let digits: String =
                                rest.chars().take_while(char::is_ascii_digit).collect();
                            if let Ok(n) = digits.parse() {
                                objstm_reached.insert(n);
                            }
                        }
                    }
                }
                _ => xref_listed = false,
            }
        }
        if !xref_listed {
            rep.problems.push(
                "revisions: qpdf could not list which object streams the cross-reference tables use, so the object streams of this encrypted file were not checked"
                    .into(),
            );
        }
    }
    let lost: Vec<u32> = streams
        .iter()
        .filter(|s| {
            let ty = carve::name_value(&s.dict, "/Type");
            ty.as_deref() != Some("XRef")
        })
        .filter_map(|s| {
            let n = s.obj?.0;
            let gone = if is_objstm(s) {
                xref_listed && !objstm_reached.contains(&n)
            } else {
                !reached.contains(&n)
            };
            gone.then_some(n)
        })
        .collect();
    if !lost.is_empty() && !copies.is_empty() {
        rep.problems.push(format!(
            "revisions: the file is encrypted and {} stream object(s) (such as {}) are not reached through any cross-reference table, so they cannot be decrypted and were not checked",
            lost.len(),
            lost[0]
        ));
    }
    Some(copies)
}

// ---------------------------------------------------------------------------------------------
// The pass over the streams: carve, decode, images (hashes) and fonts.

fn carve_pass(ctx: &Ctx, rep: &mut Report) {
    let revs = revisions(ctx, rep);
    let image_needles: Vec<(usize, &str)> = ctx
        .spec
        .needles
        .iter()
        .enumerate()
        .filter(|(_, n)| n.kind == NeedleKind::ImageSha256)
        .map(|(i, n)| (i, n.value.as_str()))
        .collect();
    let mut pass = Pass {
        ctx,
        rep,
        image_needles,
        prefix: String::new(),
        resolvers: Vec::new(),
        drawn: Vec::new(),
        roles: Vec::new(),
    };
    match &revs.copies {
        None => carve::carve(ctx.data, &mut pass),
        Some(copies) => {
            for (label, bytes) in copies {
                pass.prefix.clone_from(label);
                carve::carve(bytes, &mut pass);
            }
        }
    }
}

struct Pass<'c, 'r> {
    ctx: &'c Ctx<'c>,
    rep: &'r mut Report,
    image_needles: Vec<(usize, &'c str)>,
    /// Said first in the place of a hit: which copy of the file the stream is in.
    prefix: String,
    resolvers: Vec<Resolver>,
    /// The characters that the streams of the current buffer draw.
    drawn: Vec<BTreeSet<char>>,
    /// The content streams of the current buffer, found from its structure.
    roles: Vec<HashSet<u32>>,
}

impl Pass<'_, '_> {
    fn place(&self, c: &Carved) -> String {
        format!("{}stream {}", self.prefix, c.describe())
    }

    fn decode_data(&mut self, doc: &Doc, entry: Option<&Entry>, data: &[u8], place: &str) {
        let pieces = textdec::pieces(data);
        if !pieces
            .iter()
            .any(|p| matches!(p, textdec::Piece::Show { .. }))
        {
            return;
        }
        let Some(res) = self.resolvers.last() else {
            return;
        };
        let dec = textdec::decode(doc, res, entry, &pieces);
        let on = self.ctx.on(Layer::Decode);
        if on {
            for p in &dec.problems {
                self.rep.problem(format!(
                    "decode: text in {place} was not checked, because of its font: {p}"
                ));
            }
        }
        let mut found: BTreeSet<usize> = BTreeSet::new();
        let mut maybe: BTreeSet<usize> = BTreeSet::new();
        for variant in &dec.variants {
            if let Some(d) = self.drawn.last_mut() {
                d.extend(textdec::chars_of(variant));
            }
            if on {
                let r = self.ctx.set.search_decoded_wild(variant);
                for h in r.hits {
                    found.insert(h.needle);
                    push(
                        self.rep,
                        Layer::Decode,
                        &h,
                        format!("{place}, read through its fonts"),
                    );
                }
                maybe.extend(r.possible);
            }
        }
        // A code that a font cannot map may hide a phrase. That is "could not check", unless some
        // way to read the fonts leaves no code unmapped.
        if on && !dec.has_complete_reading {
            for i in maybe.difference(&found) {
                self.rep.problem(format!(
                    "decode: text in {place} may hold {} behind codes that its fonts cannot map, so it was not checked for it",
                    self.ctx.spec.label(*i)
                ));
            }
        }
    }
}

impl Visitor for Pass<'_, '_> {
    fn begin(&mut self, doc: &Doc, _within: &str) {
        self.resolvers.push(Resolver::new(doc));
        self.drawn.push(BTreeSet::new());
        self.roles.push(roles::content_streams(doc));
    }

    fn stream(&mut self, c: &Carved) {
        let ctx = self.ctx;
        let place = self.place(c);
        if (ctx.on(Layer::Carve) || ctx.on(Layer::Decode)) && !c.is_image {
            if let Some(p) = &c.decoded.problem {
                self.rep
                    .problem(format!("carve: {place} was not fully decoded: {p}"));
            }
        }
        if ctx.on(Layer::Carve) {
            for h in ctx.set.search(&c.decoded.data, &[]) {
                push(self.rep, Layer::Carve, &h, place.clone());
            }
            for h in ctx.set.search_split(&c.decoded.data) {
                push(self.rep, Layer::Carve, &h, format!("{place}, text object"));
            }
        }
        if ctx.on(Layer::Images) && c.is_image && !self.image_needles.is_empty() {
            let stored = carve::sha256_hex(c.raw);
            let decoded = carve::sha256_hex(&c.decoded.data);
            for (i, want) in &self.image_needles {
                let how = if *want == stored {
                    "as stored"
                } else if *want == decoded {
                    "with filters undone"
                } else {
                    continue;
                };
                self.rep.hits.push(Hit {
                    layer: Layer::Images,
                    subject: Subject::Needle(*i),
                    offset: Some(c.body_offset),
                    location: format!("image {}{} {how}", self.prefix, c.describe()),
                    form: None,
                });
            }
        }
        if (ctx.on(Layer::Decode) || ctx.on(Layer::Fonts))
            && !c.is_image
            && c.decoded.stopped_at.is_none()
        {
            let no_roles = HashSet::new();
            match roles::classify(c, self.roles.last().unwrap_or(&no_roles)) {
                Kind::Content => self.decode_data(c.doc, c.entry, &c.decoded.data, &place),
                Kind::Other => self.rep.note(format!(
                    "{place} is neither an image, a font nor a content stream: only the byte layers checked it"
                )),
                Kind::Unsure(why) => {
                    if ctx.on(Layer::Decode) {
                        self.rep.problem(format!(
                            "decode: {place} may be a content stream, but {why}, so its text was not checked"
                        ));
                    }
                }
                Kind::Image | Kind::Data => {}
            }
        }
    }

    fn end(&mut self, doc: &Doc, within: &str) {
        let ctx = self.ctx;
        if ctx.on(Layer::Decode) || ctx.on(Layer::Fonts) {
            // A page whose contents are several streams: a text object may run over them.
            let pages: Vec<(u32, Vec<u32>)> = doc
                .entries()
                .filter(|e| e.value.get("Type").and_then(Obj::name) == Some("Page"))
                .filter_map(|e| match doc.dget(&e.value, "Contents") {
                    Obj::Array(a) if a.len() > 1 => Some((
                        e.num,
                        a.iter()
                            .filter_map(|x| match x {
                                Obj::Ref(n) => Some(*n),
                                _ => None,
                            })
                            .collect(),
                    )),
                    _ => None,
                })
                .collect();
            for (num, parts) in pages {
                let mut data = Vec::new();
                for n in parts {
                    if let Some(d) = doc.latest(n).and_then(|e| doc.stream_data(e)) {
                        data.extend_from_slice(&d.data);
                        data.push(b'\n');
                    }
                }
                let entry = doc.latest(num);
                let place = format!(
                    "{}{}the content streams of page object {num}, joined",
                    self.prefix, within
                );
                self.decode_data(doc, entry, &data, &place);
            }
        }
        if ctx.on(Layer::Fonts) {
            self.fonts(doc, within);
        }
        self.resolvers.pop();
        self.drawn.pop();
        self.roles.pop();
    }
}

impl Pass<'_, '_> {
    /// Subset fonts that still map characters of the removed texts, when no remaining content
    /// draws those characters.
    fn fonts(&mut self, doc: &Doc, within: &str) {
        let drawn = self.drawn.last().cloned().unwrap_or_default();
        let texts: Vec<(usize, BTreeSet<char>)> = self
            .ctx
            .spec
            .needles
            .iter()
            .enumerate()
            .filter(|(_, n)| n.kind != NeedleKind::ImageSha256)
            .map(|(i, n)| (i, crate::fonts::char_set(&crate::search::norm(&n.value))))
            .collect();
        for e in doc.entries() {
            let sub = e.value.get("Subtype").and_then(Obj::name);
            if !matches!(sub, Some("Type0" | "Type1" | "TrueType" | "MMType1")) {
                continue;
            }
            if e.value.get("BaseFont").is_none() {
                continue;
            }
            let tr = crate::fonts::traces(doc, &e.value);
            if !tr.subset {
                continue;
            }
            for (i, chars) in &texts {
                let kept: Vec<char> = chars
                    .iter()
                    .copied()
                    .filter(|c| !drawn.contains(c) && tr.chars.contains_key(c))
                    .collect();
                if kept.is_empty() {
                    continue;
                }
                let glyphs = match &tr.cid_set {
                    Some(set) => {
                        let has = kept.iter().any(|c| {
                            tr.chars[c].iter().any(|code| {
                                set.get((*code / 8) as usize)
                                    .is_some_and(|b| b & (0x80 >> (code % 8)) != 0)
                            })
                        });
                        if has {
                            "the glyphs are in /CIDSet"
                        } else {
                            "the glyphs are not in /CIDSet"
                        }
                    }
                    None => "/CIDSet does not say whether the glyphs are in the font program",
                };
                self.rep.hits.push(Hit {
                    layer: Layer::Fonts,
                    subject: Subject::Needle(*i),
                    offset: None,
                    location: format!(
                        "{}{}subset font object {}: ToUnicode or /Differences still map {} of its {} distinct characters and no content draws them; {glyphs}",
                        self.prefix,
                        within,
                        e.num,
                        kept.len(),
                        chars.len()
                    ),
                    form: Some(crate::search::Form {
                        encoding: crate::search::Encoding::Utf8,
                        via: crate::search::Via::Font,
                    }),
                });
            }
        }
    }
}

/// What a reader can copy from the pages, in three reading orders.
fn text(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    for mode in ["", "-raw", "-layout"] {
        let mut args: Vec<OsString> = vec![OsString::from("-enc"), OsString::from("UTF-8")];
        args.extend(ctx.poppler_password());
        if !mode.is_empty() {
            args.push(OsString::from(mode));
        }
        args.push(ctx.path.as_os_str().to_owned());
        args.push(OsString::from("-"));
        let refs: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
        let out = tools::pdftotext(&refs)?;
        let label = if mode.is_empty() {
            "reading order"
        } else {
            mode
        };
        if out.status != 0 {
            rep.problems.push(format!(
                "text: pdftotext {label} failed (exit {}): {}",
                out.status,
                first_line(&out.stderr)
            ));
            continue;
        }
        // Poppler says what it could not read on stderr and still exits with 0.
        if let Some(l) = out.stderr.lines().find(|l| l.contains("Error")) {
            rep.problem(format!("text: pdftotext {label} reported: {}", l.trim()));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        for h in ctx.set.search_text(&text) {
            let page = text.as_bytes()[..h.offset.min(text.len())]
                .iter()
                .filter(|b| **b == 0x0c)
                .count()
                + 1;
            push(
                rep,
                Layer::Text,
                &h,
                format!("pdftotext {label}, page {page}"),
            );
        }
    }
    Ok(())
}

/// Pixels as Poppler draws them: `pdfimages` writes each image of the pages as PNM, so an image
/// that was stored in another colour space or with other filters still has the same hash.
fn pdfimages(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let wanted: Vec<(usize, &str)> = ctx
        .spec
        .needles
        .iter()
        .enumerate()
        .filter(|(_, n)| n.kind == NeedleKind::ImageSha256)
        .map(|(i, n)| (i, n.value.as_str()))
        .collect();
    if wanted.is_empty() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let prefix = dir.path().join("img");
    let mut args: Vec<OsString> = ctx.poppler_password();
    args.push(ctx.path.as_os_str().to_owned());
    args.push(prefix.as_os_str().to_owned());
    let refs: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
    let out = tools::pdfimages(&refs)?;
    if out.status != 0 {
        rep.problems.push(format!(
            "images: pdfimages failed (exit {}): {}",
            out.status,
            first_line(&out.stderr)
        ));
        return Ok(());
    }
    let mut names: Vec<_> = std::fs::read_dir(dir.path())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    names.sort();
    for path in names {
        let bytes = std::fs::read(&path)?;
        let Some(payload) = pnm_payload(&bytes) else {
            continue;
        };
        let hash = carve::sha256_hex(payload);
        for (i, want) in &wanted {
            if *want == hash {
                rep.hits.push(Hit {
                    layer: Layer::Images,
                    subject: Subject::Needle(*i),
                    offset: None,
                    location: format!(
                        "image drawn by pdfimages ({})",
                        path.file_name()
                            .map_or(String::new(), |n| n.to_string_lossy().into_owned())
                    ),
                    form: None,
                });
            }
        }
    }
    Ok(())
}

/// The pixel bytes of a binary PNM file (P4, P5, P6).
pub fn pnm_payload(b: &[u8]) -> Option<&[u8]> {
    let mut pos = 0usize;
    let token = |pos: &mut usize| -> Option<&[u8]> {
        loop {
            while *pos < b.len() && b[*pos].is_ascii_whitespace() {
                *pos += 1;
            }
            if b.get(*pos) == Some(&b'#') {
                while *pos < b.len() && b[*pos] != b'\n' {
                    *pos += 1;
                }
            } else {
                break;
            }
        }
        let s = *pos;
        while *pos < b.len() && !b[*pos].is_ascii_whitespace() {
            *pos += 1;
        }
        (*pos > s).then(|| &b[s..*pos])
    };
    let magic = token(&mut pos)?;
    let numbers = match magic {
        b"P4" => 2,
        b"P5" | b"P6" => 3,
        _ => return None,
    };
    for _ in 0..numbers {
        token(&mut pos)?;
    }
    // One white-space byte separates the header from the pixels.
    b.get(pos + 1..)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pnm_header_is_skipped() {
        assert_eq!(pnm_payload(b"P6\n2 1\n255\nabcdef"), Some(&b"abcdef"[..]));
        assert_eq!(pnm_payload(b"P5\n# comment\n1 1 255\nx"), Some(&b"x"[..]));
        assert_eq!(pnm_payload(b"P4\n8 1\n\xff"), Some(&b"\xff"[..]));
        assert_eq!(pnm_payload(b"junk"), None);
    }
}
