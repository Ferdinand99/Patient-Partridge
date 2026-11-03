//! Writing the empty signature into a document that PDFium has written out.
//!
//! The engine holds no key and no certificate. What it does for a signature is everything except
//! the signature itself: it writes the document out as for a save, reads that file with the same
//! bounded reader the page tools use, and adds a part to the end of it (the way a program saves a
//! change without writing the file again): the signature field with its look, the signature
//! dictionary with a `/ByteRange` that is already right and a hole of the size the app asked for,
//! the catalog and the form that name the field, and the page that shows it. The hole is `<`, as
//! many hexadecimal zeros as the signature will take, and `>`. The app hashes the file except the
//! hole, has the signer make the signature, and writes it into the hole.
//!
//! What is refused, and why:
//!
//! - an encrypted document (the strings and streams of the new objects would need the key of the
//!   document);
//! - a document with an XFA form (a dynamic form is drawn from data, not from the fields, and a
//!   signature over the file would not mean what it looks like);
//! - a document that holds signatures already, or anything that looks like one (a full rewrite by
//!   PDFium has broken them; a new signature has to be added to the original bytes, which this
//!   module does not do);
//! - a document whose structure could not be read, or is damaged where the signature has to go.
//!
//! The reader never trusts a length and counts the bytes it reads, like the page tool reader it
//! is built on.

use std::collections::{BTreeMap, HashSet};
use std::ops::Range;

use partridge_protocol::MAX_SAVE_BYTES;

use crate::pagetree::{
    self, dictionary, entry, number, reference_number, skip_value, skip_white, Entry, Found,
    Object, Table, TreeError,
};
use crate::signlook::{num, run_length, Look};

/// Most fields (all levels) the walk of the form looks at.
const MAX_FIELD_NODES: usize = 200_000;
/// Deepest nesting of fields the walk follows.
const MAX_FIELD_DEPTH: usize = 64;

/// A work limit like the one of the page tree: the walks may read this many times the size of the
/// document, and a little more.
const WORK_TIMES: usize = 8;
const WORK_EXTRA: usize = 1 << 20;

/// What the signature says and where it goes. Nothing in it is secret.
pub struct Spec<'a> {
    /// The place among the pages (from 0).
    pub page: usize,
    pub place: Place<'a>,
    pub name: &'a str,
    pub reason: &'a str,
    pub location: &'a str,
    /// Seconds since 1970, from the clock of the computer.
    pub signed_at: i64,
    /// Bytes to keep free for the signature.
    pub reserve: usize,
    /// The look of the box, for a visible signature.
    pub look: Option<&'a Look>,
}

pub enum Place<'a> {
    /// A signature that shows nothing: a field with no size.
    Invisible,
    /// A new field with this box in the coordinates of the page.
    New { bounds: [f32; 4] },
    /// A field of the document, by its full name.
    Existing { name: &'a str },
}

/// Why the document cannot get a signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Encrypted,
    Xfa,
    AlreadySigned,
    /// The structure of the document could not be read, or is not one the signature can go into.
    Structure(String),
    /// The place, the field or the texts cannot be used.
    Refused(String),
    /// The document with the signature would be bigger than a save may be.
    TooLarge,
}

impl From<TreeError> for Refusal {
    fn from(e: TreeError) -> Refusal {
        Refusal::Structure(e.words().replace(
            "so this change cannot be made to it safely",
            "so it cannot be signed safely",
        ))
    }
}

fn structure(why: &str) -> Refusal {
    Refusal::Structure(format!(
        "the document is built in a way the signature cannot be written into ({why})"
    ))
}

/// The file with the empty signature, and where the hole is.
pub struct Prepared {
    pub bytes: Vec<u8>,
    /// The place of the `<` that opens the hole.
    pub hole_start: usize,
    /// The place just after the `>` that closes it.
    pub hole_end: usize,
    /// The name of the field the signature is in.
    pub field: String,
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || haystack.len() < needle.len() {
        return 0;
    }
    let first = needle[0];
    let mut n = 0;
    let mut i = 0;
    while let Some(at) = haystack[i..].iter().position(|b| *b == first) {
        let at = i + at;
        if haystack.get(at..at + needle.len()) == Some(needle) {
            n += 1;
            i = at + needle.len();
        } else {
            i = at + 1;
        }
    }
    n
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|at| from + at)
}

