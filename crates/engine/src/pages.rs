//! The page tools: delete, reorder, rotate, crop, insert and extract pages.
//!
//! Everything here changes the document in memory, through PDFium. Nothing is written to disk; the
//! app saves the document with the same requests as always (`SaveBegin` and the pieces), so a page
//! change goes through the checks of the save path like any other change.
//!
//! What PDFium does, and what this module builds on (all of it tested in `tests/pages.rs`):
//!
//! - A save writes only the objects that can still be reached from the document. A page that was
//!   deleted takes its content, its pictures, its fonts, its annotations and their texts with it,
//!   unless something that stays points at them. So a deleted page is gone from the file, not just
//!   hidden. A bookmark that pointed at it stays in the list and leads nowhere.
//! - Rotating and cropping change the page's entries in place. Nothing else is touched.
//! - A page that is copied from another document (an insert, an extract) is copied with everything
//!   it refers to, but not with what refers to it: bookmarks, the form's field tree, links to pages
//!   that come later in the same copy, the structure tree and the signature are not carried over.
//!   PDFium also leaves the `/Parent` of a copied annotation (a pop-up note's note, a field's
//!   widget's field) pointing at the number it had in the other document, which means something
//!   else, or nothing, in this one. That is a limit of the copy in PDFium; the README says so.
//! - A move copies nothing: PDFium's own move changes the order in the page tree, so links,
//!   bookmarks, form fields and the structure of the pages stay as they were. The pages that can
//!   stay in their order are not touched, and the others are moved in a few steps.
//! - A copy of pages points into the document it came from until the document is written out and
//!   read in again. The document is therefore put through exactly that, with the other document
//!   still alive, before the other document is let go of.
//!
//! To take a change back the engine keeps copies of the document as it was (snapshots, in memory,
//! at most [`MAX_UNDO_BYTES`] and [`MAX_UNDO_STEPS`] of them, the oldest let go first), each with
//! the number of pages it must show when it is read in again; a copy that shows another number is
//! refused, because the document it would put in place is not the one that was kept. `Restore`
//! loads one and keeps the state it leaves, so that the same step can be redone. Rotating and
//! cropping are taken back by the app with the opposite change and need no snapshot.

use std::collections::VecDeque;
use std::os::raw::{c_int, c_ulong};

use partridge_protocol::{
    ErrorCode, PageSize, PagesRequest, PagesResponse, ResponseBody, MAX_BLANK_SIDE, MAX_BOX_VALUE,
    MAX_EDIT_PAGES, MAX_IMPORT_BYTES, MAX_SAVE_BYTES, MAX_SAVE_CHUNK, MAX_UNDO_BYTES,
    MAX_UNDO_STEPS, MIN_BLANK_SIDE, MIN_CROP_SIDE,
};
use pdfium_render::prelude::*;

use crate::{fail, sane_size, save, Engine, Failure};

/// A password kept in memory while its document is open (the engine has to load copies of the
/// document with it). It is overwritten with zeros when it is let go of.
pub struct Secret(pub(crate) String);

impl Drop for Secret {
    fn drop(&mut self) {
        // SAFETY: the bytes are overwritten in place and the string is dropped right after, so no
        // one reads it as text again. The writes are volatile so that they are not removed.
        unsafe {
            for byte in self.0.as_bytes_mut() {
                std::ptr::write_volatile(byte, 0);
            }
        }
    }
}

struct Snapshot {
    token: u32,
    bytes: Vec<u8>,
    /// How many pages the document had when the copy was taken, and so must have when it is read
    /// in again.
    pages: usize,
}

struct Import {
    size: usize,
    bytes: Vec<u8>,
}

/// What the page tools keep for one document.
pub struct PageTools {
    snapshots: VecDeque<Snapshot>,
    next_token: u32,
    import: Option<Import>,
    max_undo_bytes: usize,
    max_undo_steps: usize,
    /// Test builds only: the step of the next change at which PDFium is made to fail.
    #[cfg(debug_assertions)]
    fail_at: Option<usize>,
    /// Test builds only: how many pages more than there are the checks after a change see.
    #[cfg(debug_assertions)]
    miscount: isize,
    /// Test builds only: the pages seem to be one place off when a move is checked.
    #[cfg(debug_assertions)]
    scramble: bool,
}

/// Test builds only: true at the step a test chose, to see what a change that fails half way
/// leaves behind.
#[cfg(debug_assertions)]
fn injected(fail_at: Option<usize>, step: usize) -> bool {
    fail_at == Some(step)
}

#[cfg(not(debug_assertions))]
fn injected(_fail_at: Option<usize>, _step: usize) -> bool {
    false
}

impl Default for PageTools {
    fn default() -> Self {
        PageTools {
            snapshots: VecDeque::new(),
            next_token: 1,
            import: None,
            max_undo_bytes: MAX_UNDO_BYTES,
            max_undo_steps: MAX_UNDO_STEPS,
            #[cfg(debug_assertions)]
            fail_at: None,
            #[cfg(debug_assertions)]
            miscount: 0,
            #[cfg(debug_assertions)]
            scramble: false,
        }
    }
}

impl PageTools {
    /// Makes the copies of the document smaller or fewer than they are by default (for tests).
    #[doc(hidden)]
    pub fn with_limits(bytes: usize, steps: usize) -> PageTools {
        PageTools {
            max_undo_bytes: bytes,
            max_undo_steps: steps,
            ..PageTools::default()
        }
    }

    fn keep(&mut self, bytes: Vec<u8>, pages: usize) -> u32 {
        let token = self.next_token;
        self.next_token = self.next_token.wrapping_add(1).max(1);
        self.snapshots.push_back(Snapshot {
            token,
            bytes,
            pages,
        });
        self.evict();
        // The copy just kept may be the one that was let go of, when it alone is too big.
        if self.snapshots.iter().any(|s| s.token == token) {
            token
        } else {
            0
        }
    }

    fn evict(&mut self) {
        let mut total: usize = self.snapshots.iter().map(|s| s.bytes.len()).sum();
        while self.snapshots.len() > self.max_undo_steps || total > self.max_undo_bytes {
            match self.snapshots.pop_front() {
                Some(old) => total -= old.bytes.len(),
                None => break,
            }
        }
    }

    /// Makes the next change fail at its `step`-th step (for tests).
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn failing_at(mut self, step: usize) -> PageTools {
        self.fail_at = Some(step);
        self
    }

    /// Makes the checks of the number of pages after a change see `extra` pages more than there
    /// are (for tests: PDFium itself always counts right, so without this the checks are never put
    /// to the test).
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn miscounting(mut self, extra: isize) -> PageTools {
        self.miscount = extra;
        self
    }

    /// Makes the check of the order after a move see the pages one place off (for tests).
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn scrambling(mut self) -> PageTools {
        self.scramble = true;
        self
    }

    fn scrambled(&self) -> bool {
        #[cfg(debug_assertions)]
        {
            self.scramble
        }
        #[cfg(not(debug_assertions))]
        {
            false
        }
    }

    fn miscount(&self) -> isize {
        #[cfg(debug_assertions)]
        {
            self.miscount
        }
        #[cfg(not(debug_assertions))]
        {
            0
        }
    }

    /// The step a test chose for the change that is starting, once.
    fn fail_at(&mut self) -> Option<usize> {
        #[cfg(debug_assertions)]
        {
            self.fail_at.take()
        }
        #[cfg(not(debug_assertions))]
        {
            None
        }
    }

    /// Keeps a copy of some bytes as a change would, saying that the document in them has `pages`
    /// pages (for tests: a copy that does not read back with that many pages is refused).
    #[doc(hidden)]
    pub fn keep_for_tests(&mut self, bytes: Vec<u8>, pages: usize) -> u32 {
        self.keep(bytes, pages)
    }

    /// How many bytes of copies are held, for tests.
    #[doc(hidden)]
    pub fn held_bytes(&self) -> usize {
        self.snapshots.iter().map(|s| s.bytes.len()).sum()
    }

