//! The form designer's writer: makes form fields in a document, changes the ones it made, and
//! takes them away.
//!
//! PDFium cannot create form fields, and its calls cannot set the integer and array entries a
//! field needs, so the designer writes the objects itself. It reads the bytes PDFium wrote when it
//! saved the document (a plain file: objects in the open, one cross reference table), makes the new
//! and the changed objects, and appends them as an incremental update: new objects for the fields,
//! their widgets and the pictures of the check boxes, radio buttons and places to sign, and new
//! revisions of the page lists of annotations and of the document's form. The engine then loads the
//! result as the document, and the next save is again PDFium's own full rewrite, which also drops
//! what the update replaced.
//!
//! This is a second writer of PDF structure next to PDFium, and its input is the output of a parse
//! of a hostile file, so it fails closed: whatever it cannot find or does not understand, it
//! refuses, and it writes nothing until every change of a batch has worked. A batch is one reload.
//!
//! A new field never carries an action or a script. There is no way to ask for one.
//!
//! The module does not use PDFium. The geometry of the pages it needs (their turn and the part that
//! is visible) is given to it, so it can be tried on any bytes.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use partridge_protocol::{AuthorOp, DesignedWidget, ErrorCode, MAX_PAGES};

use crate::pagetree;

pub mod appearance;
pub mod file;
pub mod geometry;
pub mod rules;
pub mod syntax;

mod build;
pub(crate) mod glue;
mod model;
mod ops;

pub use file::Unsupported;
pub use geometry::PageGeo;

use file::Parsed;
use syntax::{get, parse_ref, skip_ws};

/// Why the designer did not do what it was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorError {
    /// It cannot work on this document at all.
    Unsupported(String),
    /// It cannot make this change; the message says why for the reader.
    Refused(String),
}

impl AuthorError {
    pub fn into_failure(self) -> (ErrorCode, String) {
        match self {
            AuthorError::Unsupported(m) => (ErrorCode::AuthorUnsupported, m),
            AuthorError::Refused(m) => (ErrorCode::AuthorRefused, m),
        }
    }

    pub fn message(&self) -> &str {
        match self {
            AuthorError::Unsupported(m) | AuthorError::Refused(m) => m,
        }
    }
}

impl From<Unsupported> for AuthorError {
    fn from(u: Unsupported) -> Self {
        AuthorError::Unsupported(u.message())
    }
}

/// The document is built in a way the designer does not work with. `why` is for the one who reads
/// the code and the tests; the reader of the document is told it in plain words.
pub(crate) fn odd(why: &str) -> AuthorError {
    let _ = why;
    AuthorError::Unsupported(
        "this document is built in a way the form designer cannot work with yet".to_string(),
    )
}

/// Which part of the document an object is, for the objects the writer keeps a copy of and writes
/// back at the end. Two parts that are one object would write over each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Catalog,
    Form,
    Fields,
    Resources,
    Fonts,
    Page(usize),
    Annots(usize),
}

/// Where a part of the form lives in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    /// As a value inside the dictionary above it.
    Direct,
    /// As an object of its own.
    Object(u32),
    /// Not there yet.
    Missing,
}

/// The annotations of a page, as the page lists them.
pub(crate) struct PageAnnots {
    /// The text of each entry (a reference, or a dictionary written in place).
    pub items: Vec<Vec<u8>>,
    /// For each entry, the place it had in the list before: `None` for one that is new.
    pub map: Vec<Option<usize>>,
    pub place: Place,
    /// The list differs from the file's, so the page gets a new revision.
    pub dirty: bool,
}

pub(crate) struct FieldList {
    pub items: Vec<Vec<u8>>,
    pub place: Place,
}

/// The form of the document: its dictionary and the list of its fields.
pub(crate) struct Acro {
    pub place: Place,
    pub dict: Vec<u8>,
    pub fields: Option<FieldList>,
    pub touched: bool,
}

