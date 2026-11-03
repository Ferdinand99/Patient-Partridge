//! Carving: every `stream .. endstream` in the bytes of a file is found by scanning, without the
//! cross-reference table, and its filters are undone. This reaches what a reader never shows:
//! old revisions (incremental updates), objects that nothing refers to, object streams, and
//! attachments, also when they are compressed.
//!
//! Filters that are undone: Flate (also a damaged stream, as far as it goes), LZW, ASCII85,
//! ASCIIHex, RunLength, and the PNG and TIFF (8 bit) predictors. Filters that make images
//! (DCT, JPX, CCITT, JBIG2) and encryption stop the chain; the bytes are searched as they are.

use std::io::Read;
use std::ops::Range;

use flate2::read::{DeflateDecoder, ZlibDecoder};
use sha2::{Digest, Sha256};

use crate::search::find_all;
use crate::strings::{is_delim, is_space};

/// The most bytes one stream may inflate to.
const MAX_OUTPUT: usize = 256 * 1024 * 1024;
/// How far back from `stream` the dictionary is looked for.
const MAX_DICT: usize = 64 * 1024;
/// How deep a stream inside a decoded stream (an attached PDF) is looked into.
const MAX_DEPTH: usize = 3;

/// A stream found in a buffer.
#[derive(Debug, Clone)]
pub struct Stream {
    /// Object number and generation, if the text before the dictionary reads `N G obj`.
    pub obj: Option<(u32, u32)>,
    /// The bytes between `stream` and `endstream`, without the end of line before `endstream`.
    pub body: Range<usize>,
    /// The bytes between `obj` and `stream`.
    pub dict: Vec<u8>,
}

impl Stream {
    pub fn has_filter(&self) -> bool {
        key_pos(&self.dict, "/Filter").is_some()
    }
}