    /// How many copies are held, for tests.
    #[doc(hidden)]
    pub fn held_steps(&self) -> usize {
        self.snapshots.len()
    }
}

/// The size of every page, as the window lays them out.
pub fn page_sizes(document: &PdfDocument<'_>) -> Vec<PageSize> {
    let list = document.pages();
    (0..list.len())
        .map(|i| match list.page_size(i) {
            Ok(r) => sane_size(r.width().value, r.height().value),
            Err(_) => sane_size(0.0, 0.0),
        })
        .collect()
}

/// Page numbers as the range text PDFium reads: counted from 1, consecutive ascending pages joined
/// as `3-5`. The order is kept (`3,1,2` copies page 3 first).
pub fn range_string(pages: &[u32]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < pages.len() {
        let mut j = i;
        while j + 1 < pages.len() && pages[j + 1] == pages[j].wrapping_add(1) {
            j += 1;
        }
        if !out.is_empty() {
            out.push(',');
        }
        if j == i {
            out.push_str(&(pages[i] as u64 + 1).to_string());
        } else {
            out.push_str(&format!("{}-{}", pages[i] as u64 + 1, pages[j] as u64 + 1));
        }
        i = j + 1;
    }
    out
}

/// Which places of a new order can keep the page that is there: the longest run of places whose
/// pages are already in the order they were in. The others are the pages that have to be moved.
pub fn stable_places(order: &[u32]) -> Vec<bool> {
    let n = order.len();
    let mut tails: Vec<usize> = Vec::new();
    let mut previous = vec![usize::MAX; n];
    for i in 0..n {
        let at = tails.partition_point(|&t| order[t] < order[i]);
        if at > 0 {
            previous[i] = tails[at - 1];
        }
        if at == tails.len() {
            tails.push(i);
        } else {
            tails[at] = i;
        }
    }
    let mut stable = vec![false; n];
    let mut next = tails.last().copied();
    while let Some(i) = next {
        stable[i] = true;
        next = (previous[i] != usize::MAX).then_some(previous[i]);
    }
    stable
}

/// The box a crop leaves: `visible` is `[left, bottom, right, top]` in page coordinates, `turns`
/// is how many quarter turns clockwise the page is drawn, and `margins` are `[left, top, right,
/// bottom]` on the page as it is drawn. The margins are turned back to the page's own sides.
pub fn cropped_box(visible: [f32; 4], turns: u8, margins: [f32; 4]) -> [f32; 4] {
    let [ml, mt, mr, mb] = margins;
    // Insets in page coordinates: (left, bottom, right, top).
    let (il, ib, ir, it) = match turns % 4 {
        0 => (ml, mb, mr, mt),
        1 => (mt, ml, mb, mr),
        2 => (mr, mt, ml, mb),
        _ => (mb, mr, mt, ml),
    };
    [
        visible[0] + il,
        visible[1] + ib,
        visible[2] - ir,
        visible[3] - it,
    ]
}

fn turns_of(rotation: PdfPageRenderRotation) -> u8 {
    match rotation {
        PdfPageRenderRotation::None => 0,
        PdfPageRenderRotation::Degrees90 => 1,
        PdfPageRenderRotation::Degrees180 => 2,
        PdfPageRenderRotation::Degrees270 => 3,
    }
}

fn rotation_of(turns: u8) -> PdfPageRenderRotation {
    match turns % 4 {
        0 => PdfPageRenderRotation::None,
        1 => PdfPageRenderRotation::Degrees90,
        2 => PdfPageRenderRotation::Degrees180,
        _ => PdfPageRenderRotation::Degrees270,
    }
}

fn rect_of(r: &PdfRect) -> [f32; 4] {
    let (l, b, rt, t) = (
        r.left().value,
        r.bottom().value,
        r.right().value,
        r.top().value,
    );
    [l.min(rt), b.min(t), l.max(rt), b.max(t)]
}

/// A box the page tools work with: finite numbers that the protocol can carry (the answer of a crop
/// holds the boxes, and the app refuses numbers above [`MAX_BOX_VALUE`]), and sides of a sane size.
fn usable(b: &[f32; 4]) -> bool {
    b.iter().all(|v| v.is_finite() && v.abs() <= MAX_BOX_VALUE)
        && b[2] - b[0] > 0.0
        && b[3] - b[1] > 0.0
        && b[2] - b[0] <= 14_400.0
        && b[3] - b[1] <= 14_400.0
}

/// The part of the page that is shown: the crop box inside the media box, in page coordinates, as
/// `[left, bottom, right, top]`. `None` when the page has no size that can be worked with.
fn visible_box(page: &PdfPage<'_>) -> Option<[f32; 4]> {
    let media = page
        .boundaries()
        .media()
        .ok()
        .map(|b| rect_of(&b.bounds))
        .unwrap_or([0.0, 0.0, 612.0, 792.0]);
    let shown = match page.boundaries().crop().ok().map(|b| rect_of(&b.bounds)) {
        Some(crop) => {
            let both = [
                crop[0].max(media[0]),
                crop[1].max(media[1]),
                crop[2].min(media[2]),
                crop[3].min(media[3]),
            ];
            if both[0] < both[2] && both[1] < both[3] {
                both
            } else {
                media
            }
        }
        None => media,
    };
    usable(&shown).then_some(shown)
}

fn pdf_rect(b: &[f32; 4]) -> PdfRect {
    PdfRect::new_from_values(b[1], b[0], b[3], b[2])
}

/// Checks a list of pages: not empty, every page there, and sorted without repeats.
fn page_list(pages: &[u32], count: usize) -> Result<Vec<u32>, Failure> {
    if pages.is_empty() {
        return fail(ErrorCode::InvalidRequest, "no pages were named");
    }
    if pages.len() > MAX_EDIT_PAGES {
        return fail(ErrorCode::InvalidRequest, "too many pages were named");
    }
    let mut sorted = pages.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.last().is_some_and(|&p| p as usize >= count) {
        return fail(
            ErrorCode::PageOutOfRange,
            "a page that was named does not exist",
        );
    }
    Ok(sorted)
}

fn failed(message: &str) -> Failure {
    (ErrorCode::PageChangeFailed, message.to_string())
}

/// Answers one request of the page tools.
pub(crate) fn handle(
    engine: &mut Engine,
    pdfium: &'static Pdfium,
    request: PagesRequest,
    progress: &mut dyn FnMut(u32),
) -> Result<ResponseBody, Failure> {
    let response = match request {
        PagesRequest::Delete { doc, pages } => delete(engine, pdfium, doc, &pages, progress)?,
        PagesRequest::Reorder { doc, order } => reorder(engine, pdfium, doc, &order, progress)?,
        PagesRequest::Rotate { doc, pages, turns } => rotate(engine, doc, &pages, turns)?,
        PagesRequest::Crop {
            doc,
            pages,
            margins,
        } => crop(engine, doc, &pages, margins)?,
        PagesRequest::SetBoxes { doc, pages, boxes } => set_boxes(engine, doc, &pages, &boxes)?,
        PagesRequest::InsertBlank {
            doc,
            at,
            width,
            height,
        } => insert_blank(engine, pdfium, doc, at, width, height, progress)?,
        PagesRequest::Restore { doc, snapshot } => {
            restore(engine, pdfium, doc, snapshot, progress)?
        }
        PagesRequest::ImportBegin { doc, size } => import_begin(engine, doc, size)?,
        PagesRequest::ImportChunk { doc, offset, data } => import_chunk(engine, doc, offset, data)?,
        PagesRequest::ImportEnd { doc, at } => import_end(engine, pdfium, doc, at, progress)?,
        PagesRequest::ImportAbort { doc } => {
            if let Some(tools) = engine.tools.get_mut(&doc) {
                tools.import = None;
            }
            PagesResponse::Done
        }
        PagesRequest::ExportBegin { doc, pages } => {
            export_begin(engine, pdfium, doc, &pages, progress)?
        }
    };
    Ok(ResponseBody::Pages(response))
}