/// What the designer did to the document.
pub struct Outcome {
    /// The bytes of the document with the update appended.
    pub bytes: Vec<u8>,
    /// For each page whose list of annotations changed: for every entry of the new list, the place
    /// it had in the old list, or `None` for an entry that is new.
    pub maps: BTreeMap<usize, Vec<Option<usize>>>,
    /// The pages with a widget that is new or has a new look: they are drawn once so that PDFium
    /// makes the pictures of the fields that have none.
    pub redraw: BTreeSet<usize>,
}

pub struct Author {
    pub(crate) file: Parsed,
    pub(crate) geo: HashMap<usize, PageGeo>,
    pub(crate) page_objs: Vec<u32>,
    pub(crate) root: u32,
    /// Objects that are new or have a new revision, by number.
    pub(crate) out: BTreeMap<u32, Vec<u8>>,
    pub(crate) next: u32,
    pub(crate) pages: BTreeMap<usize, PageAnnots>,
    pub(crate) catalog: Option<Vec<u8>>,
    pub(crate) acro: Option<Acro>,
    pub(crate) walk: Option<model::Walk>,
    pub(crate) owners: Option<HashMap<u32, usize>>,
    pub(crate) helv: Option<u32>,
    pub(crate) zadb: Option<u32>,
    pub(crate) fonts_in_dr: bool,
    pub(crate) redraw: BTreeSet<usize>,
    /// The parts of the document the writer keeps a copy of, by object number.
    pub(crate) claims: HashMap<u32, Role>,
    /// The page objects, for a look at whether an object is a page.
    pub(crate) page_set: HashSet<u32>,
    /// The lists of annotations that more than one page names, found once.
    pub(crate) shared_annots: Option<HashSet<u32>>,
}

impl Author {
    /// Reads the bytes PDFium saved. `geo` holds the geometry of the pages the caller expects to
    /// be named (a page that is named and not there is refused). The pages are the ones the file's
    /// own count says it has.
    pub fn new(bytes: Vec<u8>, geo: HashMap<usize, PageGeo>) -> Result<Author, AuthorError> {
        let file = file::parse(bytes)?;
        let count = Self::declared_count(&file)?;
        Self::open(file, geo, count)
    }

    /// As [`Author::new`], with the number of pages that PDFium says the document has: the page
    /// tree has to list exactly that many.
    pub fn with_count(
        bytes: Vec<u8>,
        geo: HashMap<usize, PageGeo>,
        count: usize,
    ) -> Result<Author, AuthorError> {
        let file = file::parse(bytes)?;
        Self::open(file, geo, count)
    }

    /// The number of pages at the top of the page tree, as the file writes it.
    fn declared_count(file: &Parsed) -> Result<usize, AuthorError> {
        let root = get(&file.trailer, "Root")
            .and_then(parse_ref)
            .map(|r| r.0)
            .ok_or_else(|| odd("no catalog"))?;
        let catalog = file.obj(root).ok_or_else(|| odd("no catalog"))?;
        let top = get(catalog, "Pages")
            .and_then(parse_ref)
            .map(|r| r.0)
            .ok_or_else(|| odd("no pages"))?;
        let tree = file.obj(top).ok_or_else(|| odd("no page tree"))?;
        get(tree, "Count")
            .and_then(syntax::parse_int)
            .and_then(|c| usize::try_from(c).ok())
            .filter(|c| *c <= MAX_PAGES)
            .ok_or_else(|| odd("no usable count of pages"))
    }

    fn open(
        file: Parsed,
        geo: HashMap<usize, PageGeo>,
        count: usize,
    ) -> Result<Author, AuthorError> {
        let root = get(&file.trailer, "Root")
            .and_then(parse_ref)
            .map(|r| r.0)
            .ok_or_else(|| odd("no catalog"))?;
        let catalog = file.obj(root).ok_or_else(|| odd("no catalog"))?;
        if get(catalog, "Pages").is_none() {
            return Err(odd("no pages"));
        }
        // The pages are found the way the page tools find them, by the reader that checks the
        // counts of the nodes: a node whose count is wrong makes PDFium number the pages other
        // than the file's lists do, and a field would be put on the wrong page.
        let tree = pagetree::read(&file.bytes, count, false).map_err(|e| odd(&e.reason()))?;
        let page_set: HashSet<u32> = tree.pages.iter().copied().collect();
        if page_set.len() != tree.pages.len() {
            // One page object at two places of the order: a change to one page is a change to both.
            return Err(odd("a page that the page tree lists twice"));
        }
        let next = file.size;
        let mut author = Author {
            file,
            geo,
            page_set,
            page_objs: tree.pages,
            root,
            out: BTreeMap::new(),
            next,
            pages: BTreeMap::new(),
            catalog: None,
            acro: None,
            walk: None,
            owners: None,
            helv: None,
            zadb: None,
            fonts_in_dr: false,
            redraw: BTreeSet::new(),
            claims: HashMap::new(),
            shared_annots: None,
        };
        author.load_acro()?;
        Ok(author)
    }

