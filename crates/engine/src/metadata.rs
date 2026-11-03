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
//! at before they leave the engine, and a protected document whose file would hold an XMP stream
//! is not written at all (see [`leaves_metadata_in_the_clear`]). The one case that is right is the
//! document level stream of a file that says `/EncryptMetadata false`: it is plain in the
//! original, the parser leaves it as it is, and the writer writes it as it was read.
//!
//! The file that is looked at was written by PDFium (a classic cross-reference table, every object
//! in its own place), but the dictionaries in it hold what the input held, so nothing in it is
//! taken on trust. The check reads the structure with a small tokenizer (names, strings, nested
//! dictionaries and arrays), keeps every object to the bytes between its offset and the next
//! one, so that the whole check is one pass over the file, and counts the metadata type names in
//! the plain bytes as a second opinion. Whatever it cannot read or bound counts as unsafe.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

/// How many bytes are read between two reports of progress.
const PROGRESS_STEP: usize = 4 * 1024 * 1024;

/// How deep dictionaries and arrays may go before the file is not read any further.
const MAX_DEPTH: usize = 64;

/// Whether the bytes PDFium wrote for a document hold an XMP metadata stream that is not safely
/// written. Nothing is unsafe in a file that is not protected (the trailer has no `/Encrypt`).
/// In a protected file an XMP stream (`/Type /Metadata`, `/Subtype /XML`) is unsafe, except the
/// one the catalog names when the `/Encrypt` dictionary has `/EncryptMetadata false` at its top
/// level and the stream still starts like XML.
///
/// A file whose structure cannot be read or bounded counts as unsafe when it has a metadata name
/// at all, and so does a file with more metadata type names than the structure accounts for.
/// `progress` is told how far the check has come now and then.
pub fn leaves_metadata_in_the_clear(file: &[u8], progress: &mut dyn FnMut(u32)) -> bool {
    // PDFium writes names as they are, so a metadata stream is always found by its name. Most
    // files have none, and are done here.
    let Some(typed_names) = count_names(file, progress) else {
        return false;
    };
    let Some(table) = Table::read(file) else {
        return true;
    };
    let Some(trailer) = parse_dictionary(file, table.trailer) else {
        return true;
    };
    let encrypt = match trailer.get(b"Encrypt") {
        // Not protected: nothing to leave in the clear.
        None => return false,
        Some(Value::Ref(number)) => *number,
        Some(_) => return true,
    };
    let Some(encrypt) = table.dictionary(file, encrypt) else {
        return true;
    };
    // `/EncryptMetadata` counts at the top level of the encryption dictionary only, as PDFium
    // reads it, and only as the plain word `false`.
    if !matches!(encrypt.get(b"EncryptMetadata"), Some(Value::Bool(false))) {
        // Every XMP stream is unsafe, and a type name is reason enough to suspect one.
        return typed_names > 0;
    }

    // The metadata is meant to be clear: only one stream may be, and it is the catalog's.
    let mut streams: Vec<(u32, usize)> = Vec::new();
    let mut typed_objects = 0usize;
    let mut since_progress = 0usize;
    for (index, &(offset, number)) in table.sorted.iter().enumerate() {
        let Some(object) = table.read_at(file, index) else {
            return true;
        };
        if let Some(dict) = &object.dict {
            if dict.is_name(b"Type", b"Metadata") {
                typed_objects += 1;
                if dict.is_name(b"Subtype", b"XML") {
                    if let Some(data) = object.data {
                        streams.push((number, data));
                    }
                }
            }
        }
        since_progress += table.length_of(index);
        if since_progress >= PROGRESS_STEP {
            since_progress = 0;
            progress(offset.min(u32::MAX as usize) as u32);
        }
    }
    // The second opinion: a name that the structure does not account for is a stream it missed.
    if typed_names > typed_objects {
        return true;
    }
    let (stream, data) = match streams[..] {
        [] => return false,
        [one] => one,
        _ => return true,
    };
    let catalog_stream = match trailer.get(b"Root") {
        Some(Value::Ref(root)) => {
            table
                .dictionary(file, *root)
                .and_then(|catalog| match catalog.get(b"Metadata") {
                    Some(Value::Ref(number)) => Some(*number),
                    _ => None,
                })
        }
        _ => None,
    };
    // What PDFium writes for a compressed stream is its compressed bytes without a `/Filter`.
    catalog_stream != Some(stream) || !looks_like_xml(&file[data..file.len().min(data + 256)])
}

