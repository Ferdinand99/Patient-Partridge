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

use crate::content::{for_each_op, mul, shown, Matrix, Op, ShownItem, IDENTITY, WORD_SPACE};
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
    /// The text position moved (a new text object, `Td`, `TD`, `T*`, `Tm`, `'`, `"`, or a `TJ`
    /// offset that kerns back by more than the run before it is wide): where the next string
    /// starts on the page, after the matrix of the page, and the size of the text. Hundredths of a
    /// point. `dx` and `dy` are the direction in which the text runs on the page (a unit vector, in
    /// thousandths): `(1000, 0)` for text that is not turned.
    Pos {
        x: i64,
        y: i64,
        size: i64,
        dx: i64,
        dy: i64,
    },
}

/// The graphics state that the text position depends on.
#[derive(Clone)]
struct TextState {
    ctm: Matrix,
    font: String,
    size: f64,
    leading: f64,
    /// The horizontal scale (`Tz`), as a factor.
    scale: f64,
}

/// What a glyph is at least wide, and what it is on average, in em: the fonts are not known where
/// the text is cut into pieces, so where a run ends is an estimate.
const GLYPH_MIN: f64 = 0.25;
const GLYPH_MEAN: f64 = 0.5;

fn hundredths(v: f64) -> i64 {
    if v.is_finite() {
        (v * 100.0).round().clamp(-1e15, 1e15) as i64
    } else {
        0
    }
}

fn thousandths(v: f64) -> i64 {
    if v.is_finite() {
        (v * 1000.0).round().clamp(-1e6, 1e6) as i64
    } else {
        0
    }
}

/// The strings that a content stream shows, with the font that was selected for each, and where
/// each run of text starts on the page.
pub fn pieces(data: &[u8]) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut gs = TextState {
        ctm: IDENTITY,
        font: String::new(),
        size: 0.0,
        leading: 0.0,
        scale: 1.0,
    };
    let mut stack: Vec<TextState> = Vec::new();
    let (mut tm, mut tlm) = (IDENTITY, IDENTITY);
    // How far the position has moved along the line since the matrix was set (estimated, in text
    // space), and how wide the run since the last position is at least.
    let (mut adv, mut run) = (0.0f64, 0.0f64);
    let at = |tm: &Matrix, adv: f64, gs: &TextState| -> Piece {
        let m = mul(&mul(&[1.0, 0.0, 0.0, 1.0, adv, 0.0], tm), &gs.ctm);
        let scale = (m[0] * m[3] - m[1] * m[2]).abs().sqrt();
        let len = m[0].hypot(m[1]);
        let (dx, dy) = if len > 1e-9 {
            (m[0] / len, m[1] / len)
        } else {
            (1.0, 0.0)
        };
        Piece::Pos {
            x: hundredths(m[4]),
            y: hundredths(m[5]),
            size: hundredths(gs.size.abs() * scale),
            dx: thousandths(dx),
            dy: thousandths(dy),
        }
    };
    for_each_op(data, &mut |op: &Op| match op.name {
        b"q" => stack.push(gs.clone()),
        b"Q" => {
            if let Some(s) = stack.pop() {
                gs = s;
            }
        }
        b"cm" => {
            let n = op.nums();
            if n.len() == 6 {
                gs.ctm = mul(&[n[0], n[1], n[2], n[3], n[4], n[5]], &gs.ctm);
            }
        }
        b"Tf" => {
            if let Some(n) = op.name_arg() {
                gs.font = n.iter().map(|b| char::from(*b)).collect();
            }
            if let Some(s) = op.nums().last() {
                gs.size = *s;
            }
        }
        b"TL" => {
            if let Some(l) = op.nums().last() {
                gs.leading = *l;
            }
        }
        b"Tz" => {
            if let Some(z) = op.nums().last() {
                gs.scale = *z / 100.0;
            }
        }
        b"BT" => {
            tm = IDENTITY;
            tlm = IDENTITY;
            (adv, run) = (0.0, 0.0);
            out.push(Piece::Sep);
            out.push(at(&tm, adv, &gs));
        }
        b"ET" => out.push(Piece::Sep),
        b"Tm" => {
            let n = op.nums();
            if n.len() == 6 {
                tlm = [n[0], n[1], n[2], n[3], n[4], n[5]];
                tm = tlm;
            }
            (adv, run) = (0.0, 0.0);
            out.push(Piece::Sep);
            out.push(at(&tm, adv, &gs));
        }
        b"Td" | b"TD" | b"T*" => {
            let n = op.nums();
            let (tx, ty) = match op.name {
                b"T*" => (0.0, -gs.leading),
                _ if n.len() == 2 => (n[0], n[1]),
                _ => (0.0, 0.0),
            };
            if op.name == b"TD" {
                gs.leading = -ty;
            }
            tlm = mul(&[1.0, 0.0, 0.0, 1.0, tx, ty], &tlm);
            tm = tlm;
            (adv, run) = (0.0, 0.0);
            out.push(Piece::Sep);
            out.push(at(&tm, adv, &gs));
        }
        _ => {
            if let Some(items) = shown(op) {
                // `'` and `"` move to the next line first.
                if matches!(op.name, b"'" | b"\"") {
                    tlm = mul(&[1.0, 0.0, 0.0, 1.0, 0.0, -gs.leading], &tlm);
                    tm = tlm;
                    (adv, run) = (0.0, 0.0);
                    out.push(Piece::Sep);
                    out.push(at(&tm, adv, &gs));
                }
                out.push(Piece::Sep);
                let unit = gs.size.abs() * gs.scale;
                for it in items {
                    match it {
                        ShownItem::Str(bytes) => {
                            adv += bytes.len() as f64 * GLYPH_MEAN * gs.size * gs.scale;
                            run += bytes.len() as f64 * GLYPH_MIN * unit;
                            out.push(Piece::Show {
                                font: gs.font.clone(),
                                bytes,
                            });
                        }
                        ShownItem::Kern(k) => {
                            if k <= WORD_SPACE {
                                out.push(Piece::Sep);
                            }
                            // A positive offset moves the text position back. Back by more than
                            // the run so far is wide: what follows is drawn before it, to the
                            // left, and is a run of its own.
                            let back = k / 1000.0 * gs.size * gs.scale;
                            adv -= back;
                            if back * gs.size.signum() > run {
                                out.push(Piece::Sep);
                                out.push(at(&tm, adv, &gs));
                                run = 0.0;
                            } else {
                                run = (run - back * gs.size.signum()).max(0.0);
                            }
                        }
                    }
                }
            }
        }
    });
    out
}

