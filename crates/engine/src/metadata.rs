//! XMP metadata streams in a password protected file.
//!
//! PDFium does not write the XMP metadata stream of a protected document the way it writes every
//! other stream. `CPDF_Stream::WriteTo` leaves any stream with `/Type /Metadata` and `/Subtype /XML`
//! out of the encryption, whatever the `/Encrypt` dictionary says, while the parser decrypts such a
//! stream when it reads the file (the catalog's stream is skipped only when `/EncryptMetadata` is
//! false). So a save of a protected document that has XMP metadata:
//!
//! - writes the metadata as plain text into a file that says it is encrypted (a reader that does
//!   not decrypt it reads the title, the author and the keywords without the password, and a
//!   reader that does gets garbage), and
//! - when the stream was compressed, writes the compressed bytes without the `/Filter` key, so
//!   the stream is not XML at all.
//!
//! A file that PDFium wrote this way and that is opened and saved again has its metadata turned
//! into garbage, because the plain bytes are decrypted on the way in.
//!
//! The engine cannot make PDFium encrypt the stream: the objects of a document cannot be changed
//! through the interface this engine uses, and the key of the file is not available to it. It
//! also must not write the decrypted metadata into the file. So the bytes PDFium wrote are looked
//! at before they are handed over, and a protected document whose file would hold an XMP stream is
//! not written at all (see [`leaves_metadata_in_the_clear`]). The one case that is right is the
//! document level stream of a file that says `/EncryptMetadata false`: it is plain in the
//! original, the parser leaves it as it is, and the writer writes it as it was read.

use std::collections::HashMap;

/// Whether the bytes PDFium wrote for a protected document hold an XMP metadata stream that is
/// not safely written: any stream with `/Type /Metadata` and `/Subtype /XML`, except the one that
/// the catalog names, in a file whose `/Encrypt` dictionary has `/EncryptMetadata false`.
///
/// A file whose structure cannot be read here counts as unsafe when it has such a name at all.
/// The check goes by the cross-reference table that PDFium writes, not by looking for words in
/// the bytes, so that neither the plain text of the metadata nor anything else in the file can
/// make it see objects that are not there.
pub fn leaves_metadata_in_the_clear(file: &[u8]) -> bool {
    // The names in a dictionary are never encrypted, so a metadata stream is always found by
    // its name; most files have none, and are done here.
    if find(file, b"/Metadata", 0).is_none() {
        return false;
    }
    let Some(table) = Table::read(file) else {
        return true;
    };
    let mut streams: Vec<u32> = Vec::new();
    let mut all_xml = true;
    for &(number, offset) in &table.entries {
        let Some(object) = read_object(file, number, offset) else {
            return true;
        };
        if let Some(data) = object.data {
            let dict = squeeze(object.dict);
            if find(&dict, b"/Type/Metadata", 0).is_some()
                && find(&dict, b"/Subtype/XML", 0).is_some()
            {
                streams.push(number);
                all_xml &= looks_like_xml(&file[data..]);
            }
        }
    }
    if streams.is_empty() {
        return false;
    }
    // What is left is the one stream that a file with `/EncryptMetadata false` keeps plain. It is
    // written as PDFium read it, which is right only when it is the XML itself: a compressed
    // stream is written without its `/Filter` key (PDFium drops it and keeps the compressed
    // bytes).
    if !all_xml {
        return true;
    }
    let dictionary_of = |key: &[u8]| {
        let number = reference(table.trailer, key)?;
        let offset = table.offset_of(number)?;
        read_object(file, number, offset).map(|object| object.dict)
    };
    let encrypted_metadata = match dictionary_of(b"/Encrypt") {
        Some(dict) => find(&squeeze(dict), b"/EncryptMetadatafalse", 0).is_none(),
        None => return true,
    };
    if encrypted_metadata {
        return true;
    }
    let catalog_stream = dictionary_of(b"/Root").and_then(|dict| reference(dict, b"/Metadata"));
    streams != [catalog_stream.unwrap_or(0)]
}

