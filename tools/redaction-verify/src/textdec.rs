//! Reading the text of a content stream the way a viewer does: the strings that `Tj TJ ' "` show
//! are turned into characters through the font that `Tf` selected (a `ToUnicode` map, the encoding
//! of a simple font, `/Differences`). This runs on every content stream of the file, drawn or not:
//! the streams of forms that no page draws, of hidden optional content, of annotations, of objects
//! that nothing refers to, and of old revisions.
//!
//! Where the resources of a stream are not known (an object nothing refers to), every font that
//! has the resource name is tried, and if there is none, every font of the file.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::content::{for_each_op, shown, Op, ShownItem, WORD_SPACE};
use crate::fonts::{self, Mapper, GAP};
use crate::pdfobj::{Doc, Entry, Obj};

/// A piece of the text of a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Show {
        font: String,
        bytes: Vec<u8>,
    },
    /// A word space, a new line or the end of a text object.
    Sep,
}

/// The strings that a content stream shows, with the font that was selected for each.
pub fn pieces(data: &[u8]) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut font = String::new();
    let mut stack: Vec<String> = Vec::new();
    for_each_op(data, &mut |op: &Op| match op.name {
        b"q" => stack.push(font.clone()),
        b"Q" => {
            if let Some(f) = stack.pop() {
                font = f;
            }
        }
        b"Tf" => {
            if let Some(n) = op.name_arg() {
                font = n.iter().map(|b| char::from(*b)).collect();
            }
        }
        b"BT" | b"ET" | b"Td" | b"TD" | b"T*" | b"Tm" => out.push(Piece::Sep),
        _ => {
            if let Some(items) = shown(op) {
                out.push(Piece::Sep);
                for it in items {
                    match it {
                        ShownItem::Str(bytes) => out.push(Piece::Show {
                            font: font.clone(),
                            bytes,
                        }),
                        ShownItem::Kern(k) if k <= WORD_SPACE => out.push(Piece::Sep),
                        ShownItem::Kern(_) => {}
                    }
                }
            }
        }
    });
    out
}

/// Which fonts a content stream may use, found in the objects of one buffer.
pub struct Resolver {
    /// Resource name -> font values (references or dictionaries), from every `/Font` resource
    /// dictionary of the buffer.
    by_name: HashMap<String, Vec<Obj>>,
    /// Stream object number -> the `/Font` dictionaries of the resources that reach it (pages that
    /// draw it, resources whose `/XObject` entry names it).
    owner_fonts: HashMap<u32, Vec<Obj>>,
    all_fonts: Vec<u32>,
    cache: RefCell<HashMap<u32, Rc<Vec<Mapper>>>>,
}

fn is_font_dict(d: &Obj) -> bool {
    d.get("Type").and_then(Obj::name) == Some("Font")
        || matches!(
            d.get("Subtype").and_then(Obj::name),
            Some("Type0" | "Type1" | "TrueType" | "Type3" | "MMType1")
        ) && d.get("BaseFont").is_some()
}

impl Resolver {
    pub fn new(doc: &Doc) -> Resolver {
        let mut r = Resolver {
            by_name: HashMap::new(),
            owner_fonts: HashMap::new(),
            all_fonts: Vec::new(),
            cache: RefCell::new(HashMap::new()),
        };
        for e in doc.entries() {
            if is_font_dict(&e.value) && !r.all_fonts.contains(&e.num) {
                r.all_fonts.push(e.num);
            }
            r.collect_names(doc, &e.value, 0);
            let res = e
                .value
                .get("Resources")
                .cloned()
                .or_else(|| inherited(doc, e));
            if let Some(res) = res {
                let rd = doc.resolve(&res);
                let fonts = doc.dget(rd, "Font").clone();
                if fonts.is_dict() {
                    let mut targets: Vec<u32> = Vec::new();
                    match e.value.get("Contents") {
                        Some(Obj::Ref(n)) => targets.push(*n),
                        Some(Obj::Array(a)) => targets.extend(a.iter().filter_map(|x| match x {
                            Obj::Ref(n) => Some(*n),
                            _ => None,
                        })),
                        _ => {}
                    }
                    for (_, v) in doc.dget(rd, "XObject").entries() {
                        if let Obj::Ref(n) = v {
                            targets.push(*n);
                        }
                    }
                    for t in targets {
                        r.owner_fonts.entry(t).or_default().push(fonts.clone());
                    }
                }
            }
        }
        r
    }