/// A run of text that starts at one place on the page.
struct Group {
    x: i64,
    y: i64,
    size: i64,
    /// The direction of the text on the page (thousandths of a unit vector).
    dir: (i64, i64),
    text: Vec<char>,
}

fn push_space(text: &mut Vec<char>) {
    if text.last().is_some_and(|c| *c != ' ') {
        text.push(' ');
    }
}

/// The order of the runs of text as a reader sees them: by line from the top (runs whose starts are
/// less than a third of a line apart are on one line), and by position along the line. Runs that
/// are turned the same way have lines of their own, along their own direction, so that text that is
/// turned is read along its own lines. The groups of one direction come in the order of the first
/// run of each.
fn reading_order(groups: &[Group]) -> Vec<usize> {
    let mut buckets: Vec<((i64, i64), Vec<usize>)> = Vec::new();
    for (i, g) in groups.iter().enumerate() {
        // Directions within a tenth of a unit are the same.
        let key = (
            (g.dir.0 + 50).div_euclid(100),
            (g.dir.1 + 50).div_euclid(100),
        );
        match buckets.iter_mut().find(|b| b.0 == key) {
            Some(b) => b.1.push(i),
            None => buckets.push((key, vec![i])),
        }
    }
    let mut order = Vec::with_capacity(groups.len());
    for (_, members) in &buckets {
        let (dx, dy) = match groups[members[0]].dir {
            (0, 0) => (1000, 0),
            d => d,
        };
        // Along the line, and across it (from the top for text that is not turned), in the same
        // units as the positions times a thousand.
        let along = |g: &Group| g.x * dx + g.y * dy;
        let across = |g: &Group| g.x * dy - g.y * dx;
        let mut idx = members.clone();
        idx.sort_by(|a, b| {
            across(&groups[*a])
                .cmp(&across(&groups[*b]))
                .then(along(&groups[*a]).cmp(&along(&groups[*b])))
        });
        let mut lines: Vec<Vec<usize>> = Vec::new();
        for i in idx {
            match lines.last_mut() {
                Some(line) => {
                    let first = &groups[line[0]];
                    let tolerance = (first.size.max(groups[i].size) / 3).max(100) * 1000;
                    if (across(first) - across(&groups[i])).abs() <= tolerance {
                        line.push(i);
                    } else {
                        lines.push(vec![i]);
                    }
                }
                None => lines.push(vec![i]),
            }
        }
        for line in &mut lines {
            line.sort_by_key(|i| (along(&groups[*i]), *i));
        }
        order.extend(lines.concat());
    }
    order
}