/// What a change asks of the permissions of a protected document (an unprotected one allows all).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Right {
    /// Nothing: taking a change back.
    Free,
    /// Insert, delete, move and turn pages (assembling the document).
    Assemble,
    /// Change what a page holds: its boxes.
    Modify,
    /// Copy content out of the document into another.
    Extract,
}

/// `/P` bits of the permissions, counted from 1 as the PDF specification does: modify the contents
/// (4), copy content (5), assemble the document (11).
const PERMIT_MODIFY: u32 = 1 << 3;
const PERMIT_EXTRACT: u32 = 1 << 4;
const PERMIT_ASSEMBLE: u32 = 1 << 10;

/// Refuses a change that the document's permissions do not allow, as the branches for notes and
/// forms refuse theirs. The permissions are those of the security handler's `/P` entry (the call
/// that answers for the owner, so a document opened with its owner password allows what the owner
/// may do). A document that is not protected allows everything.
fn permitted(document: &PdfDocument<'_>, right: Right) -> Result<(), Failure> {
    let (needed, what) = match right {
        Right::Free => return Ok(()),
        Right::Assemble => (
            PERMIT_ASSEMBLE,
            "the document does not allow pages to be inserted, deleted, moved or turned",
        ),
        Right::Modify => (
            PERMIT_MODIFY,
            "the document does not allow the contents of its pages to be changed",
        ),
        Right::Extract => (
            PERMIT_EXTRACT,
            "the document does not allow its pages to be copied",
        ),
    };
    let revision = document.permissions().security_handler_revision();
    if let Ok(PdfSecurityHandlerRevision::Unprotected) = revision {
        return Ok(());
    }
    let Some(handle) = raw_document(document) else {
        return fail(ErrorCode::PagesNotAllowed, what);
    };
    // SAFETY: a read of the permissions, with the live document's handle.
    let bits = unsafe { document.bindings().FPDF_GetDocPermissions(handle) } as u32;
    // Before revision 3 there is no bit of its own for assembling: changing the contents covers it.
    let needed = match (right, revision) {
        (Right::Assemble, Ok(PdfSecurityHandlerRevision::Revision2)) => PERMIT_MODIFY,
        _ => needed,
    };
    if bits & needed != 0 {
        Ok(())
    } else {
        fail(ErrorCode::PagesNotAllowed, what)
    }
}

/// The raw handle PDFium has for a document. `pdfium-render` keeps it to itself; the text it gives
/// for `Debug` names it, and the page tools fail their tests if that ever stops being so.
#[doc(hidden)]
pub fn raw_document(document: &PdfDocument<'_>) -> Option<FPDF_DOCUMENT> {
    let text = format!("{document:?}");
    let start = text.find("0x")? + 2;
    let hex: String = text[start..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    let value = usize::from_str_radix(&hex, 16).ok()?;
    if value == 0 || value % std::mem::align_of::<usize>() != 0 {
        return None;
    }
    // The handle must be the one of this document: it has as many pages as the document.
    let handle = value as FPDF_DOCUMENT;
    // SAFETY: the value was printed by the document for its own handle; PDFium only reads it.
    let pages = unsafe { document.bindings().FPDF_GetPageCount(handle) };
    (pages >= 0 && pages as usize == document.pages().len() as usize).then_some(handle)
}

/// Moves pages inside the document so that the first of `indices` becomes page `dest` and the rest
/// follow it in the order given (PDFium takes them out of the page tree and puts them back, and
/// copies nothing). No page of this document may be open while this runs.
fn move_pages(document: &PdfDocument<'_>, indices: &[c_int], dest: usize) -> Result<(), Failure> {
    let Some(handle) = raw_document(document) else {
        return Err(failed("the document cannot be moved about"));
    };
    // SAFETY: `handle` is the live document's, `indices` is a valid array of the length given. PDFium
    // checks the numbers and refuses a list with a number out of range or repeated.
    let done = unsafe {
        document.bindings().FPDF_MovePages(
            handle,
            indices.as_ptr(),
            indices.len() as c_ulong,
            dest as c_int,
        )
    };
    if document.bindings().is_true(done) {
        Ok(())
    } else {
        Err(failed("PDFium could not move the pages"))
    }
}

/// The document may be changed: it is open, no save or copy is holding its bytes, it has no more
/// pages than the tools take, and its permissions allow what is asked. Returns the number of pages.
fn editable(engine: &Engine, doc: u32, right: Right) -> Result<usize, Failure> {
    editable_with(engine, doc, false, right)
}

/// As [`editable`], and for a change to the list of pages itself (a delete, a move, a page added
/// or copied) the list must be sound: every place in it is a page that can be found. PDFium
/// crashes when it adds a page to a list that holds something that is not a page (found by the
/// fuzz target), so such a document is refused here, before PDFium is asked.
fn editable_list(engine: &Engine, doc: u32, right: Right) -> Result<usize, Failure> {
    editable_with(engine, doc, true, right)
}

fn editable_with(engine: &Engine, doc: u32, sound: bool, right: Right) -> Result<usize, Failure> {
    let document = engine.document(doc)?;
    permitted(document, right)?;
    if engine.saves.contains_key(&doc) {
        return fail(
            ErrorCode::Busy,
            "a save or a copy of the document is under way",
        );
    }
    let count = document.pages().len() as usize;
    if count > MAX_EDIT_PAGES {
        return fail(
            ErrorCode::TooManyPages,
            format!("the page tools work on documents of up to {MAX_EDIT_PAGES} pages"),
        );
    }
    if sound {
        let list = document.pages();
        if let Some(bad) = (0..list.len()).find(|i| !page_is_sound(document, *i)) {
            return fail(
                ErrorCode::PageChangeFailed,
                format!(
                    "the list of pages of this document is damaged (page {} cannot be found), so its pages cannot be added, deleted or moved",
                    bad + 1
                ),
            );
        }
    }
    Ok(count)
}

/// A page that can be found and has a size.
fn page_is_sound(document: &PdfDocument<'_>, index: PdfPageIndex) -> bool {
    document.pages().page_size(index).is_ok()
}

/// Reads the page tree out of a document as PDFium writes it (a classic table of the places of the
/// objects, the catalog, the nodes of the tree) and says whether it is as it should be: the root
/// that the catalog names is a node (it has `/Kids`, and its `/Type`, if it has one, is `/Pages`),
/// every kid of every node is an object that is there with a dictionary, and the pages at the
/// leaves are as many as the document says it has. Bytes that cannot be read that way (a table of
/// another kind) are taken as a sound tree: a document is not refused for how it is written.
fn page_tree_is_sound(bytes: &[u8], count: usize) -> bool {
    let Some(table) = XrefTable::read(bytes) else {
        return true;
    };
    let Some(root) = table.catalog_pages() else {
        return false;
    };
    let mut leaves = 0usize;
    let mut visited = 0usize;
    table.walk(root, 0, &mut leaves, &mut visited) && leaves == count
}

/// The places of the objects of a document written by PDFium, and the way to read their dictionaries.
struct XrefTable<'a> {
    bytes: &'a [u8],
    /// Object number to the place of `N G obj`; None for a free entry.
    places: Vec<Option<usize>>,
    root: u32,
}

/// The words of `bytes`, separated by white space.
fn words(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes
        .split(|b| b.is_ascii_whitespace())
        .filter(|w| !w.is_empty())
}

fn number_of(word: Option<&[u8]>) -> Option<usize> {
    std::str::from_utf8(word?).ok()?.parse().ok()
}