    fn collect_names(&mut self, doc: &Doc, v: &Obj, depth: usize) {
        if depth > 6 {
            return;
        }
        match v {
            Obj::Dict(d) => {
                for (k, val) in d {
                    if k == "Font" {
                        let fd = doc.resolve(val);
                        for (name, fv) in fd.entries() {
                            self.by_name
                                .entry(name.clone())
                                .or_default()
                                .push(fv.clone());
                        }
                    }
                    self.collect_names(doc, val, depth + 1);
                }
            }
            Obj::Array(a) => {
                for x in a {
                    self.collect_names(doc, x, depth + 1);
                }
            }
            _ => {}
        }
    }

    fn maps_of(&self, doc: &Doc, font: &Obj) -> Vec<Mapper> {
        match font {
            Obj::Ref(n) => {
                if let Some(c) = self.cache.borrow().get(n) {
                    return c.as_ref().clone();
                }
                let mut maps = Vec::new();
                for e in doc.all(*n) {
                    if e.value.is_dict() {
                        maps.extend(fonts::mappers(doc, &e.value));
                    }
                }
                let rc = Rc::new(maps);
                self.cache.borrow_mut().insert(*n, rc.clone());
                rc.as_ref().clone()
            }
            Obj::Dict(_) => fonts::mappers(doc, font),
            _ => Vec::new(),
        }
    }

    /// The ways to read the strings that a stream shows with the font of resource name `name`.
    pub fn mappers_for(&self, doc: &Doc, entry: Option<&Entry>, name: &str) -> Vec<Mapper> {
        let mut dicts: Vec<Obj> = Vec::new();
        if let Some(e) = entry {
            if let Some(res) = e.value.get("Resources") {
                dicts.push(doc.dget(doc.resolve(res), "Font").clone());
            }
            if let Some(o) = self.owner_fonts.get(&e.num) {
                dicts.extend(o.iter().cloned());
            }
        }
        let mut cands: Vec<Obj> = Vec::new();
        for d in &dicts {
            if let Some(v) = doc.resolve(d).get(name) {
                cands.push(v.clone());
            }
        }
        if cands.is_empty() {
            cands = self.by_name.get(name).cloned().unwrap_or_default();
        }
        let mut maps: Vec<Mapper> = cands.iter().flat_map(|c| self.maps_of(doc, c)).collect();
        if maps.is_empty() {
            maps = self
                .all_fonts
                .iter()
                .flat_map(|n| self.maps_of(doc, &Obj::Ref(*n)))
                .collect();
        }
        if maps.is_empty() {
            maps = vec![
                Mapper::Table(Rc::new(fonts::table(fonts::Base::Standard))),
                Mapper::Table(Rc::new(fonts::table(fonts::Base::WinAnsi))),
                Mapper::Table(Rc::new(fonts::table(fonts::Base::MacRoman))),
            ];
        }
        maps
    }
}

/// Resources of a page that has none of its own: those of the nearest parent that has some.
fn inherited(doc: &Doc, e: &Entry) -> Option<Obj> {
    let mut cur = e.value.get("Parent")?;
    for _ in 0..40 {
        let p = doc.resolve(cur);
        if let Some(r) = p.get("Resources") {
            return Some(r.clone());
        }
        cur = p.get("Parent")?;
    }
    None
}

/// The most ways to read one stream that are tried (every combination of the candidate fonts of
/// the names it uses). More than that is "could not check".
const MAX_COMBINATIONS: usize = 256;

/// The most characters that all the ways to read one stream may have together.
const MAX_CHARS: usize = 8_000_000;

/// The text of a stream in each way the fonts allow.
pub struct Decoded {
    /// One text per way to read the fonts (separators as spaces, unknown characters as GAP).
    pub variants: Vec<Vec<char>>,
    /// Why some text could not be read.
    pub problems: Vec<String>,
    /// Some way to read the fonts gives a text with no unknown character. Where there is none,
    /// a phrase may hide behind the characters that a font cannot map.
    pub has_complete_reading: bool,
}