/// The bytes of a PDF string (a literal or a hexadecimal one) turned into text: UTF-16 with a
/// byte order mark, else the characters of the first 256 code points (which is what PDFDocEncoding
/// is, near enough for names).
pub fn string_value(bytes: &[u8]) -> Option<String> {
    let raw: Vec<u8> = match bytes.first()? {
        b'(' => {
            let inner = bytes.get(1..bytes.len().checked_sub(1)?)?;
            let mut out = Vec::new();
            let mut i = 0;
            while i < inner.len() {
                if inner[i] != b'\\' {
                    out.push(inner[i]);
                    i += 1;
                    continue;
                }
                i += 1;
                let c = *inner.get(i)?;
                i += 1;
                match c {
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'0'..=b'7' => {
                        let mut v = u32::from(c - b'0');
                        for _ in 0..2 {
                            match inner.get(i) {
                                Some(d @ b'0'..=b'7') => {
                                    v = v * 8 + u32::from(d - b'0');
                                    i += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push(v as u8);
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
            out
        }
        b'<' => {
            let inner = bytes.get(1..bytes.len().checked_sub(1)?)?;
            let digits: Vec<u8> = inner.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
            let mut out = Vec::new();
            for pair in digits.chunks(2) {
                let hi = (pair[0] as char).to_digit(16)?;
                let lo = pair.get(1).map_or(Some(0), |d| (*d as char).to_digit(16))?;
                out.push((hi * 16 + lo) as u8);
            }
            out
        }
        _ => return None,
    };
    if raw.starts_with(&[0xfe, 0xff]) {
        let units: Vec<u16> = raw[2..]
            .chunks_exact(2)
            .map(|p| u16::from_be_bytes([p[0], p[1]]))
            .collect();
        return String::from_utf16(&units).ok();
    }
    if raw.starts_with(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8(raw[3..].to_vec()).ok();
    }
    Some(raw.iter().map(|b| *b as char).collect())
}

/// Text as a PDF string: a literal when every character is printable ASCII, else UTF-16 with a
/// byte order mark, in hexadecimal.
pub fn text_string(text: &str) -> String {
    if text.chars().all(|c| (' '..='~').contains(&c)) {
        let mut out = String::from("(");
        for c in text.chars() {
            if matches!(c, '\\' | '(' | ')') {
                out.push('\\');
            }
            out.push(c);
        }
        out.push(')');
        return out;
    }
    let mut out = String::from("<FEFF");
    for unit in text.encode_utf16() {
        out.push_str(&format!("{unit:04X}"));
    }
    out.push('>');
    out
}

/// `D:20261103140500Z`.
fn pdf_date(unix: i64) -> String {
    let (y, m, d) = crate::sigcheck::civil(unix.div_euclid(86_400));
    let s = unix.rem_euclid(86_400);
    format!(
        "(D:{y:04}{m:02}{d:02}{:02}{:02}{:02}Z)",
        s / 3600,
        s % 3600 / 60,
        s % 60
    )
}

// ---- reading and changing dictionaries -----------------------------------------------------------------

/// `<<...>>` for the entries of a dictionary, with the keys of `changes` (each with its new value as
/// text) replaced or added. Keys that stand twice and are changed leave only the new one.
fn changed_dictionary(b: &[u8], entries: &[Entry], changes: &[(&str, String)]) -> String {
    let mut out = String::from("<<");
    for e in entries {
        if changes.iter().any(|(k, _)| e.key == k.as_bytes()) {
            continue;
        }
        let raw = &b[e.at..e.value.start];
        let raw = String::from_utf8_lossy(raw);
        out.push_str(raw.trim_end());
        out.push(' ');
        out.push_str(&String::from_utf8_lossy(&b[e.value.clone()]));
        out.push('\n');
    }
    for (key, value) in changes {
        out.push('/');
        out.push_str(key);
        out.push(' ');
        out.push_str(value);
        out.push('\n');
    }
    out.push_str(">>");
    out
}

/// A list with `reference` added at its end. `value` is the list as written, `[...]`.
fn list_with(b: &[u8], value: &Range<usize>, reference: &str) -> Option<String> {
    let text = std::str::from_utf8(b.get(value.clone())?).ok()?;
    let text = text.trim_end();
    let inner = text.strip_suffix(']')?;
    text.starts_with('[').then(|| format!("{inner} {reference}]"))
}

/// What a walk of the form found about one field.
struct FieldNode {
    name: String,
    number: u32,
    terminal: bool,
    /// The field type, as the file gives it or as the parents give it.
    kind: Option<Vec<u8>>,
    has_value: bool,
    /// The field has no kids.
    merged: bool,
}

/// Every field of the form, all levels: its full name, its object, its type.
fn walk_fields(
    table: &Table<'_>,
    fields: &Range<usize>,
) -> Found<Vec<FieldNode>> {
    let b = table.bytes;
    let mut out: Vec<FieldNode> = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    // (the list of fields to read, the name so far, the type so far, the depth)
    let mut stack: Vec<(Range<usize>, String, Option<Vec<u8>>, usize)> = Vec::new();
    stack.push((fields.clone(), String::new(), None, 0));
    while let Some((list, prefix, kind, depth)) = stack.pop() {
        let Some(list) = table.resolve(&list)? else {
            continue;
        };
        if b.get(list.start) != Some(&b'[') {
            return Err(TreeError::Damaged("the list of fields is not a list"));
        }
        let mut p = list.start + 1;
        loop {
            p = skip_white(b, p);
            if b.get(p) == Some(&b']') || p >= list.end {
                break;
            }
            let Some(end) = skip_value(b, p) else {
                return Err(TreeError::Damaged("the list of fields cannot be read"));
            };
            let item = p..end;
            p = end;
            let Some(n) = reference_number(b, &item) else {
                continue;
            };
            if !seen.insert(n) {
                continue;
            }
            if out.len() >= MAX_FIELD_NODES {
                return Err(TreeError::Unreadable("the form has too many fields"));
            }
            let Some(object) = table.object(n)? else {
                continue;
            };
            let Some(entries) = dictionary(b, &object.value) else {
                continue;
            };
            let own_name = match entry(&entries, b"T") {
                Some(v) => match table.resolve(v)? {
                    Some(v) => string_value(&b[v]).unwrap_or_default(),
                    None => String::new(),
                },
                None => String::new(),
            };
            let name = match (prefix.is_empty(), own_name.is_empty()) {
                (_, true) => prefix.clone(),
                (true, false) => own_name.clone(),
                (false, false) => format!("{prefix}.{own_name}"),
            };
            let own_kind = match entry(&entries, b"FT") {
                Some(v) => table.name_of(v)?,
                None => None,
            };
            let kind_here = own_kind.or_else(|| kind.clone());
            let kids = match entry(&entries, b"Kids") {
                Some(v) => table.resolve(v)?,
                None => None,
            };
            let has_value = match entry(&entries, b"V") {
                Some(v) => match table.resolve(v)? {
                    Some(v) => b.get(v.clone()) != Some(&b"null"[..]),
                    None => false,
                },
                None => false,
            };
            // A field whose kids are all widgets (no name of their own) is the end of the tree; a
            // field with named kids is a parent.
            let mut parent = false;
            if let Some(kids) = &kids {
                if b.get(kids.start) == Some(&b'[') {
                    let mut q = kids.start + 1;
                    loop {
                        q = skip_white(b, q);
                        if b.get(q) == Some(&b']') || q >= kids.end {
                            break;
                        }
                        let Some(kend) = skip_value(b, q) else { break };
                        let kid = q..kend;
                        q = kend;
                        if let Some(kn) = reference_number(b, &kid) {
                            if let Some(ko) = table.object(kn)? {
                                if let Some(ke) = dictionary(b, &ko.value) {
                                    if entry(&ke, b"T").is_some() || entry(&ke, b"Kids").is_some() {
                                        parent = true;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            out.push(FieldNode {
                name: name.clone(),
                number: n,
                terminal: !parent,
                kind: kind_here.clone(),
                has_value,
                merged: kids.is_none(),
            });
            if parent {
                if depth + 1 > MAX_FIELD_DEPTH {
                    return Err(TreeError::Unreadable("the fields are nested too deep"));
                }
                if let Some(v) = entry(&entries, b"Kids") {
                    stack.push((v.clone(), name, kind_here, depth + 1));
                }
            }
        }
    }
    Ok(out)
}

// ---- the update --------------------------------------------------------------------------------------------

/// The objects that are written again or new, by number.
struct Update {
    objects: BTreeMap<u32, (u32, Vec<u8>)>,
    next: u32,
}

impl Update {
    fn rewrite(&mut self, number: u32, generation: u32, body: Vec<u8>) {
        self.objects.insert(number, (generation, body));
    }

    /// The number of a new object. Its body is given to [`Update::rewrite`] (with generation 0).
    fn number(&mut self) -> Result<u32, Refusal> {
        let number = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| structure("too many objects"))?;
        Ok(number)
    }
}

fn stream_object(dictionary: &str, data: &[u8]) -> Vec<u8> {
    let mut out = format!("<<{dictionary} /Length {}>>\nstream\n", data.len()).into_bytes();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\nendstream");
    out
}

/// True when only `endobj` follows the value of an object (so that an object with a stream is
/// never written again as a plain dictionary).
fn plain_object(b: &[u8], value: &Range<usize>) -> bool {
    let after = skip_white(b, value.end);
    b.get(after..).is_some_and(|t| t.starts_with(b"endobj"))
}

/// The dictionary of object `n`, with its entries, for an object that is a plain dictionary.
fn plain_dictionary(
    table: &Table<'_>,
    n: u32,
    what: &str,
) -> Result<(Object, Vec<Entry>), Refusal> {
    let b = table.bytes;
    let object = table
        .object(n)?
        .ok_or_else(|| structure(&format!("the {what} is not there")))?;
    if !plain_object(b, &object.value) {
        return Err(structure(&format!(
            "the {what} is followed by more than a dictionary"
        )));
    }
    let entries = dictionary(b, &object.value)
        .ok_or_else(|| structure(&format!("the {what} cannot be read")))?;
    Ok((object, entries))
}

/// A list that stands as an object of its own, with `reference` added at its end.
fn rewrite_list(
    update: &mut Update,
    table: &Table<'_>,
    n: u32,
    reference: &str,
    what: &str,
) -> Result<(), Refusal> {
    let b = table.bytes;
    let object = table
        .object(n)?
        .ok_or_else(|| structure(&format!("the {what} is not there")))?;
    if !plain_object(b, &object.value) {
        return Err(structure(&format!("the {what} is followed by more than a list")));
    }
    let body = list_with(b, &object.value, reference)
        .ok_or_else(|| structure(&format!("the {what} is not a list")))?;
    update.rewrite(n, object.generation, body.into_bytes());
    Ok(())
}

/// A list that is written in place in a dictionary, or stands as an object of its own, with
/// `reference` added at its end: the text of the change to the dictionary, if the list is in
/// place. An object that holds the list is written again.
fn add_to_list(
    update: &mut Update,
    table: &Table<'_>,
    value: &Range<usize>,
    reference: &str,
    what: &str,
) -> Result<Option<String>, Refusal> {
    let b = table.bytes;
    match reference_number(b, value) {
        Some(n) => {
            rewrite_list(update, table, n, reference, what)?;
            Ok(None)
        }
        None => list_with(b, value, reference)
            .map(Some)
            .ok_or_else(|| structure(&format!("the {what} is not a list"))),
    }
}

/// Adds the empty signature to `base`. `pages` is the number of pages PDFium says the document has.
pub fn prepare(base: Vec<u8>, pages: usize, spec: &Spec<'_>) -> Result<Prepared, Refusal> {
    let b: &[u8] = &base;
    if count(b, b"/ByteRange") > 0 || count(b, b"/DocMDP") > 0 {
        return Err(Refusal::AlreadySigned);
    }
    let limit = b.len().saturating_mul(WORK_TIMES).saturating_add(WORK_EXTRA);
    let table = Table::read(b, limit)?;
    if entry(&table.trailer, b"Encrypt").is_some() {
        return Err(Refusal::Encrypted);
    }
    let tree = pagetree::read(b, pages, false)?;
    if tree.encrypted {
        return Err(Refusal::Encrypted);
    }
    let page_number = *tree
        .pages
        .get(spec.page)
        .ok_or_else(|| Refusal::Refused("the page is not in the document".to_string()))?;

    let size_value =
        entry(&table.trailer, b"Size").ok_or_else(|| structure("the trailer has no size"))?;
    let old_size = number(b, size_value)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| structure("the size of the table of objects is not a number"))?;
    let root_value =
        entry(&table.trailer, b"Root").ok_or_else(|| structure("the file names no catalog"))?;
    let root_number = reference_number(b, root_value)
        .ok_or_else(|| structure("the catalog is not an object"))?;
    let (root, catalog) = plain_dictionary(&table, root_number, "catalog")?;

    let mut update = Update {
        objects: BTreeMap::new(),
        next: old_size,
    };
    let mut catalog_changes: Vec<(&str, String)> = Vec::new();

    // The version: a signature of this kind is a feature of version 1.7.
    let version = match entry(&catalog, b"Version") {
        Some(v) => table.name_of(v)?,
        None => None,
    };
    if version.as_deref().is_none_or(|v| v < b"1.7".as_slice()) {
        catalog_changes.push(("Version", "/1.7".to_string()));
    }

    // The form: where it is, whether it is one that can take a signature, and its fields.
    let acro_value = entry(&catalog, b"AcroForm").cloned();
    let (acro_object, acro_entries): (Option<u32>, Option<Vec<Entry>>) = match &acro_value {
        None => (None, None),
        Some(v) => match reference_number(b, v) {
            Some(n) => {
                let (_, entries) = plain_dictionary(&table, n, "form")?;
                (Some(n), Some(entries))
            }
            None => (
                None,
                Some(dictionary(b, v).ok_or_else(|| structure("the form cannot be read"))?),
            ),
        },
    };
    if acro_entries
        .as_ref()
        .is_some_and(|e| entry(e, b"XFA").is_some())
    {
        return Err(Refusal::Xfa);
    }
    let nodes = match acro_entries.as_ref().and_then(|e| entry(e, b"Fields")) {
        Some(f) => walk_fields(&table, f)?,
        None => Vec::new(),
    };

    // The field: a field of the document, or a new one with a name no field has.
    let mut existing: Option<(u32, Object, Vec<Entry>)> = None;
    let field_name: String = match &spec.place {
        Place::Existing { name } => {
            let Some(node) = nodes.iter().find(|n| n.name == *name && n.terminal) else {
                return Err(Refusal::Refused(
                    "the document has no signature field of that name".to_string(),
                ));
            };
            if node.kind.as_deref() != Some(b"Sig".as_slice()) {
                return Err(Refusal::Refused(
                    "that field is not a signature field".to_string(),
                ));
            }
            if node.has_value {
                return Err(Refusal::AlreadySigned);
            }
            if !node.merged {
                return Err(structure(
                    "the signature field is kept apart from the box that shows it",
                ));
            }
            // Two fields of the same name are not told apart.
            if nodes.iter().filter(|n| n.name == *name).count() > 1 {
                return Err(Refusal::Refused(
                    "more than one field has that name".to_string(),
                ));
            }
            let (object, entries) = plain_dictionary(&table, node.number, "field")?;
            existing = Some((node.number, object, entries));
            (*name).to_string()
        }
        Place::New { .. } | Place::Invisible => (1u32..)
            .map(|n| format!("Signature{n}"))
            .find(|candidate| !nodes.iter().any(|f| f.name == *candidate))
            .expect("a name is free"),
    };

    // The numbers of the new objects: the signature, then its look and the pictures in it, then the
    // field (when it is new).
    let sig_number = update.number()?;
    let look_numbers = match spec.look {
        Some(look) => {
            let form = update.number()?;
            let image = match &look.raster {
                Some(r) => {
                    let alpha = if r.alpha.is_some() {
                        Some(update.number()?)
                    } else {
                        None
                    };
                    Some((update.number()?, alpha))
                }
                None => None,
            };
            Some((form, image))
        }
        None => None,
    };
    let new_widget = if existing.is_none() {
        Some(update.number()?)
    } else {
        None
    };
    let widget_ref = new_widget.map(|n| format!("{n} 0 R"));

    // The form gets the flag that says the document is signed (and, for a new field, the field in
    // its list). It is written in the catalog or, when the catalog names it, in its own object.
    let sig_flags_old = match acro_entries.as_ref().and_then(|e| entry(e, b"SigFlags")) {
        Some(v) => match table.resolve(v)? {
            Some(v) => number(b, &v).unwrap_or(0),
            None => 0,
        },
        None => 0,
    };
    let sig_flags = (sig_flags_old | 3).to_string();
    match &acro_entries {
        Some(entries) => {
            let mut changes: Vec<(&str, String)> = vec![("SigFlags", sig_flags)];
            if let Some(reference) = &widget_ref {
                match entry(entries, b"Fields") {
                    None => changes.push(("Fields", format!("[{reference}]"))),
                    Some(f) => {
                        if let Some(text) = add_to_list(&mut update, &table, f, reference, "list of fields")? {
                            changes.push(("Fields", text));
                        }
                    }
                }
            }
            let text = changed_dictionary(b, entries, &changes);
            match acro_object {
                Some(n) => {
                    let generation = table.object(n)?.map_or(0, |o| o.generation);
                    update.rewrite(n, generation, text.into_bytes());
                }
                None => catalog_changes.push(("AcroForm", text)),
            }
        }
        None => {
            let reference = widget_ref
                .as_ref()
                .ok_or_else(|| structure("the form is not there"))?;
            catalog_changes.push((
                "AcroForm",
                format!("<</Fields [{reference}]\n/SigFlags {sig_flags}>>"),
            ));
        }
    }
    update.rewrite(
        root_number,
        root.generation,
        changed_dictionary(b, &catalog, &catalog_changes).into_bytes(),
    );

    // The page shows a new field.
    if let Some(reference) = &widget_ref {
        let (page_object, page_entries) = plain_dictionary(&table, page_number, "page")?;
        let change = match entry(&page_entries, b"Annots") {
            None => Some(format!("[{reference}]")),
            Some(a) => add_to_list(&mut update, &table, a, reference, "list of annotations")?,
        };
        if let Some(text) = change {
            let text = changed_dictionary(b, &page_entries, &[("Annots", text)]);
            update.rewrite(page_number, page_object.generation, text.into_bytes());
        }
    }

    // The signature dictionary. The byte range and the hole are written as placeholders of their
    // final width, and filled in once the place of everything is known.
    let zeros = "0".repeat(10);
    let mut sig = format!(
        "<</Type /Sig\n/Filter /Adobe.PPKLite\n/SubFilter /ETSI.CAdES.detached\n/ByteRange [{zeros} {zeros} {zeros} {zeros}]\n/Contents <{}>\n/M {}\n/Name {}\n",
        "0".repeat(spec.reserve * 2),
        pdf_date(spec.signed_at),
        text_string(spec.name)
    );
    if !spec.reason.is_empty() {
        sig.push_str(&format!("/Reason {}\n", text_string(spec.reason)));
    }
    if !spec.location.is_empty() {
        sig.push_str(&format!("/Location {}\n", text_string(spec.location)));
    }
    sig.push_str(">>");
    update.rewrite(sig_number, 0, sig.into_bytes());

    // The look.
    if let (Some(look), Some((form_number, image))) = (spec.look, look_numbers) {
        let mut resources = String::from(
            "<</Font <</F1 <</Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding>>>>",
        );
        if let (Some(raster), Some((image_number, alpha_number))) = (&look.raster, image) {
            resources.push_str(&format!("\n/XObject <</Im0 {image_number} 0 R>>"));
            let mut dictionary = format!(
                "\n/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /RunLengthDecode",
                raster.width, raster.height
            );
            if let (Some(alpha), Some(alpha_number)) = (&raster.alpha, alpha_number) {
                dictionary.push_str(&format!(" /SMask {alpha_number} 0 R"));
                let gray = format!(
                    "\n/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /RunLengthDecode",
                    raster.width, raster.height
                );
                update.rewrite(alpha_number, 0, stream_object(&gray, &run_length(alpha)));
            }
            update.rewrite(
                image_number,
                0,
                stream_object(&dictionary, &run_length(&raster.rgb)),
            );
        }
        resources.push_str(">>");
        let form = format!(
            "\n/Type /XObject /Subtype /Form /BBox [0 0 {} {}] /Matrix [{} {} {} {} 0 0] /Resources {resources}",
            num(look.width),
            num(look.height),
            num(look.matrix[0]),
            num(look.matrix[1]),
            num(look.matrix[2]),
            num(look.matrix[3]),
        );
        update.rewrite(form_number, 0, stream_object(&form, &look.content));
    }
    let appearance = look_numbers.map(|(form, _)| form);

    // The field.
    match existing {
        Some((n, object, entries)) => {
            let mut changes: Vec<(&str, String)> = vec![
                ("V", format!("{sig_number} 0 R")),
                ("F", "132".to_string()),
            ];
            if let Some(form) = appearance {
                changes.push(("AP", format!("<</N {form} 0 R>>")));
            }
            update.rewrite(
                n,
                object.generation,
                changed_dictionary(b, &entries, &changes).into_bytes(),
            );
        }
        None => {
            let widget_number = new_widget.expect("a new field has a number");
            let rect = match &spec.place {
                Place::New { bounds } => format!(
                    "[{} {} {} {}]",
                    num(bounds[0]),
                    num(bounds[1]),
                    num(bounds[2]),
                    num(bounds[3])
                ),
                _ => "[0 0 0 0]".to_string(),
            };
            let look = appearance
                .map(|form| format!("/AP <</N {form} 0 R>>\n"))
                .unwrap_or_default();
            let widget = format!(
                "<</Type /Annot /Subtype /Widget /FT /Sig\n/T {}\n/Rect {rect}\n/F 132\n/P {page_number} 0 R\n/V {sig_number} 0 R\n{look}>>",
                text_string(&field_name)
            );
            update.rewrite(widget_number, 0, widget.into_bytes());
        }
    }

    // The bytes: the objects in the order of their numbers, a table of them, and a trailer that
    // points back to the table before.
    let base_len = b.len() + usize::from(!b.ends_with(b"\n"));
    let mut tail: Vec<u8> = Vec::new();
    let mut places: Vec<(u32, u32, usize)> = Vec::with_capacity(update.objects.len());
    for (number, (generation, body)) in &update.objects {
        places.push((*number, *generation, base_len + tail.len()));
        let head = format!("{number} {generation} obj\n");
        let needed = head.len() + body.len() + 8;
        if b.len().saturating_add(tail.len()).saturating_add(needed) > MAX_SAVE_BYTES {
            return Err(Refusal::TooLarge);
        }
        tail.try_reserve(needed).map_err(|_| Refusal::TooLarge)?;
        tail.extend_from_slice(head.as_bytes());
        tail.extend_from_slice(body);
        tail.extend_from_slice(b"\nendobj\n");
    }
    let table_at = base_len + tail.len();
    tail.extend_from_slice(b"xref\n");
    let mut i = 0;
    while i < places.len() {
        // A run of consecutive numbers is one part of the table.
        let mut j = i + 1;
        while j < places.len() && places[j].0 == places[j - 1].0 + 1 {
            j += 1;
        }
        tail.extend_from_slice(format!("{} {}\n", places[i].0, j - i).as_bytes());
        for (_, generation, at) in &places[i..j] {
            tail.extend_from_slice(format!("{at:010} {generation:05} n \n").as_bytes());
        }
        i = j;
    }
    tail.extend_from_slice(format!("trailer\n<< /Size {} /Root ", update.next).as_bytes());
    tail.extend_from_slice(&b[root_value.clone()]);
    for key in ["Info", "ID"] {
        if let Some(v) = entry(&table.trailer, key.as_bytes()) {
            tail.extend_from_slice(format!(" /{key} ").as_bytes());
            tail.extend_from_slice(&b[v.clone()]);
        }
    }
    tail.extend_from_slice(
        format!(" /Prev {} >>\nstartxref\n{table_at}\n%%EOF\n", table.start).as_bytes(),
    );

    let total = base_len + tail.len();
    if total > MAX_SAVE_BYTES {
        return Err(Refusal::TooLarge);
    }
    drop(table);
    let mut bytes = base;
    bytes
        .try_reserve_exact(tail.len() + 1)
        .map_err(|_| Refusal::TooLarge)?;
    if bytes.len() < base_len {
        bytes.push(b'\n');
    }
    bytes.extend_from_slice(&tail);
    debug_assert_eq!(bytes.len(), total);

    // The hole and the byte range, in the part that was added.
    let unwritten = || structure("the signature was not written");
    let range_at = find(&bytes, b"/ByteRange [", base_len).ok_or_else(unwritten)? + 12;
    let contents_at =
        find(&bytes, b"/Contents <", range_at).ok_or_else(unwritten)? + 10;
    let hole_end = contents_at + 1 + spec.reserve * 2 + 1;
    if bytes.get(contents_at) != Some(&b'<') || bytes.get(hole_end - 1) != Some(&b'>') {
        return Err(unwritten());
    }
    let range = format!(
        "{:010} {contents_at:010} {hole_end:010} {:010}",
        0,
        total - hole_end
    );
    debug_assert_eq!(range.len(), 43);
    bytes[range_at..range_at + 43].copy_from_slice(range.as_bytes());
    Ok(Prepared {
        bytes,
        hole_start: contents_at,
        hole_end,
        field: field_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_are_read_the_way_pdf_writes_them() {
        assert_eq!(string_value(b"(Signature1)").as_deref(), Some("Signature1"));
        assert_eq!(string_value(b"(a\\(b\\)\\\\c)").as_deref(), Some("a(b)\\c"));
        assert_eq!(string_value(b"(\\101\\102)").as_deref(), Some("AB"));
        assert_eq!(string_value(b"<FEFF00410042>").as_deref(), Some("AB"));
        assert_eq!(string_value(b"<4142 43>").as_deref(), Some("ABC"));
        assert_eq!(string_value(b"<414>").as_deref(), Some("A@"));
        assert_eq!(string_value(b"(\xe5)").as_deref(), Some("\u{e5}"));
        assert_eq!(string_value(b"/Name"), None);
        assert_eq!(string_value(b""), None);
        assert_eq!(string_value(b"(unfinished"), Some("unfinishe".to_string()));
    }

    #[test]
    fn text_is_written_as_a_literal_or_as_utf_16() {
        assert_eq!(text_string("Anna (Alvorlig)"), "(Anna \\(Alvorlig\\))");
        assert_eq!(text_string("a\\b"), "(a\\\\b)");
        assert_eq!(text_string("\u{c5}se"), "<FEFF00C50073006500>");
        assert_eq!(text_string("\u{1f600}"), "<FEFFD83DDE00>");
        for text in ["plain", "\u{c5}se \u{d8}", "\u{6d4b}\u{8bd5}", "(x)\\"] {
            assert_eq!(string_value(text_string(text).as_bytes()).as_deref(), Some(text));
        }
    }

    #[test]
    fn dates_are_written_in_the_pdf_way() {
        assert_eq!(pdf_date(0), "(D:19700101000000Z)");
        assert_eq!(pdf_date(1_781_524_800), "(D:20260615120000Z)");
        assert_eq!(pdf_date(951_782_400 + 86_399), "(D:20000229235959Z)");
    }

    #[test]
    fn marks_are_counted_whole() {
        assert_eq!(count(b"/ByteRange /ByteRange x /ByteRang", b"/ByteRange"), 2);
        assert_eq!(count(b"", b"/ByteRange"), 0);
        assert_eq!(count(b"//ByteRange", b"/ByteRange"), 1);
    }

    #[test]
    fn a_dictionary_is_written_again_with_its_changes() {
        let text = b"<< /A 1 /B (two) /A 3 /C [1 2] >>";
        let entries = dictionary(text, &(0..text.len())).unwrap();
        let out = changed_dictionary(text, &entries, &[("A", "9".into()), ("D", "/x".into())]);
        let again = dictionary(out.as_bytes(), &(0..out.len())).unwrap();
        let get = |k: &str| {
            let v = entry(&again, k.as_bytes()).unwrap();
            out[v.clone()].to_string()
        };
        assert_eq!(get("A"), "9");
        assert_eq!(get("B"), "(two)");
        assert_eq!(get("C"), "[1 2]");
        assert_eq!(get("D"), "/x");
        assert_eq!(again.iter().filter(|e| e.key == b"A").count(), 1);
    }

    #[test]
    fn a_list_gets_a_reference_at_its_end() {
        let text = b"[1 0 R 2 0 R ]";
        assert_eq!(list_with(text, &(0..text.len()), "7 0 R").unwrap(), "[1 0 R 2 0 R 7 0 R]");
        let text = b"[]";
        assert_eq!(list_with(text, &(0..2), "7 0 R").unwrap(), "[ 7 0 R]");
        let text = b"<< >>";
        assert!(list_with(text, &(0..5), "7 0 R").is_none());
    }
}