impl<'a> XrefTable<'a> {
    fn read(bytes: &'a [u8]) -> Option<XrefTable<'a>> {
        let start = rfind(bytes, b"startxref")?;
        let at = number_of(words(&bytes[start + 9..]).next())?;
        let table = bytes.get(at..)?;
        if !table.starts_with(b"xref") {
            return None;
        }
        let mut rest = words(&table[4..]);
        let mut places: Vec<Option<usize>> = Vec::new();
        loop {
            let first = rest.next()?;
            if first == b"trailer" {
                break;
            }
            let first = number_of(Some(first))?;
            let entries = number_of(rest.next())?;
            if entries > 1_000_000 || first > 1_000_000 {
                return None;
            }
            if places.len() < first + entries {
                places.resize(first + entries, None);
            }
            for i in 0..entries {
                let offset = number_of(rest.next())?;
                let _generation = number_of(rest.next())?;
                let kind = rest.next()?;
                places[first + i] = (kind == b"n").then_some(offset);
            }
        }
        let trailer = &table[find(table, b"trailer")?..];
        let root = reference_after(trailer, b"/Root")?;
        Some(XrefTable {
            bytes,
            places,
            root,
        })
    }

    /// The dictionary of object `number`: the text from after `obj` up to the stream or the end.
    fn dictionary(&self, number: u32) -> Option<&'a [u8]> {
        let place = (*self.places.get(number as usize)?)?;
        let rest = self.bytes.get(place..)?;
        let header_end = find(rest, b"obj")? + 3;
        let body = rest.get(header_end..)?;
        // The nearest end first, so that the cost of a look at an object is that of the object.
        let end = find(body, b"endobj").unwrap_or(body.len());
        let end = find(&body[..end], b"stream").unwrap_or(end);
        let text = &body[..end];
        // Only white space may come before the dictionary.
        let open = text.iter().take_while(|b| b.is_ascii_whitespace()).count();
        text[open..].starts_with(b"<<").then_some(&text[open..])
    }

    fn catalog_pages(&self) -> Option<u32> {
        reference_after(self.dictionary(self.root)?, b"/Pages")
    }

    /// True when the node is sound; adds the pages at its leaves to `leaves`.
    fn walk(&self, number: u32, depth: usize, leaves: &mut usize, visited: &mut usize) -> bool {
        *visited += 1;
        if depth > 64 || *visited > 2_000_000 {
            return false;
        }
        let Some(dict) = self.dictionary(number) else {
            return false;
        };
        let kind = name_after(dict, b"/Type");
        let Some(kids) = array_after(dict, b"/Kids") else {
            // A leaf: a page. A dictionary that says it is something else is not one.
            if kind.is_some_and(|k| k != b"/Page") {
                return false;
            }
            *leaves += 1;
            return true;
        };
        if kind.is_some_and(|k| k != b"/Pages") {
            return false;
        }
        // Every entry of the list must be a reference: a name or an array in it is a kid that PDFium
        // reads as nothing, which its move of pages does not survive.
        let Some(kids) = all_references(kids) else {
            return false;
        };
        kids.into_iter()
            .all(|kid| self.walk(kid, depth + 1, leaves, visited))
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

fn skip_white(bytes: &[u8]) -> &[u8] {
    let n = bytes.iter().take_while(|b| b.is_ascii_whitespace()).count();
    &bytes[n..]
}

/// What follows `key` among the keys of the dictionary itself (not those of a dictionary inside
/// it, where the same name may mean something else).
fn value_of<'a>(dict: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let mut dicts = 0usize;
    let mut arrays = 0usize;
    let mut i = 0;
    while i < dict.len() {
        match dict[i] {
            b'<' if dict.get(i + 1) == Some(&b'<') => {
                dicts += 1;
                i += 2;
            }
            b'>' if dict.get(i + 1) == Some(&b'>') => {
                dicts = dicts.saturating_sub(1);
                i += 2;
                if dicts == 0 {
                    break;
                }
            }
            b'[' => {
                arrays += 1;
                i += 1;
            }
            b']' => {
                arrays = arrays.saturating_sub(1);
                i += 1;
            }
            b'(' => {
                // A string: up to the closing bracket that matches, a backslash takes the next byte.
                let mut open = 1usize;
                i += 1;
                while i < dict.len() && open > 0 {
                    match dict[i] {
                        b'\\' => i += 1,
                        b'(' => open += 1,
                        b')' => open -= 1,
                        _ => {}
                    }
                    i += 1;
                }
            }
            b'<' => {
                i += dict[i..]
                    .iter()
                    .position(|b| *b == b'>')
                    .map_or(1, |p| p + 1);
            }
            b'/' => {
                let end = dict[i + 1..]
                    .iter()
                    .position(|b| b.is_ascii_whitespace() || b"/<>[]()".contains(b))
                    .map_or(dict.len(), |p| i + 1 + p);
                if dicts == 1 && arrays == 0 && &dict[i..end] == key {
                    return Some(&dict[end..]);
                }
                i = end.max(i + 1);
            }
            _ => i += 1,
        }
    }
    None
}

/// The name that follows `key` (`/Type` then `/Pages`).
fn name_after<'a>(dict: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let rest = skip_white(value_of(dict, key)?);
    if !rest.starts_with(b"/") {
        return None;
    }
    let end = rest[1..]
        .iter()
        .position(|b| b.is_ascii_whitespace() || b"/<>[]()".contains(b))
        .map_or(rest.len(), |p| p + 1);
    Some(&rest[..end])
}

/// The text between the brackets of the array that follows `key`.
fn array_after<'a>(dict: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let rest = skip_white(value_of(dict, key)?);
    if !rest.starts_with(b"[") {
        return None;
    }
    let end = rest.iter().position(|b| *b == b']')?;
    Some(&rest[1..end])
}

/// The object number of the reference (`N G R`) that follows `key`.
fn reference_after(dict: &[u8], key: &[u8]) -> Option<u32> {
    references_in(value_of(dict, key)?).first().copied()
}

/// The object numbers of the references (`N G R`) that make up `text` and nothing else.
fn all_references(text: &[u8]) -> Option<Vec<u32>> {
    let all: Vec<&[u8]> = words(text).collect();
    if all.len() % 3 != 0 {
        return None;
    }
    all.chunks(3)
        .map(|chunk| {
            if chunk[2] != b"R" || number_of(Some(chunk[1])).is_none() {
                return None;
            }
            u32::try_from(number_of(Some(chunk[0]))?).ok()
        })
        .collect()
}

/// The object numbers of the references (`N G R`) at the start of `text`, one after another.
fn references_in(text: &[u8]) -> Vec<u32> {
    let all: Vec<&[u8]> = words(text).collect();
    let mut out = Vec::new();
    for chunk in all.chunks(3) {
        if chunk.len() < 3 || chunk[2] != b"R" {
            break;
        }
        let Some(n) = std::str::from_utf8(chunk[0])
            .ok()
            .and_then(|w| w.parse::<u32>().ok())
        else {
            break;
        };
        out.push(n);
    }
    out
}

/// The password the document was opened with, in a copy that is overwritten with zeros when it is
/// let go of, like the one the engine keeps.
fn password_of(engine: &Engine, doc: u32) -> Option<Secret> {
    engine.passwords.get(&doc).map(|s| Secret(s.0.clone()))
}

fn password_str(password: &Option<Secret>) -> Option<&str> {
    password.as_ref().map(|s| s.0.as_str())
}

/// The document written out as it is now (up to `limit` bytes), after making sure that the file it
/// is read from is what it was. `Ok(None)` when it is bigger than `limit`.
fn serialise(
    engine: &Engine,
    doc: u32,
    limit: usize,
    progress: &mut dyn FnMut(u32),
) -> Result<Option<Vec<u8>>, Failure> {
    engine.check_source(progress)?;
    let document = engine.document(doc)?;
    match save::serialise_up_to(document, limit, progress) {
        Ok(bytes) => {
            engine.check_source(progress)?;
            Ok(Some(bytes))
        }
        Err((ErrorCode::SaveTooLarge, _)) => Ok(None),
        Err(other) => Err(other),
    }
}

fn tools(engine: &mut Engine, doc: u32) -> &mut PageTools {
    engine.tools.entry(doc).or_default()
}

/// Takes a copy of the document for taking the change back. 0 when the document is too big to
/// keep a copy of.
fn take_snapshot(
    engine: &mut Engine,
    doc: u32,
    progress: &mut dyn FnMut(u32),
) -> Result<u32, Failure> {
    let limit = tools(engine, doc).max_undo_bytes;
    let pages = engine.document(doc)?.pages().len() as usize;
    match serialise(engine, doc, limit, progress)? {
        Some(bytes) => Ok(tools(engine, doc).keep(bytes, pages)),
        None => Ok(0),
    }
}

