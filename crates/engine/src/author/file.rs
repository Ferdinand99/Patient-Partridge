//! The bytes of a document as PDFium saved them, read far enough to find objects.
//!
//! PDFium's save writes the plain shape: a header, the objects in the open, one classic cross
//! reference table and a trailer. That is all this reader understands. A document with a stream for
//! its cross references, with objects packed in streams, with an earlier part (`/Prev`) or with
//! encryption is refused, and so is anything that does not add up. The reader does not follow the
//! file past what it needs and does not trust a number in it for a size.

use std::collections::HashMap;

use super::syntax::{self, skip_ws, token_end, value_end};

/// Most objects a document may have for the designer to work on it.
pub const MAX_OBJECTS: u32 = 4_000_000;

/// Why the designer cannot work on a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unsupported {
    Encrypted,
    Structure(&'static str),
}

impl Unsupported {
    pub fn message(&self) -> String {
        match self {
            Unsupported::Encrypted => {
                "this document is encrypted, so fields cannot be added to it".to_string()
            }
            Unsupported::Structure(why) => format!(
                "the structure of this document is not one the form designer can write into ({why})"
            ),
        }
    }
}

fn odd(why: &'static str) -> Unsupported {
    Unsupported::Structure(why)
}

/// Where an object starts, and its generation.
#[derive(Debug, Clone, Copy)]
struct At {
    offset: usize,
    generation: u16,
}

pub struct Parsed {
    pub bytes: Vec<u8>,
    objects: HashMap<u32, At>,
    pub trailer: Vec<u8>,
    pub xref_pos: usize,
    pub size: u32,
}

fn number(bytes: &[u8], i: &mut usize) -> Option<u64> {
    *i = skip_ws(bytes, *i);
    let e = token_end(bytes, *i);
    if e == *i || e - *i > 19 || !bytes[*i..e].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let n = std::str::from_utf8(&bytes[*i..e]).ok()?.parse().ok()?;
    *i = e;
    Some(n)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - needle.len())
        .rev()
        .find(|&at| &hay[at..at + needle.len()] == needle)
}

/// Reads the cross reference table and the trailer of the bytes PDFium saved.
pub fn parse(bytes: Vec<u8>) -> Result<Parsed, Unsupported> {
    // Only the end of the file is searched for the start of the table.
    let tail_from = bytes.len().saturating_sub(2048);
    let pos = rfind(&bytes[tail_from..], b"startxref")
        .map(|p| p + tail_from)
        .ok_or(odd("no startxref"))?;
    let mut i = pos + 9;
    let xref_pos =
        number(&bytes, &mut i).ok_or(odd("a start of the table that is not a number"))? as usize;
    if xref_pos >= bytes.len() || !bytes[xref_pos..].starts_with(b"xref") {
        return Err(odd("cross references that are not a table"));
    }
    i = xref_pos + 4;
    let mut objects = HashMap::new();
    let mut entries_seen = 0u64;
    loop {
        i = skip_ws(&bytes, i);
        if bytes[i.min(bytes.len())..].starts_with(b"trailer") {
            i += 7;
            break;
        }
        let start = number(&bytes, &mut i).ok_or(odd("a damaged cross reference table"))?;
        let count = number(&bytes, &mut i).ok_or(odd("a damaged cross reference table"))?;
        // An entry takes at least 16 bytes: the count cannot be more than the bytes left allow.
        if count > ((bytes.len() - i.min(bytes.len())) / 16) as u64 {
            return Err(odd("a cross reference table longer than the file"));
        }
        entries_seen += count;
        if entries_seen > u64::from(MAX_OBJECTS)
            || start.saturating_add(count) > u64::from(MAX_OBJECTS)
        {
            return Err(odd("too many objects"));
        }
        for k in 0..count {
            let offset = number(&bytes, &mut i).ok_or(odd("a damaged cross reference entry"))?;
            let generation =
                number(&bytes, &mut i).ok_or(odd("a damaged cross reference entry"))?;
            i = skip_ws(&bytes, i);
            let flag = *bytes.get(i).ok_or(odd("a damaged cross reference entry"))?;
            i += 1;
            match flag {
                b'n' => {
                    if offset as usize >= bytes.len() || generation > u64::from(u16::MAX) {
                        return Err(odd("an object that is not in the file"));
                    }
                    objects.insert(
                        (start + k) as u32,
                        At {
                            offset: offset as usize,
                            generation: generation as u16,
                        },
                    );
                }
                b'f' => {}
                _ => return Err(odd("a damaged cross reference entry")),
            }
        }
    }
    let ts = skip_ws(&bytes, i);
    let te = value_end(&bytes, ts).ok_or(odd("a damaged trailer"))?;
    let trailer = bytes[ts..te].to_vec();
    if syntax::get(&trailer, "Encrypt").is_some() {
        return Err(Unsupported::Encrypted);
    }
    if syntax::get(&trailer, "Prev").is_some() || syntax::get(&trailer, "XRefStm").is_some() {
        return Err(odd("a file in several parts"));
    }
    let size = syntax::get(&trailer, "Size")
        .and_then(syntax::parse_int)
        .filter(|s| (1..=i64::from(MAX_OBJECTS)).contains(s))
        .ok_or(odd("no usable size in the trailer"))? as u32;
    if objects.keys().any(|n| *n >= size) {
        return Err(odd("an object past the size in the trailer"));
    }
    Ok(Parsed {
        bytes,
        objects,
        trailer,
        xref_pos,
        size,
    })
}