/// The runs of text in reading order, joined with a space; `None` when that is the order of the
/// stream (the search of the stream order has read it already).
fn ordered_text(groups: &[Group]) -> Option<Vec<char>> {
    let live: Vec<&Group> = groups
        .iter()
        .filter(|g| g.text.iter().any(|c| *c != ' '))
        .collect();
    if live.len() < 2 {
        return None;
    }
    let owned: Vec<Group> = live
        .iter()
        .map(|g| Group {
            x: g.x,
            y: g.y,
            size: g.size,
            dir: g.dir,
            text: Vec::new(),
        })
        .collect();
    let order = reading_order(&owned);
    if order.iter().enumerate().all(|(i, o)| i == *o) {
        return None;
    }
    let mut text: Vec<char> = Vec::new();
    for i in order {
        push_space(&mut text);
        text.extend(live[i].text.iter().copied());
    }
    Some(text)
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

    /// The ways to read the strings that a stream shows with the font of resource name `name`, each
    /// with the identity of the font dictionary it reads. Readings with the same identity are
    /// readings of one font (the assumed encoding, read both ways).
    pub fn mappers_for(&self, doc: &Doc, entry: Option<&Entry>, name: &str) -> Vec<(u64, Mapper)> {
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
        let tagged = |objs: &[Obj]| -> Vec<(u64, Mapper)> {
            objs.iter()
                .flat_map(|c| {
                    let id = identity(c);
                    self.maps_of(doc, c).into_iter().map(move |m| (id, m))
                })
                .collect()
        };
        let mut maps = tagged(&cands);
        if maps.is_empty() {
            let all: Vec<Obj> = self.all_fonts.iter().map(|n| Obj::Ref(*n)).collect();
            maps = tagged(&all);
        }
        if maps.is_empty() {
            // No font at all: one unknown font, read three ways.
            maps = vec![
                Mapper::Table(Rc::new(fonts::table(fonts::Base::Standard))),
                Mapper::Table(Rc::new(fonts::table(fonts::Base::WinAnsi))),
                Mapper::Table(Rc::new(fonts::table(fonts::Base::MacRoman))),
            ]
            .into_iter()
            .map(|m| (0, m))
            .collect();
        }
        maps
    }
}