/// A copy of a document read in again, with the pages it must have. A copy that shows another
/// number is not the document that was kept (PDFium does not always read back what it wrote, on a
/// damaged file), and putting it in place would replace the user's document with another one.
fn load_copy(
    pdfium: &'static Pdfium,
    bytes: Vec<u8>,
    password: Option<&str>,
    pages: usize,
) -> Result<PdfDocument<'static>, Failure> {
    let loaded = pdfium
        .load_pdf_from_byte_vec(bytes, password)
        .map_err(|_| failed("the copy of the document could not be read"))?;
    let found = loaded.pages().len() as usize;
    if found != pages || found == 0 {
        return Err(failed(&format!(
            "the copy of the document has {found} pages, not the {pages} it was made from"
        )));
    }
    Ok(loaded)
}

/// A change failed half way. The document is put back from the copy kept for this change, and the
/// failure to give is the one that happened. When it cannot be put back (no copy was kept because
/// the document is too big, or the copy does not load), what is left is neither the old document nor
/// the new one and must never be saved: the document is closed, and the failure says so. The window
/// takes it as a lost document, like a stopped engine; the file on the disk is untouched.
fn recover(
    engine: &mut Engine,
    pdfium: &'static Pdfium,
    doc: u32,
    token: u32,
    original: Failure,
) -> Failure {
    let password = password_of(engine, doc);
    // The copy stays: the change did not happen, so it still describes the document.
    let kept = engine
        .tools
        .get(&doc)
        .and_then(|t| t.snapshots.iter().find(|s| s.token == token))
        .map(|s| (s.bytes.clone(), s.pages));
    match kept {
        Some((bytes, pages)) if token != 0 => {
            put_back(engine, pdfium, doc, bytes, pages, password, original)
        }
        _ => lose(engine, doc),
    }
}

fn put_back(
    engine: &mut Engine,
    pdfium: &'static Pdfium,
    doc: u32,
    bytes: Vec<u8>,
    pages: usize,
    password: Option<Secret>,
    original: Failure,
) -> Failure {
    match load_copy(pdfium, bytes, password_str(&password), pages) {
        Ok(loaded) => {
            engine.docs.insert(doc, loaded);
            original
        }
        Err(_) => lose(engine, doc),
    }
}

fn lose(engine: &mut Engine, doc: u32) -> Failure {
    engine.docs.remove(&doc);
    (
        ErrorCode::UnknownDocument,
        "a change to the pages failed half way and the document could not be put back, so it was closed; the file on the disk is untouched".to_string(),
    )
}

fn changed(
    engine: &Engine,
    doc: u32,
    snapshot: u32,
    count: usize,
) -> Result<PagesResponse, Failure> {
    changed_with(engine, doc, snapshot, count, 0)
}

fn changed_with(
    engine: &Engine,
    doc: u32,
    snapshot: u32,
    count: usize,
    form_fields: u32,
) -> Result<PagesResponse, Failure> {
    Ok(PagesResponse::Changed {
        pages: page_sizes(engine.document(doc)?),
        snapshot,
        count: count as u32,
        form_fields,
    })
}

/// Clears the text typed into the text fields of a page that is about to be deleted, and counts
/// the form fields on it. PDFium keeps the list of fields of a form (the form refers to them, not
/// the page), so a field on a deleted page stays in the file; what was typed into it should not.
/// Only text fields are cleared here: other kinds keep what they hold, and a field that is grouped
/// under a parent field keeps the value its parent holds. The default value, the tooltip and the
/// appearance of the fields on the page are cleared as well (see [`clear_widget_text`]). The text
/// a tag holds for the page (its alternative text) is out of reach and stays in the file.
fn clear_form_fields(document: &PdfDocument<'_>, index: u32) -> u32 {
    let fields = {
        let Ok(page) = document.pages().get(index as PdfPageIndex) else {
            return 0;
        };
        let mut fields = 0;
        for mut annotation in page.annotations().iter() {
            if let Some(field) = annotation.as_form_field_mut() {
                fields += 1;
                if let Some(text) = field.as_text_field_mut() {
                    let _ = text.set_value("");
                }
            }
        }
        fields
    };
    clear_widget_text(document, index);
    fields
}

/// What PDFium's subtype number is for a widget (`FPDF_ANNOT_WIDGET`), and its number for a string
/// (`FPDF_OBJECT_STRING`).
const SUBTYPE_WIDGET: i32 = 20;
const OBJECT_STRING: i32 = 3;

/// Overwrites the other text a form field of a page keeps in its own dictionary, and drops its
/// appearance (the picture of what was typed, which holds the text as drawing instructions): the
/// default value, the tooltip, the value of fields other than text fields when it is a string, the
/// rich text value. Fields grouped under a parent keep what the parent holds, which is out of reach.
fn clear_widget_text(document: &PdfDocument<'_>, index: u32) {
    let Some(handle) = raw_document(document) else {
        return;
    };
    let bindings = document.bindings();
    // SAFETY: `handle` is the live document's; the page and every annotation handle are opened and
    // closed here, and PDFium only gets handles it gave out.
    unsafe {
        let page = bindings.FPDF_LoadPage(handle, index as c_int);
        if page.is_null() {
            return;
        }
        let count = bindings.FPDFPage_GetAnnotCount(page).max(0);
        for position in 0..count {
            let annotation = bindings.FPDFPage_GetAnnot(page, position);
            if annotation.is_null() {
                continue;
            }
            if bindings.FPDFAnnot_GetSubtype(annotation) == SUBTYPE_WIDGET {
                for key in ["V", "DV", "TU", "RV"] {
                    if bindings.FPDFAnnot_HasKey(annotation, key) != 0
                        && bindings.FPDFAnnot_GetValueType(annotation, key) == OBJECT_STRING
                    {
                        bindings.FPDFAnnot_SetStringValue_str(annotation, key, "");
                    }
                }
                bindings.FPDFAnnot_SetAP(
                    annotation,
                    0 as FPDF_ANNOT_APPEARANCEMODE,
                    std::ptr::null(),
                );
            }
            bindings.FPDFPage_CloseAnnot(annotation);
        }
        bindings.FPDF_ClosePage(page);
    }
}

/// Empties the signature a copied signature field holds, so that the data of a signature (the
/// byte ranges, the signer's certificate in `/Contents`) does not travel with the page. The look of
/// the field stays. Returns how many fields were emptied.
fn drop_copied_signatures(document: &PdfDocument<'_>) -> usize {
    let Some(handle) = raw_document(document) else {
        return 0;
    };
    let bindings = document.bindings();
    let mut emptied = 0;
    // SAFETY: as in `clear_widget_text`.
    unsafe {
        for index in 0..bindings.FPDF_GetPageCount(handle).max(0) {
            let page = bindings.FPDF_LoadPage(handle, index);
            if page.is_null() {
                continue;
            }
            let count = bindings.FPDFPage_GetAnnotCount(page).max(0);
            for position in 0..count {
                let annotation = bindings.FPDFPage_GetAnnot(page, position);
                if annotation.is_null() {
                    continue;
                }
                if bindings.FPDFAnnot_GetSubtype(annotation) == SUBTYPE_WIDGET
                    && bindings.FPDFAnnot_HasKey(annotation, "V") != 0
                    && bindings.FPDFAnnot_GetValueType(annotation, "V") != OBJECT_STRING
                    && is_signature_field(bindings, annotation)
                    && bindings.FPDFAnnot_SetStringValue_str(annotation, "V", "") != 0
                {
                    emptied += 1;
                    // The copy leaves `/Parent` as it was in the other document, where it means
                    // some other object here, which may be the signature itself.
                    if bindings.FPDFAnnot_HasKey(annotation, "Parent") != 0 {
                        bindings.FPDFAnnot_SetStringValue_str(annotation, "Parent", "");
                    }
                }
                bindings.FPDFPage_CloseAnnot(annotation);
            }
            bindings.FPDF_ClosePage(page);
        }
    }
    emptied
}