fn find_from(hay: &[u8], pat: &[u8], from: usize) -> Option<usize> {
    if from >= hay.len() || hay.len() - from < pat.len() {
        return None;
    }
    let last = hay.len() - pat.len();
    let mut i = from;
    while i <= last {
        let p = hay[i..=last].iter().position(|&b| b == pat[0])?;
        i += p;
        if &hay[i..i + pat.len()] == pat {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Finds all streams, in file order.
pub fn find_streams(buf: &[u8]) -> Vec<Stream> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(kw) = find_from(buf, b"stream", from) {
        from = kw + 6;
        if kw >= 3 && &buf[kw - 3..kw] == b"end" {
            continue;
        }
        let body_start = match buf.get(kw + 6) {
            Some(b'\n') => kw + 7,
            Some(b'\r') if buf.get(kw + 7) == Some(&b'\n') => kw + 8,
            Some(b'\r') => kw + 7,
            _ => continue,
        };
        let (end, next) = match find_from(buf, b"endstream", body_start) {
            Some(e) => (e, e + 9),
            None => (buf.len(), buf.len()),
        };
        let mut body_end = end;
        if body_end > body_start && buf[body_end - 1] == b'\n' {
            body_end -= 1;
            if body_end > body_start && buf[body_end - 1] == b'\r' {
                body_end -= 1;
            }
        } else if body_end > body_start && buf[body_end - 1] == b'\r' {
            body_end -= 1;
        }
        let (obj, dict) = dictionary_before(buf, kw);
        out.push(Stream {
            obj,
            body: body_start..body_end,
            dict,
        });
        from = next;
    }
    out
}

/// The dictionary text before the `stream` keyword at `kw` and the object id before it.
fn dictionary_before(buf: &[u8], kw: usize) -> (Option<(u32, u32)>, Vec<u8>) {
    let lo = kw.saturating_sub(MAX_DICT);
    let window = &buf[lo..kw];
    let mut pos = None;
    let mut search_end = window.len();
    while let Some(p) = window[..search_end].windows(3).rposition(|w| w == b"obj") {
        if p >= 3 && &window[p - 3..p] == b"end" {
            search_end = p;
            continue;
        }
        pos = Some(p);
        break;
    }
    let Some(p) = pos else {
        return (None, window.to_vec());
    };
    let dict = window[p + 3..].to_vec();
    // Read "N G " backwards.
    let mut j = p;
    let skip_space = |j: &mut usize| {
        while *j > 0 && is_space(window[*j - 1]) {
            *j -= 1;
        }
    };
    let read_num = |j: &mut usize| -> Option<u32> {
        let end = *j;
        while *j > 0 && window[*j - 1].is_ascii_digit() {
            *j -= 1;
        }
        std::str::from_utf8(&window[*j..end]).ok()?.parse().ok()
    };
    skip_space(&mut j);
    let generation = read_num(&mut j);
    skip_space(&mut j);
    let number = read_num(&mut j);
    (number.zip(generation), dict)
}

// ---------------------------------------------------------------------------------------------
// Dictionary reading. A stream dictionary is read as text; there is no full parser, because the
// verifier must also work on files that no parser accepts.

/// Position just after `key` and its white space, if `key` is a key of the dictionary text.
pub fn key_pos(dict: &[u8], key: &str) -> Option<usize> {
    let k = key.as_bytes();
    for p in find_all(dict, k) {
        let after = p + k.len();
        if after >= dict.len() || is_space(dict[after]) || is_delim(dict[after]) {
            let mut v = after;
            while v < dict.len() && is_space(dict[v]) {
                v += 1;
            }
            return Some(v);
        }
    }
    None
}

fn name_at(dict: &[u8], pos: usize) -> Option<String> {
    if dict.get(pos) != Some(&b'/') {
        return None;
    }
    let mut j = pos + 1;
    while j < dict.len() && !is_space(dict[j]) && !is_delim(dict[j]) {
        j += 1;
    }
    Some(String::from_utf8_lossy(&dict[pos + 1..j]).into_owned())
}

/// The name that is the value of `key`, without the slash.
pub fn name_value(dict: &[u8], key: &str) -> Option<String> {
    name_at(dict, key_pos(dict, key)?)
}

fn int_value(dict: &[u8], key: &str) -> Option<i64> {
    let p = key_pos(dict, key)?;
    let mut j = p;
    if dict.get(j) == Some(&b'-') {
        j += 1;
    }
    while j < dict.len() && dict[j].is_ascii_digit() {
        j += 1;
    }
    std::str::from_utf8(&dict[p..j]).ok()?.parse().ok()
}

/// `<< .. >>` that starts at `pos`, nested pairs counted.
fn dict_at(dict: &[u8], pos: usize) -> Option<Range<usize>> {
    if dict.get(pos..pos + 2)? != b"<<" {
        return None;
    }
    let mut depth = 0usize;
    let mut j = pos;
    while j + 1 < dict.len() {
        if &dict[j..j + 2] == b"<<" {
            depth += 1;
            j += 2;
        } else if &dict[j..j + 2] == b">>" {
            depth -= 1;
            j += 2;
            if depth == 0 {
                return Some(pos..j);
            }
        } else {
            j += 1;
        }
    }
    None
}

fn filter_names(dict: &[u8]) -> Vec<String> {
    let Some(p) = key_pos(dict, "/Filter") else {
        return Vec::new();
    };
    match dict.get(p) {
        Some(b'/') => name_at(dict, p).into_iter().collect(),
        Some(b'[') => {
            let mut out = Vec::new();
            let mut j = p + 1;
            while j < dict.len() && dict[j] != b']' {
                if dict[j] == b'/' {
                    if let Some(n) = name_at(dict, j) {
                        j += n.len();
                        out.push(n);
                    }
                }
                j += 1;
            }
            out
        }
        _ => Vec::new(),
    }
}

#[derive(Debug, Clone, Copy)]
struct Params {
    predictor: i64,
    colors: usize,
    bits: usize,
    columns: usize,
    early_change: bool,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            predictor: 1,
            colors: 1,
            bits: 8,
            columns: 1,
            early_change: true,
        }
    }
}

fn read_params(d: &[u8]) -> Params {
    let n = |k: &str, default: i64| int_value(d, k).unwrap_or(default).clamp(1, 1 << 20) as usize;
    Params {
        predictor: int_value(d, "/Predictor").unwrap_or(1),
        colors: n("/Colors", 1),
        bits: n("/BitsPerComponent", 8),
        columns: n("/Columns", 1),
        early_change: int_value(d, "/EarlyChange").unwrap_or(1) != 0,
    }
}