/// Which font dictionary a candidate is: the object number, or a hash of an inline dictionary.
fn identity(font: &Obj) -> u64 {
    match font {
        Obj::Ref(n) => u64::from(*n) + 1,
        other => {
            let mut h = DefaultHasher::new();
            format!("{other:?}").hash(&mut h);
            h.finish() | 1 << 63
        }
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

/// One way to read the fonts of a stream.
pub struct Variant {
    /// The text in the order of the stream (separators as spaces, unknown characters as GAP).
    pub text: Vec<char>,
    /// The same text with its runs sorted by position on the page (lines from the top, then from
    /// the left). `None` when that is the order of the stream.
    pub ordered: Option<Vec<char>>,
    /// Another reading of the same fonts leaves no code unmapped, so the gaps of this one are not
    /// in doubt. A reading that uses another font dictionary does not count: when the font in use
    /// is not known, a reading that could hold the phrase is the unsure case.
    pub cleared: bool,
}

/// The text of a stream in each way the fonts allow.
pub struct Decoded {
    pub variants: Vec<Variant>,
    /// Why some text could not be read.
    pub problems: Vec<String>,
}

/// Reads the pieces of a stream.
pub fn decode(doc: &Doc, res: &Resolver, entry: Option<&Entry>, pieces: &[Piece]) -> Decoded {
    let mut maps: BTreeMap<&str, Vec<(u64, Mapper)>> = BTreeMap::new();
    for p in pieces {
        if let Piece::Show { font, .. } = p {
            maps.entry(font.as_str())
                .or_insert_with(|| res.mappers_for(doc, entry, font));
        }
    }
    let mut problems = Vec::new();
    let mut usable: Vec<(&str, Vec<(u64, &Mapper)>)> = Vec::new();
    for (name, ms) in &maps {
        for (_, m) in ms {
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
        let mut distinct: Vec<(u64, &Mapper)> = Vec::new();
        for (id, m) in ms.iter().filter(|(_, m)| m.unknown().is_none()) {
            let mut h = DefaultHasher::new();
            for s in &strings {
                m.decode(s).hash(&mut h);
            }
            if !seen.contains(&h.finish()) {
                seen.push(h.finish());
                distinct.push((*id, m));
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
    // Each way is two texts in memory: in the order of the stream and in reading order.
    let size: usize = pieces
        .iter()
        .map(|p| match p {
            Piece::Show { bytes, .. } => bytes.len() + 1,
            Piece::Sep | Piece::Pos { .. } => 1,
        })
        .sum();
    let allowed = (MAX_CHARS / 2 / size.max(1)).clamp(1, MAX_COMBINATIONS);
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
    // (text, ordered text, font dictionaries read, gaps)
    type Reading = (Vec<char>, Option<Vec<char>>, Vec<u64>, usize);
    let mut readings: Vec<Reading> = Vec::with_capacity(n);
    for k in 0..n {
        // The candidate of each font in this combination (a number system with one digit per font).
        let mut rest = k;
        let mut pick: HashMap<&str, Option<&Mapper>> = HashMap::new();
        let mut which: Vec<u64> = Vec::new();
        for ((name, ms), c) in usable.iter().zip(&counts) {
            let chosen = ms.get(rest % c);
            pick.insert(*name, chosen.map(|(_, m)| *m));
            which.push(chosen.map_or(0, |(id, _)| *id));
            rest /= c;
        }
        let mut text: Vec<char> = Vec::new();
        let mut groups = vec![Group {
            x: 0,
            y: 0,
            size: 0,
            dir: (1000, 0),
            text: Vec::new(),
        }];
        let mut gaps = 0usize;
        for p in pieces {
            match p {
                Piece::Sep => {
                    push_space(&mut text);
                    if let Some(g) = groups.last_mut() {
                        push_space(&mut g.text);
                    }
                }
                Piece::Pos { x, y, size, dx, dy } => groups.push(Group {
                    x: *x,
                    y: *y,
                    size: *size,
                    dir: (*dx, *dy),
                    text: Vec::new(),
                }),
                Piece::Show { font, bytes } => {
                    let chars = match pick.get(font.as_str()).copied().flatten() {
                        Some(m) => m.decode(bytes),
                        None => vec![GAP; bytes.len()],
                    };
                    gaps += chars.iter().filter(|c| **c == GAP).count();
                    if let Some(g) = groups.last_mut() {
                        g.text.extend(chars.iter().copied());
                    }
                    text.extend(chars);
                }
            }
        }
        readings.push((text, ordered_text(&groups), which, gaps));
    }
    let complete = |gaps: usize| gaps == 0 && problems.is_empty();
    let complete_fonts: Vec<Vec<u64>> = readings
        .iter()
        .filter(|r| complete(r.3))
        .map(|r| r.2.clone())
        .collect();
    let cleared: Vec<bool> = readings
        .iter()
        .map(|r| complete_fonts.contains(&r.2))
        .collect();
    let variants = readings
        .into_iter()
        .zip(cleared)
        .map(|((text, ordered, _, _), cleared)| Variant {
            text,
            ordered,
            cleared,
        })
        .collect();
    Decoded { variants, problems }
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
            .filter_map(|x| match x {
                Piece::Show { font, bytes } => {
                    Some(format!("{font}:{}", String::from_utf8_lossy(bytes)))
                }
                Piece::Sep => Some("_".into()),
                Piece::Pos { .. } => None,
            })
            .collect();
        assert_eq!(
            shown,
            ["_", "_", "F1:Ola", "_", "F1:Nordmann", "_", "F2:x", "_"]
        );
    }

    #[test]
    fn the_text_position_follows_the_matrices() {
        let p = pieces(b"q 2 0 0 2 10 20 cm BT /F1 12 Tf 14 TL 5 6 Td (a) Tj T* (b) Tj ET Q");
        let at: Vec<(i64, i64, i64)> = p
            .iter()
            .filter_map(|x| match x {
                Piece::Pos { x, y, size, .. } => Some((*x, *y, *size)),
                _ => None,
            })
            .collect();
        // BT: the origin of the page matrix; Td: (5, 6) scaled by 2 and moved; T*: 14 lower, which
        // is 28 points on the page.
        assert_eq!(at, [(1000, 2000, 0), (2000, 3200, 2400), (2000, 400, 2400)]);
    }

    #[test]
    fn runs_are_read_in_position_order_when_that_differs_from_the_stream() {
        let groups = |runs: &[(i64, i64, &str)]| -> Vec<Group> {
            runs.iter()
                .map(|(x, y, t)| Group {
                    x: *x * 100,
                    y: *y * 100,
                    size: 1200,
                    dir: (1000, 0),
                    text: t.chars().collect(),
                })
                .collect()
        };
        let text = |g: &[Group]| ordered_text(g).map(|t| t.into_iter().collect::<String>());
        // The surname is drawn first, to the right of the first name.
        assert_eq!(
            text(&groups(&[(110, 600, "Nordmann"), (72, 600, "Ola")])),
            Some("Ola Nordmann".to_owned())
        );
        // A run a little above on the same line stays on the line.
        assert_eq!(
            text(&groups(&[(110, 602, "Nordmann"), (72, 600, "Ola")])),
            Some("Ola Nordmann".to_owned())
        );
        // Lines from the top.
        assert_eq!(
            text(&groups(&[(72, 500, "second"), (72, 600, "first")])),
            Some("first second".to_owned())
        );
        // Already in order: nothing new to read.
        assert_eq!(
            text(&groups(&[(72, 600, "Ola"), (110, 600, "Nordmann")])),
            None
        );
    }

    fn starts(data: &[u8]) -> Vec<(i64, i64)> {
        pieces(data)
            .iter()
            .filter_map(|p| match p {
                Piece::Pos { x, y, .. } => Some((*x, *y)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_tj_offset_that_kerns_back_past_the_run_starts_a_new_one() {
        // Nordmann is drawn at 110, and the offset takes the position back by 94 points, which is
        // more than the run is wide: Ola starts to the left of where Nordmann starts.
        let at = starts(b"BT /F1 12 Tf 110 600 Td [(Nordmann) 7834 (Ola)] TJ ET");
        assert_eq!(at.len(), 3);
        assert!(at[2].0 < at[1].0, "{at:?}");
        // A small offset (a kern of a letter pair) starts nothing.
        let at = starts(b"BT /F1 12 Tf 110 600 Td [(Nord) 20 (mann)] TJ ET");
        assert_eq!(at.len(), 2);
        // A move to the right starts nothing either.
        let at = starts(b"BT /F1 12 Tf 110 600 Td [(Nord) -20000 (mann)] TJ ET");
        assert_eq!(at.len(), 2);
    }

    #[test]
    fn turned_text_is_sorted_along_its_own_lines() {
        let turned = |x: i64, y: i64, t: &str| Group {
            x: x * 100,
            y: y * 100,
            size: 1200,
            // Text that runs up the page.
            dir: (0, 1000),
            text: t.chars().collect(),
        };
        let groups = [turned(300, 422, "Nordmann"), turned(300, 400, "Ola")];
        assert_eq!(
            ordered_text(&groups).map(|t| t.into_iter().collect::<String>()),
            Some("Ola Nordmann".to_owned())
        );
        // The same runs as text that is not turned would be read from the top, which is the
        // order of the stream here.
        let flat = |x: i64, y: i64, t: &str| Group {
            dir: (1000, 0),
            ..turned(x, y, t)
        };
        assert_eq!(
            ordered_text(&[flat(300, 422, "Nordmann"), flat(300, 400, "Ola")]),
            None
        );
    }

    #[test]
    fn text_that_is_turned_is_sorted_beside_text_that_is_not() {
        let run = |x: i64, y: i64, dir: (i64, i64), t: &str| Group {
            x: x * 100,
            y: y * 100,
            size: 1200,
            dir,
            text: t.chars().collect(),
        };
        // A line of ordinary text first, then two turned runs that were drawn out of order.
        let groups = [
            run(72, 720, (1000, 0), "Quarterly report"),
            run(300, 422, (0, 1000), "Nordmann"),
            run(300, 400, (0, 1000), "Ola"),
        ];
        assert_eq!(
            ordered_text(&groups).map(|t| t.into_iter().collect::<String>()),
            Some("Quarterly report Ola Nordmann".to_owned())
        );
    }
}
