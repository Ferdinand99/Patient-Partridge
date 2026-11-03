//! Reading PDF string syntax out of a byte buffer: literal strings `( .. )` with escapes and
//! hex strings `< .. >`. The bytes of the decoded string are what a PDF reader would see, so a
//! secret written as `<53454352 4554>` or `(SEC\122ET)` is found by comparing the decoded bytes.

/// A string found in a buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfString {
    /// Offset of the opening `(` or `<`.
    pub start: usize,
    /// Offset just after the closing `)` or `>`.
    pub end: usize,
    pub bytes: Vec<u8>,
    pub kind: StringKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringKind {
    /// A literal string that has no escape: its bytes are in the buffer as they are.
    Literal,
    /// A literal string with at least one backslash escape.
    Escaped,
    Hex,
}

/// The longest string that is read. A `(` in binary data that never closes must not make the scan
/// quadratic.
const MAX_STRING: usize = 64 * 1024;

pub fn is_space(c: u8) -> bool {
    matches!(c, 0 | 9 | 10 | 12 | 13 | 32)
}

pub fn is_delim(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Parses a literal string that starts at `buf[i] == b'('`. Returns the decoded bytes, the offset
/// after the closing parenthesis, and whether an escape was used.
pub fn parse_literal(buf: &[u8], i: usize) -> Option<(Vec<u8>, usize, bool)> {
    debug_assert_eq!(buf.get(i), Some(&b'('));
    let mut out = Vec::new();
    let mut depth = 1usize;
    let mut j = i + 1;
    let mut escaped = false;
    let limit = (i + MAX_STRING).min(buf.len());
    while j < limit {
        let c = buf[j];
        match c {
            b'\\' => {
                escaped = true;
                j += 1;
                let e = *buf.get(j)?;
                match e {
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'0'..=b'7' => {
                        let mut v = u32::from(e - b'0');
                        let mut digits = 1;
                        while digits < 3 {
                            match buf.get(j + 1) {
                                Some(d @ b'0'..=b'7') => {
                                    v = v * 8 + u32::from(d - b'0');
                                    j += 1;
                                    digits += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push((v & 0xff) as u8);
                    }
                    b'\r' => {
                        // A backslash before an end of line continues the string.
                        if buf.get(j + 1) == Some(&b'\n') {
                            j += 1;
                        }
                    }
                    b'\n' => {}
                    other => out.push(other),
                }
                j += 1;
            }
            b'(' => {
                depth += 1;
                out.push(c);
                j += 1;
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((out, j + 1, escaped));
                }
                out.push(c);
                j += 1;
            }
            _ => {
                out.push(c);
                j += 1;
            }
        }
    }
    None
}

/// Parses a hex string that starts at `buf[i] == b'<'` (not `<<`).
pub fn parse_hex(buf: &[u8], i: usize) -> Option<(Vec<u8>, usize)> {
    debug_assert_eq!(buf.get(i), Some(&b'<'));
    let mut out = Vec::new();
    let mut high: Option<u8> = None;
    let mut j = i + 1;
    let limit = (i + 4 * MAX_STRING).min(buf.len());
    while j < limit {
        let c = buf[j];
        let v = match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        };
        if let Some(v) = v {
            match high.take() {
                Some(h) => out.push(h << 4 | v),
                None => high = Some(v),
            }
        } else if c == b'>' {
            if let Some(h) = high {
                out.push(h << 4);
            }
            return Some((out, j + 1));
        } else if !is_space(c) {
            return None;
        }
        j += 1;
    }
    None
}

/// Every string in `buf`, except inside the `masked` byte ranges (sorted, not overlapping). The
/// scan does not look inside a string it has read.
pub fn extract(buf: &[u8], masked: &[(usize, usize)]) -> Vec<PdfString> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut m = 0;
    while i < buf.len() {
        while m < masked.len() && masked[m].1 <= i {
            m += 1;
        }
        if m < masked.len() && masked[m].0 <= i {
            i = masked[m].1;
            continue;
        }
        match buf[i] {
            b'(' => {
                if let Some((bytes, end, escaped)) = parse_literal(buf, i) {
                    out.push(PdfString {
                        start: i,
                        end,
                        bytes,
                        kind: if escaped {
                            StringKind::Escaped
                        } else {
                            StringKind::Literal
                        },
                    });
                    i = end;
                } else {
                    i += 1;
                }
            }
            b'<' => {
                if buf.get(i + 1) == Some(&b'<') {
                    i += 2;
                } else if let Some((bytes, end)) = parse_hex(buf, i) {
                    out.push(PdfString {
                        start: i,
                        end,
                        bytes,
                        kind: StringKind::Hex,
                    });
                    i = end;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_escapes_and_nesting() {
        let (b, end, esc) = parse_literal(b"(a\\101(b)\\)\\\nz) tail", 0).unwrap();
        assert_eq!(b, b"aA(b))z");
        assert!(esc);
        assert_eq!(end, 15);
    }

    #[test]
    fn hex_with_spaces_and_odd_digit() {
        let (b, end) = parse_hex(b"<53 45\n4>", 0).unwrap();
        assert_eq!(b, [0x53, 0x45, 0x40]);
        assert_eq!(end, 9);
        assert!(parse_hex(b"<53 zz>", 0).is_none());
    }

    #[test]
    fn extract_skips_dict_brackets_and_masks() {
        let buf = b"<</A <41>/B (x)>> (y)";
        let all = extract(buf, &[]);
        assert_eq!(all.len(), 3);
        let masked = extract(buf, &[(0, 14)]);
        assert_eq!(masked.len(), 1);
        assert_eq!(masked[0].bytes, b"y");
    }
}