/// True when the widget names itself as a signature field (`/FT /Sig`).
///
/// # Safety
/// `annotation` must be a live annotation handle of `bindings`.
unsafe fn is_signature_field(
    bindings: &dyn PdfiumLibraryBindings,
    annotation: FPDF_ANNOTATION,
) -> bool {
    if bindings.FPDFAnnot_HasKey(annotation, "FT") == 0 {
        return false;
    }
    let mut units = [0u16; 8];
    let needed = bindings.FPDFAnnot_GetStringValue(
        annotation,
        "FT",
        units.as_mut_ptr(),
        std::mem::size_of_val(&units) as std::os::raw::c_ulong,
    );
    // "Sig" is three UTF-16 units and a terminator: eight bytes.
    needed == 8 && units[..3] == [b'S' as u16, b'i' as u16, b'g' as u16]
}

fn delete(
    engine: &mut Engine,
    pdfium: &'static Pdfium,
    doc: u32,
    pages: &[u32],
    progress: &mut dyn FnMut(u32),
) -> Result<PagesResponse, Failure> {
    let count = editable_list(engine, doc, Right::Assemble)?;
    let sorted = page_list(pages, count)?;
    if sorted.len() >= count {
        return fail(
            ErrorCode::InvalidRequest,
            "a document needs at least one page",
        );
    }
    let token = take_snapshot(engine, doc, progress)?;
    let fail_at = tools(engine, doc).fail_at();
    let miscount = tools(engine, doc).miscount();
    let mut form_fields = 0;
    let result = (|| {
        let document = engine.docs.get_mut(&doc).ok_or_else(gone)?;
        for &index in &sorted {
            form_fields += clear_form_fields(document, index);
        }
        for (i, &index) in sorted.iter().rev().enumerate() {
            if injected(fail_at, i) {
                return Err(failed("a failure made by a test"));
            }
            let page = document
                .pages()
                .get(index as PdfPageIndex)
                .map_err(|_| failed("a page could not be read"))?;
            page.delete()
                .map_err(|_| failed("PDFium could not delete a page"))?;
            if i % 16 == 15 {
                progress(i as u32 + 1);
            }
        }
        if document.pages().len() as isize + miscount != (count - sorted.len()) as isize {
            return Err(failed(
                "the document has the wrong number of pages after the delete",
            ));
        }
        Ok(())
    })();
    if let Err(e) = result {
        return Err(recover(engine, pdfium, doc, token, e));
    }
    changed_with(engine, doc, token, sorted.len(), form_fields)
}

fn gone() -> Failure {
    (ErrorCode::UnknownDocument, "no such document".to_string())
}

fn reorder(
    engine: &mut Engine,
    pdfium: &'static Pdfium,
    doc: u32,
    order: &[u32],
    progress: &mut dyn FnMut(u32),
) -> Result<PagesResponse, Failure> {
    let count = editable_list(engine, doc, Right::Assemble)?;
    if order.len() != count {
        return fail(
            ErrorCode::InvalidRequest,
            "the new order must name every page once",
        );
    }
    let mut seen = vec![false; count];
    for &p in order {
        match seen.get_mut(p as usize) {
            Some(slot) if !*slot => *slot = true,
            _ => {
                return fail(
                    ErrorCode::InvalidRequest,
                    "the new order must name every page once",
                )
            }
        }
    }
    if order.iter().enumerate().all(|(i, &p)| p as usize == i) {
        return changed(engine, doc, 0, 0);
    }
    // The document as it is now: the copy that takes the move back, and that the document is put
    // back from when a step fails. A document that cannot be saved cannot be reordered either.
    let Some(bytes) = serialise(engine, doc, MAX_SAVE_BYTES, progress)? else {
        return fail(
            ErrorCode::SaveTooLarge,
            "the document is too big to be reordered",
        );
    };
    let stable = stable_places(order);
    let fail_at = tools(engine, doc).fail_at();
    let miscount = tools(engine, doc).miscount();
    let scramble = tools(engine, doc).scrambled();
    let password = password_of(engine, doc);
    let moved = stable.iter().filter(|s| !**s).count();
    // PDFium's move stops the process on a page tree that lists something that is not there (a
    // kid that is a null object after the last page that can be found, found by the fuzz target):
    // the pages it can find are all sound, so the check of the list of pages did not see it. The
    // document as PDFium writes it shows it, and such a document is not moved.
    if !page_tree_is_sound(&bytes, count) {
        return fail(
            ErrorCode::PageChangeFailed,
            "the list of pages of this document is damaged (it names something that is not there), so its pages cannot be moved",
        );
    }
    let before = page_sizes(engine.document(doc)?);
    let result = (|| {
        let document = engine.docs.get_mut(&doc).ok_or_else(gone)?;
        // The pages that have to move go to the end first, in the order of the places they are
        // going to, in one step. What is left at the front is the pages that stay, in their
        // order, which is also the order they have in the new document.
        let leaving: Vec<c_int> = (0..count)
            .filter(|i| !stable[*i])
            .map(|i| order[i] as c_int)
            .collect();
        let staying = count - leaving.len();
        let at_the_end = leaving
            .iter()
            .enumerate()
            .all(|(i, &p)| p as usize == staying + i);
        let mut calls = 0;
        if !at_the_end {
            if injected(fail_at, calls) {
                return Err(failed("a failure made by a test"));
            }
            move_pages(document, &leaving, staying)?;
            calls += 1;
        }
        // Then each run of places that took a page that moved gets its pages, from the front. The
        // pages of the next run are the first of those at the end, in the order they have to go in.
        let mut place = 0;
        let mut done = 0;
        while place < count {
            if stable[place] {
                place += 1;
                continue;
            }
            let mut end = place;
            while end < count && !stable[end] {
                end += 1;
            }
            let from = staying + done;
            if from != place {
                if injected(fail_at, calls) {
                    return Err(failed("a failure made by a test"));
                }
                let run: Vec<c_int> = (from..from + (end - place)).map(|i| i as c_int).collect();
                move_pages(document, &run, place)?;
                calls += 1;
            }
            done += end - place;
            progress(end as u32);
            place = end;
        }
        if document.pages().len() as isize + miscount != count as isize {
            return Err(failed(
                "the document has the wrong number of pages after the move",
            ));
        }
        // Every place must hold the page that was asked for. A page cannot be told from another
        // by its size alone, but a move that went wrong shows up there first.
        let mut now = page_sizes(document);
        if scramble {
            now.rotate_left(1);
        }
        if now
            .iter()
            .zip(order)
            .any(|(size, &from)| *size != before[from as usize])
        {
            return Err(failed("the pages are not in the order that was asked for"));
        }
        Ok(())
    })();
    let limit = tools(engine, doc).max_undo_bytes;
    if let Err(e) = result {
        // Put the document back from the copy.
        return Err(put_back(engine, pdfium, doc, bytes, count, password, e));
    }
    let token = if bytes.len() <= limit {
        tools(engine, doc).keep(bytes, count)
    } else {
        0
    };
    changed(engine, doc, token, moved)
}

fn rotate(
    engine: &mut Engine,
    doc: u32,
    pages: &[u32],
    turns: u8,
) -> Result<PagesResponse, Failure> {
    let count = editable(engine, doc, Right::Assemble)?;
    if !(1..=3).contains(&turns) {
        return fail(
            ErrorCode::InvalidRequest,
            "a page turns by 1 to 3 quarter turns",
        );
    }
    let sorted = page_list(pages, count)?;
    {
        let document = engine.document(doc)?;
        // Look at every page first, so that a page that cannot be read changes nothing.
        let mut now = Vec::with_capacity(sorted.len());
        for &index in &sorted {
            let page = document
                .pages()
                .get(index as PdfPageIndex)
                .map_err(|_| failed("a page could not be read"))?;
            now.push(
                page.rotation()
                    .map(turns_of)
                    .map_err(|_| failed("the rotation of a page could not be read"))?,
            );
        }
        for (&index, turned) in sorted.iter().zip(now) {
            let mut page = document
                .pages()
                .get(index as PdfPageIndex)
                .map_err(|_| failed("a page could not be read"))?;
            page.set_rotation(rotation_of(turned + turns));
        }
    }
    changed(engine, doc, 0, sorted.len())
}