    /// Notes that object `n` is the part `role` of the document. Refuses when it is another part
    /// as well: the revisions of the two would write over each other.
    pub(crate) fn claim(&mut self, n: u32, role: Role) -> Result<(), AuthorError> {
        match self.claims.insert(n, role) {
            Some(other) if other != role => Err(odd("two parts of the document are one object")),
            _ => Ok(()),
        }
    }

    /// True for an object the writer keeps a copy of, or that is a page or the catalog: a field or
    /// a widget must not be one of them.
    pub(crate) fn is_structure(&self, n: u32) -> bool {
        n == self.root || self.page_set.contains(&n) || self.claims.contains_key(&n)
    }

    /// The lists of annotations (objects of their own) that more than one page names.
    pub(crate) fn shared(&mut self) -> &HashSet<u32> {
        if self.shared_annots.is_none() {
            let mut seen = HashSet::new();
            let mut shared = HashSet::new();
            for page in &self.page_objs {
                let list = self
                    .file
                    .obj(*page)
                    .and_then(|d| get(d, "Annots"))
                    .and_then(parse_ref)
                    .map(|r| r.0);
                if let Some(n) = list {
                    if !seen.insert(n) {
                        shared.insert(n);
                    }
                }
            }
            self.shared_annots = Some(shared);
        }
        self.shared_annots.get_or_insert_with(HashSet::new)
    }

    /// The bytes the author was given, when nothing was changed.
    pub fn into_bytes(self) -> Vec<u8> {
        self.file.bytes
    }

    /// How many pages the file has.
    pub fn page_count(&self) -> usize {
        self.page_objs.len()
    }

    /// The text of an object as it is now: the revision made here, or the file's own.
    pub(crate) fn cur(&self, n: u32) -> Option<&[u8]> {
        match self.out.get(&n) {
            Some(v) => Some(v.as_slice()),
            None => self.file.obj(n),
        }
    }

    /// The dictionary of an object as it is now.
    pub(crate) fn dict(&self, n: u32) -> Option<&[u8]> {
        self.cur(n)
            .filter(|d| d[skip_ws(d, 0)..].starts_with(b"<<"))
    }

    /// An object that may be given a new revision: a dictionary or an array that is not the
    /// dictionary of a stream, because the stream would be lost.
    pub(crate) fn revisable(&self, n: u32) -> Result<Vec<u8>, AuthorError> {
        if self.out.contains_key(&n) {
            return Ok(self.out[&n].clone());
        }
        let text = self.file.obj(n).ok_or_else(|| odd("a missing object"))?;
        if self.file.is_stream(n) {
            return Err(odd("a part of the form is a stream"));
        }
        Ok(text.to_vec())
    }

    pub(crate) fn alloc(&mut self) -> Result<u32, AuthorError> {
        if self.next >= file::MAX_OBJECTS {
            return Err(odd("too many objects"));
        }
        let n = self.next;
        self.next += 1;
        Ok(n)
    }

    pub(crate) fn put(&mut self, n: u32, body: impl Into<Vec<u8>>) {
        self.out.insert(n, body.into());
    }

    pub(crate) fn geometry(&self, page: usize) -> Result<PageGeo, AuthorError> {
        self.geo
            .get(&page)
            .copied()
            .ok_or_else(|| rules::refused("That page is not there."))
    }

