//! What a stream is, found from the structure of the file and not from its bytes: a content stream
//! (a page's `/Contents`, a form XObject, a tiling pattern, an appearance stream, the glyph
//! procedure of a Type 3 font), an image, a font program, a character map, an ICC profile, or
//! data of another kind.
//!
//! The role comes first. A stream that the structure makes content is always decoded as content,
//! whatever bytes or keys it has. A stream that the structure makes a font program, a character
//! map, an ICC profile or other data is data. Only a stream that nothing points to (an orphan, a
//! leftover of an old save) has no role, and only then is it judged by what it looks like: its
//! dictionary and its first bytes may say what it is, and otherwise it is read as content when it
//! parses as content (the operators outside strings are PDF operators; the bytes inside a string
//! do not count, because a string may hold any byte). An orphan that shows strings and does not
//! parse as content is a problem, never a pass.

use std::collections::HashSet;

use crate::carve::{self, Carved};
use crate::content::{for_each_op, shown};
use crate::pdfobj::{Doc, Obj};

/// What the structure of a buffer says its streams are.
#[derive(Debug, Default)]
pub struct Roles {
    /// The object numbers of the content streams.
    pub content: HashSet<u32>,
    /// The object numbers of font programs, character maps, ICC profiles, metadata and other data.
    pub data: HashSet<u32>,
}

/// The roles that the structure of the buffer gives.
pub fn of(doc: &Doc) -> Roles {
    Roles {
        content: content_streams(doc),
        data: data_streams(doc),
    }
}

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
        // The icons of a widget are forms that a viewer draws in its place.
        if let Some(mk) = e.value.get("MK") {
            for key in ["I", "RI", "IX"] {
                if let Some(icon) = doc.resolve(mk).get(key) {
                    refs_of(doc, icon, &mut out, 0);
                }
            }
        }
    }
    out
}

/// Dictionary keys whose values are streams of data, not content.
const DATA_KEYS: [&str; 10] = [
    "FontFile",
    "FontFile2",
    "FontFile3",
    "ToUnicode",
    "CIDToGIDMap",
    "CIDSet",
    "Metadata",
    "JBIG2Globals",
    "Function",
    "EF",
];

/// The object numbers of the streams that the structure of the buffer makes data: font programs,
/// character maps (`/ToUnicode`, a stream that `/Encoding` names), ICC profiles (`[/ICCBased n 0
/// R]`), metadata, embedded files and functions.
pub fn data_streams(doc: &Doc) -> HashSet<u32> {
    let mut out = HashSet::new();
    for e in doc.entries() {
        walk_data(doc, &e.value, 0, &mut out);
    }
    out
}

fn walk_data(doc: &Doc, o: &Obj, depth: usize, out: &mut HashSet<u32>) {
    if depth > 10 {
        return;
    }
    match o {
        Obj::Dict(d) => {
            for (k, v) in d {
                if DATA_KEYS.contains(&k.as_str()) {
                    refs_of(doc, v, out, 0);
                } else if k == "Encoding" {
                    // A character map is a stream; the encoding of a simple font is a dictionary.
                    if let Obj::Ref(n) = v {
                        if doc.latest(*n).is_some_and(|e| e.stream.is_some()) {
                            out.insert(*n);
                        }
                    }
                }
                walk_data(doc, v, depth + 1, out);
            }
        }
        Obj::Array(a) => {
            if a.first().and_then(Obj::name) == Some("ICCBased") {
                if let Some(Obj::Ref(n)) = a.get(1) {
                    out.insert(*n);
                }
            }
            a.iter().for_each(|x| walk_data(doc, x, depth + 1, out));
        }
        _ => {}
    }
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

/// Whether the bytes are a character map: after white space and comments, it starts with
/// `/CIDInit` (the resource header of every `ToUnicode` map and embedded CMap).
fn starts_like_a_cmap(data: &[u8]) -> bool {
    let mut i = 0;
    while i < data.len() {
        match data[i] {
            b if crate::strings::is_space(b) => i += 1,
            b'%' => {
                while i < data.len() && data[i] != b'\n' && data[i] != b'\r' {
                    i += 1;
                }
            }
            _ => break,
        }
    }
    data[i..].starts_with(b"/CIDInit")
}

/// Whether the bytes are an ICC profile: the signature `acsp` at byte 36, and the size that the
/// header gives in its first four bytes is the size of the data.
fn is_icc_profile(data: &[u8]) -> bool {
    data.len() >= 128
        && &data[36..40] == b"acsp"
        && u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize == data.len()
}

/// The kind of a carved stream. `roles` is [`of`] the buffer it was found in.
///
/// The structure decides first: a stream with a content role is content, a stream with a data role
/// is data. Only a stream with no role is looked at: its dictionary, its first bytes, and last of
/// all its operators.
pub fn classify(c: &Carved, roles: &Roles) -> Kind {
    if c.is_image {
        return Kind::Image;
    }
    let dict = c.dict;
    let ty = carve::name_value(dict, "/Type");
    let sub = carve::name_value(dict, "/Subtype");
    let numbered = c.obj.map(|(n, _)| n);
    if numbered.is_some_and(|n| roles.content.contains(&n))
        || sub.as_deref() == Some("Form")
        || carve::key_pos(dict, "/PatternType").is_some()
    {
        return Kind::Content;
    }
    if numbered.is_some_and(|n| roles.data.contains(&n)) {
        return Kind::Data;
    }
    // No role: an orphan.
    let data = &c.decoded.data;
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
        || is_icc_profile(data)
        || starts_like_a_cmap(data)
        || data.iter().all(|b| crate::strings::is_space(*b))
    {
        return Kind::Data;
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
    fn a_character_map_is_told_by_its_header_and_not_by_a_word_in_its_text() {
        assert!(starts_like_a_cmap(
            b"/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap"
        ));
        assert!(starts_like_a_cmap(
            b"%!PS-Adobe-3.0 Resource-CMap\n%%Title: x\n\n/CIDInit /ProcSet findresource begin"
        ));
        // Page content that shows the word.
        assert!(!starts_like_a_cmap(
            b"BT /F1 12 Tf 72 720 Td (How begincmap works) Tj ET"
        ));
        assert!(!starts_like_a_cmap(b"q Q /CIDInit"));
    }

    #[test]
    fn an_icc_profile_has_its_signature_and_its_size() {
        let mut profile = vec![0u8; 200];
        profile[..4].copy_from_slice(&200u32.to_be_bytes());
        profile[36..40].copy_from_slice(b"acsp");
        assert!(is_icc_profile(&profile));
        // The signature in a comment of a content stream is not a profile.
        let mut text = vec![b'x'; 200];
        text[36..40].copy_from_slice(b"acsp");
        assert!(!is_icc_profile(&text));
        profile[..4].copy_from_slice(&100u32.to_be_bytes());
        assert!(!is_icc_profile(&profile));
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
