//! A small reader for PDF objects, built on scanning: every `N G obj` found in a buffer is read, all
//! versions of an object are kept (old revisions too), and the objects of object streams are
//! opened. It does not use the cross-reference table, so it also reads objects that nothing
//! refers to. It is used to find the fonts and resources of a content stream and to walk pages.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

use crate::carve::{self, Decoded, FilterSpec};
use crate::search::find_all;
use crate::strings::{is_delim, is_space, parse_hex, parse_literal};

/// A PDF object. Names keep their bytes as Latin-1 characters, `#xx` escapes undone.
#[derive(Debug, Clone, PartialEq)]
pub enum Obj {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    Name(String),
    Str(Vec<u8>),
    Array(Vec<Obj>),
    Dict(Vec<(String, Obj)>),
    Ref(u32),
}

pub static NULL: Obj = Obj::Null;

impl Obj {
    /// The value of `key` (without the slash) of a dictionary.
    pub fn get(&self, key: &str) -> Option<&Obj> {
        match self {
            Obj::Dict(d) => d.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            Obj::Name(n) => Some(n),
            _ => None,
        }
    }

    pub fn int(&self) -> Option<i64> {
        match self {
            Obj::Int(i) => Some(*i),
            Obj::Real(r) => Some(*r as i64),
            _ => None,
        }
    }

    pub fn num(&self) -> Option<f64> {
        match self {
            Obj::Int(i) => Some(*i as f64),
            Obj::Real(r) => Some(*r),
            _ => None,
        }
    }