/// How many times the name `/Metadata` is the value of a `/Type` in the bytes, or `None` when
/// the name is not in them at all.
fn count_names(file: &[u8], progress: &mut dyn FnMut(u32)) -> Option<usize> {
    const WORD: &[u8] = b"/Metadata";
    let mut found = false;
    let mut typed = 0usize;
    let mut at = 0usize;
    let mut reported = 0usize;
    while let Some(next) = find(file, WORD, at) {
        found = true;
        let after = file.get(next + WORD.len()).copied();
        // The whole name, not the start of a longer one.
        if after.is_none_or(|b| is_space(b) || is_delimiter(b)) {
            let before = file[..next].iter().rposition(|b| !is_space(*b));
            if before.is_some_and(|end| file[..=end].ends_with(b"/Type")) {
                typed += 1;
            }
        }
        at = next + WORD.len();
        if at - reported >= PROGRESS_STEP {
            reported = at;
            progress(at.min(u32::MAX as usize) as u32);
        }
    }
    found.then_some(typed)
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

// ---- the cross-reference table ---------------------------------------------------------------

/// The objects of a classic cross-reference table, and where the trailer dictionary starts.
struct Table {
    /// (offset, object number) of every object in use, by offset.
    sorted: Vec<(usize, u32)>,
    /// Where each object starts.
    index: HashMap<u32, usize>,
    /// Where the table itself starts: the end of the last object.
    end: usize,
    /// Where the trailer dictionary starts.
    trailer: usize,
}

/// An object read from the bytes that are its own: its dictionary, when it is one, and where the
/// data of a stream starts, as a place in the whole file.
struct Parsed<'a> {
    dict: Option<Dict<'a>>,
    data: Option<usize>,
}

impl Table {
    fn read(file: &[u8]) -> Option<Table> {
        let start = rfind(file, b"startxref")?;
        let mut at = start + b"startxref".len();
        let end: usize = number(token(file, &mut at)?)?;
        if end >= file.len() {
            return None;
        }
        let mut at = end;
        if token(file, &mut at)? != b"xref" {
            return None;
        }
        let mut sorted = Vec::new();
        loop {
            let word = token(file, &mut at)?;
            if word == b"trailer" {
                break;
            }
            let first: u32 = number(word)?;
            let count: usize = number(token(file, &mut at)?)?;
            // An entry takes 20 bytes, so a count that the file cannot hold is not true.
            if count > file.len() / 18 {
                return None;
            }
            for i in 0..count {
                let offset: usize = number(token(file, &mut at)?)?;
                token(file, &mut at)?;
                if token(file, &mut at)? == b"n" {
                    // Every object ends before the table starts.
                    if offset >= end {
                        return None;
                    }
                    sorted.push((offset, first.checked_add(u32::try_from(i).ok()?)?));
                }
            }
        }
        sorted.sort_unstable();
        // Two objects at one place have no bytes of their own to be read from.
        if sorted.windows(2).any(|w| w[0].0 == w[1].0) {
            return None;
        }
        let mut index = HashMap::with_capacity(sorted.len());
        for &(offset, number) in &sorted {
            if index.insert(number, offset).is_some() {
                return None;
            }
        }
        Some(Table {
            sorted,
            index,
            end,
            trailer: at,
        })
    }

    /// How many bytes the object at `sorted[index]` has: up to the next object, or the table.
    fn length_of(&self, index: usize) -> usize {
        let (offset, _) = self.sorted[index];
        let next = self.sorted.get(index + 1).map_or(self.end, |&(o, _)| o);
        next - offset
    }

    fn read_at<'a>(&self, file: &'a [u8], index: usize) -> Option<Parsed<'a>> {
        let (offset, number) = self.sorted[index];
        read_object(
            file.get(offset..offset + self.length_of(index))?,
            number,
            offset,
        )
    }

    /// The dictionary of the object with this number.
    fn dictionary<'a>(&self, file: &'a [u8], number: u32) -> Option<Dict<'a>> {
        let offset = *self.index.get(&number)?;
        let index = self
            .sorted
            .binary_search_by_key(&offset, |&(o, _)| o)
            .ok()?;
        self.read_at(file, index)?.dict
    }
}