fn crop(
    engine: &mut Engine,
    doc: u32,
    pages: &[u32],
    margins: [f32; 4],
) -> Result<PagesResponse, Failure> {
    let count = editable(engine, doc, Right::Modify)?;
    let sorted = page_list(pages, count)?;
    if margins
        .iter()
        .any(|m| !m.is_finite() || *m < 0.0 || *m > MAX_BLANK_SIDE)
    {
        return fail(
            ErrorCode::InvalidRequest,
            "the margins of a crop are not usable",
        );
    }
    let (before, after) = {
        let document = engine.document(doc)?;
        let mut before = Vec::with_capacity(sorted.len());
        let mut after = Vec::with_capacity(sorted.len());
        // All pages are worked out before the first is changed: one page that cannot be cropped
        // that way stops the crop and leaves the document as it was.
        for &index in &sorted {
            let page = document
                .pages()
                .get(index as PdfPageIndex)
                .map_err(|_| failed("a page could not be read"))?;
            let visible = visible_box(&page)
                .ok_or_else(|| failed("a page has a size that cannot be cropped"))?;
            let turns = page.rotation().map(turns_of).unwrap_or(0);
            let new = cropped_box(visible, turns, margins);
            if !usable(&new) || new[2] - new[0] < MIN_CROP_SIDE || new[3] - new[1] < MIN_CROP_SIDE {
                return fail(
                    ErrorCode::InvalidRequest,
                    format!(
                        "the crop would leave less than {MIN_CROP_SIDE} points of page {}",
                        index + 1
                    ),
                );
            }
            before.push(visible);
            after.push(new);
        }
        for (&index, new) in sorted.iter().zip(&after) {
            let mut page = document
                .pages()
                .get(index as PdfPageIndex)
                .map_err(|_| failed("a page could not be read"))?;
            page.boundaries_mut()
                .set_crop(pdf_rect(new))
                .map_err(|_| failed("PDFium could not set the crop box"))?;
        }
        (before, after)
    };
    let pages = page_sizes(engine.document(doc)?);
    Ok(PagesResponse::Cropped {
        pages,
        before,
        after,
    })
}

fn set_boxes(
    engine: &mut Engine,
    doc: u32,
    pages: &[u32],
    boxes: &[[f32; 4]],
) -> Result<PagesResponse, Failure> {
    let count = editable(engine, doc, Right::Modify)?;
    if pages.len() != boxes.len() {
        return fail(ErrorCode::InvalidRequest, "every page needs a box");
    }
    let mut seen = std::collections::HashSet::new();
    for &p in pages {
        if p as usize >= count {
            return fail(
                ErrorCode::PageOutOfRange,
                "a page that was named does not exist",
            );
        }
        if !seen.insert(p) {
            return fail(ErrorCode::InvalidRequest, "a page was named twice");
        }
    }
    for b in boxes {
        let ordered = b[0] < b[2] && b[1] < b[3];
        if !ordered || !usable(b) {
            return fail(ErrorCode::InvalidRequest, "a box is not usable");
        }
    }
    {
        let document = engine.document(doc)?;
        for (&index, b) in pages.iter().zip(boxes) {
            let mut page = document
                .pages()
                .get(index as PdfPageIndex)
                .map_err(|_| failed("a page could not be read"))?;
            page.boundaries_mut()
                .set_crop(pdf_rect(b))
                .map_err(|_| failed("PDFium could not set the crop box"))?;
        }
    }
    changed(engine, doc, 0, pages.len())
}

fn insert_blank(
    engine: &mut Engine,
    pdfium: &'static Pdfium,
    doc: u32,
    at: u32,
    width: f32,
    height: f32,
    progress: &mut dyn FnMut(u32),
) -> Result<PagesResponse, Failure> {
    let count = editable_list(engine, doc, Right::Assemble)?;
    if at as usize > count {
        return fail(
            ErrorCode::PageOutOfRange,
            "there is no such place for a page",
        );
    }
    if count + 1 > MAX_EDIT_PAGES {
        return fail(
            ErrorCode::TooManyPages,
            format!("the page tools work on documents of up to {MAX_EDIT_PAGES} pages"),
        );
    }
    let usable_side = |v: f32| v.is_finite() && (MIN_BLANK_SIDE..=MAX_BLANK_SIDE).contains(&v);
    if !usable_side(width) || !usable_side(height) {
        return fail(
            ErrorCode::InvalidRequest,
            "the size of the page is not usable",
        );
    }
    let token = take_snapshot(engine, doc, progress)?;
    let fail_at = tools(engine, doc).fail_at();
    let miscount = tools(engine, doc).miscount();
    let result = (|| {
        let document = engine.docs.get_mut(&doc).ok_or_else(gone)?;
        if injected(fail_at, 0) {
            // After the page is added, as a failure in the check would be.
            let _ = document
                .pages_mut()
                .create_page_at_index(PdfPagePaperSize::a4(), 0);
            return Err(failed("a failure made by a test"));
        }
        document
            .pages_mut()
            .create_page_at_index(
                PdfPagePaperSize::new_custom(PdfPoints::new(width), PdfPoints::new(height)),
                at as PdfPageIndex,
            )
            .map_err(|_| failed("PDFium could not add a page"))?;
        if document.pages().len() as isize + miscount != count as isize + 1 {
            return Err(failed(
                "the document has the wrong number of pages after the insert",
            ));
        }
        Ok(())
    })();
    if let Err(e) = result {
        return Err(recover(engine, pdfium, doc, token, e));
    }
    changed(engine, doc, token, 1)
}

fn restore(
    engine: &mut Engine,
    pdfium: &'static Pdfium,
    doc: u32,
    token: u32,
    progress: &mut dyn FnMut(u32),
) -> Result<PagesResponse, Failure> {
    editable(engine, doc, Right::Free)?;
    // The copy to go back to leaves the queue first, so that keeping the copy of the state being
    // left (which makes room by letting the oldest go) cannot let go of the one that is wanted.
    // That happened for a document of more than half the limit: the step could not be taken back.
    let target = {
        let queue = &mut tools(engine, doc).snapshots;
        queue
            .iter()
            .position(|s| s.token == token)
            .and_then(|at| queue.remove(at))
    };
    let Some(target) = target else {
        return fail(
            ErrorCode::SnapshotGone,
            "the engine no longer holds a copy of the document to go back to",
        );
    };
    // The state that is left, so that the step can be done again. 0 when it is too big to keep:
    // the step can still be taken back, but not made again.
    let leaving = take_snapshot(engine, doc, progress)?;
    let password = password_of(engine, doc);
    match load_copy(pdfium, target.bytes, password_str(&password), target.pages) {
        Ok(loaded) => {
            engine.docs.insert(doc, loaded);
            changed(engine, doc, leaving, 0)
        }
        Err(e) => {
            // The document stays as it is. The copy that did not read back as the document it was
            // made from is not kept, and neither is the copy of the state that was not left.
            tools(engine, doc).snapshots.retain(|s| s.token != leaving);
            Err(e)
        }
    }
}

fn import_begin(engine: &mut Engine, doc: u32, size: u32) -> Result<PagesResponse, Failure> {
    editable(engine, doc, Right::Assemble)?;
    let size = size as usize;
    if size == 0 || size > MAX_IMPORT_BYTES {
        return fail(
            ErrorCode::InvalidRequest,
            "the size of the document is not usable",
        );
    }
    let mut bytes = Vec::new();
    if bytes.try_reserve_exact(size).is_err() {
        return fail(ErrorCode::ImportFailed, "no memory for the document");
    }
    tools(engine, doc).import = Some(Import { size, bytes });
    Ok(PagesResponse::ImportReady)
}

