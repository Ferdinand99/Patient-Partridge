//! The page tree of a document, read out of the bytes PDFium writes.
//!
//! The page tools change the list of pages through PDFium, and PDFium trusts that list more than
//! it should: a list of kids with an entry that is not a dictionary stops the process (a null
//! pointer in the walk that finds the place of a page), and a node whose count is wrong makes it
//! delete or insert at another place than asked. PDFium has no call that shows the tree, so before
//! a change that goes through the tree this module reads the tree out of a copy of the document
//! as PDFium itself writes it (a classic table of the places of the objects, no object streams)
//! and says whether the tree is one the change can be trusted with.
//!
//! This is a small reader of PDF objects, not a search for words: the table gives the place of
//! each object, an object is read with the rules of the format for names, strings, hexadecimal
//! strings, comments, arrays, dictionaries and references, and `/Kids`, `/Count` and the rest may
//! be written in place or be references, as the format lets them be. Nothing that cannot be read
//! is taken as sound: the answer is then "could not be checked", which is a different thing from
//! "damaged" and is worded as such.
//!
//! The same walk shows what a page inherits from the nodes above it (`/Resources`, `/MediaBox`,
//! `/CropBox`, `/Rotate`). PDFium's move of pages puts a page under another node and keeps none of
//! what the old node gave it, so a document whose nodes below the root hold such entries needs the
//! entries written onto its pages before pages are moved (see [`hoisted`]).

use std::collections::HashSet;
use std::ops::Range;

/// PDFium's own limit on the depth of a page tree.
const MAX_LEVELS: usize = 1024;
/// Most nodes (inner nodes and pages together) one walk looks at.
const MAX_NODES: usize = 500_000;
/// Deepest nesting of arrays and dictionaries that is skipped over.
const MAX_NESTING: usize = 10_000;
/// Most keys in a dictionary the walk reads.
const MAX_KEYS: usize = 65_536;
/// How many references are followed to a value.
const MAX_HOPS: usize = 16;
/// The part at the end of the file where `startxref` is looked for.
const TAIL: usize = 2048;

