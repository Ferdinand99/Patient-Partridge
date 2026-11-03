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
    /// A token that a strict reader refuses: a number with a stray sign or letter, a name with a
    /// bad escape, a string that does not end. Poppler reads some of these as something, and the
    /// tool does not try to guess which: the content is "could not check".
    Bad,
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
    /// For `BI`: where the data of the image ends is in doubt (no length that fits, and no `EI`
    /// that the content after it confirms), so what follows may have been read wrongly.
    pub inline_unsure: bool,
    /// An operand is [`Operand::Bad`] (also inside an array).
    pub bad: bool,
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

/// The bytes of a name with its `#xx` escapes undone; `None` when a `#` is not followed by two
/// hex digits.
fn unescape(raw: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(raw.len());
    let mut j = 0;
    while j < raw.len() {
        if raw[j] == b'#' {
            let v = raw
                .get(j + 1..j + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
                .and_then(|h| u8::from_str_radix(h, 16).ok())?;
            out.push(v);
            j += 3;
            continue;
        }
        out.push(raw[j]);
        j += 1;
    }
    Some(out)
}

fn name(raw: &[u8]) -> Operand {
    unescape(raw).map_or(Operand::Bad, Operand::Name)
}

fn is_bad(o: &Operand) -> bool {
    match o {
        Operand::Bad => true,
        Operand::Array(a) => a.iter().any(is_bad),
        _ => false,
    }
}

/// A number as the grammar of PDF has it: an optional sign, digits with at most one point, at
/// least one digit, and nothing else. Poppler's lexer ends a number at a second sign and skips a
/// minus after the point, so it reads `1+19000` as two numbers and `110.-0` as 110; this reads
/// them as what they are, a token that is not a number.
pub fn strict_number(tok: &[u8]) -> Option<f64> {
    let mut i = 0;
    if matches!(tok.first(), Some(b'+' | b'-')) {
        i = 1;
    }
    let digits = |from: usize| {
        tok[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let int = digits(i);
    i += int;
    let mut frac = 0;
    if tok.get(i) == Some(&b'.') {
        i += 1;
        frac = digits(i);
        i += frac;
    }
    if i != tok.len() || int + frac == 0 {
        return None;
    }
    std::str::from_utf8(tok).ok()?.parse::<f64>().ok()
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
    strict_number(tok).map_or(Operand::Bad, Operand::Num)
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
                None => {
                    items.push(Operand::Bad);
                    i += 1;
                }
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
                None => {
                    items.push(Operand::Bad);
                    i += 1;
                }
            },
            b'[' if depth < 8 => {
                let (inner, e) = parse_array(buf, i + 1, depth + 1);
                items.push(Operand::Array(inner));
                i = e;
            }
            b'/' => {
                let e = token_end(buf, i + 1);
                items.push(name(&buf[i + 1..e]));
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
    for_each_op_while(buf, &mut |op| {
        f(op);
        true
    });
}

/// Whether some inline image of the content has an end that is in doubt (see
/// [`Op::inline_unsure`]).
pub fn has_unsure_inline_image(buf: &[u8]) -> bool {
    if !buf.windows(2).any(|w| w == b"BI") {
        return false;
    }
    let mut unsure = false;
    for_each_op(buf, &mut |op| unsure |= op.inline_unsure);
    unsure
}

/// Whether some operand of the content is a token that a strict reader refuses (see
/// [`Operand::Bad`]), or an operator of the specification has operands other than the ones that the
/// specification gives it (see [`crate::profile`]): readers differ on those.
pub fn has_bad_operand(buf: &[u8]) -> bool {
    let mut bad = false;
    for_each_op(buf, &mut |op| {
        bad |= op.bad || crate::profile::operand_problem(op).is_some();
    });
    bad
}

/// Calls `f` for each operator of the content stream `buf` until it returns `false`.
pub fn for_each_op_while<'a>(buf: &'a [u8], f: &mut dyn FnMut(&Op<'a>) -> bool) {
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
                None => {
                    first.get_or_insert(start);
                    args.push(Operand::Bad);
                    i += 1;
                }
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
                None => {
                    first.get_or_insert(start);
                    args.push(Operand::Bad);
                    i += 1;
                }
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
                args.push(name(&buf[i + 1..e]));
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
                let go = if tok == b"BI" {
                    let img = inline_image(buf, i);
                    i = img.after;
                    f(&Op {
                        name: tok,
                        args: Vec::new(),
                        offset,
                        inline: Some((img.dict, img.data)),
                        inline_unsure: img.unsure,
                        bad: false,
                    })
                } else {
                    let args_bad = args.iter().any(is_bad);
                    f(&Op {
                        name: tok,
                        args: std::mem::take(&mut args),
                        offset,
                        inline: None,
                        inline_unsure: false,
                        bad: args_bad,
                    })
                };
                if !go {
                    return;
                }
                args.clear();
                first = None;
            }
        }
    }
}

