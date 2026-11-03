//! What a stream is, found from the structure of the file and not from its bytes: a content stream
//! (a page's `/Contents`, a form XObject, a tiling pattern, an appearance stream, the glyph
//! procedure of a Type 3 font), an image, a font program, or data of another kind.
//!
//! A stream that nothing points to (an orphan, a leftover of an old save) has no role. It is read
//! as content when it parses as content: the operators outside strings are PDF operators. The
//! bytes inside a string do not count, because a string may hold any byte. A stream that has
//! showing operators and does not parse as content is a problem, never a pass.

use std::collections::HashSet;

use crate::carve::{self, Carved};
use crate::content::{for_each_op, shown};
use crate::pdfobj::{Doc, Obj};

/// The object numbers of the streams that the structure of the buffer makes content streams.
pub fn content_streams(doc: &Doc) -> HashSet<u32> {
    let mut out = HashSet::new();
    for e in doc.entries() {
        if e.stream.is_some()
            && (e.value.get("Subtype").and_then(Obj::name) == Some("Form")
                || e.value.get("PatternType").and_then(Obj::int) == Some(1))
        {
            out.insert(e.num);
        }
        walk(doc, &e.value, 0, &mut out);
    }
    out
}

/// The object numbers a value points to: a reference, an array of references, a reference to an
/// array, or a dictionary of references (the states of an appearance).
fn refs_of(doc: &Doc, o: &Obj, out: &mut HashSet<u32>, depth: usize) {
    match o {
        Obj::Ref(n) => {
            out.insert(*n);
            if depth < 3 {
                if let Some(e) = doc.latest(*n) {
                    if e.stream.is_none() {
                        refs_of(doc, &e.value, out, depth + 1);
                    }
                }
            }
        }
        Obj::Array(a) => a.iter().for_each(|x| refs_of(doc, x, out, depth)),
        Obj::Dict(d) => d.iter().for_each(|(_, x)| refs_of(doc, x, out, depth)),
        _ => {}
    }
}

fn walk(doc: &Doc, o: &Obj, depth: usize, out: &mut HashSet<u32>) {
    if depth > 10 {
        return;
    }
    match o {
        Obj::Dict(d) => {
            for (k, v) in d {
                match k.as_str() {
                    "Contents" => refs_of(doc, v, out, 0),
                    "AP" => {
                        for state in ["N", "D", "R"] {
                            if let Some(s) = doc.resolve(v).get(state) {
                                refs_of(doc, s, out, 0);
                            }
                        }
                    }
                    "CharProcs" => refs_of(doc, v, out, 0),
                    _ => {}
                }
                walk(doc, v, depth + 1, out);
            }
        }
        Obj::Array(a) => a.iter().for_each(|x| walk(doc, x, depth + 1, out)),
        _ => {}
    }
}

/// What a stream is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// Samples of an image. The byte layers and the image layer look at it.
    Image,
    /// A font program, a character map or another kind of data that holds no page content.
    Data,
    /// A content stream: its strings are text, read through fonts.
    Content,
    /// Neither an image, a font nor content. Only the byte layers looked at it.
    Other,
    /// It shows strings, and it does not parse as content: it was not read as text.
    Unsure(String),
}