/// Why a page tree is not one a change can be trusted with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeError {
    /// What was read breaks the rules of the format in a way that matters here: a place in a list
    /// of kids holds something that is not a page or a node, a node says it has another number of
    /// pages than it lists, and so on.
    Damaged(&'static str),
    /// The tree could not be read: a part of it was written in a way this reader does not follow.
    /// This says nothing about the document.
    Unreadable(&'static str),
}

impl TreeError {
    /// What is the matter, in a few words.
    pub fn reason(&self) -> String {
        match self {
            TreeError::Damaged(why) | TreeError::Unreadable(why) => (*why).to_string(),
        }
    }

    /// The words that tell what is the matter, for a refusal.
    pub fn words(&self) -> String {
        match self {
            TreeError::Damaged(why) => format!(
                "the list of pages of this document does not follow the format ({why}), so its pages cannot be changed"
            ),
            TreeError::Unreadable(why) => format!(
                "the list of pages of this document could not be checked ({why}), so its pages cannot be changed. This says nothing about the document itself"
            ),
        }
    }
}

type Found<T> = Result<T, TreeError>;

fn is_white(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

fn is_regular(b: u8) -> bool {
    !is_white(b) && !is_delimiter(b)
}

/// The position after white space and comments.
fn skip_white(b: &[u8], mut p: usize) -> usize {
    loop {
        match b.get(p) {
            Some(&c) if is_white(c) => p += 1,
            Some(b'%') => {
                while let Some(&c) = b.get(p) {
                    if c == b'\n' || c == b'\r' {
                        break;
                    }
                    p += 1;
                }
            }
            _ => return p,
        }
    }
}

/// The end of the run of ordinary characters that starts at `p`.
fn token_end(b: &[u8], mut p: usize) -> usize {
    while b.get(p).is_some_and(|&c| is_regular(c)) {
        p += 1;
    }
    p
}

/// The end of the name that starts at `p` (at the slash).
fn name_end(b: &[u8], p: usize) -> usize {
    token_end(b, p + 1)
}

/// The end of the string in round brackets that starts at `p`. Brackets nest, a backslash takes
/// the next character.
fn string_end(b: &[u8], p: usize) -> Option<usize> {
    let mut open = 1usize;
    let mut q = p + 1;
    loop {
        match *b.get(q)? {
            b'\\' => q += 2,
            b'(' => {
                open += 1;
                q += 1;
            }
            b')' => {
                open -= 1;
                q += 1;
                if open == 0 {
                    return Some(q);
                }
            }
            _ => q += 1,
        }
    }
}

/// The end of the hexadecimal string that starts at `p`.
fn hex_end(b: &[u8], p: usize) -> Option<usize> {
    let close = b.get(p + 1..)?.iter().position(|&c| c == b'>')?;
    Some(p + 1 + close + 1)
}

fn is_uint(text: &[u8]) -> bool {
    !text.is_empty() && text.len() <= 10 && text.iter().all(u8::is_ascii_digit)
}

fn uint(text: &[u8]) -> Option<u64> {
    if !is_uint(text) {
        return None;
    }
    std::str::from_utf8(text).ok()?.parse().ok()
}

/// If the integer token `start..end` begins a reference (`N G R`), the end of the reference.
fn reference_end(b: &[u8], start: usize, end: usize) -> Option<usize> {
    if !is_uint(&b[start..end]) {
        return None;
    }
    let p = skip_white(b, end);
    let q = token_end(b, p);
    if !is_uint(&b[p..q]) {
        return None;
    }
    let p = skip_white(b, q);
    let r = token_end(b, p);
    (&b[p..r] == b"R").then_some(r)
}

/// The end of the object that starts at `start` (after white space), without reading what is in
/// it: arrays and dictionaries are followed to their end with strings, names and comments skipped
/// as the format says.
fn skip_value(b: &[u8], start: usize) -> Option<usize> {
    let mut stack: Vec<u8> = Vec::new();
    let mut p = start;
    loop {
        p = skip_white(b, p);
        let c = *b.get(p)?;
        let end = match c {
            b'<' if b.get(p + 1) == Some(&b'<') => {
                stack.push(b'd');
                if stack.len() > MAX_NESTING {
                    return None;
                }
                p += 2;
                continue;
            }
            b'>' if b.get(p + 1) == Some(&b'>') => {
                if stack.pop()? != b'd' {
                    return None;
                }
                p += 2;
                if stack.is_empty() {
                    return Some(p);
                }
                continue;
            }
            b'[' => {
                stack.push(b'a');
                if stack.len() > MAX_NESTING {
                    return None;
                }
                p += 1;
                continue;
            }
            b']' => {
                if stack.pop()? != b'a' {
                    return None;
                }
                p += 1;
                if stack.is_empty() {
                    return Some(p);
                }
                continue;
            }
            b'(' => string_end(b, p)?,
            b'<' => hex_end(b, p)?,
            b'/' => name_end(b, p),
            b')' | b'>' | b'{' | b'}' => return None,
            _ => token_end(b, p),
        };
        if end <= p {
            return None;
        }
        if stack.is_empty() {
            return Some(reference_end(b, p, end).unwrap_or(end));
        }
        p = end;
    }
}

/// The name as the format reads it: `#41` is `A`.
fn decode_name(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'#' && i + 2 < raw.len() {
            let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
            if let (Some(h), Some(l)) = (hex(raw[i + 1]), hex(raw[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(raw[i]);
        i += 1;
    }
    out
}

/// The entries of the dictionary at `value`: the decoded key and the place of its value. A key that
/// stands twice counts with its last value, as PDFium keeps it.
fn dictionary(b: &[u8], value: &Range<usize>) -> Option<Vec<(Vec<u8>, Range<usize>)>> {
    if !b.get(value.clone())?.starts_with(b"<<") {
        return None;
    }
    let mut entries: Vec<(Vec<u8>, Range<usize>)> = Vec::new();
    let mut p = value.start + 2;
    loop {
        p = skip_white(b, p);
        if b.get(p..p + 2) == Some(b">>") {
            p += 2;
            break;
        }
        if b.get(p) != Some(&b'/') {
            return None;
        }
        let key_end = name_end(b, p);
        let key = decode_name(&b[p + 1..key_end]);
        let start = skip_white(b, key_end);
        let end = skip_value(b, start)?;
        if entries.len() >= MAX_KEYS {
            return None;
        }
        entries.push((key, start..end));
        p = end;
    }
    (p <= value.end).then_some(entries)
}

fn entry<'a>(entries: &'a [(Vec<u8>, Range<usize>)], key: &[u8]) -> Option<&'a Range<usize>> {
    entries
        .iter()
        .rev()
        .find(|(k, _)| k.as_slice() == key)
        .map(|(_, v)| v)
}

/// The object number of the reference that `value` is, if it is one and nothing else.
fn reference_number(b: &[u8], value: &Range<usize>) -> Option<u32> {
    let end = token_end(b, value.start);
    if reference_end(b, value.start, end)? != value.end {
        return None;
    }
    u32::try_from(uint(&b[value.start..end])?).ok()
}

/// The items of the array at `value`.
fn array_items(b: &[u8], value: &Range<usize>) -> Option<Vec<Range<usize>>> {
    if b.get(value.start) != Some(&b'[') {
        return None;
    }
    let mut items = Vec::new();
    let mut p = value.start + 1;
    loop {
        p = skip_white(b, p);
        match b.get(p)? {
            b']' => return (p < value.end).then_some(items),
            _ => {
                let end = skip_value(b, p)?;
                items.push(p..end);
                p = end;
            }
        }
    }
}

fn number(b: &[u8], value: &Range<usize>) -> Option<i64> {
    let text = std::str::from_utf8(b.get(value.clone())?).ok()?;
    let text = text.trim();
    if let Ok(n) = text.parse::<i64>() {
        return Some(n);
    }
    let real: f64 = text.parse().ok()?;
    (real.is_finite() && real.abs() < 1e15).then_some(real as i64)
}

/// The keys a page can inherit from the nodes above it.
pub const INHERITED: [&str; 4] = ["Resources", "MediaBox", "CropBox", "Rotate"];

/// The table of the places of the objects, and the trailer, of a document PDFium wrote.
struct Table<'a> {
    bytes: &'a [u8],
    sections: Vec<Section>,
    trailer: Vec<(Vec<u8>, Range<usize>)>,
    /// Where the table starts, as the file says.
    start: usize,
}

struct Section {
    first: u64,
    count: u64,
    /// The place of the first entry.
    at: usize,
}

impl<'a> Table<'a> {
    fn read(bytes: &'a [u8]) -> Found<Table<'a>> {
        let unreadable = |why| Err(TreeError::Unreadable(why));
        let tail = bytes.len().saturating_sub(TAIL);
        let Some(word) = bytes[tail..]
            .windows(9)
            .rposition(|w| w == b"startxref")
            .map(|at| tail + at)
        else {
            return unreadable("the file has no table of its objects");
        };
        let p = skip_white(bytes, word + 9);
        let q = token_end(bytes, p);
        let Some(start) = uint(&bytes[p..q]).and_then(|n| usize::try_from(n).ok()) else {
            return unreadable("the place of the table of objects is not a number");
        };
        if !bytes.get(start..).is_some_and(|t| t.starts_with(b"xref")) {
            return unreadable("the table of objects is of another kind");
        }
        let mut sections = Vec::new();
        let mut p = start + 4;
        loop {
            p = skip_white(bytes, p);
            if bytes.get(p..).is_some_and(|t| t.starts_with(b"trailer")) {
                p += 7;
                break;
            }
            let q = token_end(bytes, p);
            let Some(first) = uint(&bytes[p..q]) else {
                return unreadable("a part of the table of objects cannot be read");
            };
            let p2 = skip_white(bytes, q);
            let q2 = token_end(bytes, p2);
            let Some(count) = uint(&bytes[p2..q2]) else {
                return unreadable("a part of the table of objects cannot be read");
            };
            let at = skip_white(bytes, q2);
            let Some(size) = usize::try_from(count).ok().and_then(|c| c.checked_mul(20)) else {
                return unreadable("the table of objects is too long");
            };
            if at.checked_add(size).is_none_or(|end| end > bytes.len()) {
                return unreadable("the table of objects is cut short");
            }
            if sections.len() >= 100_000 {
                return unreadable("the table of objects has too many parts");
            }
            sections.push(Section { first, count, at });
            p = at + size;
        }
        let value_start = skip_white(bytes, p);
        let Some(end) = skip_value(bytes, value_start) else {
            return unreadable("the trailer cannot be read");
        };
        let Some(trailer) = dictionary(bytes, &(value_start..end)) else {
            return unreadable("the trailer cannot be read");
        };
        if entry(&trailer, b"Prev").is_some() || entry(&trailer, b"XRefStm").is_some() {
            return unreadable("the file is written in more than one part");
        }
        Ok(Table {
            bytes,
            sections,
            trailer,
            start,
        })
    }

    /// The place and generation of object `number`; `None` when the table has no such object (or
    /// calls it free).
    fn place(&self, number: u32) -> Found<Option<(usize, u32)>> {
        let number = u64::from(number);
        let Some(s) = self
            .sections
            .iter()
            .find(|s| number >= s.first && number - s.first < s.count)
        else {
            return Ok(None);
        };
        let at = s.at + 20 * (number - s.first) as usize;
        let e = &self.bytes[at..at + 20];
        let shape = e[..10].iter().all(u8::is_ascii_digit)
            && e[10] == b' '
            && e[11..16].iter().all(u8::is_ascii_digit)
            && e[16] == b' '
            && matches!(e[17], b'n' | b'f')
            && is_white(e[18])
            && is_white(e[19]);
        if !shape {
            return Err(TreeError::Unreadable(
                "an entry of the table of objects cannot be read",
            ));
        }
        if e[17] == b'f' {
            return Ok(None);
        }
        let offset = uint(&e[..10]).and_then(|n| usize::try_from(n).ok());
        let generation = uint(&e[11..16]).and_then(|n| u32::try_from(n).ok());
        match (offset, generation) {
            (Some(o), Some(g)) if o < self.bytes.len() => Ok(Some((o, g))),
            _ => Err(TreeError::Unreadable(
                "an entry of the table of objects cannot be read",
            )),
        }
    }

    /// The value of object `number`: the place of what stands between `obj` and `endobj`. `None`
    /// for an object that is not there.
    fn object(&self, number: u32) -> Found<Option<Object>> {
        let Some((offset, generation)) = self.place(number)? else {
            return Ok(None);
        };
        let b = self.bytes;
        let mismatch = Err(TreeError::Unreadable(
            "the table of objects does not match the objects",
        ));
        let p = skip_white(b, offset);
        let q = token_end(b, p);
        if uint(&b[p..q]) != Some(u64::from(number)) {
            return mismatch;
        }
        let p = skip_white(b, q);
        let q = token_end(b, p);
        if uint(&b[p..q]) != Some(u64::from(generation)) {
            return mismatch;
        }
        let p = skip_white(b, q);
        let q = token_end(b, p);
        if &b[p..q] != b"obj" {
            return mismatch;
        }
        let start = skip_white(b, q);
        let Some(end) = skip_value(b, start) else {
            return Err(TreeError::Unreadable("an object cannot be read"));
        };
        Ok(Some(Object {
            value: start..end,
            generation,
        }))
    }

    /// Follows references from `value` to the value they lead to. `None` when a reference leads
    /// to an object that is not there.
    fn resolve(&self, value: &Range<usize>) -> Found<Option<Range<usize>>> {
        let mut value = value.clone();
        for _ in 0..MAX_HOPS {
            match reference_number(self.bytes, &value) {
                Some(n) => match self.object(n)? {
                    Some(o) => value = o.value,
                    None => return Ok(None),
                },
                None => return Ok(Some(value)),
            }
        }
        Err(TreeError::Unreadable("a chain of references is too long"))
    }

    fn name_of(&self, value: &Range<usize>) -> Found<Option<Vec<u8>>> {
        Ok(self.resolve(value)?.and_then(|v| {
            (self.bytes.get(v.start) == Some(&b'/'))
                .then(|| decode_name(&self.bytes[v.start + 1..v.end]))
        }))
    }
}

struct Object {
    value: Range<usize>,
    generation: u32,
}

/// One node or page as read.
struct Node {
    number: u32,
    entries: Vec<(Vec<u8>, Range<usize>)>,
    value: Range<usize>,
    generation: u32,
}

/// Where the value to write onto a page comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A value that stands in the bytes: the one the page inherits.
    Bytes(Range<usize>),
    /// The value a page has when nothing above it gives it one, written out so that a node it is
    /// moved under cannot give it one.
    Text(&'static str),
}

/// What to write onto a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hoist {
    /// The object number of the page.
    pub page: u32,
    pub generation: u32,
    /// The place of the page's own dictionary in the bytes.
    pub dictionary: Range<usize>,
    /// The keys and their values.
    pub values: Vec<(&'static str, Source)>,
}

/// What a walk of the tree found.
#[derive(Debug)]
pub struct Tree {
    /// The object number of each page, in the order of the document.
    pub pages: Vec<u32>,
    /// Whether the document is encrypted (its strings cannot be copied from one object to another).
    pub encrypted: bool,
    /// The pages that need entries written onto them before they can be moved.
    pub hoists: Vec<Hoist>,
    /// The keys that some node below the root holds.
    pub kept_by_inner_nodes: Vec<&'static str>,
    start: usize,
    size: Option<Range<usize>>,
    root: Option<Range<usize>>,
    info: Option<Range<usize>>,
    id: Option<Range<usize>>,
}

struct Frame {
    node: Node,
    kids: Vec<Range<usize>>,
    next: usize,
    claimed: Option<i64>,
    leaves: usize,
}

fn node_of(table: &Table<'_>, number: u32) -> Found<Node> {
    let Some(object) = table.object(number)? else {
        return Err(TreeError::Damaged(
            "a place in a list of kids holds something that is not there",
        ));
    };
    let Some(entries) = dictionary(table.bytes, &object.value) else {
        return Err(TreeError::Damaged(
            "a place in a list of kids holds something that is not a page or a node",
        ));
    };
    Ok(Node {
        number,
        entries,
        value: object.value,
        generation: object.generation,
    })
}

/// Reads the tree of the document written in `bytes`. `pages` is the number of pages PDFium says
/// the document has; the tree must list as many. `parents` also asks that every page and node
/// names the node that lists it as its parent, which is what the inheritance of a page follows.
pub fn read(bytes: &[u8], pages: usize, parents: bool) -> Found<Tree> {
    let table = Table::read(bytes)?;
    let Some(root_entry) = entry(&table.trailer, b"Root") else {
        return Err(TreeError::Unreadable("the file names no catalog"));
    };
    let Some(catalog) = table.resolve(root_entry)? else {
        return Err(TreeError::Unreadable("the catalog is not there"));
    };
    let Some(catalog) = dictionary(bytes, &catalog) else {
        return Err(TreeError::Unreadable("the catalog cannot be read"));
    };
    let Some(root_pages) = entry(&catalog, b"Pages") else {
        return Err(TreeError::Damaged("the catalog has no list of pages"));
    };
    let Some(root_number) = reference_number(bytes, root_pages) else {
        return Err(TreeError::Unreadable(
            "the list of pages is not an object of its own",
        ));
    };

    let mut tree = Tree {
        pages: Vec::new(),
        encrypted: entry(&table.trailer, b"Encrypt").is_some(),
        hoists: Vec::new(),
        kept_by_inner_nodes: Vec::new(),
        start: table.start,
        size: entry(&table.trailer, b"Size").cloned(),
        root: Some(root_entry.clone()),
        info: entry(&table.trailer, b"Info").cloned(),
        id: entry(&table.trailer, b"ID").cloned(),
    };
    let mut visited: HashSet<u32> = HashSet::new();
    let mut seen_nodes = 0usize;
    let mut stack: Vec<Frame> = Vec::new();
    // For each key, whether a node below the root holds it.
    let mut inner_holds = [false; 4];
    // The pages to look at again once everything is known: the node chain is only on the stack now.
    struct Leaf {
        node: Node,
        inherited: [Option<Range<usize>>; 4],
    }
    let mut leaves: Vec<Leaf> = Vec::new();

    let open = |table: &Table<'_>, node: Node| -> Found<Frame> {
        let b = table.bytes;
        let kind = match entry(&node.entries, b"Type") {
            Some(v) => table.name_of(v)?,
            None => None,
        };
        if kind.as_deref().is_some_and(|k| k != b"Pages") {
            return Err(TreeError::Damaged(
                "a node of the list of pages says it is not a node",
            ));
        }
        let Some(kids_value) = entry(&node.entries, b"Kids") else {
            return Err(TreeError::Damaged(
                "a node of the list of pages has no list of kids",
            ));
        };
        let Some(kids_value) = table.resolve(kids_value)? else {
            return Err(TreeError::Damaged(
                "the list of kids of a node is not there",
            ));
        };
        let Some(kids) = array_items(b, &kids_value) else {
            return Err(TreeError::Damaged("the kids of a node are not a list"));
        };
        let claimed = match entry(&node.entries, b"Count") {
            Some(v) => match table.resolve(v)? {
                Some(v) => number(b, &v),
                None => None,
            },
            None => None,
        };
        if claimed.is_none() {
            return Err(TreeError::Damaged(
                "a node of the list of pages does not give the number of its pages",
            ));
        }
        Ok(Frame {
            node,
            kids,
            next: 0,
            claimed,
            leaves: 0,
        })
    };

    visited.insert(root_number);
    let root_node = node_of(&table, root_number)?;
    if entry(&root_node.entries, b"Kids").is_none() {
        return Err(TreeError::Damaged("the list of pages has no list of kids"));
    }
    stack.push(open(&table, root_node)?);
    seen_nodes += 1;

    while let Some(top) = stack.len().checked_sub(1) {
        if stack[top].next >= stack[top].kids.len() {
            // The node is done: it must hold as many pages as it says.
            let done = stack.pop().expect("the stack has a top");
            if let Some(claimed) = done.claimed {
                if claimed != done.leaves as i64 {
                    return Err(TreeError::Damaged(
                        "a node says it has another number of pages than the ones it lists",
                    ));
                }
            }
            if let Some(parent) = stack.last_mut() {
                parent.leaves += done.leaves;
            }
            continue;
        }
        let kid = stack[top].kids[stack[top].next].clone();
        stack[top].next += 1;
        seen_nodes += 1;
        if seen_nodes > MAX_NODES {
            return Err(TreeError::Unreadable("the list of pages is too big"));
        }
        let container = stack[top].node.number;
        let Some(number) = reference_number(bytes, &kid) else {
            return Err(if bytes.get(kid.start..kid.start + 2) == Some(b"<<") {
                TreeError::Unreadable("a page is written in the list of its parent")
            } else {
                TreeError::Damaged("a list of kids holds something that is not a reference")
            });
        };
        let node = node_of(&table, number)?;
        if parents {
            let named = match entry(&node.entries, b"Parent") {
                Some(v) => reference_number(bytes, v),
                None => None,
            };
            if named != Some(container) {
                return Err(TreeError::Damaged(
                    "a page or a node does not name the node that lists it as its parent",
                ));
            }
        }
        if entry(&node.entries, b"Kids").is_some() {
            if !visited.insert(number) {
                return Err(TreeError::Damaged(
                    "a node of the list of pages is listed twice or leads back to itself",
                ));
            }
            if stack.len() >= MAX_LEVELS {
                return Err(TreeError::Unreadable("the list of pages is too deep"));
            }
            for (i, key) in INHERITED.iter().enumerate() {
                if entry(&node.entries, key.as_bytes()).is_some() {
                    inner_holds[i] = true;
                }
            }
            let frame = open(&table, node)?;
            stack.push(frame);
        } else {
            let kind = match entry(&node.entries, b"Type") {
                Some(v) => table.name_of(v)?,
                None => None,
            };
            if kind.as_deref().is_some_and(|k| k != b"Page") {
                return Err(TreeError::Damaged(
                    "a page of the list of pages says it is not a page",
                ));
            }
            if tree.pages.len() >= pages {
                return Err(TreeError::Damaged(
                    "the list names more pages than the document has",
                ));
            }
            let mut inherited: [Option<Range<usize>>; 4] = [None, None, None, None];
            for (i, key) in INHERITED.iter().enumerate() {
                if entry(&node.entries, key.as_bytes()).is_some() {
                    continue;
                }
                inherited[i] = stack
                    .iter()
                    .rev()
                    .find_map(|f| entry(&f.node.entries, key.as_bytes()).cloned());
            }
            tree.pages.push(number);
            stack[top].leaves += 1;
            leaves.push(Leaf { node, inherited });
        }
    }
    if tree.pages.len() != pages {
        return Err(TreeError::Damaged(
            "the document has another number of pages than the list names",
        ));
    }

    for (i, key) in INHERITED.iter().enumerate() {
        if inner_holds[i] {
            tree.kept_by_inner_nodes.push(key);
        }
    }
    let mut written: HashSet<u32> = HashSet::new();
    for leaf in leaves {
        let mut values = Vec::new();
        // The size the page has, its own or the one it inherits: the crop box is this when nothing
        // gives the page one.
        let media = entry(&leaf.node.entries, b"MediaBox")
            .cloned()
            .or_else(|| leaf.inherited[1].clone());
        for (i, key) in INHERITED.iter().enumerate() {
            // Only what some node below the root holds can change for a page that moves; and a page
            // keeps what it has of its own.
            if !inner_holds[i] || entry(&leaf.node.entries, key.as_bytes()).is_some() {
                continue;
            }
            let source = match (&leaf.inherited[i], i) {
                (Some(v), _) => Source::Bytes(v.clone()),
                (None, 0) => Source::Text("<< >>"),
                (None, 1) => Source::Text("[0 0 612 792]"),
                (None, 2) => match &media {
                    Some(m) => Source::Bytes(m.clone()),
                    None => Source::Text("[0 0 612 792]"),
                },
                (None, _) => Source::Text("0"),
            };
            values.push((*key, source));
        }
        if values.is_empty() || !written.insert(leaf.node.number) {
            continue;
        }
        tree.hoists.push(Hoist {
            page: leaf.node.number,
            generation: leaf.node.generation,
            dictionary: leaf.node.value,
            values,
        });
    }
    Ok(tree)
}

/// `bytes` with the entries of `tree.hoists` written onto their pages, as a part added to the end
/// of the file (the way a program saves a change without writing the file again): the pages that
/// get entries are written once more with the entries added, followed by a table of those objects
/// and a trailer that points back to the table before. Nothing else is touched. A page that has an
/// entry of its own keeps it.
pub fn hoisted(bytes: &[u8], tree: &Tree) -> Found<Vec<u8>> {
    if tree.hoists.is_empty() {
        return Ok(bytes.to_vec());
    }
    let cannot = |why| Err(TreeError::Unreadable(why));
    let (Some(size), Some(root)) = (&tree.size, &tree.root) else {
        return cannot("the trailer of the file is not complete");
    };
    if tree.encrypted {
        // Strings are encrypted with a key of the object they stand in; copied to another object
        // they would not read. Names and numbers are not.
        let has_string = tree.hoists.iter().any(|h| {
            h.values.iter().any(|(_, v)| {
                let Source::Bytes(v) = v else {
                    return false;
                };
                let mut p = v.start;
                while p < v.end {
                    match bytes[p] {
                        b'(' => return true,
                        b'<' if bytes.get(p + 1) != Some(&b'<') => return true,
                        b'<' | b'>' => p += 1,
                        _ => {}
                    }
                    p += 1;
                }
                false
            })
        });
        if has_string {
            return cannot("the document is encrypted and the entries hold text");
        }
    }
    let mut out = Vec::with_capacity(bytes.len() + tree.hoists.len() * 256);
    out.extend_from_slice(bytes);
    if !out.ends_with(b"\n") {
        out.push(b'\n');
    }
    let mut hoists: Vec<&Hoist> = tree.hoists.iter().collect();
    hoists.sort_by_key(|h| h.page);
    let mut places: Vec<(u32, u32, usize)> = Vec::with_capacity(hoists.len());
    for h in hoists {
        let dictionary = &bytes[h.dictionary.clone()];
        let Some(body) = dictionary.strip_suffix(b">>") else {
            return cannot("a page does not end the way a dictionary does");
        };
        // Between the dictionary and the end of the object there may be only white space.
        let after = skip_white(bytes, h.dictionary.end);
        if !bytes[after..].starts_with(b"endobj") {
            return cannot("a page is followed by more than a dictionary");
        }
        places.push((h.page, h.generation, out.len()));
        out.extend_from_slice(format!("{} {} obj\n", h.page, h.generation).as_bytes());
        out.extend_from_slice(body);
        for (key, value) in &h.values {
            out.extend_from_slice(b"\n/");
            out.extend_from_slice(key.as_bytes());
            out.push(b' ');
            match value {
                Source::Bytes(range) => out.extend_from_slice(&bytes[range.clone()]),
                Source::Text(text) => out.extend_from_slice(text.as_bytes()),
            }
        }
        out.extend_from_slice(b"\n>>\nendobj\n");
    }
    let table_at = out.len();
    out.extend_from_slice(b"xref\n");
    for (number, generation, at) in &places {
        out.extend_from_slice(format!("{number} 1\n{at:010} {generation:05} n \n").as_bytes());
    }
    out.extend_from_slice(b"trailer\n<< /Size ");
    out.extend_from_slice(&bytes[size.clone()]);
    out.extend_from_slice(b" /Root ");
    out.extend_from_slice(&bytes[root.clone()]);
    for (key, value) in [(&b" /Info "[..], &tree.info), (&b" /ID "[..], &tree.id)] {
        if let Some(value) = value {
            out.extend_from_slice(key);
            out.extend_from_slice(&bytes[value.clone()]);
        }
    }
    out.extend_from_slice(
        format!(" /Prev {} >>\nstartxref\n{table_at}\n%%EOF\n", tree.start).as_bytes(),
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document of the shape PDFium writes: objects, a classic table, a trailer.
    fn document(objects: &[&str], trailer_extra: &str) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut places = vec![0usize];
        for (i, body) in objects.iter().enumerate() {
            places.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let table = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n", places.len()).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for at in &places[1..] {
            out.extend_from_slice(format!("{at:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R {trailer_extra} >>\nstartxref\n{table}\n%%EOF\n",
                places.len()
            )
            .as_bytes(),
        );
        out
    }

    const PAGE: &str = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>";

    fn flat(kids: &str, count: usize) -> Vec<u8> {
        document(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                &format!("<< /Type /Pages /Kids [{kids}] /Count {count} >>"),
                PAGE,
                PAGE,
                PAGE,
            ],
            "",
        )
    }

    #[test]
    fn an_ordinary_tree_is_read() {
        let tree = read(&flat("3 0 R 4 0 R 5 0 R", 3), 3, true).unwrap();
        assert_eq!(tree.pages, vec![3, 4, 5]);
        assert!(tree.hoists.is_empty());
    }

    #[test]
    fn kids_may_be_a_reference_and_count_too() {
        let bytes = document(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages /Kids 6 0 R /Count 7 0 R >>",
                PAGE,
                PAGE,
                PAGE,
                "[3 0 R 4 0 R 5 0 R]",
                "3",
            ],
            "",
        );
        assert_eq!(read(&bytes, 3, true).unwrap().pages, vec![3, 4, 5]);
    }

    #[test]
    fn words_inside_strings_and_names_are_not_taken_for_syntax() {
        let node = "<< /Comment (a stream of pages) /Other (endobj >> ) /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /Type /Pages /N#61me /x >>";
        let bytes = document(
            &["<< /Type /Catalog /Pages 2 0 R >>", node, PAGE, PAGE, PAGE],
            "",
        );
        assert_eq!(read(&bytes, 3, true).unwrap().pages, vec![3, 4, 5]);
        // A key written with an escape is the key it spells.
        let spelled = document(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages /K#69ds [3 0 R 4 0 R 5 0 R] /Count 3 >>",
                PAGE,
                PAGE,
                PAGE,
            ],
            "",
        );
        assert_eq!(read(&spelled, 3, true).unwrap().pages, vec![3, 4, 5]);
    }

    #[test]
    fn a_comment_is_skipped() {
        let bytes = document(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages % ] >> endobj\n /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>",
                PAGE,
                PAGE,
                PAGE,
            ],
            "",
        );
        assert_eq!(read(&bytes, 3, true).unwrap().pages, vec![3, 4, 5]);
    }

    #[test]
    fn a_kid_that_is_not_there_or_not_a_reference_is_damaged() {
        for kids in [
            "3 0 R 4 0 R 5 0 R 99 0 R",
            "3 0 R 4 0 R 5 0 R /MediaBox [0 0 0 0]",
            "3 0 R 4 0 R 5 0 R 42",
            "3 0 R 4 0 R 5 0 R (text)",
        ] {
            let got = read(&flat(kids, 3), 3, false);
            assert!(matches!(got, Err(TreeError::Damaged(_))), "{kids}: {got:?}");
        }
    }

    #[test]
    fn a_kid_that_is_a_null_object_is_damaged() {
        let bytes = document(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 6 0 R] /Count 3 >>",
                PAGE,
                PAGE,
                PAGE,
                "null",
            ],
            "",
        );
        assert!(matches!(read(&bytes, 3, false), Err(TreeError::Damaged(_))));
    }

    #[test]
    fn the_counts_must_add_up() {
        assert!(matches!(
            read(&flat("3 0 R 4 0 R 5 0 R", 2), 3, false),
            Err(TreeError::Damaged(_))
        ));
        assert!(matches!(
            read(&flat("3 0 R 4 0 R 5 0 R", 3), 2, false),
            Err(TreeError::Damaged(_))
        ));
        let inner = document(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 3 >>",
                "<< /Type /Pages /Parent 2 0 R /Kids [4 0 R 6 0 R] /Count 1 >>",
                "<< /Type /Page /Parent 3 0 R >>",
                "<< /Type /Page /Parent 2 0 R >>",
                "<< /Type /Page /Parent 3 0 R >>",
            ],
            "",
        );
        assert!(matches!(read(&inner, 3, true), Err(TreeError::Damaged(_))));
    }

    #[test]
    fn a_node_that_leads_back_or_is_listed_twice_is_damaged() {
        let round = document(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
                "<< /Type /Pages /Parent 2 0 R /Kids [2 0 R 4 0 R] /Count 1 >>",
                "<< /Type /Page /Parent 3 0 R >>",
            ],
            "",
        );
        assert!(matches!(read(&round, 1, true), Err(TreeError::Damaged(_))));
    }

    #[test]
    fn a_table_that_cannot_be_read_is_unreadable_and_never_sound() {
        let mut bytes = flat("3 0 R 4 0 R 5 0 R", 3);
        let at = bytes.windows(4).rposition(|w| w == b"xref").unwrap();
        bytes[at] = b'y';
        assert!(matches!(
            read(&bytes, 3, false),
            Err(TreeError::Unreadable(_))
        ));
        assert!(matches!(read(b"", 0, false), Err(TreeError::Unreadable(_))));
        assert!(matches!(
            read(b"%PDF-1.7\nstartxref\n999999\n%%EOF", 0, false),
            Err(TreeError::Unreadable(_))
        ));
    }

    #[test]
    fn a_big_object_number_is_read() {
        // Object 2000000 in a table of one part with a hole in it.
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut at = Vec::new();
        for (n, body) in [
            (1u32, "<< /Type /Catalog /Pages 2 0 R >>"),
            (2, "<< /Type /Pages /Kids [3 0 R 2000000 0 R] /Count 2 >>"),
            (3, "<< /Type /Page /Parent 2 0 R >>"),
            (2_000_000, "<< /Type /Page /Parent 2 0 R >>"),
        ] {
            at.push((n, out.len()));
            out.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
        }
        let table = out.len();
        out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 3\n");
        for (_, a) in &at[..3] {
            out.extend_from_slice(format!("{a:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("2000000 1\n{:010} 00000 n \n", at[3].1).as_bytes());
        out.extend_from_slice(
            format!("trailer\n<< /Size 2000001 /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n")
                .as_bytes(),
        );
        assert_eq!(read(&out, 2, true).unwrap().pages, vec![3, 2_000_000]);
    }

    #[test]
    fn entries_that_pages_inherit_from_inner_nodes_are_found() {
        let bytes = document(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 3 /MediaBox [0 0 50 50] /Rotate 90 >>",
                "<< /Type /Pages /Parent 2 0 R /Kids [5 0 R 6 0 R] /Count 2 /Resources << /Font << >> >> /MediaBox [0 0 99 99] >>",
                "<< /Type /Page /Parent 2 0 R >>",
                "<< /Type /Page /Parent 3 0 R >>",
                "<< /Type /Page /Parent 3 0 R /MediaBox [0 0 7 7] >>",
            ],
            "",
        );
        let tree = read(&bytes, 3, true).unwrap();
        assert_eq!(tree.pages, vec![5, 6, 4]);
        assert_eq!(tree.kept_by_inner_nodes, vec!["Resources", "MediaBox"]);
        let text = |h: &Hoist| {
            h.values
                .iter()
                .map(|(k, v)| {
                    let text = match v {
                        Source::Bytes(r) => std::str::from_utf8(&bytes[r.clone()]).unwrap(),
                        Source::Text(t) => t,
                    };
                    format!("{k}={text}")
                })
                .collect::<Vec<_>>()
        };
        let by_page = |n: u32| tree.hoists.iter().find(|h| h.page == n).map(text);
        // Page 4 sits under the root and gets the root's size, so that no node it moves under can
        // change it, and no resources, so that a node it moves under cannot give it some. Rotate is
        // held by the root only: left alone.
        assert_eq!(
            by_page(4),
            Some(vec![
                "Resources=<< >>".to_string(),
                "MediaBox=[0 0 50 50]".to_string()
            ])
        );
        assert_eq!(
            by_page(5),
            Some(vec![
                "Resources=<< /Font << >> >>".to_string(),
                "MediaBox=[0 0 99 99]".to_string()
            ])
        );
        assert_eq!(
            by_page(6),
            Some(vec!["Resources=<< /Font << >> >>".to_string()])
        );
    }

    #[test]
    fn a_tree_that_keeps_things_at_the_root_only_needs_no_hoist() {
        let bytes = document(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 50 50] /Rotate 90 /Resources << >> >>",
                "<< /Type /Page /Parent 2 0 R >>",
                "<< /Type /Page /Parent 2 0 R /Rotate 180 >>",
            ],
            "",
        );
        let tree = read(&bytes, 2, true).unwrap();
        assert!(tree.hoists.is_empty());
        assert!(tree.kept_by_inner_nodes.is_empty());
    }

    #[test]
    fn the_added_part_is_a_well_formed_update_that_names_the_old_table() {
        let bytes = document(
            &[
                "<< /Type /Catalog /Pages 2 0 R >>",
                "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
                "<< /Type /Pages /Parent 2 0 R /Kids [4 0 R] /Count 1 /Rotate 90 >>",
                "<< /Type /Page /Parent 3 0 R >>",
            ],
            "/Info 1 0 R",
        );
        let tree = read(&bytes, 1, true).unwrap();
        let out = hoisted(&bytes, &tree).unwrap();
        assert!(out.starts_with(&bytes));
        let added = String::from_utf8(out[bytes.len()..].to_vec()).unwrap();
        assert!(
            added.contains("4 0 obj\n<< /Type /Page /Parent 3 0 R \n/Rotate 90\n>>\nendobj"),
            "{added}"
        );
        assert!(added.contains("xref\n4 1\n"), "{added}");
        assert!(added.contains("/Prev "), "{added}");
        assert!(added.contains("/Info 1 0 R"), "{added}");
        // The entries of the table are 20 bytes each.
        let table = added.find("xref\n4 1\n").unwrap() + 9;
        assert_eq!(&added[table + 19..table + 20], "\n");
        assert_eq!(added[table..].find("trailer").unwrap(), 20);
    }
}