/// Reads an object from the bytes between its start and the next object. `offset` is where the
/// bytes start in the file.
fn read_object(slice: &[u8], number: u32, offset: usize) -> Option<Parsed<'_>> {
    let mut at = 0;
    if number_of(token(slice, &mut at)?)? != number {
        return None;
    }
    token(slice, &mut at)?;
    if token(slice, &mut at)? != b"obj" {
        return None;
    }
    skip_space(slice, &mut at);
    // Only a dictionary can be a stream. What else the object is does not matter here.
    if !slice[at..].starts_with(b"<<") {
        return Some(Parsed {
            dict: None,
            data: None,
        });
    }
    let dict = parse_dictionary(slice, at)?;
    let mut at = dict.end;
    skip_space(slice, &mut at);
    let mut data = None;
    if slice[at..].starts_with(b"stream") {
        at += b"stream".len();
        // The keyword is followed by CR LF or LF.
        if slice.get(at) == Some(&b'\r') {
            at += 1;
        }
        if slice.get(at) == Some(&b'\n') {
            at += 1;
        }
        data = Some(offset + at);
    }
    Some(Parsed {
        dict: Some(dict),
        data,
    })
}

// ---- the tokenizer ---------------------------------------------------------------------------

/// What the top level of a dictionary holds for a key. Strings, arrays and nested dictionaries
/// are skipped over, with whatever is in them, and are `Other`.
#[derive(Debug, Clone, PartialEq)]
enum Value<'a> {
    Name(Cow<'a, [u8]>),
    Ref(u32),
    Bool(bool),
    Other,
}

/// The top level of a dictionary, and where it ends.
#[derive(Debug, Clone)]
struct Dict<'a> {
    entries: Vec<(Cow<'a, [u8]>, Value<'a>)>,
    end: usize,
}

impl<'a> Dict<'a> {
    fn get(&self, key: &[u8]) -> Option<&Value<'a>> {
        self.entries
            .iter()
            .find(|(k, _)| k.as_ref() == key)
            .map(|(_, v)| v)
    }

    fn is_name(&self, key: &[u8], name: &[u8]) -> bool {
        matches!(self.get(key), Some(Value::Name(n)) if n.as_ref() == name)
    }
}

/// Reads the dictionary that starts at `at` (`<<`, after white space). `None` when it is not well
/// formed, has a key twice, or goes deeper than [`MAX_DEPTH`].
fn parse_dictionary(bytes: &[u8], at: usize) -> Option<Dict<'_>> {
    let mut pos = at;
    skip_space(bytes, &mut pos);
    if bytes.get(pos..pos + 2)? != b"<<" {
        return None;
    }
    pos += 2;
    let mut entries: Vec<(Cow<'_, [u8]>, Value<'_>)> = Vec::new();
    // A key twice is not a dictionary to trust. A short dictionary is searched; a long one gets a
    // set of its keys, so that the cost does not grow with the square of their number.
    let mut seen: Option<HashSet<Cow<'_, [u8]>>> = None;
    loop {
        skip_space(bytes, &mut pos);
        if bytes.get(pos..pos + 2)? == b">>" {
            return Some(Dict {
                entries,
                end: pos + 2,
            });
        }
        let key = name(bytes, &mut pos)?;
        let twice = match &mut seen {
            Some(set) => !set.insert(key.clone()),
            None => {
                let twice = entries.iter().any(|(k, _)| *k == key);
                if !twice && entries.len() >= 16 {
                    let mut set: HashSet<Cow<'_, [u8]>> =
                        entries.iter().map(|(k, _)| k.clone()).collect();
                    set.insert(key.clone());
                    seen = Some(set);
                }
                twice
            }
        };
        if twice {
            return None;
        }
        let value = value(bytes, &mut pos)?;
        entries.push((key, value));
    }
}