/// Whether a stream starts like an XML text: a byte order mark that XMP allows, white space, `<`.
fn looks_like_xml(data: &[u8]) -> bool {
    let data = data
        .strip_prefix(&[0xEF, 0xBB, 0xBF][..])
        .or_else(|| data.strip_prefix(&[0xFE, 0xFF][..]))
        .or_else(|| data.strip_prefix(&[0xFF, 0xFE][..]))
        .unwrap_or(data);
    // (UTF-16 text has a zero byte next to each character, and a zero counts as white space.)
    data.iter().copied().find(|b| !is_space(*b)) == Some(b'<')
}

/// The objects of a classic cross-reference table and its trailer dictionary.
struct Table<'a> {
    /// (object number, offset) of every object in use.
    entries: Vec<(u32, usize)>,
    index: HashMap<u32, usize>,
    trailer: &'a [u8],
}

impl<'a> Table<'a> {
    fn read(file: &'a [u8]) -> Option<Table<'a>> {
        let start = rfind(file, b"startxref")?;
        let mut at = start + b"startxref".len();
        let offset: usize = std::str::from_utf8(token(file, &mut at)?)
            .ok()?
            .parse()
            .ok()?;
        let mut at = offset;
        if token(file, &mut at)? != b"xref" {
            return None;
        }
        let mut entries = Vec::new();
        loop {
            let word = token(file, &mut at)?;
            if word == b"trailer" {
                break;
            }
            let first: u32 = std::str::from_utf8(word).ok()?.parse().ok()?;
            let count: usize = std::str::from_utf8(token(file, &mut at)?)
                .ok()?
                .parse()
                .ok()?;
            // An entry takes 20 bytes, so a count that the file cannot hold is not true.
            if count > file.len() / 18 {
                return None;
            }
            for i in 0..count {
                let position: usize = std::str::from_utf8(token(file, &mut at)?)
                    .ok()?
                    .parse()
                    .ok()?;
                token(file, &mut at)?;
                let kind = token(file, &mut at)?;
                if kind == b"n" {
                    entries.push((first.checked_add(u32::try_from(i).ok()?)?, position));
                }
            }
        }
        let end = find(file, b"startxref", at)?;
        let index = entries.iter().copied().collect();
        Some(Table {
            entries,
            index,
            trailer: &file[at..end],
        })
    }

    fn offset_of(&self, number: u32) -> Option<usize> {
        self.index.get(&number).copied()
    }
}

/// An object as PDFium writes it: the text between `N G obj` and its `stream` or `endobj`, and
/// where the data of a stream starts.
struct Object<'a> {
    dict: &'a [u8],
    data: Option<usize>,
}

fn read_object(file: &[u8], number: u32, offset: usize) -> Option<Object<'_>> {
    let mut at = offset;
    let n: u32 = std::str::from_utf8(token(file, &mut at)?)
        .ok()?
        .parse()
        .ok()?;
    token(file, &mut at)?;
    if n != number || token(file, &mut at)? != b"obj" {
        return None;
    }
    let tail = file.get(at..)?;
    let stream = find(tail, b"stream", 0);
    let end = find(tail, b"endobj", 0);
    match (stream, end) {
        (Some(s), Some(e)) if s < e => {
            // The keyword is followed by CR LF or LF.
            let mut data = at + s + b"stream".len();
            if file.get(data) == Some(&b'\r') {
                data += 1;
            }
            if file.get(data) == Some(&b'\n') {
                data += 1;
            }
            Some(Object {
                dict: &tail[..s],
                data: Some(data),
            })
        }
        (_, Some(e)) => Some(Object {
            dict: &tail[..e],
            data: None,
        }),
        _ => None,
    }
}