    pub fn array(&self) -> Option<&[Obj]> {
        match self {
            Obj::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn is_dict(&self) -> bool {
        matches!(self, Obj::Dict(_))
    }

    pub fn entries(&self) -> &[(String, Obj)] {
        match self {
            Obj::Dict(d) => d,
            _ => &[],
        }
    }
}

fn skip_ws(buf: &[u8], mut i: usize) -> usize {
    loop {
        while i < buf.len() && is_space(buf[i]) {
            i += 1;
        }
        if buf.get(i) == Some(&b'%') {
            while i < buf.len() && buf[i] != b'\n' && buf[i] != b'\r' {
                i += 1;
            }
        } else {
            return i;
        }
    }
}

fn token_end(buf: &[u8], mut i: usize) -> usize {
    while i < buf.len() && !is_space(buf[i]) && !is_delim(buf[i]) {
        i += 1;
    }
    i
}

/// Reads one object that starts at or after `pos`. Returns it and the offset after it.
pub fn parse(buf: &[u8], pos: usize) -> Option<(Obj, usize)> {
    parse_at(buf, pos, 0)
}

fn parse_name(buf: &[u8], i: usize) -> (String, usize) {
    let end = token_end(buf, i + 1);
    let raw = &buf[i + 1..end];
    let mut out = String::with_capacity(raw.len());
    let mut j = 0;
    while j < raw.len() {
        if raw[j] == b'#' && j + 2 < raw.len() {
            let hex = std::str::from_utf8(&raw[j + 1..j + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v as char);
                j += 3;
                continue;
            }
        }
        out.push(raw[j] as char);
        j += 1;
    }
    (out, end)
}

fn parse_at(buf: &[u8], i: usize, depth: usize) -> Option<(Obj, usize)> {
    if depth > 40 {
        return None;
    }
    let i = skip_ws(buf, i);
    let c = *buf.get(i)?;
    match c {
        b'<' if buf.get(i + 1) == Some(&b'<') => {
            let mut j = i + 2;
            let mut entries = Vec::new();
            loop {
                j = skip_ws(buf, j);
                if buf.get(j..j + 2) == Some(b">>") {
                    return Some((Obj::Dict(entries), j + 2));
                }
                if buf.get(j) != Some(&b'/') {
                    return None;
                }
                let (key, after) = parse_name(buf, j);
                let (val, end) = parse_at(buf, after, depth + 1)?;
                entries.push((key, val));
                j = end;
            }
        }
        b'<' => parse_hex(buf, i).map(|(b, e)| (Obj::Str(b), e)),
        b'(' => parse_literal(buf, i).map(|(b, e, _)| (Obj::Str(b), e)),
        b'[' => {
            let mut j = i + 1;
            let mut items = Vec::new();
            loop {
                j = skip_ws(buf, j);
                match buf.get(j)? {
                    b']' => return Some((Obj::Array(items), j + 1)),
                    _ => {
                        let (v, e) = parse_at(buf, j, depth + 1)?;
                        items.push(v);
                        j = e;
                    }
                }
            }
        }
        b'/' => {
            let (n, e) = parse_name(buf, i);
            Some((Obj::Name(n), e))
        }
        b'+' | b'-' | b'.' | b'0'..=b'9' => {
            let end = token_end(buf, i);
            let tok = std::str::from_utf8(&buf[i..end]).ok()?;
            if let Ok(n) = tok.parse::<i64>() {
                if n >= 0 {
                    // `N G R`
                    let j = skip_ws(buf, end);
                    let e2 = token_end(buf, j);
                    if e2 > j && buf[j..e2].iter().all(u8::is_ascii_digit) {
                        let k = skip_ws(buf, e2);
                        if buf.get(k) == Some(&b'R')
                            && buf.get(k + 1).is_none_or(|c| is_space(*c) || is_delim(*c))
                        {
                            return Some((Obj::Ref(u32::try_from(n).ok()?), k + 1));
                        }
                    }
                }
                return Some((Obj::Int(n), end));
            }
            tok.parse::<f64>().ok().map(|r| (Obj::Real(r), end))
        }
        _ => {
            let end = token_end(buf, i);
            match &buf[i..end] {
                b"true" => Some((Obj::Bool(true), end)),
                b"false" => Some((Obj::Bool(false), end)),
                b"null" => Some((Obj::Null, end)),
                _ => None,
            }
        }
    }
}

/// One version of one object.
#[derive(Debug, Clone)]
pub struct Entry {
    pub num: u32,
    pub gen: u32,
    /// Offset of the `obj` keyword (of the container for an object in an object stream).
    pub pos: usize,
    /// The value; the dictionary for a stream.
    pub value: Obj,
    /// The raw body of the stream.
    pub stream: Option<Range<usize>>,
    pub in_objstm: bool,
}

/// All objects of a buffer.
pub struct Doc<'a> {
    pub buf: &'a [u8],
    objs: BTreeMap<u32, Vec<Entry>>,
    by_body: HashMap<usize, (u32, usize)>,
    trailers: Vec<Obj>,
}

fn inside(ranges: &[(usize, usize)], pos: usize) -> bool {
    let idx = ranges.partition_point(|r| r.0 <= pos);
    idx > 0 && pos < ranges[idx - 1].1
}

impl<'a> Doc<'a> {
    pub fn scan(buf: &'a [u8]) -> Doc<'a> {
        let streams = carve::find_streams(buf);
        let bodies: Vec<(usize, usize)> =
            streams.iter().map(|s| (s.body.start, s.body.end)).collect();
        let body_set: HashMap<usize, Range<usize>> = streams
            .iter()
            .map(|s| (s.body.start, s.body.clone()))
            .collect();
        let mut doc = Doc {
            buf,
            objs: BTreeMap::new(),
            by_body: HashMap::new(),
            trailers: Vec::new(),
        };
        for p in find_all(buf, b"obj") {
            if inside(&bodies, p) {
                continue;
            }
            if p == 0 || !is_space(buf[p - 1]) {
                continue;
            }
            if buf.get(p + 3).is_some_and(|c| c.is_ascii_alphanumeric()) {
                continue;
            }
            // `N G ` before the keyword.
            let mut j = p;
            let skip_space = |j: &mut usize| {
                while *j > 0 && is_space(buf[*j - 1]) {
                    *j -= 1;
                }
            };
            let read_num = |j: &mut usize| -> Option<u32> {
                let end = *j;
                while *j > 0 && buf[*j - 1].is_ascii_digit() {
                    *j -= 1;
                }
                std::str::from_utf8(&buf[*j..end]).ok()?.parse().ok()
            };
            skip_space(&mut j);
            let Some(gen) = read_num(&mut j) else {
                continue;
            };
            skip_space(&mut j);
            let Some(num) = read_num(&mut j) else {
                continue;
            };
            let Some((value, end)) = parse(buf, p + 3) else {
                continue;
            };
            let mut stream = None;
            let k = skip_ws(buf, end);
            if value.is_dict() && buf.get(k..k + 6) == Some(b"stream") {
                let start = match buf.get(k + 6) {
                    Some(b'\n') => k + 7,
                    Some(b'\r') if buf.get(k + 7) == Some(&b'\n') => k + 8,
                    Some(b'\r') => k + 7,
                    _ => k + 6,
                };
                stream = body_set.get(&start).cloned();
            }
            if let Some(r) = &stream {
                doc.by_body
                    .insert(r.start, (num, doc.objs.get(&num).map_or(0, Vec::len)));
            }
            doc.objs.entry(num).or_default().push(Entry {
                num,
                gen,
                pos: p,
                value,
                stream,
                in_objstm: false,
            });
        }
        for p in find_all(buf, b"trailer") {
            if inside(&bodies, p) {
                continue;
            }
            if let Some((v, _)) = parse(buf, p + 7) {
                if v.is_dict() {
                    doc.trailers.push(v);
                }
            }
        }
        doc.open_object_streams();
        doc
    }

    fn open_object_streams(&mut self) {
        let containers: Vec<Entry> = self
            .objs
            .values()
            .flatten()
            .filter(|e| {
                e.stream.is_some() && e.value.get("Type").and_then(Obj::name) == Some("ObjStm")
            })
            .cloned()
            .collect();
        for c in containers {
            let Some(d) = self.stream_data(&c) else {
                continue;
            };
            let (Some(n), Some(first)) = (
                self.dget(&c.value, "N").int(),
                self.dget(&c.value, "First").int(),
            ) else {
                continue;
            };
            let data = d.data;
            let mut pos = 0;
            let mut pairs = Vec::new();
            for _ in 0..n.clamp(0, 1_000_000) {
                let Some((Obj::Int(num), e1)) = parse(&data, pos) else {
                    break;
                };
                let Some((Obj::Int(off), e2)) = parse(&data, e1) else {
                    break;
                };
                pos = e2;
                pairs.push((num, off));
            }
            for (num, off) in pairs {
                let at = usize::try_from(first + off).unwrap_or(usize::MAX);
                if let Some((value, _)) = data.get(at..).and_then(|s| parse(s, 0)) {
                    self.objs.entry(num as u32).or_default().push(Entry {
                        num: num as u32,
                        gen: 0,
                        pos: c.pos,
                        value,
                        stream: None,
                        in_objstm: true,
                    });
                }
            }
        }
        for v in self.objs.values_mut() {
            v.sort_by_key(|e| e.pos);
        }
        // Entries moved: the map from body offsets to versions must be rebuilt.
        self.by_body.clear();
        let pairs: Vec<(usize, u32, usize)> = self
            .objs
            .iter()
            .flat_map(|(n, v)| {
                v.iter()
                    .enumerate()
                    .filter_map(|(i, e)| e.stream.as_ref().map(|s| (s.start, *n, i)))
            })
            .collect();
        for (start, n, i) in pairs {
            self.by_body.insert(start, (n, i));
        }
    }

    pub fn nums(&self) -> impl Iterator<Item = u32> + '_ {
        self.objs.keys().copied()
    }

    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &Entry> {
        self.objs.values().flatten()
    }