/// One parameter set for each filter.
fn filter_params(dict: &[u8], count: usize) -> Vec<Params> {
    let key = key_pos(dict, "/DecodeParms").or_else(|| key_pos(dict, "/DP"));
    let mut out = vec![Params::default(); count];
    let Some(p) = key else {
        return out;
    };
    match dict.get(p) {
        Some(b'<') => {
            if let Some(r) = dict_at(dict, p) {
                let one = read_params(&dict[r]);
                out.iter_mut().for_each(|o| *o = one);
            }
        }
        Some(b'[') => {
            let mut j = p + 1;
            let mut idx = 0;
            while j < dict.len() && dict[j] != b']' && idx < count {
                if dict[j] == b'<' {
                    if let Some(r) = dict_at(dict, j) {
                        out[idx] = read_params(&dict[r.clone()]);
                        j = r.end;
                        idx += 1;
                        continue;
                    }
                } else if dict[j..].starts_with(b"null") {
                    idx += 1;
                    j += 4;
                    continue;
                }
                j += 1;
            }
        }
        _ => {}
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Filters.

/// The result of undoing the filters of one stream.
#[derive(Debug, Clone)]
pub struct Decoded {
    pub data: Vec<u8>,
    /// The filters that were undone, in order.
    pub applied: Vec<String>,
    /// A filter that was not undone (images, encryption); `data` is what it would read.
    pub stopped_at: Option<String>,
}

pub fn decode(raw: &[u8], dict: &[u8]) -> Decoded {
    let mut names = filter_names(dict);
    if names.is_empty() && key_pos(dict, "/Filter").is_some() && raw.first() == Some(&0x78) {
        // A filter entry that cannot be read, such as an indirect object. A zlib header says Flate.
        names.push("FlateDecode".into());
    }
    let params = filter_params(dict, names.len());
    let mut data = raw.to_vec();
    let mut applied = Vec::new();
    let mut stopped_at = None;
    for (name, p) in names.iter().zip(params) {
        match name.as_str() {
            "FlateDecode" | "Fl" => data = predict(inflate(&data), &p),
            "LZWDecode" | "LZW" => data = predict(lzw(&data, p.early_change), &p),
            "ASCIIHexDecode" | "AHx" => data = ascii_hex(&data),
            "ASCII85Decode" | "A85" => data = ascii85(&data),
            "RunLengthDecode" | "RL" => data = run_length(&data),
            other => {
                stopped_at = Some(other.to_owned());
                break;
            }
        }
        applied.push(name.clone());
    }
    Decoded {
        data,
        applied,
        stopped_at,
    }
}

fn read_partial<R: Read>(mut r: R) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match r.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                out.extend_from_slice(&chunk[..n]);
                if out.len() > MAX_OUTPUT {
                    break;
                }
            }
        }
    }
    out
}

/// Inflates as far as the data goes: a damaged stream gives what could be read.
fn inflate(data: &[u8]) -> Vec<u8> {
    let out = read_partial(ZlibDecoder::new(data));
    if out.is_empty() {
        return read_partial(DeflateDecoder::new(data));
    }
    out
}

fn ascii_hex(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut high: Option<u8> = None;
    for &c in data {
        let v = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            b'>' => break,
            _ => continue,
        };
        match high.take() {
            Some(h) => out.push(h << 4 | v),
            None => high = Some(v),
        }
    }
    if let Some(h) = high {
        out.push(h << 4);
    }
    out
}

fn ascii85(data: &[u8]) -> Vec<u8> {
    let data = data
        .iter()
        .position(|c| !is_space(*c))
        .map_or(data, |p| &data[p..]);
    let data = data.strip_prefix(b"<~").unwrap_or(data);
    let mut out = Vec::new();
    let mut group = [0u32; 5];
    let mut n = 0;
    for &c in data {
        match c {
            b'~' => break,
            b'z' if n == 0 => out.extend_from_slice(&[0, 0, 0, 0]),
            b'!'..=b'u' => {
                group[n] = u32::from(c - b'!');
                n += 1;
                if n == 5 {
                    let v = group
                        .iter()
                        .fold(0u32, |a, &d| a.wrapping_mul(85).wrapping_add(d));
                    out.extend_from_slice(&v.to_be_bytes());
                    n = 0;
                }
            }
            _ => {}
        }
    }
    if n >= 2 {
        for g in group.iter_mut().skip(n) {
            *g = 84;
        }
        let v = group
            .iter()
            .fold(0u32, |a, &d| a.wrapping_mul(85).wrapping_add(d));
        out.extend_from_slice(&v.to_be_bytes()[..n - 1]);
    }
    out
}