/// An inline image as the tokenizer found it.
struct InlineImage<'a> {
    dict: &'a [u8],
    data: &'a [u8],
    /// The offset after `EI`.
    after: usize,
    unsure: bool,
}

/// The integer after `/L` or `/Length` in the dictionary text of an inline image.
fn declared_length(dict: &[u8]) -> Option<usize> {
    let mut toks = dict.split(|b| is_space(*b)).filter(|t| !t.is_empty());
    while let Some(t) = toks.next() {
        if t == b"/L" || t == b"/Length" {
            return std::str::from_utf8(toks.next()?).ok()?.parse().ok();
        }
    }
    None
}

/// The number of bytes of the data of an unfiltered inline image, from its dictionary.
fn computed_length(dict: &[u8]) -> Option<usize> {
    use crate::pdfobj::{parse, Obj};
    let text = [b"<< ".as_slice(), dict, b" >>"].concat();
    let (d, _) = parse(&text, 0)?;
    let get = |a: &str, b: &str| d.get(a).or_else(|| d.get(b));
    if get("F", "Filter").is_some() {
        return None;
    }
    let num = |o: Option<&Obj>| {
        o.and_then(Obj::int)
            .filter(|v| *v > 0 && *v < 1_000_000_000)
    };
    let w = usize::try_from(num(get("W", "Width"))?).ok()?;
    let h = usize::try_from(num(get("H", "Height"))?).ok()?;
    let mask = matches!(get("IM", "ImageMask"), Some(Obj::Bool(true)));
    let (ncomp, bits) = if mask {
        (1, 1)
    } else {
        let ncomp = match get("CS", "ColorSpace")? {
            Obj::Name(n) => match n.as_str() {
                "G" | "DeviceGray" | "CalGray" => 1,
                "RGB" | "DeviceRGB" | "CalRGB" => 3,
                "CMYK" | "DeviceCMYK" => 4,
                _ => return None,
            },
            Obj::Array(a) => match a.first()?.name()? {
                "I" | "Indexed" => 1,
                _ => return None,
            },
            _ => return None,
        };
        let bits = usize::try_from(num(get("BPC", "BitsPerComponent"))?).ok()?;
        (ncomp, bits)
    };
    let row = w.checked_mul(ncomp)?.checked_mul(bits)?.div_ceil(8);
    row.checked_mul(h)
}

/// Whether the bytes after an `EI` read as content: operands and then a known operator, and no
/// binary bytes.
fn reads_as_content(buf: &[u8], mut i: usize) -> bool {
    let n = buf.len();
    for _ in 0..8 {
        while i < n && is_space(buf[i]) {
            i += 1;
        }
        if i >= n {
            return true;
        }
        let c = buf[i];
        match c {
            b'%' => {
                while i < n && buf[i] != b'\n' && buf[i] != b'\r' {
                    i += 1;
                }
            }
            b'(' => match parse_literal(buf, i) {
                Some((_, e, _)) => i = e,
                None => return false,
            },
            b'<' if buf.get(i + 1) == Some(&b'<') => i = skip_dict(buf, i),
            b'<' => match parse_hex(buf, i) {
                Some((_, e)) => i = e,
                None => return false,
            },
            b'[' => i = parse_array(buf, i + 1, 0).1,
            b'/' => i = token_end(buf, i + 1).max(i + 1),
            _ if is_delim(c) => return false,
            _ => {
                let e = token_end(buf, i);
                let tok = &buf[i..e];
                if tok.iter().any(|b| !(0x21..0x7F).contains(b)) {
                    return false;
                }
                if matches!(tok[0], b'0'..=b'9' | b'+' | b'-' | b'.') {
                    if std::str::from_utf8(tok)
                        .ok()
                        .and_then(|t| t.parse::<f64>().ok())
                        .is_none()
                    {
                        return false;
                    }
                } else {
                    // The first operator decides.
                    return crate::roles::is_operator(tok) && tok != b"EI";
                }
                i = e.max(i + 1);
            }
        }
    }
    true
}