fn value<'a>(bytes: &'a [u8], pos: &mut usize) -> Option<Value<'a>> {
    skip_space(bytes, pos);
    match *bytes.get(*pos)? {
        b'/' => Some(Value::Name(name(bytes, pos)?)),
        b'(' | b'[' | b'<' => {
            skip_value(bytes, pos)?;
            Some(Value::Other)
        }
        b'0'..=b'9' | b'+' | b'-' | b'.' => {
            let word = token(bytes, pos)?;
            // `N G R` is a reference: the two numbers and the R are looked ahead for.
            if let Some(object) = number::<u32>(word) {
                let mut ahead = *pos;
                let generation = token(bytes, &mut ahead);
                let r = token(bytes, &mut ahead);
                if generation.is_some_and(|g| number::<u32>(g).is_some()) && r == Some(b"R") {
                    *pos = ahead;
                    return Some(Value::Ref(object));
                }
            }
            Some(Value::Other)
        }
        _ => match token(bytes, pos)? {
            b"true" => Some(Value::Bool(true)),
            b"false" => Some(Value::Bool(false)),
            b"null" => Some(Value::Other),
            _ => None,
        },
    }
}

/// Moves over one string, hex string, array or dictionary, whatever is in it.
fn skip_value(bytes: &[u8], pos: &mut usize) -> Option<()> {
    // The closers that are waited for, innermost last.
    let mut open: Vec<u8> = Vec::new();
    loop {
        skip_space(bytes, pos);
        match *bytes.get(*pos)? {
            b'(' => skip_string(bytes, pos)?,
            b'<' if bytes.get(*pos + 1) == Some(&b'<') => {
                open.push(b'>');
                *pos += 2;
            }
            b'<' => {
                let end = bytes.get(*pos..)?.iter().position(|b| *b == b'>')?;
                *pos += end + 1;
            }
            b'[' => {
                open.push(b']');
                *pos += 1;
            }
            b'>' if bytes.get(*pos + 1) == Some(&b'>') => {
                if open.pop() != Some(b'>') {
                    return None;
                }
                *pos += 2;
            }
            b']' => {
                if open.pop() != Some(b']') {
                    return None;
                }
                *pos += 1;
            }
            b'/' => {
                name(bytes, pos)?;
            }
            b'%' => {
                let end = bytes
                    .get(*pos..)?
                    .iter()
                    .position(|b| *b == b'\n' || *b == b'\r')
                    .unwrap_or(bytes.len() - *pos);
                *pos += end;
            }
            b'>' | b')' | b'{' | b'}' => return None,
            _ => {
                token(bytes, pos)?;
            }
        }
        if open.len() > MAX_DEPTH {
            return None;
        }
        if open.is_empty() {
            return Some(());
        }
    }
}

/// Moves over a string in parentheses: nested pairs and `\` escapes.
fn skip_string(bytes: &[u8], pos: &mut usize) -> Option<()> {
    let mut depth = 0usize;
    loop {
        match *bytes.get(*pos)? {
            b'\\' => *pos += 2,
            b'(' => {
                depth += 1;
                *pos += 1;
            }
            b')' => {
                depth = depth.checked_sub(1)?;
                *pos += 1;
                if depth == 0 {
                    return Some(());
                }
            }
            _ => *pos += 1,
        }
    }
}

/// A name, as the bytes after the `/`, with the `#xx` escapes resolved.
fn name<'a>(bytes: &'a [u8], pos: &mut usize) -> Option<Cow<'a, [u8]>> {
    skip_space(bytes, pos);
    if *bytes.get(*pos)? != b'/' {
        return None;
    }
    let start = *pos + 1;
    let mut end = start;
    while bytes
        .get(end)
        .is_some_and(|b| !is_space(*b) && !is_delimiter(*b))
    {
        end += 1;
    }
    *pos = end;
    let raw = &bytes[start..end];
    if !raw.contains(&b'#') {
        return Some(Cow::Borrowed(raw));
    }
    let hex = |b: Option<&u8>| b.and_then(|b| (*b as char).to_digit(16));
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'#' {
            if let (Some(hi), Some(lo)) = (hex(raw.get(i + 1)), hex(raw.get(i + 2))) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(raw[i]);
        i += 1;
    }
    Some(Cow::Owned(out))
}

