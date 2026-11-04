//! The small part of PDF syntax the form designer reads and writes.
//!
//! The designer reads the bytes that PDFium wrote when it saved the document, not the file the user
//! opened, so they have the plain shape PDFium gives them. They still came from a hostile file, so
//! every reader here works on a slice, never goes past its end, never recurses deeper than
//! [`MAX_DEPTH`], and answers `None` for what it cannot make sense of.

/// Deepest nesting of arrays and dictionaries the readers follow.
pub const MAX_DEPTH: usize = 48;

pub fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\n' | b'\r' | b'\t' | 0 | 12)
}

pub fn is_delim(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// The first place at or after `i` that is not white space or a comment.
pub fn skip_ws(s: &[u8], mut i: usize) -> usize {
    while i < s.len() {
        if is_ws(s[i]) {
            i += 1;
        } else if s[i] == b'%' {
            while i < s.len() && s[i] != b'\n' && s[i] != b'\r' {
                i += 1;
            }
        } else {
            break;
        }
    }
    i
}

/// The end of the run of ordinary characters that starts at `i`.
pub fn token_end(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && !is_ws(s[i]) && !is_delim(s[i]) {
        i += 1;
    }
    i
}

/// The end of the value that starts at the first non-white-space place at or after `i`.
pub fn value_end(s: &[u8], i: usize) -> Option<usize> {
    value_end_at(s, i, 0)
}

fn value_end_at(s: &[u8], i: usize, depth: usize) -> Option<usize> {
    if depth > MAX_DEPTH {
        return None;
    }
    let i = skip_ws(s, i);
    let first = *s.get(i)?;
    match first {
        b'(' => {
            let mut nesting = 0usize;
            let mut j = i;
            while j < s.len() {
                match s[j] {
                    b'\\' => j += 1,
                    b'(' => nesting += 1,
                    b')' => {
                        nesting -= 1;
                        if nesting == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            None
        }
        b'<' if s.get(i + 1) == Some(&b'<') => {
            let mut j = i + 2;
            loop {
                j = skip_ws(s, j);
                match *s.get(j)? {
                    b'>' => {
                        return (s.get(j + 1) == Some(&b'>')).then_some(j + 2);
                    }
                    b'/' => {}
                    _ => return None,
                }
                j = value_end_at(s, j, depth + 1)?;
                j = value_end_at(s, j, depth + 1)?;
            }
        }
        b'<' => {
            let mut j = i + 1;
            while j < s.len() {
                match s[j] {
                    b'>' => return Some(j + 1),
                    c if c.is_ascii_hexdigit() || is_ws(c) => j += 1,
                    _ => return None,
                }
            }
            None
        }
        b'[' => {
            let mut j = i + 1;
            loop {
                j = skip_ws(s, j);
                if *s.get(j)? == b']' {
                    return Some(j + 1);
                }
                j = value_end_at(s, j, depth + 1)?;
            }
        }
        b'/' => Some(token_end(s, i + 1)),
        b')' | b'>' | b']' | b'{' | b'}' => None,
        _ => {
            let e = token_end(s, i);
            if e == i {
                return None;
            }
            // "n g R"
            if s[i..e].iter().all(u8::is_ascii_digit) {
                let j = skip_ws(s, e);
                let e2 = token_end(s, j);
                if e2 > j && s[j..e2].iter().all(u8::is_ascii_digit) {
                    let k = skip_ws(s, e2);
                    if s.get(k) == Some(&b'R') && token_end(s, k) == k + 1 {
                        return Some(k + 1);
                    }
                }
            }
            Some(e)
        }
    }
}

/// One entry of a dictionary: the decoded key and where the key and the value sit.
#[derive(Debug, Clone)]
pub struct Entry {
    pub key: Vec<u8>,
    /// The place of the `/` of the key.
    pub key_start: usize,
    pub value_start: usize,
    pub value_end: usize,
}

/// The entries of the dictionary that `d` starts with. `None` when it is not one or is damaged.
pub fn entries(d: &[u8]) -> Option<Vec<Entry>> {
    let mut i = skip_ws(d, 0);
    if !d[i..].starts_with(b"<<") {
        return None;
    }
    i += 2;
    let mut out = Vec::new();
    loop {
        i = skip_ws(d, i);
        match *d.get(i)? {
            b'>' => return Some(out),
            b'/' => {}
            _ => return None,
        }
        let key_end = token_end(d, i + 1);
        let value_start = skip_ws(d, key_end);
        let value_end = value_end(d, value_start)?;
        out.push(Entry {
            key: decode_name(&d[i + 1..key_end]),
            key_start: i,
            value_start,
            value_end,
        });
        i = value_end;
    }
}

/// The text of the value of `key` in the dictionary `d`.
pub fn get<'a>(d: &'a [u8], key: &str) -> Option<&'a [u8]> {
    entries(d)?
        .into_iter()
        .find(|e| e.key == key.as_bytes())
        .map(|e| &d[e.value_start..e.value_end])
}

/// The dictionary with `key` set to `value` (replaced, or added before the closing `>>`).
pub fn set(d: &[u8], key: &str, value: &[u8]) -> Option<Vec<u8>> {
    let ents = entries(d)?;
    if let Some(e) = ents.iter().find(|e| e.key == key.as_bytes()) {
        let mut out = d[..e.value_start].to_vec();
        out.extend_from_slice(value);
        out.extend_from_slice(&d[e.value_end..]);
        return Some(out);
    }
    let start = skip_ws(d, 0);
    let end = value_end(d, start)?;
    // The last two characters are the closing `>>`.
    let close = end.checked_sub(2)?;
    let mut out = d[..close].to_vec();
    out.extend_from_slice(b" /");
    out.extend_from_slice(encode_name(key.as_bytes()).as_bytes());
    out.push(b' ');
    out.extend_from_slice(value);
    out.push(b' ');
    out.extend_from_slice(&d[close..]);
    Some(out)
}

/// The dictionary without `key`.
pub fn remove(d: &[u8], key: &str) -> Option<Vec<u8>> {
    let ents = entries(d)?;
    match ents.iter().find(|e| e.key == key.as_bytes()) {
        Some(e) => {
            let mut out = d[..e.key_start].to_vec();
            out.push(b' ');
            out.extend_from_slice(&d[e.value_end..]);
            Some(out)
        }
        None => Some(d.to_vec()),
    }
}

/// The text of the items of the array that `v` is.
pub fn array_items(v: &[u8]) -> Option<Vec<&[u8]>> {
    let mut i = skip_ws(v, 0);
    if v.get(i) != Some(&b'[') {
        return None;
    }
    i += 1;
    let mut out = Vec::new();
    loop {
        i = skip_ws(v, i);
        if *v.get(i)? == b']' {
            return Some(out);
        }
        let end = value_end(v, i)?;
        out.push(&v[i..end]);
        i = end;
    }
}

/// An array made of items as they stand in the file. The items are bytes and stay bytes: a string
/// in a dictionary that is written in place may hold any byte, and none of it may be read as text.
pub fn array_bytes(items: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + items.iter().map(|i| i.len() + 1).sum::<usize>());
    out.push(b'[');
    for item in items {
        out.extend_from_slice(item);
        out.push(b' ');
    }
    out.push(b']');
    out
}

/// `n g R` as `(n, g)`.
pub fn parse_ref(v: &[u8]) -> Option<(u32, u16)> {
    let t = std::str::from_utf8(v).ok()?;
    let mut it = t.split_ascii_whitespace();
    let n = it.next()?.parse().ok()?;
    let g = it.next()?.parse().ok()?;
    (it.next()? == "R" && it.next().is_none()).then_some((n, g))
}

pub fn parse_int(v: &[u8]) -> Option<i64> {
    std::str::from_utf8(v).ok()?.trim().parse().ok()
}

pub fn parse_f32(v: &[u8]) -> Option<f32> {
    let x: f32 = std::str::from_utf8(v).ok()?.trim().parse().ok()?;
    x.is_finite().then_some(x)
}

/// Four numbers in an array.
pub fn parse_box(v: &[u8]) -> Option<[f32; 4]> {
    let items = array_items(v)?;
    if items.len() != 4 {
        return None;
    }
    let mut out = [0.0f32; 4];
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = parse_f32(item)?;
    }
    Some(out)
}