impl Parsed {
    /// The text of the value of object `n` (for a stream, its dictionary).
    pub fn obj(&self, n: u32) -> Option<&[u8]> {
        let (start, end) = self.span(n)?;
        Some(&self.bytes[start..end])
    }

    /// True when object `n` is a stream: its dictionary is followed by data.
    pub fn is_stream(&self, n: u32) -> bool {
        match self.span(n) {
            Some((_, end)) => {
                let after = skip_ws(&self.bytes, end);
                self.bytes[after.min(self.bytes.len())..].starts_with(b"stream")
            }
            None => false,
        }
    }

    /// Where the value of object `n` starts and ends.
    pub fn span(&self, n: u32) -> Option<(usize, usize)> {
        let at = self.objects.get(&n)?;
        let s = &self.bytes;
        let mut i = at.offset;
        let number_end = token_end(s, i);
        if s.get(i..number_end)? != n.to_string().as_bytes() {
            return None;
        }
        i = skip_ws(s, number_end);
        let gen_end = token_end(s, i);
        if s.get(i..gen_end)? != at.generation.to_string().as_bytes() {
            return None;
        }
        i = skip_ws(s, gen_end);
        let kw_end = token_end(s, i);
        if s.get(i..kw_end)? != b"obj" {
            return None;
        }
        let start = skip_ws(s, kw_end);
        let end = value_end(s, start)?;
        Some((start, end))
    }

    /// The generation of object `n`.
    pub fn generation(&self, n: u32) -> u16 {
        self.objects.get(&n).map_or(0, |a| a.generation)
    }

    /// `n g R` for an object that is in the file, `n 0 R` otherwise.
    pub fn reference(&self, n: u32) -> String {
        format!("{n} {} R", self.generation(n))
    }

    pub fn has(&self, n: u32) -> bool {
        self.objects.contains_key(&n)
    }

    /// The numbers of the objects, in no order.
    pub fn numbers(&self) -> impl Iterator<Item = u32> + '_ {
        self.objects.keys().copied()
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offs = Vec::new();
        for (n, body) in [
            (1u32, "<< /Type /Catalog /Pages 2 0 R >>"),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>"),
        ] {
            offs.push(out.len());
            out.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
        for o in &offs {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        out
    }

    #[test]
    fn a_plain_file_is_read() {
        let p = parse(tiny()).unwrap();
        assert_eq!(p.size, 4);
        assert_eq!(p.len(), 3);
        assert!(p.obj(1).unwrap().starts_with(b"<< /Type /Catalog"));
        assert!(p.obj(0).is_none());
        assert_eq!(p.reference(2), "2 0 R");
        assert!(p.obj(7).is_none());
    }

    #[test]
    fn what_is_not_a_plain_file_is_refused() {
        assert_eq!(
            parse(b"no pdf here".to_vec()).err(),
            Some(Unsupported::Structure("no startxref"))
        );
        let mut enc = String::from_utf8(tiny()).unwrap();
        enc = enc.replace("/Root 1 0 R", "/Root 1 0 R /Encrypt 9 0 R");
        assert_eq!(parse(enc.into_bytes()).err(), Some(Unsupported::Encrypted));
        let prev = String::from_utf8(tiny())
            .unwrap()
            .replace("/Root 1 0 R", "/Root 1 0 R /Prev 5");
        assert!(parse(prev.into_bytes()).is_err());
        // An xref stream.
        let mut bytes = tiny();
        let at = bytes.windows(4).rposition(|w| w == b"xref").unwrap();
        bytes[at] = b'X';
        assert!(parse(bytes).is_err());
        // A table that claims more entries than the file has bytes.
        let big = String::from_utf8(tiny())
            .unwrap()
            .replace("xref\n0 4\n", "xref\n0 4000000\n");
        assert!(parse(big.into_bytes()).is_err());
        // A size smaller than an object number.
        let small = String::from_utf8(tiny())
            .unwrap()
            .replace("/Size 4", "/Size 2");
        assert!(parse(small.into_bytes()).is_err());
    }

    #[test]
    fn an_offset_that_points_elsewhere_gives_no_object() {
        let mut p = parse(tiny()).unwrap();
        let wrong = p.objects[&2].offset;
        p.objects.get_mut(&3).unwrap().offset = wrong;
        assert!(p.obj(3).is_none());
    }
}