fn number<T: std::str::FromStr>(word: &[u8]) -> Option<T> {
    std::str::from_utf8(word).ok()?.parse().ok()
}

fn number_of(word: &[u8]) -> Option<u32> {
    number(word)
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\r' | b'\n' | b'\t' | 0x0c | 0)
}

fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

fn skip_space(bytes: &[u8], at: &mut usize) {
    while bytes.get(*at).is_some_and(|b| is_space(*b)) {
        *at += 1;
    }
}

/// The next word of regular characters; `at` moves past it.
fn token<'a>(bytes: &'a [u8], at: &mut usize) -> Option<&'a [u8]> {
    skip_space(bytes, at);
    let start = *at;
    while bytes
        .get(*at)
        .is_some_and(|b| !is_space(*b) && !is_delimiter(*b))
    {
        *at += 1;
    }
    (*at > start).then(|| &bytes[start..*at])
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

    fn unsafe_file(file: &[u8]) -> bool {
        leaves_metadata_in_the_clear(file, &mut |_| {})
    }

    /// A file in the form PDFium writes: objects with a classic table and a trailer.
    fn file(objects: &[&str], trailer: &str) -> Vec<u8> {
        let mut out = b"%PDF-1.7\r\n%\xA1\xB3\xC5\xD7\r\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            out.extend_from_slice(b"\r\n");
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\r\n{body}\r\nendobj\r\n", i + 1).as_bytes());
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
    const PROTECTED: &str = "/Root 1 0 R /Encrypt 2 0 R /Size 5";
    const CLEAR: &str = "<</EncryptMetadata false/Filter/Standard>>";
    const ENCRYPTED: &str = "<</Filter/Standard/V 4>>";

    fn catalog(metadata: &str) -> String {
        format!("<</Metadata {metadata} 0 R /Pages 3 0 R /Type/Catalog>>")
    }

    fn four(encrypt: &str, xmp: &str) -> Vec<u8> {
        file(&[&catalog("4"), encrypt, "<</Type/Pages>>", xmp], PROTECTED)
    }

    #[test]
    fn a_file_without_a_metadata_stream_is_fine() {
        let f = file(
            &[
                "<</Pages 3 0 R /Type/Catalog>>",
                ENCRYPTED,
                "<</Type/Pages>>",
            ],
            "/Root 1 0 R /Encrypt 2 0 R /Size 4",
        );
        assert!(!unsafe_file(&f));
    }

    #[test]
    fn a_file_that_is_not_protected_is_fine_whatever_it_holds() {
        let f = file(
            &[&catalog("2"), XMP, "<</Type/Pages>>"],
            "/Root 1 0 R /Size 4",
        );
        assert!(!unsafe_file(&f));
    }

    #[test]
    fn a_metadata_stream_in_a_file_that_encrypts_it_is_not_safe() {
        assert!(unsafe_file(&four(ENCRYPTED, XMP)));
        assert!(unsafe_file(&four(
            "<</Filter/Standard/EncryptMetadata true>>",
            XMP
        )));
    }

    #[test]
    fn the_catalog_stream_of_a_file_that_leaves_it_plain_is_fine() {
        assert!(!unsafe_file(&four(CLEAR, XMP)));
    }

    #[test]
    fn a_plain_stream_that_is_not_xml_is_not_fine() {
        // What PDFium writes for a compressed stream: the compressed bytes, and no `/Filter`.
        let squashed = "<</Length 5/Subtype/XML/Type/Metadata>>stream\r\nx\u{9c}abc\r\nendstream";
        assert!(unsafe_file(&four(CLEAR, squashed)));
        assert!(looks_like_xml(b"\xEF\xBB\xBF \r\n<?xpacket"));
        assert!(looks_like_xml(b"\xFE\xFF\0<\0?"));
        assert!(!looks_like_xml(b"x\x9c\x01"));
        assert!(!looks_like_xml(b""));
    }

    #[test]
    fn a_second_stream_or_one_the_catalog_does_not_name_is_not_fine() {
        let f = file(
            &[&catalog("4"), CLEAR, "<</Type/Pages>>", XMP, XMP],
            "/Root 1 0 R /Encrypt 2 0 R /Size 6",
        );
        assert!(unsafe_file(&f));
        let f = file(
            &[
                "<</Pages 3 0 R /Type/Catalog>>",
                CLEAR,
                "<</Type/Pages>>",
                XMP,
            ],
            PROTECTED,
        );
        assert!(unsafe_file(&f));
    }

    #[test]
    fn a_stream_with_another_type_or_subtype_is_not_a_metadata_stream() {
        let f = file(
            &[
                &catalog("4"),
                ENCRYPTED,
                "<</Type/Pages>>",
                "<</Length 3/Subtype/XML/Type/EmbeddedFile>>stream\r\nabc\r\nendstream",
                "<</Length 3/Subtype/Image/Type/Metadata>>stream\r\nabc\r\nendstream",
            ],
            "/Root 1 0 R /Encrypt 2 0 R /Size 6",
        );
        // The second is a stream of that type, though not an XML one: the check does not tell,
        // and the answer is the careful one. The first is not a metadata stream at all.
        assert!(unsafe_file(&f));
        let f = file(
            &[
                &catalog("4"),
                ENCRYPTED,
                "<</Type/Pages>>",
                "<</Length 3/Subtype/XML/Type/EmbeddedFile>>stream\r\nabc\r\nendstream",
            ],
            PROTECTED,
        );
        assert!(!unsafe_file(&f));
    }

    #[test]
    fn a_metadata_name_in_the_data_of_a_stream_is_a_reason_to_refuse() {
        // The name is counted by the second opinion wherever it is, and no dictionary of the table
        // accounts for it. The check is careful: it asks for the stream and does not find it.
        let fake = "<</Length 60>>stream
5 0 obj
<</Subtype/XML/Type/Metadata>>stream
endstream";
        for encrypt in [ENCRYPTED, CLEAR] {
            let f = file(
                &[
                    "<</Pages 3 0 R /Type/Catalog>>",
                    encrypt,
                    "<</Type/Pages>>",
                    fake,
                ],
                PROTECTED,
            );
            assert!(unsafe_file(&f), "{encrypt}");
        }
    }

    #[test]
    fn a_file_that_cannot_be_read_is_not_safe_when_it_names_a_metadata_stream() {
        assert!(unsafe_file(b"%PDF-1.7\r\n/Type/Metadata"));
        assert!(!unsafe_file(b"%PDF-1.7\r\nnothing here"));
        let mut f = four(ENCRYPTED, XMP);
        let at = rfind(&f, b"startxref").unwrap();
        f.truncate(at);
        assert!(unsafe_file(&f));
    }

    // ---- crafted files: every one of these is unsafe, and was not found so by a search for words

    #[test]
    fn a_name_that_looks_like_a_keyword_does_not_hide_a_stream() {
        for word in ["stream", "endobj", "obj"] {
            let xmp = format!(
                "<</A/{word}/Length 12/Subtype/XML/Type/Metadata>>stream\r\n<x:xmpmeta/>\r\nendstream"
            );
            assert!(unsafe_file(&four(ENCRYPTED, &xmp)), "{word}");
        }
    }

    #[test]
    fn encrypt_metadata_counts_at_the_top_level_of_the_encryption_dictionary_only() {
        for encrypt in [
            "<</Filter/Standard/CF<</StdCF<</EncryptMetadata false>>>>>>",
            "<</Filter/Standard/Note(/EncryptMetadata false)>>",
            "<</Filter/Standard/Note(a \\) /EncryptMetadata false)>>",
            "<</Filter/Standard/List[/EncryptMetadata false]>>",
            "<</Filter/Standard/EncryptMetadata/false>>",
            "<</Filter/Standard/EncryptMetadata(false)>>",
        ] {
            assert!(unsafe_file(&four(encrypt, XMP)), "{encrypt}");
        }
        // The same word where PDFium reads it, spelled with escapes in the name.
        assert!(!unsafe_file(&four(
            "<</Filter/Standard/EncryptMetadat#61 false>>",
            XMP
        )));
    }

    #[test]
    fn keys_of_the_trailer_are_read_at_the_top_level_only() {
        let objects = [&catalog("4"), CLEAR, "<</Type/Pages>>", XMP];
        // A key inside an array that comes first does not hide the real one.
        let f = file(&objects, "/AAA[/Encrypt 5 0 R]/Root 1 0 R /Encrypt 2 0 R");
        assert!(!unsafe_file(&f));
        // And the real one is the one that counts: here it is the unsafe one.
        let objects = [&catalog("4"), ENCRYPTED, "<</Type/Pages>>", XMP, CLEAR];
        let f = file(
            &objects,
            "/AAA[/Encrypt 5 0 R]/Root 1 0 R /Encrypt 2 0 R /Size 6",
        );
        assert!(unsafe_file(&f));
        let f = file(
            &objects,
            "/AAA<</Encrypt 5 0 R>>/BBB(/Encrypt 5 0 R)/Root 1 0 R /Encrypt 2 0 R /Size 6",
        );
        assert!(unsafe_file(&f));
        // The catalog named in an array does not count either.
        let objects = [
            &catalog("4"),
            CLEAR,
            "<</Type/Pages>>",
            XMP,
            "<</Metadata 9 0 R>>",
        ];
        let f = file(
            &objects,
            "/AAA[/Root 5 0 R]/Root 1 0 R /Encrypt 2 0 R /Size 6",
        );
        assert!(!unsafe_file(&f));
        // A key twice is not a file to trust.
        let f = file(&objects, "/Root 1 0 R /Root 5 0 R /Encrypt 2 0 R /Size 6");
        assert!(unsafe_file(&f));
    }

    #[test]
    fn a_metadata_name_that_the_structure_does_not_show_is_counted_by_the_second_opinion() {
        // A dictionary that does not read (a key twice) is not trusted.
        let twice = "<</Type/Metadata/Type/Metadata/Subtype/XML/Length 12>>stream
<a/>
endstream";
        assert!(unsafe_file(&four(CLEAR, twice)));
        // A type name below the top level of a dictionary is not the type of an object, and the
        // stream that is meant to be the clear one is not alone with its name any more.
        let nested = "<</A<</Type/Metadata>>>>";
        let f = file(
            &[&catalog("4"), CLEAR, "<</Type/Pages>>", XMP, nested],
            "/Root 1 0 R /Encrypt 2 0 R /Size 6",
        );
        assert!(unsafe_file(&f));
    }

    #[test]
    fn objects_that_are_only_a_number_apart_cannot_be_told_apart_and_are_not_trusted() {
        let mut f = four(CLEAR, XMP);
        // Two entries of the table that point to one place.
        let table = rfind(&f, b"\r\nxref\r\n").unwrap();
        let first = find(&f, b" 00000 n", table).unwrap();
        let second = find(&f, b" 00000 n", first + 1).unwrap();
        let (a, b) = (first - 10, second - 10);
        let copy: Vec<u8> = f[a..a + 10].to_vec();
        f[b..b + 10].copy_from_slice(&copy);
        assert!(unsafe_file(&f));
    }

    #[test]
    fn a_name_that_looks_like_a_keyword_does_not_hide_a_stream_in_a_file_that_leaves_the_metadata_clear(
    ) {
        // The structure is read in this branch. A second stream whose dictionary has a name that
        // looks like a keyword is found, and the file is refused.
        for word in ["stream", "endobj", "obj"] {
            let second = format!(
                "<</A/{word}/Length 12/Subtype/XML/Type/Metadata>>stream\r\n<x:xmpmeta/>\r\nendstream"
            );
            let f = file(
                &[&catalog("4"), CLEAR, "<</Type/Pages>>", XMP, &second],
                "/Root 1 0 R /Encrypt 2 0 R /Size 6",
            );
            assert!(unsafe_file(&f), "{word}");
            // The same name in the dictionary of the clear stream itself changes nothing.
            assert!(!unsafe_file(&four(CLEAR, &second)), "{word}");
        }
        // A string with an escaped parenthesis and the key inside it, before the real key.
        let encrypt =
            "<</Filter/Standard/Note(a \\) /EncryptMetadata true)/EncryptMetadata false>>";
        assert!(!unsafe_file(&four(encrypt, XMP)));
    }

    #[test]
    fn a_dictionary_of_many_keys_is_read_in_one_pass() {
        // The keys of a trailer and of an encryption dictionary come from the input.
        let keys: String = (0..200_000)
            .map(|i| format!("/K{i} 1"))
            .collect::<Vec<_>>()
            .join(" ");
        let encrypt = format!("<</EncryptMetadata false/Filter/Standard {keys}>>");
        let f = file(
            &[&catalog("4"), &encrypt, "<</Type/Pages>>", XMP],
            PROTECTED,
        );
        let start = std::time::Instant::now();
        assert!(!unsafe_file(&f));
        let took = start.elapsed();
        eprintln!("an encryption dictionary of 200 000 keys: {took:?}");
        assert!(took.as_millis() < 1000, "took {took:?}");
        let f = file(
            &[&catalog("4"), CLEAR, "<</Type/Pages>>", XMP],
            &format!("{PROTECTED} {keys}"),
        );
        let start = std::time::Instant::now();
        assert!(!unsafe_file(&f));
        let took = start.elapsed();
        eprintln!("a trailer of 200 000 keys: {took:?}");
        assert!(took.as_millis() < 1000, "took {took:?}");
        // A key twice is still found among them.
        let f = file(
            &[&catalog("4"), CLEAR, "<</Type/Pages>>", XMP],
            &format!("{PROTECTED} {keys} /K777 2"),
        );
        assert!(unsafe_file(&f));
    }

    #[test]
    fn deep_nesting_is_not_read() {
        let deep = format!(
            "<</Filter/Standard/X{}{}>>",
            "[".repeat(100),
            "]".repeat(100)
        );
        assert!(unsafe_file(&four(&deep, XMP)));
        let f = file(&[&catalog("4"), &deep, "<</Type/Pages>>", XMP], PROTECTED);
        assert!(unsafe_file(&f));
    }

    #[test]
    fn a_file_of_a_hundred_thousand_objects_is_checked_in_one_pass() {
        // The metadata is meant to be clear, so that every object is read. The objects are small,
        // as the structure elements of a tagged document are.
        let mut bodies: Vec<String> = vec![
            catalog("4"),
            CLEAR.to_string(),
            "<</Type/Pages>>".to_string(),
            XMP.to_string(),
        ];
        for i in 0..100_000 {
            bodies.push(format!("<</Type/StructElem/S/P/P 3 0 R/K {i}/Pg 3 0 R>>"));
        }
        let refs: Vec<&str> = bodies.iter().map(String::as_str).collect();
        let f = file(&refs, "/Root 1 0 R /Encrypt 2 0 R /Size 100005");
        let start = std::time::Instant::now();
        assert!(!unsafe_file(&f));
        let took = start.elapsed();
        eprintln!("a hundred thousand objects: {took:?}");
        // A debug build is slow; the bound is far from what a pass over the file takes, and far
        // from what a search from every object to the next stream took (minutes).
        assert!(took.as_millis() < 1000, "took {took:?}");
        // And when the metadata is encrypted no object is read at all.
        bodies[1] = ENCRYPTED.to_string();
        let refs: Vec<&str> = bodies.iter().map(String::as_str).collect();
        let f = file(&refs, "/Root 1 0 R /Encrypt 2 0 R /Size 100005");
        let start = std::time::Instant::now();
        assert!(unsafe_file(&f));
        assert!(start.elapsed().as_millis() < 1000);
    }

    #[test]
    fn progress_is_reported_while_a_big_file_is_read() {
        let filler = "<</Type/Filler/Length 1>>stream\r\nx\r\nendstream";
        let mut bodies: Vec<String> = vec![
            catalog("4"),
            CLEAR.to_string(),
            "<</Type/Pages>>".to_string(),
            XMP.to_string(),
        ];
        let big = format!(
            "<</Length {0}>>stream\r\n{1}\r\nendstream",
            5 << 20,
            " ".repeat(5 << 20)
        );
        bodies.push(big.clone());
        bodies.push(big);
        bodies.push(filler.to_string());
        let refs: Vec<&str> = bodies.iter().map(String::as_str).collect();
        let f = file(&refs, "/Root 1 0 R /Encrypt 2 0 R /Size 8");
        let mut calls = 0;
        assert!(!leaves_metadata_in_the_clear(&f, &mut |_| calls += 1));
        assert!(calls >= 1);
    }
}
