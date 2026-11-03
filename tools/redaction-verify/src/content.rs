//! A forgiving reader for page content streams, only to find text that is drawn with several
//! operators. `[(SEC) -20 (RET)] TJ` and `(SEC) Tj (RET) Tj` show the word SECRET, but the bytes
//! SECRET are nowhere. The reader joins the strings of one text object (BT .. ET).

use crate::strings::{is_delim, is_space, parse_hex, parse_literal};

/// The strings of one text object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextObject {
    /// Offset of the first showing operator's operands in the buffer.
    pub offset: usize,
    /// All strings, one after the other.
    pub joined: Vec<u8>,
    /// The strings of each showing operator, a space between the operators.
    pub spaced: Vec<u8>,
}

#[derive(Default)]
struct State {
    groups: Vec<Vec<u8>>,
    strings: usize,
    start: Option<usize>,
    out: Vec<TextObject>,
}

impl State {
    fn flush(&mut self) {
        if self.strings >= 2 {
            let joined: Vec<u8> = self.groups.concat();
            let spaced: Vec<u8> = self.groups.join(&b' ');
            self.out.push(TextObject {
                offset: self.start.unwrap_or(0),
                joined,
                spaced,
            });
        }
        self.groups.clear();
        self.strings = 0;
        self.start = None;
    }
}

/// Text objects of a content stream that show two or more strings. An object with one string is
/// left out: the plain search of the buffer sees it.
pub fn text_objects(buf: &[u8]) -> Vec<TextObject> {
    let n = buf.len();
    let mut st = State::default();
    let mut pending: Vec<Vec<u8>> = Vec::new();
    let mut pending_start: Option<usize> = None;
    let mut i = 0;
    while i < n {
        let c = buf[i];
        match c {
            _ if is_space(c) => i += 1,
            b'%' => {
                while i < n && buf[i] != b'\n' && buf[i] != b'\r' {
                    i += 1;
                }
            }
            b'(' => {
                if let Some((bytes, end, _)) = parse_literal(buf, i) {
                    pending_start.get_or_insert(i);
                    pending.push(bytes);
                    i = end;
                } else {
                    i += 1;
                }
            }
            b'<' => {
                if buf.get(i + 1) == Some(&b'<') {
                    i += 2;
                } else if let Some((bytes, end)) = parse_hex(buf, i) {
                    pending_start.get_or_insert(i);
                    pending.push(bytes);
                    i = end;
                } else {
                    i += 1;
                }
            }
            b'/' => {
                i += 1;
                while i < n && !is_space(buf[i]) && !is_delim(buf[i]) {
                    i += 1;
                }
            }
            _ if is_delim(c) => i += 1,
            _ => {
                let s = i;
                while i < n && !is_space(buf[i]) && !is_delim(buf[i]) {
                    i += 1;
                }
                let tok = &buf[s..i];
                if matches!(tok[0], b'0'..=b'9' | b'+' | b'-' | b'.') {
                    continue;
                }
                match tok {
                    b"BT" | b"ET" => {
                        st.flush();
                        pending.clear();
                        pending_start = None;
                    }
                    b"Tj" | b"TJ" | b"'" | b"\"" => {
                        if !pending.is_empty() {
                            st.start = st.start.or(pending_start);
                            st.strings += pending.len();
                            st.groups.push(pending.concat());
                        }
                        pending.clear();
                        pending_start = None;
                    }
                    b"BI" => {
                        pending.clear();
                        pending_start = None;
                        i = skip_inline_image(buf, i);
                    }
                    _ => {
                        pending.clear();
                        pending_start = None;
                    }
                }
            }
        }
    }
    st.flush();
    st.out
}

/// Moves past `ID <data> EI` of an inline image; `i` is just after `BI`.
fn skip_inline_image(buf: &[u8], i: usize) -> usize {
    let n = buf.len();
    let mut j = i;
    // The dictionary ends with ID followed by one white-space byte.
    while j + 2 < n {
        if buf[j] == b'I' && buf[j + 1] == b'D' && is_space(buf[j - 1]) && is_space(buf[j + 2]) {
            j += 3;
            break;
        }
        j += 1;
    }
    while j + 1 < n {
        if buf[j] == b'E'
            && buf[j + 1] == b'I'
            && is_space(buf[j - 1])
            && (j + 2 >= n || is_space(buf[j + 2]))
        {
            return j + 2;
        }
        j += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_a_tj_array_and_two_tj_operators() {
        let objs =
            text_objects(b"BT /F1 12 Tf [(SEC) -20 (RET)] TJ ET BT (JO) Tj 0 -14 Td (HN) Tj ET");
        assert_eq!(objs.len(), 2);
        assert_eq!(objs[0].joined, b"SECRET");
        assert_eq!(objs[1].joined, b"JOHN");
        assert_eq!(objs[1].spaced, b"JO HN");
    }

    #[test]
    fn hex_strings_count_and_a_single_string_is_left_out() {
        let objs = text_objects(b"BT <0053> Tj <0045> Tj ET BT (one) Tj ET");
        assert_eq!(objs.len(), 1);
        assert_eq!(objs[0].joined, [0, 0x53, 0, 0x45]);
    }

    #[test]
    fn inline_image_data_is_skipped() {
        let objs = text_objects(b"BT (A) Tj BI /W 1 /H 1 ID (x) Tj EI (B) Tj ET");
        assert_eq!(objs.len(), 1);
        assert_eq!(objs[0].joined, b"AB");
    }
}