/// The name that `v` is, decoded (`#20` and the like), without the slash.
pub fn parse_name(v: &[u8]) -> Option<Vec<u8>> {
    let v = &v[skip_ws(v, 0)..];
    (v.first() == Some(&b'/')).then(|| decode_name(&v[1..]))
}

pub fn decode_name(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'#' && i + 3 <= raw.len() {
            let hex = std::str::from_utf8(&raw[i + 1..i + 3]).ok();
            if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(raw[i]);
        i += 1;
    }
    out
}

/// A name as it is written after the slash: anything but plain characters as `#` and two digits.
pub fn encode_name(raw: &[u8]) -> String {
    let mut out = String::new();
    for &b in raw {
        if (0x21..=0x7e).contains(&b) && !is_delim(b) && b != b'#' {
            out.push(b as char);
        } else {
            out.push_str(&format!("#{b:02X}"));
        }
    }
    out
}

/// The text of a string value (literal or hex) as characters. A string that starts with the
/// byte order mark of UTF-16 is read as that, one that starts with that of UTF-8 as that, and any
/// other as Latin-1, which is PDFDocEncoding for the letters a name or a tooltip is made of.
pub fn decode_text(v: &[u8]) -> Option<String> {
    let bytes = string_bytes(v)?;
    if bytes.starts_with(&[0xfe, 0xff]) {
        let units: Vec<u16> = bytes[2..]
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return Some(String::from_utf16_lossy(&units));
    }
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Some(String::from_utf8_lossy(&bytes[3..]).into_owned());
    }
    Some(bytes.iter().map(|&b| b as char).collect())
}

