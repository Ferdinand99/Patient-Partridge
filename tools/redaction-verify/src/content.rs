//! A forgiving reader for content streams. It gives the operators with their operands, and it
//! joins the text that is shown with several operators: `[(SEC) -20 (RET)] TJ` and
//! `(SEC) Tj (RET) Tj` show the word SECRET, but the bytes SECRET are nowhere.
//!
//! A kerning offset in a `TJ` array at or below [`WORD_SPACE`] is a word space (pdfTeX writes every
//! word space this way). Text is joined over text objects (`BT .. ET`) and over moves of the text
//! position, up to the end of the object or stream it sits in.

use crate::strings::{is_delim, is_space, parse_hex, parse_literal};

/// A `TJ` offset at or below this (in thousandths of the font size, negative moves right) counts
/// as a word space.
pub const WORD_SPACE: f64 = -100.0;

/// A transformation matrix `[a b c d e f]`, as the operands of `cm` and `Tm` give it.
pub type Matrix = [f64; 6];

pub const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `a` followed by `b`: the point goes through `a` first.
pub fn mul(a: &Matrix, b: &Matrix) -> Matrix {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

/// The point `(x, y)` moved by the matrix.
pub fn apply(m: &Matrix, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Num(f64),
    Name(Vec<u8>),
    Str(Vec<u8>),
    Array(Vec<Operand>),
    Other,
}

/// One operator with its operands.
#[derive(Debug)]
pub struct Op<'a> {
    pub name: &'a [u8],
    pub args: Vec<Operand>,
    /// Offset of the first operand, or of the operator when there is none.
    pub offset: usize,
    /// For `BI`: the dictionary text and the data of the inline image.
    pub inline: Option<(&'a [u8], &'a [u8])>,
}

impl Op<'_> {
    pub fn nums(&self) -> Vec<f64> {
        self.args
            .iter()
            .filter_map(|a| match a {
                Operand::Num(n) => Some(*n),
                _ => None,
            })
            .collect()
    }

    pub fn name_arg(&self) -> Option<&[u8]> {
        self.args.iter().find_map(|a| match a {
            Operand::Name(n) => Some(n.as_slice()),
            _ => None,
        })
    }
}