fn import_chunk(
    engine: &mut Engine,
    doc: u32,
    offset: u32,
    data: Vec<u8>,
) -> Result<PagesResponse, Failure> {
    engine.document(doc)?;
    if data.len() > MAX_SAVE_CHUNK {
        return fail(ErrorCode::InvalidRequest, "the piece is too long");
    }
    let Some(import) = engine.tools.get_mut(&doc).and_then(|t| t.import.as_mut()) else {
        return fail(ErrorCode::InvalidRequest, "no insert is under way");
    };
    if offset as usize != import.bytes.len() {
        return fail(
            ErrorCode::InvalidRequest,
            "the piece does not start where the last ended",
        );
    }
    if import.bytes.len() + data.len() > import.size {
        return fail(
            ErrorCode::InvalidRequest,
            "the pieces are more than the size announced",
        );
    }
    import.bytes.extend_from_slice(&data);
    Ok(PagesResponse::ImportAck {
        received: import.bytes.len() as u32,
    })
}

fn import_end(
    engine: &mut Engine,
    pdfium: &'static Pdfium,
    doc: u32,
    at: u32,
    progress: &mut dyn FnMut(u32),
) -> Result<PagesResponse, Failure> {
    let count = editable_list(engine, doc, Right::Assemble)?;
    let Some(import) = engine.tools.get_mut(&doc).and_then(|t| t.import.take()) else {
        return fail(ErrorCode::InvalidRequest, "no insert is under way");
    };
    if import.bytes.len() != import.size {
        return fail(ErrorCode::InvalidRequest, "the document is not complete");
    }
    if at as usize > count {
        return fail(
            ErrorCode::PageOutOfRange,
            "there is no such place for the pages",
        );
    }
    // The document to insert is untrusted bytes, even though another engine wrote them: it is read
    // here, in this sandbox, and nothing else looks at it.
    let source = pdfium
        .load_pdf_from_byte_vec(import.bytes, None)
        .map_err(|_| {
            (
                ErrorCode::ImportFailed,
                "the document to insert could not be read".to_string(),
            )
        })?;
    let added = source.pages().len() as usize;
    if added == 0 {
        return fail(
            ErrorCode::ImportFailed,
            "the document to insert has no pages",
        );
    }
    if count + added > MAX_EDIT_PAGES {
        return fail(
            ErrorCode::TooManyPages,
            format!("the page tools work on documents of up to {MAX_EDIT_PAGES} pages"),
        );
    }
    // The document as it is now: the copy that takes the insert back, and that the document is put
    // back from when a step fails.
    let Some(bytes) = serialise(engine, doc, MAX_SAVE_BYTES, progress)? else {
        return fail(
            ErrorCode::SaveTooLarge,
            "the document is too big for pages to be inserted",
        );
    };
    let password = password_of(engine, doc);
    let miscount = tools(engine, doc).miscount();
    let result = (|| {
        let document = engine.docs.get_mut(&doc).ok_or_else(gone)?;
        document
            .pages_mut()
            .copy_page_range_from_document(
                &source,
                0..=(added as PdfPageIndex - 1),
                at as PdfPageIndex,
            )
            .map_err(|_| failed("PDFium could not insert the pages"))?;
        if document.pages().len() as isize + miscount != (count + added) as isize {
            return Err(failed(
                "the document has the wrong number of pages after the insert",
            ));
        }
        // The copied pages still point into `source` for some things that PDFium does not carry
        // over (a pop-up note's `/Parent`). Written out and read in again, with `source` still
        // alive, the document is one that stands on its own. Dropping `source` before that left
        // the next save reading memory that was freed.
        let written = save::serialise(document)?;
        load_copy(pdfium, written, password_str(&password), count + added)
    })();
    match result {
        Ok(loaded) => {
            engine.docs.insert(doc, loaded);
            drop(source);
        }
        Err(e) => {
            let original = put_back(engine, pdfium, doc, bytes, count, password, e);
            drop(source);
            return Err(original);
        }
    }
    let limit = tools(engine, doc).max_undo_bytes;
    let token = if bytes.len() <= limit {
        tools(engine, doc).keep(bytes, count)
    } else {
        0
    };
    changed(engine, doc, token, added)
}

fn export_begin(
    engine: &mut Engine,
    pdfium: &'static Pdfium,
    doc: u32,
    pages: &[u32],
    progress: &mut dyn FnMut(u32),
) -> Result<PagesResponse, Failure> {
    let count = editable_list(engine, doc, Right::Extract)?;
    if pages.is_empty() || pages.len() > MAX_EDIT_PAGES {
        return fail(
            ErrorCode::InvalidRequest,
            "the pages to copy are not usable",
        );
    }
    if pages.iter().any(|&p| p as usize >= count) {
        return fail(
            ErrorCode::PageOutOfRange,
            "a page that was named does not exist",
        );
    }
    engine.check_source(progress)?;
    let document = engine.document(doc)?;
    let mut fresh = pdfium
        .create_new_pdf()
        .map_err(|_| failed("PDFium could not make a new document"))?;
    fresh
        .pages_mut()
        .copy_pages_from_document(document, &range_string(pages), 0)
        .map_err(|_| failed("PDFium could not copy the pages"))?;
    if fresh.pages().len() as usize != pages.len() {
        return Err(failed("the new document has the wrong number of pages"));
    }
    // The pages come without the signature; its data must not stay behind in the bytes. An object
    // that was copied in is written whether or not anything still refers to it, so the emptied
    // signature would still be written; written out and read in again, it is an object of a loaded
    // file, which a save leaves out when nothing refers to it.
    if drop_copied_signatures(&fresh) > 0 {
        let written = save::serialise_up_to(&fresh, MAX_SAVE_BYTES, progress)?;
        fresh = load_copy(pdfium, written, None, pages.len())?;
    }
    let bytes = save::serialise_up_to(&fresh, MAX_SAVE_BYTES, progress)?;
    let (size, print) = (bytes.len(), save::fingerprint(&fresh, progress));
    let ResponseBody::Fingerprint {
        pages: printed,
        failed: unreadable,
        digest,
    } = print
    else {
        return Err(failed("the new document could not be looked at"));
    };
    if unreadable > 0 {
        return fail(
            ErrorCode::PageChangeFailed,
            "a page of the new document could not be read",
        );
    }
    let ResponseBody::DocInfo { signatures, .. } = save::doc_info(&fresh, None, progress) else {
        return Err(failed("the new document could not be looked at"));
    };
    let source_security = save::security_revision(document);
    engine.check_source(progress)?;
    engine.saves.insert(doc, bytes);
    Ok(PagesResponse::Exported {
        size: size as u32,
        pages: printed,
        signatures,
        source_security,
        digest,
    })
}

/// Draws and changes a throw-away document once, before the sandbox is applied, so that PDFium
/// sets up what these code paths set up on first use while it can still ask the system for it.
pub fn warm_up(pdfium: &'static Pdfium, source: &PdfDocument<'_>) {
    let Ok(mut fresh) = pdfium.create_new_pdf() else {
        return;
    };
    let _ = fresh.pages_mut().copy_pages_from_document(source, "1", 0);
    let _ = fresh
        .pages_mut()
        .create_page_at_index(PdfPagePaperSize::a4(), 0);
    let _ = move_pages(&fresh, &[1], 0);
    clear_widget_text(&fresh, 0);
    drop_copied_signatures(&fresh);
    if let Ok(mut page) = fresh.pages().get(0) {
        page.set_rotation(PdfPageRenderRotation::Degrees90);
        let _ = page
            .boundaries_mut()
            .set_crop(PdfRect::new_from_values(10.0, 10.0, 100.0, 100.0));
    }
    if let Ok(bytes) = save::serialise(&fresh) {
        if let Ok(again) = pdfium.load_pdf_from_byte_slice(&bytes, None) {
            if let Ok(page) = again.pages().get(0) {
                let _ = page.delete();
            }
            let _ = save::serialise(&again);
        }
    }
}