/// The bytes a literal or hex string stands for.
pub fn string_bytes(v: &[u8]) -> Option<Vec<u8>> {
    let v = &v[skip_ws(v, 0)..];
    match *v.first()? {
        b'(' => {
            let end = value_end(v, 0)?;
            let inner = &v[1..end - 1];
            let mut out = Vec::with_capacity(inner.len());
            let mut i = 0;
            while i < inner.len() {
                let c = inner[i];
                if c != b'\\' {
                    out.push(c);
                    i += 1;
                    continue;
                }
                i += 1;
                let Some(&e) = inner.get(i) else { break };
                i += 1;
                match e {
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'0'..=b'7' => {
                        let mut n = u32::from(e - b'0');
                        let mut digits = 1;
                        while digits < 3 && i < inner.len() && (b'0'..=b'7').contains(&inner[i]) {
                            n = n * 8 + u32::from(inner[i] - b'0');
                            i += 1;
                            digits += 1;
                        }
                        out.push((n & 0xff) as u8);
                    }
                    b'\r' => {
                        if inner.get(i) == Some(&b'\n') {
                            i += 1;
                        }
                    }
                    b'\n' => {}
                    other => out.push(other),
                }
            }
            Some(out)
        }
        b'<' if v.get(1) != Some(&b'<') => {
            let end = value_end(v, 0)?;
            let digits: Vec<u8> = v[1..end - 1]
                .iter()
                .copied()
                .filter(|b| !is_ws(*b))
                .collect();
            let mut out = Vec::with_capacity(digits.len() / 2 + 1);
            for pair in digits.chunks(2) {
                let hi = (pair[0] as char).to_digit(16)?;
                let lo = pair.get(1).map_or(Some(0), |b| (*b as char).to_digit(16))?;
                out.push((hi * 16 + lo) as u8);
            }
            Some(out)
        }
        _ => None,
    }
}

/// A string value for `s`: a literal in plain ASCII, otherwise UTF-16 in hex with the byte order mark.
pub fn encode_text(s: &str) -> String {
    if s.chars().all(|c| (' '..='~').contains(&c)) {
        let mut out = String::from("(");
        for c in s.chars() {
            if matches!(c, '(' | ')' | '\\') {
                out.push('\\');
            }
            out.push(c);
        }
        out.push(')');
        out
    } else {
        let mut out = String::from("<FEFF");
        for unit in s.encode_utf16() {
            out.push_str(&format!("{unit:04X}"));
        }
        out.push('>');
        out
    }
}