fn unescape(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut j = 0;
    while j < raw.len() {
        if raw[j] == b'#' && j + 2 < raw.len() {
            if let Some(v) = std::str::from_utf8(&raw[j + 1..j + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(v);
                j += 3;
                continue;
            }
        }
        out.push(raw[j]);
        j += 1;
    }
    out
}

fn token_end(buf: &[u8], mut i: usize) -> usize {
    while i < buf.len() && !is_space(buf[i]) && !is_delim(buf[i]) {
        i += 1;
    }
    i
}

/// Past `<< .. >>` that starts at `i`, nested pairs counted and strings skipped.
fn skip_dict(buf: &[u8], i: usize) -> usize {
    let n = buf.len();
    let mut depth = 0usize;
    let mut j = i;
    while j < n {
        match buf[j] {
            b'<' if buf.get(j + 1) == Some(&b'<') => {
                depth += 1;
                j += 2;
            }
            b'>' if buf.get(j + 1) == Some(&b'>') => {
                depth = depth.saturating_sub(1);
                j += 2;
                if depth == 0 {
                    return j;
                }
            }
            b'(' => j = parse_literal(buf, j).map_or(j + 1, |(_, e, _)| e),
            b'<' => j = parse_hex(buf, j).map_or(j + 1, |(_, e)| e),
            _ => j += 1,
        }
    }
    n
}

fn number(tok: &[u8]) -> Operand {
    std::str::from_utf8(tok)
        .ok()
        .and_then(|t| t.parse::<f64>().ok())
        .map_or(Operand::Other, Operand::Num)
}

fn parse_array(buf: &[u8], mut i: usize, depth: usize) -> (Vec<Operand>, usize) {
    let n = buf.len();
    let mut items = Vec::new();
    while i < n {
        let c = buf[i];
        match c {
            _ if is_space(c) => i += 1,
            b']' => return (items, i + 1),
            b'(' => match parse_literal(buf, i) {
                Some((b, e, _)) => {
                    items.push(Operand::Str(b));
                    i = e;
                }
                None => i += 1,
            },
            b'<' if buf.get(i + 1) == Some(&b'<') => {
                items.push(Operand::Other);
                i = skip_dict(buf, i);
            }
            b'<' => match parse_hex(buf, i) {
                Some((b, e)) => {
                    items.push(Operand::Str(b));
                    i = e;
                }
                None => i += 1,
            },
            b'[' if depth < 8 => {
                let (inner, e) = parse_array(buf, i + 1, depth + 1);
                items.push(Operand::Array(inner));
                i = e;
            }
            b'/' => {
                let e = token_end(buf, i + 1);
                items.push(Operand::Name(unescape(&buf[i + 1..e])));
                i = e;
            }
            _ if is_delim(c) => i += 1,
            _ => {
                let e = token_end(buf, i);
                items.push(number(&buf[i..e]));
                i = e.max(i + 1);
            }
        }
    }
    (items, n)
}

/// Calls `f` for each operator of the content stream `buf`.
pub fn for_each_op<'a>(buf: &'a [u8], f: &mut dyn FnMut(&Op<'a>)) {
    let n = buf.len();
    let mut args: Vec<Operand> = Vec::new();
    let mut first: Option<usize> = None;
    let mut i = 0;
    while i < n {
        let c = buf[i];
        if is_space(c) {
            i += 1;
            continue;
        }
        let start = i;
        match c {
            b'%' => {
                while i < n && buf[i] != b'\n' && buf[i] != b'\r' {
                    i += 1;
                }
            }
            b'(' => match parse_literal(buf, i) {
                Some((b, e, _)) => {
                    first.get_or_insert(start);
                    args.push(Operand::Str(b));
                    i = e;
                }
                None => i += 1,
            },
            b'<' if buf.get(i + 1) == Some(&b'<') => {
                first.get_or_insert(start);
                args.push(Operand::Other);
                i = skip_dict(buf, i);
            }
            b'<' => match parse_hex(buf, i) {
                Some((b, e)) => {
                    first.get_or_insert(start);
                    args.push(Operand::Str(b));
                    i = e;
                }
                None => i += 1,
            },
            b'[' => {
                first.get_or_insert(start);
                let (items, e) = parse_array(buf, i + 1, 0);
                args.push(Operand::Array(items));
                i = e;
            }
            b'/' => {
                first.get_or_insert(start);
                let e = token_end(buf, i + 1);
                args.push(Operand::Name(unescape(&buf[i + 1..e])));
                i = e;
            }
            _ if is_delim(c) => i += 1,
            _ => {
                let e = token_end(buf, i);
                let tok = &buf[i..e];
                i = e.max(i + 1);
                if matches!(tok[0], b'0'..=b'9' | b'+' | b'-' | b'.') {
                    first.get_or_insert(start);
                    args.push(number(tok));
                    continue;
                }
                if tok == b"true" || tok == b"false" || tok == b"null" {
                    first.get_or_insert(start);
                    args.push(Operand::Other);
                    continue;
                }
                let offset = first.unwrap_or(start);
                if tok == b"BI" {
                    let (dict, data, after) = inline_image(buf, i);
                    f(&Op {
                        name: tok,
                        args: Vec::new(),
                        offset,
                        inline: Some((dict, data)),
                    });
                    i = after;
                } else {
                    f(&Op {
                        name: tok,
                        args: std::mem::take(&mut args),
                        offset,
                        inline: None,
                    });
                }
                args.clear();
                first = None;
            }
        }
    }
}

/// The dictionary text, the data and the offset after `EI` of an inline image; `i` is just after
/// `BI`.
fn inline_image(buf: &[u8], i: usize) -> (&[u8], &[u8], usize) {
    let n = buf.len();
    let mut j = i;
    let mut dict_end = n;
    // The dictionary ends with ID followed by one white-space byte.
    while j + 2 < n {
        if buf[j] == b'I'
            && buf[j + 1] == b'D'
            && j > 0
            && is_space(buf[j - 1])
            && is_space(buf[j + 2])
        {
            dict_end = j;
            j += 3;
            break;
        }
        j += 1;
    }
    if dict_end == n {
        return (&buf[i..n], &[], n);
    }
    let data_start = j;
    while j + 1 < n {
        if buf[j] == b'E'
            && buf[j + 1] == b'I'
            && j > 0
            && is_space(buf[j - 1])
            && (j + 2 >= n || is_space(buf[j + 2]))
        {
            let data_end = j.saturating_sub(1).max(data_start);
            return (&buf[i..dict_end], &buf[data_start..data_end], j + 2);
        }
        j += 1;
    }
    (&buf[i..dict_end], &buf[data_start..], n)
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShownItem {
    Str(Vec<u8>),
    Kern(f64),
}

/// The strings and kerning offsets that one showing operator shows.
pub fn shown(op: &Op) -> Option<Vec<ShownItem>> {
    match op.name {
        b"Tj" | b"'" | b"\"" => op.args.iter().rev().find_map(|a| match a {
            Operand::Str(s) => Some(vec![ShownItem::Str(s.clone())]),
            _ => None,
        }),
        b"TJ" => op.args.iter().find_map(|a| match a {
            Operand::Array(items) => Some(
                items
                    .iter()
                    .filter_map(|x| match x {
                        Operand::Str(s) => Some(ShownItem::Str(s.clone())),
                        Operand::Num(v) => Some(ShownItem::Kern(*v)),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        }),
        _ => None,
    }
}

/// The strings of a run of text, joined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextChain {
    /// Offset of the first string in the buffer.
    pub offset: usize,
    /// All strings, one after the other.
    pub joined: Vec<u8>,
    /// The strings with a space where the text has a word space: a large `TJ` offset, a new
    /// showing operator, a move of the text position, the start or end of a text object.
    pub spaced: Vec<u8>,
    pub strings: usize,
}

#[derive(Default)]
struct Chain {
    joined: Vec<u8>,
    spaced: Vec<u8>,
    strings: usize,
    start: Option<usize>,
}

impl Chain {
    fn space(&mut self) {
        if !self.spaced.is_empty() && self.spaced.last() != Some(&b' ') {
            self.spaced.push(b' ');
        }
    }
}

/// Runs of text in a buffer that show two or more strings. A run ends where the buffer says that
/// an object or a stream ends. A run with one string is left out: the plain search sees it.
pub fn text_chains(buf: &[u8]) -> Vec<TextChain> {
    let mut out = Vec::new();
    let mut ch = Chain::default();
    let flush = |ch: &mut Chain, out: &mut Vec<TextChain>| {
        if ch.strings >= 2 {
            while ch.spaced.last() == Some(&b' ') {
                ch.spaced.pop();
            }
            out.push(TextChain {
                offset: ch.start.unwrap_or(0),
                joined: std::mem::take(&mut ch.joined),
                spaced: std::mem::take(&mut ch.spaced),
                strings: ch.strings,
            });
        }
        *ch = Chain::default();
    };
    for_each_op(buf, &mut |op| match op.name {
        b"endobj" | b"endstream" | b"stream" | b"obj" => flush(&mut ch, &mut out),
        b"BT" | b"ET" | b"Td" | b"TD" | b"T*" | b"Tm" => ch.space(),
        _ => {
            if let Some(items) = shown(op) {
                ch.space();
                for it in items {
                    match it {
                        ShownItem::Str(s) => {
                            ch.start.get_or_insert(op.offset);
                            ch.strings += 1;
                            ch.joined.extend_from_slice(&s);
                            ch.spaced.extend_from_slice(&s);
                        }
                        ShownItem::Kern(k) if k <= WORD_SPACE => ch.space(),
                        ShownItem::Kern(_) => {}
                    }
                }
            }
        }
    });
    flush(&mut ch, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_a_tj_array_and_two_tj_operators() {
        let objs =
            text_chains(b"BT /F1 12 Tf [(SEC) -20 (RET)] TJ ET BT (JO) Tj 0 -14 Td (HN) Tj ET");
        assert_eq!(objs.len(), 1);
        assert_eq!(objs[0].joined, b"SECRETJOHN");
        assert_eq!(objs[0].spaced, b"SECRET JO HN");
    }

    #[test]
    fn a_large_kerning_offset_is_a_word_space() {
        let c = text_chains(b"BT [(Ola)-333(Nordmann)] TJ ET");
        assert_eq!(c[0].joined, b"OlaNordmann");
        assert_eq!(c[0].spaced, b"Ola Nordmann");
        let small = text_chains(b"BT [(Ol)-20(a)] TJ ET");
        assert_eq!(small[0].spaced, b"Ola");
    }

    #[test]
    fn text_is_joined_over_text_objects_but_not_over_streams() {
        let c = text_chains(b"BT (Ola) Tj ET BT (Nordmann) Tj ET");
        assert_eq!(c[0].spaced, b"Ola Nordmann");
        let c = text_chains(b"BT (A) Tj ET\nendstream\nBT (B) Tj ET");
        assert!(c.is_empty());
    }

    #[test]
    fn hex_strings_count_and_a_single_string_is_left_out() {
        let objs = text_chains(b"BT <0053> Tj <0045> Tj ET endobj BT (one) Tj ET");
        assert_eq!(objs.len(), 1);
        assert_eq!(objs[0].joined, [0, 0x53, 0, 0x45]);
    }

    #[test]
    fn inline_image_data_is_skipped() {
        let objs = text_chains(b"BT (A) Tj BI /W 1 /H 1 ID (x) Tj EI (B) Tj ET");
        assert_eq!(objs.len(), 1);
        assert_eq!(objs[0].joined, b"AB");
    }

    #[test]
    fn operators_come_with_their_operands() {
        let mut seen = Vec::new();
        for_each_op(
            b"q 1 0 0 1 10 20 cm /F1 12 Tf << /A (x) >> BDC (hi) Tj Q",
            &mut |op| {
                seen.push((String::from_utf8_lossy(op.name).into_owned(), op.args.len()));
            },
        );
        assert_eq!(
            seen,
            [
                ("q".to_owned(), 0),
                ("cm".to_owned(), 6),
                ("Tf".to_owned(), 2),
                ("BDC".to_owned(), 1),
                ("Tj".to_owned(), 1),
                ("Q".to_owned(), 0),
            ]
        );
    }
}