/// Whether a token is an operator of the content stream language.
fn is_operator(name: &[u8]) -> bool {
    matches!(
        name,
        b"b" | b"B"
            | b"b*"
            | b"B*"
            | b"BDC"
            | b"BI"
            | b"BMC"
            | b"BT"
            | b"BX"
            | b"c"
            | b"cm"
            | b"CS"
            | b"cs"
            | b"d"
            | b"d0"
            | b"d1"
            | b"Do"
            | b"DP"
            | b"EI"
            | b"EMC"
            | b"ET"
            | b"EX"
            | b"f"
            | b"F"
            | b"f*"
            | b"G"
            | b"g"
            | b"gs"
            | b"h"
            | b"i"
            | b"ID"
            | b"j"
            | b"J"
            | b"K"
            | b"k"
            | b"l"
            | b"m"
            | b"M"
            | b"MP"
            | b"n"
            | b"q"
            | b"Q"
            | b"re"
            | b"RG"
            | b"rg"
            | b"ri"
            | b"s"
            | b"S"
            | b"SC"
            | b"sc"
            | b"SCN"
            | b"scn"
            | b"sh"
            | b"T*"
            | b"Tc"
            | b"Td"
            | b"TD"
            | b"Tf"
            | b"Tj"
            | b"TJ"
            | b"TL"
            | b"Tm"
            | b"Tr"
            | b"Ts"
            | b"Tw"
            | b"Tz"
            | b"v"
            | b"w"
            | b"W"
            | b"W*"
            | b"y"
            | b"'"
            | b"\""
    )
}

/// How the operators of `data` look: the number of operators, how many are known operators, and
/// how many show a string.
fn survey(data: &[u8]) -> (usize, usize, usize) {
    let (mut total, mut known, mut showing) = (0, 0, 0);
    for_each_op(data, &mut |op| {
        total += 1;
        if is_operator(op.name) {
            known += 1;
        }
        if shown(op).is_some_and(|items| !items.is_empty()) {
            showing += 1;
        }
    });
    (total, known, showing)
}

/// The kind of a carved stream. `roles` is [`content_streams`] of the buffer it was found in.
pub fn classify(c: &Carved, roles: &HashSet<u32>) -> Kind {
    if c.is_image {
        return Kind::Image;
    }
    let dict = c.dict;
    let ty = carve::name_value(dict, "/Type");
    let sub = carve::name_value(dict, "/Subtype");
    if ty.as_deref().is_some_and(|t| {
        matches!(
            t,
            "XRef" | "ObjStm" | "Metadata" | "EmbeddedFile" | "CMap" | "Sig" | "DocTimeStamp"
        )
    }) || sub
        .as_deref()
        .is_some_and(|t| matches!(t, "Type1C" | "CIDFontType0C" | "OpenType" | "XML"))
        || [
            "/Length1",
            "/Length2",
            "/Length3",
            "/CMapName",
            "/FunctionType",
        ]
        .iter()
        .any(|k| carve::key_pos(dict, k).is_some())
    {
        return Kind::Data;
    }
    let data = &c.decoded.data;
    let head = &data[..data.len().min(4096)];
    // An ICC profile has "acsp" at byte 36; a character map says what it is.
    if (data.len() >= 40 && &data[36..40] == b"acsp") || head.windows(9).any(|w| w == b"begincmap")
    {
        return Kind::Data;
    }
    if data.iter().all(|b| crate::strings::is_space(*b)) {
        return Kind::Data;
    }
    let by_role = c.obj.is_some_and(|(n, _)| roles.contains(&n))
        || sub.as_deref() == Some("Form")
        || carve::key_pos(dict, "/PatternType").is_some();
    if by_role {
        return Kind::Content;
    }
    let (total, known, showing) = survey(data);
    let looks_like_content = total > 0 && known * 10 >= total * 6;
    match (looks_like_content, showing) {
        (true, _) => Kind::Content,
        (false, 0) => Kind::Other,
        (false, n) => Kind::Unsure(format!(
            "it has {n} operator(s) that show a string, and only {known} of its {total} operators are content operators"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_with_any_byte_do_not_make_a_stream_unreadable() {
        let mut data = b"BT /F2 12 Tf 72 600 Td (".to_vec();
        data.extend(std::iter::repeat_n([0u8, 3, 0, 5], 20).flatten());
        data.extend(b") Tj ET\n");
        let (total, known, showing) = survey(&data);
        assert_eq!((total, known, showing), (5, 5, 1));
    }

    #[test]
    fn binary_data_is_not_content() {
        let data: Vec<u8> = (0..4000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let (total, known, showing) = survey(&data);
        assert!(known * 10 < total * 6, "{total} {known} {showing}");
    }
}