/// A number the way a page description writes it: at most three decimals, none when whole.
pub fn num(v: f32) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" || s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// Four numbers as an array.
pub fn num_box(b: &[f32; 4]) -> String {
    format!("[{} {} {} {}]", num(b[0]), num(b[1]), num(b[2]), num(b[3]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_end_where_they_should() {
        assert_eq!(value_end(b"(a(b)c\\)) x", 0), Some(9));
        assert_eq!(value_end(b"<< /A [1 2 (x)] /B <</C 1>> >> y", 0), Some(30));
        assert_eq!(value_end(b"12 0 R x", 0), Some(6));
        assert_eq!(value_end(b"12 0 x", 0), Some(2));
        assert_eq!(value_end(b"/Name#20x y", 0), Some(9));
        assert_eq!(value_end(b"(open", 0), None);
        assert_eq!(value_end(b"<< /A", 0), None);
        assert_eq!(value_end(b"<< 1 2 >>", 0), None);
        assert_eq!(value_end(b"]", 0), None);
    }

    #[test]
    fn nesting_is_limited() {
        let deep = vec![b'['; MAX_DEPTH + 10];
        assert_eq!(value_end(&deep, 0), None);
        let mut ok = vec![b'['; 10];
        ok.extend(vec![b']'; 10]);
        assert_eq!(value_end(&ok, 0), Some(20));
        // An unterminated string does not loop or panic.
        let mut s = b"(".to_vec();
        s.extend(std::iter::repeat_n(b'(', 5000));
        assert_eq!(value_end(&s, 0), None);
        // Closing more than it opened does not underflow.
        assert_eq!(value_end(b")", 0), None);
    }

    #[test]
    fn dictionaries_are_read_and_changed() {
        let d = b"<< /A 1 /B (two) /C [3 0 R] /D << /E 5 >> >>";
        assert_eq!(get(d, "A"), Some(&b"1"[..]));
        assert_eq!(get(d, "B"), Some(&b"(two)"[..]));
        assert_eq!(get(d, "E"), None);
        let set_a = set(d, "A", b"9").unwrap();
        assert_eq!(get(&set_a, "A"), Some(&b"9"[..]));
        assert_eq!(get(&set_a, "B"), Some(&b"(two)"[..]));
        let added = set(d, "Z", b"/Yes").unwrap();
        assert_eq!(get(&added, "Z"), Some(&b"/Yes"[..]));
        assert_eq!(get(&added, "D"), Some(&b"<< /E 5 >>"[..]));
        let gone = remove(d, "C").unwrap();
        assert_eq!(get(&gone, "C"), None);
        assert_eq!(get(&gone, "B"), Some(&b"(two)"[..]));
        assert!(entries(&gone).is_some());
        assert!(entries(b"[1 2]").is_none());
    }

    #[test]
    fn strings_round_trip() {
        assert_eq!(decode_text(b"(plain)").unwrap(), "plain");
        assert_eq!(
            decode_text(b"(a\\(b\\)\\\\c\\101\\n)").unwrap(),
            "a(b)\\cA\n"
        );
        assert_eq!(decode_text(b"<48656C6C6F>").unwrap(), "Hello");
        assert_eq!(
            decode_text(b"<FEFF00E600F800E5>").unwrap(),
            "\u{e6}\u{f8}\u{e5}"
        );
        for s in [
            "",
            "a (b) \\ c",
            "Bl\u{e5}b\u{e6}r",
            "Navn_\u{e6}\u{f8}\u{e5}_\u{65e5}\u{672c}",
        ] {
            assert_eq!(decode_text(encode_text(s).as_bytes()).unwrap(), s);
        }
    }

    #[test]
    fn an_array_of_items_keeps_every_byte_of_them() {
        // Items that no text holds: single bytes above 127, UTF-16 with its byte order mark,
        // a zero byte, and a dictionary written in place with such strings in it.
        let items: Vec<Vec<u8>> = vec![
            b"12 0 R".to_vec(),
            b"(Bl\xE5b\xE6r)".to_vec(),
            b"(\xFE\xFF\x00\xC6\x03\xA9)".to_vec(),
            b"<< /T (\x80\xFF\x00\xC3) /C [1 0 0] >>".to_vec(),
            b"<FEFF00C6>".to_vec(),
        ];
        let written = array_bytes(&items);
        let back: Vec<Vec<u8>> = array_items(&written)
            .unwrap()
            .into_iter()
            .map(<[u8]>::to_vec)
            .collect();
        assert_eq!(back, items);
        assert_eq!(array_bytes(&[]), b"[]");
        // Arbitrary bytes, as long as they are strings, come back the same.
        for byte in 0..=255u8 {
            if matches!(byte, b'(' | b')' | b'\\') {
                continue;
            }
            let item = vec![b'(', byte, byte, b')'];
            let written = array_bytes(std::slice::from_ref(&item));
            let back = array_items(&written).unwrap();
            assert_eq!(back, vec![item.as_slice()], "byte {byte}");
        }
    }

    #[test]
    fn names_round_trip() {
        for raw in [&b"Yes"[..], b"A B", b"x#y", b"(z)", "\u{e6}".as_bytes()] {
            let written = encode_name(raw);
            assert_eq!(decode_name(written.as_bytes()), raw);
            assert!(!written.contains(' '));
        }
        assert_eq!(decode_name(b"A#20B"), b"A B");
        assert_eq!(decode_name(b"A#2"), b"A#2");
        assert_eq!(decode_name(b"#"), b"#");
    }

    #[test]
    fn numbers_are_short() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(0.5), "0.5");
        assert_eq!(num(-0.0004), "0");
        assert_eq!(num(12.34567), "12.346");
        assert_eq!(num(f32::NAN), "0");
        assert_eq!(parse_box(b"[1 2.5 3 4]"), Some([1.0, 2.5, 3.0, 4.0]));
        assert_eq!(parse_box(b"[1 2 3]"), None);
        assert_eq!(parse_ref(b"12 3 R"), Some((12, 3)));
        assert_eq!(parse_ref(b"12 R"), None);
    }
}