fn run_length(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let len = data[i];
        i += 1;
        match len {
            128 => break,
            0..=127 => {
                let end = (i + usize::from(len) + 1).min(data.len());
                out.extend_from_slice(&data[i..end]);
                i = end;
            }
            _ => {
                if let Some(&b) = data.get(i) {
                    out.extend(std::iter::repeat_n(b, 257 - usize::from(len)));
                }
                i += 1;
            }
        }
    }
    out
}

fn lzw(data: &[u8], early_change: bool) -> Vec<u8> {
    let early = usize::from(early_change);
    let fresh = || -> Vec<Vec<u8>> {
        let mut t: Vec<Vec<u8>> = (0..=255u8).map(|b| vec![b]).collect();
        t.push(Vec::new());
        t.push(Vec::new());
        t
    };
    let mut table = fresh();
    let mut out = Vec::new();
    let mut prev: Option<Vec<u8>> = None;
    let mut bits = 9usize;
    let mut acc: u32 = 0;
    let mut have = 0usize;
    let mut bytes = data.iter();
    loop {
        while have < bits {
            match bytes.next() {
                Some(&b) => {
                    acc = acc << 8 | u32::from(b);
                    have += 8;
                }
                None => return out,
            }
        }
        let code = ((acc >> (have - bits)) & ((1 << bits) - 1)) as usize;
        have -= bits;
        acc &= (1 << have) - 1;
        match code {
            256 => {
                table = fresh();
                bits = 9;
                prev = None;
                continue;
            }
            257 => break,
            _ => {}
        }
        let entry = if code < table.len() {
            table[code].clone()
        } else if let (true, Some(p)) = (code == table.len(), &prev) {
            let mut e = p.clone();
            e.push(p[0]);
            e
        } else {
            break;
        };
        out.extend_from_slice(&entry);
        if out.len() > MAX_OUTPUT {
            break;
        }
        if let Some(p) = &prev {
            if table.len() < 4096 {
                let mut e = p.clone();
                e.push(entry[0]);
                table.push(e);
            }
        }
        prev = Some(entry);
        let size = table.len() + early;
        bits = if size >= 2048 {
            12
        } else if size >= 1024 {
            11
        } else if size >= 512 {
            10
        } else {
            9
        };
    }
    out
}

fn predict(data: Vec<u8>, p: &Params) -> Vec<u8> {
    match p.predictor {
        2 if p.bits == 8 => {
            let row = p.colors * p.columns;
            let mut data = data;
            for chunk in data.chunks_mut(row) {
                for i in p.colors..chunk.len() {
                    chunk[i] = chunk[i].wrapping_add(chunk[i - p.colors]);
                }
            }
            data
        }
        10..=15 => {
            let bpp = (p.colors * p.bits).div_ceil(8).max(1);
            let row = (p.colors * p.bits * p.columns).div_ceil(8);
            let mut out: Vec<u8> = Vec::with_capacity(data.len());
            let mut prev = vec![0u8; row];
            for chunk in data.chunks(row + 1) {
                let tag = chunk[0];
                let mut cur = chunk[1..].to_vec();
                for i in 0..cur.len() {
                    let a = if i >= bpp { cur[i - bpp] } else { 0 };
                    let b = prev.get(i).copied().unwrap_or(0);
                    let c = if i >= bpp {
                        prev.get(i - bpp).copied().unwrap_or(0)
                    } else {
                        0
                    };
                    cur[i] = cur[i].wrapping_add(match tag {
                        1 => a,
                        2 => b,
                        3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                        4 => paeth(a, b, c),
                        _ => 0,
                    });
                }
                out.extend_from_slice(&cur);
                cur.resize(row, 0);
                prev = cur;
            }
            out
        }
        _ => data,
    }
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (ia, ib, ic) = (i32::from(a), i32::from(b), i32::from(c));
    let p = ia + ib - ic;
    let (pa, pb, pc) = ((p - ia).abs(), (p - ib).abs(), (p - ic).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

// ---------------------------------------------------------------------------------------------
// Carving.

/// A stream with its filters undone.
#[derive(Debug)]
pub struct Carved<'a> {
    pub obj: Option<(u32, u32)>,
    /// Where it was found: empty for the file, or the chain of streams it sits in.
    pub within: String,
    /// Offset of the stream body in the buffer it was found in (the file, or a decoded stream).
    pub body_offset: usize,
    pub dict: &'a [u8],
    pub raw: &'a [u8],
    pub decoded: &'a Decoded,
    pub is_image: bool,
}

impl Carved<'_> {
    pub fn describe(&self) -> String {
        let obj = self
            .obj
            .map_or_else(|| "stream".to_owned(), |(n, g)| format!("object {n} {g}"));
        let filters = if self.decoded.applied.is_empty() {
            "none".to_owned()
        } else {
            self.decoded.applied.join("+")
        };
        let mut s = format!(
            "{}{} (body at {}, filters {})",
            self.within, obj, self.body_offset, filters
        );
        if let Some(stop) = &self.decoded.stopped_at {
            s.push_str(&format!(", not undone: {stop}"));
        }
        s
    }
}