    pub fn all(&self, n: u32) -> &[Entry] {
        self.objs.get(&n).map_or(&[], Vec::as_slice)
    }

    pub fn latest(&self, n: u32) -> Option<&Entry> {
        self.objs.get(&n).and_then(|v| v.last())
    }

    /// The entry whose stream body starts at `start`.
    pub fn at_body(&self, start: usize) -> Option<&Entry> {
        let (n, i) = self.by_body.get(&start)?;
        self.objs.get(n)?.get(*i)
    }

    pub fn trailers(&self) -> &[Obj] {
        &self.trailers
    }

    pub fn resolve<'s>(&'s self, o: &'s Obj) -> &'s Obj {
        let mut cur = o;
        for _ in 0..16 {
            match cur {
                Obj::Ref(n) => match self.latest(*n) {
                    Some(e) => cur = &e.value,
                    None => return &NULL,
                },
                _ => return cur,
            }
        }
        &NULL
    }

    /// The value of `key` in the dictionary `d`, references followed.
    pub fn dget<'s>(&'s self, d: &'s Obj, key: &str) -> &'s Obj {
        d.get(key).map_or(&NULL, |v| self.resolve(v))
    }

    /// The stream of an entry with its filters undone.
    pub fn stream_data(&self, e: &Entry) -> Option<Decoded> {
        let body = e.stream.as_ref()?;
        let spec = FilterSpec::from_obj(&e.value, self);
        Some(carve::decode_spec(&self.buf[body.clone()], &spec))
    }