/// The object number that `key` refers to in a dictionary written as `/Key N G R`.
fn reference(dict: &[u8], key: &[u8]) -> Option<u32> {
    let mut from = 0;
    while let Some(at) = find(dict, key, from) {
        let mut cursor = at + key.len();
        // The key is the whole name (`/Metadata`, not `/MetadataStream`).
        if dict.get(cursor).is_some_and(|b| is_space(*b)) {
            if let Some(number) = token(dict, &mut cursor) {
                if let Ok(n) = std::str::from_utf8(number).unwrap_or("").parse() {
                    return Some(n);
                }
            }
        }
        from = at + key.len();
    }
    None
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\r' | b'\n' | b'\t' | 0x0c | 0)
}

/// The next word delimited by white space; `at` moves past it.
fn token<'a>(bytes: &'a [u8], at: &mut usize) -> Option<&'a [u8]> {
    while bytes.get(*at).is_some_and(|b| is_space(*b)) {
        *at += 1;
    }
    let start = *at;
    while bytes.get(*at).is_some_and(|b| !is_space(*b)) {
        *at += 1;
    }
    (*at > start).then(|| &bytes[start..*at])
}

/// The bytes without the white space, so that `/Type /Metadata` and `/Type/Metadata` look alike.
fn squeeze(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().copied().filter(|b| !is_space(*b)).collect()
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let mut at = from;
    let first = *needle.first()?;
    while let Some(skip) = haystack.get(at..)?.iter().position(|b| *b == first) {
        at += skip;
        if haystack.get(at..at + needle.len())? == needle {
            return Some(at);
        }
        at += 1;
    }
    None
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file in the form PDFium writes: objects with a classic table and a trailer.
    fn file(objects: &[&str], trailer: &str) -> Vec<u8> {
        let mut out = b"%PDF-1.7\r\n%\xA1\xB3\xC5\xD7\r\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(
                format!("\r\n{} 0 obj\r\n{body}\r\nendobj\r\n", i + 1).as_bytes(),
            );
            // The offset points at the number, after the blank line.
            *offsets.last_mut().unwrap() += 2;
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\r\n0 {}\r\n0000000000 65535 f\r\n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n\r\n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\r\n<<{trailer}>>\r\nstartxref\r\n{xref}\r\n%%EOF\r\n").as_bytes(),
        );
        out
    }

    const XMP: &str = "<</Length 12/Subtype/XML/Type/Metadata>>stream\r\n<x:xmpmeta/>\r\nendstream";

    fn catalog(metadata: &str) -> String {
        format!("<</Metadata {metadata} 0 R /Pages 3 0 R /Type/Catalog>>")
    }

    #[test]
    fn a_file_without_a_metadata_stream_is_fine() {
        let f = file(
            &[
                "<</Pages 3 0 R /Type/Catalog>>",
                "<</Filter/Standard>>",
                "<</Type/Pages>>",
            ],
            "/Root 1 0 R /Encrypt 2 0 R /Size 4",
        );
        assert!(!leaves_metadata_in_the_clear(&f));
    }

    #[test]
    fn a_metadata_stream_in_a_file_that_encrypts_it_is_not_safe() {
        let objects = [
            &catalog("4")[..],
            "<</Filter/Standard/V 4>>",
            "<</Type/Pages>>",
            XMP,
        ];
        let f = file(&objects, "/Root 1 0 R /Encrypt 2 0 R /Size 5");
        assert!(leaves_metadata_in_the_clear(&f));
        // The same stream is as unsafe when the dictionary names the key without a value.
        let objects = [
            &catalog("4")[..],
            "<</Filter/Standard/EncryptMetadata true>>",
            "<</Type/Pages>>",
            XMP,
        ];
        let f = file(&objects, "/Root 1 0 R /Encrypt 2 0 R /Size 5");
        assert!(leaves_metadata_in_the_clear(&f));
    }

    #[test]
    fn the_catalog_stream_of_a_file_that_leaves_it_plain_is_fine() {
        let objects = [
            &catalog("4")[..],
            "<</Filter/Standard/EncryptMetadata false>>",
            "<</Type/Pages>>",
            XMP,
        ];
        let f = file(&objects, "/Root 1 0 R /Encrypt 2 0 R /Size 5");
        assert!(!leaves_metadata_in_the_clear(&f));
    }

    #[test]
    fn a_plain_stream_that_is_not_xml_is_not_fine() {
        // What PDFium writes for a compressed stream: the compressed bytes, and no `/Filter`.
        let squashed = "<</Length 5/Subtype/XML/Type/Metadata>>stream\r\nx\u{9c}abc\r\nendstream";
        let objects = [
            &catalog("4")[..],
            "<</Filter/Standard/EncryptMetadata false>>",
            "<</Type/Pages>>",
            squashed,
        ];
        let f = file(&objects, "/Root 1 0 R /Encrypt 2 0 R /Size 5");
        assert!(leaves_metadata_in_the_clear(&f));
        assert!(looks_like_xml(b"\xEF\xBB\xBF \r\n<?xpacket"));
        assert!(looks_like_xml(b"\xFE\xFF\0<\0?"));
        assert!(!looks_like_xml(b"x\x9c\x01"));
        assert!(!looks_like_xml(b""));
    }

    #[test]
    fn a_second_stream_or_one_the_catalog_does_not_name_is_not_fine() {
        let objects = [
            &catalog("4")[..],
            "<</Filter/Standard/EncryptMetadata false>>",
            "<</Type/Pages>>",
            XMP,
            XMP,
        ];
        let f = file(&objects, "/Root 1 0 R /Encrypt 2 0 R /Size 6");
        assert!(leaves_metadata_in_the_clear(&f));
        let objects = [
            "<</Pages 3 0 R /Type/Catalog>>",
            "<</Filter/Standard/EncryptMetadata false>>",
            "<</Type/Pages>>",
            XMP,
        ];
        let f = file(&objects, "/Root 1 0 R /Encrypt 2 0 R /Size 5");
        assert!(leaves_metadata_in_the_clear(&f));
    }

    #[test]
    fn a_stream_with_another_type_or_subtype_is_not_a_metadata_stream() {
        let objects = [
            &catalog("4")[..],
            "<</Filter/Standard>>",
            "<</Type/Pages>>",
            "<</Length 3/Subtype/XML/Type/EmbeddedFile>>stream\r\nabc\r\nendstream",
            "<</Length 3/Subtype/Image/Type/Metadata>>stream\r\nabc\r\nendstream",
        ];
        let f = file(&objects, "/Root 1 0 R /Encrypt 2 0 R /Size 6");
        assert!(!leaves_metadata_in_the_clear(&f));
    }

    #[test]
    fn text_that_looks_like_objects_in_a_stream_does_not_count() {
        // A plain stream that holds what a table and an object look like does not change what
        // the real table says.
        let fake =
            "<</Length 60>>stream\r\n5 0 obj\r\n<</Subtype/XML/Type/Metadata>>stream\r\nendstream";
        let objects = [
            "<</Pages 3 0 R /Type/Catalog>>",
            "<</Filter/Standard>>",
            "<</Type/Pages>>",
            fake,
        ];
        let f = file(&objects, "/Root 1 0 R /Encrypt 2 0 R /Size 5");
        assert!(!leaves_metadata_in_the_clear(&f));
    }

    #[test]
    fn a_file_that_cannot_be_read_is_not_safe_when_it_names_a_metadata_stream() {
        assert!(leaves_metadata_in_the_clear(b"%PDF-1.7\r\n/Type/Metadata"));
        assert!(!leaves_metadata_in_the_clear(b"%PDF-1.7\r\nnothing here"));
        let objects = [
            &catalog("4")[..],
            "<</Filter/Standard>>",
            "<</Type/Pages>>",
            XMP,
        ];
        let mut f = file(&objects, "/Root 1 0 R /Encrypt 2 0 R /Size 5");
        let at = rfind(&f, b"startxref").unwrap();
        f.truncate(at);
        assert!(leaves_metadata_in_the_clear(&f));
    }
}