/// Reads the pieces of a stream.
pub fn decode(doc: &Doc, res: &Resolver, entry: Option<&Entry>, pieces: &[Piece]) -> Decoded {
    let mut maps: BTreeMap<&str, Vec<Mapper>> = BTreeMap::new();
    for p in pieces {
        if let Piece::Show { font, .. } = p {
            maps.entry(font.as_str())
                .or_insert_with(|| res.mappers_for(doc, entry, font));
        }
    }
    let mut problems = Vec::new();
    let mut usable: Vec<(&str, Vec<&Mapper>)> = Vec::new();
    for (name, ms) in &maps {
        for m in ms {
            if let Some(why) = m.unknown() {
                let msg = format!("font /{name}: {why}");
                if !problems.contains(&msg) {
                    problems.push(msg);
                }
            }
        }
        // Two candidates that read every string of this font alike are one candidate (the
        // Latin encodings agree on all the letters, so most fonts have one).
        let strings: Vec<&[u8]> = pieces
            .iter()
            .filter_map(|p| match p {
                Piece::Show { font, bytes } if font == name => Some(bytes.as_slice()),
                _ => None,
            })
            .collect();
        let mut seen: Vec<u64> = Vec::new();
        let mut distinct: Vec<&Mapper> = Vec::new();
        for m in ms.iter().filter(|m| m.unknown().is_none()) {
            let mut h = DefaultHasher::new();
            for s in &strings {
                m.decode(s).hash(&mut h);
            }
            if !seen.contains(&h.finish()) {
                seen.push(h.finish());
                distinct.push(m);
            }
        }
        usable.push((name, distinct));
    }
    // Every combination of the candidates of the fonts: with shared resource names in an object
    // that nothing refers to, any font of that name may be the one in use.
    let counts: Vec<usize> = usable.iter().map(|(_, m)| m.len().max(1)).collect();
    let combinations = counts
        .iter()
        .try_fold(1usize, |a, c| a.checked_mul(*c))
        .unwrap_or(usize::MAX);
    // Each way is a text of its own in memory.
    let size: usize = pieces
        .iter()
        .map(|p| match p {
            Piece::Show { bytes, .. } => bytes.len() + 1,
            Piece::Sep => 1,
        })
        .sum();
    let allowed = (MAX_CHARS / size.max(1)).clamp(1, MAX_COMBINATIONS);
    let n = if combinations > allowed {
        problems.push(format!(
            "its fonts can be read in {} ways, and only {allowed} were tried",
            if combinations == usize::MAX {
                "too many".to_owned()
            } else {
                combinations.to_string()
            }
        ));
        allowed
    } else {
        combinations
    };
    let mut variants = Vec::with_capacity(n);
    let mut has_complete_reading = false;
    for k in 0..n {
        // The candidate of each font in this combination (a number system with one digit per font).
        let mut rest = k;
        let mut pick: HashMap<&str, Option<&Mapper>> = HashMap::new();
        for ((name, ms), c) in usable.iter().zip(&counts) {
            pick.insert(*name, ms.get(rest % c).copied());
            rest /= c;
        }
        let mut text: Vec<char> = Vec::new();
        let mut gaps = 0usize;
        for p in pieces {
            match p {
                Piece::Sep => {
                    if text.last().is_some_and(|c| *c != ' ') {
                        text.push(' ');
                    }
                }
                Piece::Show { font, bytes } => {
                    let chars = match pick.get(font.as_str()).copied().flatten() {
                        Some(m) => m.decode(bytes),
                        None => vec![GAP; bytes.len()],
                    };
                    gaps += chars.iter().filter(|c| **c == GAP).count();
                    text.extend(chars);
                }
            }
        }
        has_complete_reading |= gaps == 0 && problems.is_empty();
        variants.push(text);
    }
    Decoded {
        variants,
        problems,
        has_complete_reading,
    }
}

/// All characters (not white space, not GAP) of a text, for the font check.
pub fn chars_of(text: &[char]) -> BTreeSet<char> {
    text.iter()
        .copied()
        .filter(|c| !c.is_whitespace() && *c != GAP)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_follow_the_font_and_keep_kerning_spaces() {
        let p = pieces(b"BT /F1 12 Tf [(Ola)-333(Nordmann)] TJ /F2 12 Tf (x) Tj ET");
        let shown: Vec<_> = p
            .iter()
            .map(|x| match x {
                Piece::Show { font, bytes } => format!("{font}:{}", String::from_utf8_lossy(bytes)),
                Piece::Sep => "_".into(),
            })
            .collect();
        assert_eq!(
            shown,
            ["_", "_", "F1:Ola", "_", "F1:Nordmann", "_", "F2:x", "_"]
        );
    }
}