    /// The pages in order, each with the resources it inherits.
    pub fn pages(&self) -> Vec<(&Entry, Obj)> {
        let mut out = Vec::new();
        let root = self
            .trailers
            .iter()
            .rev()
            .find_map(|t| match t.get("Root") {
                Some(Obj::Ref(n)) => Some(*n),
                _ => None,
            })
            .or_else(|| {
                self.entries().rev().find_map(|e| {
                    (e.value.get("Type").and_then(Obj::name) == Some("Catalog")).then_some(e.num)
                })
            });
        let Some(root) = root.and_then(|n| self.latest(n)) else {
            return out;
        };
        if let Some(Obj::Ref(p)) = root.value.get("Pages") {
            self.collect_pages(*p, &Obj::Null, &mut out, 0);
        }
        out
    }

    fn collect_pages<'s>(
        &'s self,
        n: u32,
        res: &Obj,
        out: &mut Vec<(&'s Entry, Obj)>,
        depth: usize,
    ) {
        if depth > 40 || out.len() > 100_000 {
            return;
        }
        let Some(e) = self.latest(n) else { return };
        let own = e
            .value
            .get("Resources")
            .cloned()
            .unwrap_or_else(|| res.clone());
        match self.dget(&e.value, "Kids") {
            Obj::Array(kids) => {
                for k in kids {
                    if let Obj::Ref(kn) = k {
                        self.collect_pages(*kn, &own, out, depth + 1);
                    }
                }
            }
            _ => out.push((e, own)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dictionaries_refs_and_names_with_escapes() {
        let (o, _) = parse(
            b"<< /A 1 0 R /B [1 2.5 (x) <41>] /C /N#61me /D << /E true >> >>",
            0,
        )
        .unwrap();
        assert_eq!(o.get("A"), Some(&Obj::Ref(1)));
        assert_eq!(o.get("C"), Some(&Obj::Name("Name".into())));
        assert_eq!(o.get("D").and_then(|d| d.get("E")), Some(&Obj::Bool(true)));
        assert_eq!(
            o.get("B"),
            Some(&Obj::Array(vec![
                Obj::Int(1),
                Obj::Real(2.5),
                Obj::Str(b"x".to_vec()),
                Obj::Str(b"A".to_vec())
            ]))
        );
    }

    #[test]
    fn scans_all_versions_and_streams() {
        let f = b"%PDF-1.4\n1 0 obj\n<< /K 1 >>\nendobj\n2 0 obj\n<< /Length 3 >>\nstream\nabc\nendstream\nendobj\n1 0 obj\n<< /K 2 >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n";
        let d = Doc::scan(f);
        assert_eq!(d.all(1).len(), 2);
        assert_eq!(d.latest(1).unwrap().value.get("K"), Some(&Obj::Int(2)));
        let s = d.latest(2).unwrap();
        assert_eq!(&f[s.stream.clone().unwrap()], b"abc");
        assert!(d.at_body(s.stream.as_ref().unwrap().start).is_some());
        assert_eq!(d.trailers().len(), 1);
    }
}