/// Calls `each` for every stream of `buf` and of the streams inside decoded streams.
pub fn carve(buf: &[u8], each: &mut dyn FnMut(&Carved)) {
    carve_in(buf, 0, "", each);
}

fn carve_in(buf: &[u8], depth: usize, within: &str, each: &mut dyn FnMut(&Carved)) {
    for s in find_streams(buf) {
        let raw = &buf[s.body.clone()];
        let decoded = decode(raw, &s.dict);
        let is_image = name_value(&s.dict, "/Subtype").as_deref() == Some("Image");
        each(&Carved {
            obj: s.obj,
            within: within.to_owned(),
            body_offset: s.body.start,
            dict: &s.dict,
            raw,
            decoded: &decoded,
            is_image,
        });
        if depth < MAX_DEPTH && !is_image && find_from(&decoded.data, b"stream", 0).is_some() {
            let label = s.obj.map_or_else(
                || format!("{within}stream at {} > ", s.body.start),
                |(n, g)| format!("{within}object {n} {g} > "),
            );
            carve_in(&decoded.data, depth + 1, &label, each);
        }
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(data)
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write;

    fn flate(data: &[u8]) -> Vec<u8> {
        let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn finds_streams_with_object_ids_and_ignores_endstream() {
        let mut f = b"4 0 obj\n<< /Length 5 /Filter /FlateDecode >>\nstream\n".to_vec();
        f.extend(flate(b"hello"));
        f.extend(b"\nendstream\nendobj\n");
        let s = find_streams(&f);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].obj, Some((4, 0)));
        assert!(s[0].has_filter());
        let d = decode(&f[s[0].body.clone()], &s[0].dict);
        assert_eq!(d.data, b"hello");
        assert_eq!(d.applied, ["FlateDecode"]);
    }

    #[test]
    fn filter_chain_hex_then_flate() {
        let packed = flate(b"chain");
        let hex: String = packed.iter().map(|b| format!("{b:02x}")).collect();
        let dict = b"<< /Filter [/ASCIIHexDecode /FlateDecode] >>";
        let d = decode(format!("{hex}>").as_bytes(), dict);
        assert_eq!(d.data, b"chain");
        assert!(d.stopped_at.is_none());
    }

    #[test]
    fn ascii85_runlength_and_lzw() {
        // "Man " is 9jqo^ in ASCII85.
        assert_eq!(ascii85(b"<~9jqo^~>"), b"Man ");
        assert_eq!(
            run_length(&[2, b'a', b'b', b'c', 254, b'z', 128]),
            b"abczzz"
        );
        // The example in the PDF reference: 45 45 45 45 45 65 45 45 45 66.
        let lz = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        assert_eq!(lzw(&lz, true), [45, 45, 45, 45, 45, 65, 45, 45, 45, 66]);
    }

    #[test]
    fn png_up_predictor() {
        // Two rows of 3 bytes, filter tag 2 (Up).
        let raw = [2, 1, 2, 3, 2, 1, 1, 1];
        let p = Params {
            predictor: 12,
            columns: 3,
            ..Params::default()
        };
        assert_eq!(predict(raw.to_vec(), &p), [1, 2, 3, 2, 3, 4]);
    }

    #[test]
    fn carves_a_stream_inside_a_stream() {
        let inner = b"9 0 obj\n<< >>\nstream\nsecret\nendstream\nendobj\n";
        let mut f = b"4 0 obj\n<< /Filter /FlateDecode >>\nstream\n".to_vec();
        f.extend(flate(inner));
        f.extend(b"\nendstream\nendobj\n");
        let mut seen = Vec::new();
        carve(&f, &mut |c| {
            seen.push((c.within.clone(), c.decoded.data.clone()))
        });
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[1].1, b"secret");
        assert!(seen[1].0.starts_with("object 4 0"));
    }
}