/// Whether an `EI` that ends the data is at `p`: white space (or the start of the data) before it,
/// and white space or the end after it.
fn ei_at(buf: &[u8], p: usize, data_start: usize) -> bool {
    buf.get(p) == Some(&b'E')
        && buf.get(p + 1) == Some(&b'I')
        && (p <= data_start || is_space(buf[p - 1]))
        && buf.get(p + 2).is_none_or(|c| is_space(*c))
}

/// The dictionary text, the data and the offset after `EI` of an inline image; `i` is just after
/// `BI`. The end of the data is taken from `/L` or `/Length` when the dictionary has one, from the
/// size of the samples when the image has no filter, and otherwise it is the first `EI` that is
/// followed by white space and content that reads as operators, as Poppler finds it. When none of
/// these gives an end the image is `unsure`: the rest is read from the first `EI` between white
/// space, or to the end.
fn inline_image(buf: &[u8], i: usize) -> InlineImage<'_> {
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
        return InlineImage {
            dict: &buf[i..n],
            data: &[],
            after: n,
            unsure: true,
        };
    }
    let dict = &buf[i..dict_end];
    // The data starts after one white-space byte; a carriage return and a line feed may be two.
    let starts = if buf[j - 1] == b'\r' && buf.get(j) == Some(&b'\n') {
        [j, j + 1]
    } else {
        [j, j]
    };
    let declared = declared_length(dict).or_else(|| computed_length(dict));
    if let Some(len) = declared {
        for start in starts {
            let end = start.saturating_add(len);
            // A little white space between the data and EI is allowed.
            if let Some(q) = (end..=end.saturating_add(2)).find(|q| ei_at(buf, *q, end)) {
                return InlineImage {
                    dict,
                    data: &buf[start..end],
                    after: q + 2,
                    unsure: false,
                };
            }
        }
    }
    let data_start = starts[1];
    let mut first_plain: Option<usize> = None;
    for p in data_start..n.saturating_sub(1) {
        if ei_at(buf, p, data_start) {
            first_plain.get_or_insert(p);
            if reads_as_content(buf, p + 2) {
                let end = if p > data_start { p - 1 } else { p };
                return InlineImage {
                    dict,
                    data: &buf[data_start..end],
                    after: p + 2,
                    // A length was declared or computed, and the data does not end there.
                    unsure: declared.is_some(),
                };
            }
        }
    }
    match first_plain {
        Some(p) => InlineImage {
            dict,
            data: &buf[data_start..if p > data_start { p - 1 } else { p }],
            after: p + 2,
            unsure: true,
        },
        None => InlineImage {
            dict,
            data: &buf[data_start..],
            after: n,
            unsure: true,
        },
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShownItem {
    Str(Vec<u8>),
    Kern(f64),
}

/// The strings and kerning offsets that one showing operator shows. An operator with more
/// operands than it takes shows what its last operand says, as Poppler reads it (and the walk
/// refuses such an operator: see [`crate::profile`]).
pub fn shown(op: &Op) -> Option<Vec<ShownItem>> {
    match op.name {
        b"Tj" | b"'" | b"\"" => op.args.iter().rev().find_map(|a| match a {
            Operand::Str(s) => Some(vec![ShownItem::Str(s.clone())]),
            _ => None,
        }),
        b"TJ" => op.args.iter().rev().find_map(|a| match a {
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

    fn inline_of(buf: &[u8]) -> Vec<(Vec<u8>, bool, Vec<u8>)> {
        let mut out = Vec::new();
        for_each_op(buf, &mut |op| {
            if let Some((_, data)) = op.inline {
                out.push((data.to_vec(), op.inline_unsure, op.name.to_vec()));
            } else {
                out.push((Vec::new(), false, op.name.to_vec()));
            }
        });
        out
    }

    #[test]
    fn the_data_of_an_inline_image_ends_where_its_size_says() {
        // 64 bytes of samples that hold ` EI (`.
        let mut data = b"\x80 EI (".to_vec();
        data.extend(std::iter::repeat_n(0x80u8, 58));
        let buf = [
            b"BI /W 64 /H 1 /BPC 8 /CS /G ID ".as_slice(),
            &data,
            b"\nEI Q 0 g",
        ]
        .concat();
        let ops = inline_of(&buf);
        assert_eq!(ops[0].0, data);
        assert!(!ops[0].1);
        let names: Vec<_> = ops.iter().map(|o| o.2.clone()).collect();
        assert_eq!(names, [b"BI".to_vec(), b"Q".to_vec(), b"g".to_vec()]);
    }

    #[test]
    fn a_declared_length_is_used_and_checked() {
        let buf = b"BI /W 1 /H 1 /F /AHx /L 8 ID 00 EI 0> EI Q";
        let ops = inline_of(buf);
        assert_eq!(ops[0].0, b"00 EI 0>");
        assert!(!ops[0].1);
        // A length that does not fit: the first EI that content follows is taken, and the image
        // is in doubt.
        let buf = b"BI /W 1 /H 1 /F /AHx /L 40 ID 00> EI Q";
        let ops = inline_of(buf);
        assert!(ops[0].1);
        assert_eq!(ops[1].2, b"Q");
    }

    #[test]
    fn without_a_length_the_ei_must_be_followed_by_content() {
        // The first EI is data (a binary byte follows); the second is followed by an operator.
        let buf = b"BI /W 1 /H 1 /F /Fl ID ab EI \x01\x02 cd EI Q 0 g";
        let ops = inline_of(buf);
        assert_eq!(ops[0].0, b"ab EI \x01\x02 cd");
        assert!(!ops[0].1);
        // No EI that is confirmed, none at all: the end is in doubt.
        let ops = inline_of(b"BI /W 1 /H 1 /F /Fl ID abcdef");
        assert!(ops[0].1);
        assert!(has_unsure_inline_image(b"BI /W 1 /H 1 /F /Fl ID abcdef"));
        assert!(!has_unsure_inline_image(b"BI /W 1 /H 1 /F /Fl ID abc EI Q"));
    }

    #[test]
    fn a_number_is_a_number_as_the_grammar_has_it() {
        for ok in [
            "0", "12", "+5", "-5", "5.", ".5", "-.5", "+.5", "0.25", "007",
        ] {
            assert!(strict_number(ok.as_bytes()).is_some(), "{ok}");
        }
        assert_eq!(strict_number(b"-.5"), Some(-0.5));
        // Poppler reads the first as two numbers and the second as 110.
        for bad in [
            "1+19000", "110.-0", "--1", "1e5", "+", "-", ".", "1.2.3", "0x10", "+nan", "inf", "1_0",
        ] {
            assert!(strict_number(bad.as_bytes()).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_token_that_does_not_parse_cleanly_marks_the_operator() {
        let bad = |s: &[u8]| has_bad_operand(s);
        assert!(bad(b"BT [(X) 1+19000 (Y)] TJ ET"));
        assert!(bad(b"BT 110.-0 Ts ET"));
        assert!(bad(b"BT /F#1 12 Tf ET"));
        assert!(bad(b"BT /F#zz 12 Tf ET"));
        assert!(bad(b"BT (never ends Tj ET"));
        assert!(bad(b"BT <00zz> Tj ET"));
        assert!(!bad(
            br"BT /F#31 12 Tf [(X) -20.5 (Y)] TJ (a\)b) Tj <0041> Tj ET"
        ));
        // An operator that is not known is no operand.
        assert!(!bad(b"BX /Foo 1 2 zork EX"));
    }
}