    /// The names of the fields the form has now, all levels, sorted.
    pub fn names(&mut self) -> Result<Vec<String>, AuthorError> {
        let walk = self.walk()?;
        let mut names: Vec<String> = walk
            .nodes
            .values()
            .filter(|n| !n.name.is_empty())
            .map(|n| n.name.clone())
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// The widgets of a page that the designer lists.
    pub fn list_page(&mut self, page: usize) -> Result<Vec<DesignedWidget>, AuthorError> {
        self.widgets_on(page)
    }

    /// Makes the changes in order. When one fails nothing is kept: the author is not to be used
    /// again.
    pub fn apply(&mut self, ops: &[AuthorOp]) -> Result<(), AuthorError> {
        for op in ops {
            self.op(op)?;
        }
        Ok(())
    }

    /// The bytes of the document with everything that was made appended as one update.
    pub fn finish(mut self) -> Result<Outcome, AuthorError> {
        self.flush_pages()?;
        self.flush_form()?;
        let Author {
            file,
            out,
            next,
            pages,
            root,
            mut redraw,
            ..
        } = self;
        let maps: BTreeMap<usize, Vec<Option<usize>>> =
            pages.into_iter().map(|(i, p)| (i, p.map)).collect();
        redraw.extend(maps.keys().copied());
        let root_ref = file.reference(root);
        let size = file.size;
        let xref_pos = file.xref_pos;
        let generations: BTreeMap<u32, u16> =
            out.keys().map(|n| (*n, file.generation(*n))).collect();
        // The values the trailer keeps are copied as the bytes they are: the identifier of a file
        // is binary, and none of it goes through text.
        let kept: Vec<(&str, Vec<u8>)> = ["Info", "ID"]
            .into_iter()
            .filter_map(|key| get(&file.trailer, key).map(|v| (key, v.to_vec())))
            .collect();
        // What is added is counted first, so that the document's own bytes are not copied into a
        // bigger buffer, and the buffer is not left with room the document will never use.
        let added: usize = out.values().map(|body| body.len() + 48).sum::<usize>()
            + out.len() * (20 + 24)
            + kept
                .iter()
                .map(|(k, v)| k.len() + v.len() + 4)
                .sum::<usize>()
            + 160;
        let mut bytes = file.bytes;
        bytes.reserve_exact(added);
        if !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        let mut offsets: BTreeMap<u32, (usize, u16)> = BTreeMap::new();
        for (n, body) in &out {
            let generation = generations[n];
            offsets.insert(*n, (bytes.len(), generation));
            bytes.extend_from_slice(format!("{n} {generation} obj\n").as_bytes());
            bytes.extend_from_slice(body);
            bytes.extend_from_slice(b"\nendobj\n");
        }
        let xref_at = bytes.len();
        bytes.extend_from_slice(b"xref\n");
        let numbers: Vec<u32> = offsets.keys().copied().collect();
        let mut i = 0;
        while i < numbers.len() {
            let mut j = i;
            while j + 1 < numbers.len() && numbers[j + 1] == numbers[j] + 1 {
                j += 1;
            }
            bytes.extend_from_slice(format!("{} {}\n", numbers[i], j - i + 1).as_bytes());
            for n in &numbers[i..=j] {
                let (offset, generation) = offsets[n];
                bytes.extend_from_slice(format!("{offset:010} {generation:05} n \n").as_bytes());
            }
            i = j + 1;
        }
        let mut trailer = format!(
            "trailer\n<< /Size {} /Root {} /Prev {}",
            next.max(size),
            root_ref,
            xref_pos
        )
        .into_bytes();
        for (key, value) in kept {
            trailer.extend_from_slice(format!(" /{key} ").as_bytes());
            trailer.extend_from_slice(&value);
        }
        trailer.extend_from_slice(b" >>\n");
        bytes.extend_from_slice(&trailer);
        bytes.extend_from_slice(format!("startxref\n{xref_at}\n%%EOF\n").as_bytes());
        Ok(Outcome {
            bytes,
            maps,
            redraw,
        })
    }
}

#[cfg(test)]
mod tests;
