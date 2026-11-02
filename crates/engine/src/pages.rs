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
//! - A page that is copied from another document (an insert, an extract, a move) is copied with
//!   everything it refers to, but not with what refers to it: bookmarks, the form's field tree
//!   (a field gets no parent), links to pages that come later in the same copy, the structure
//!   tree and the signature are not carried over.
//! - A move is therefore done on as few pages as possible: the pages that can stay in their order
//!   are not touched, and only the others are copied from a snapshot of the document and put in
//!   their places.
//!
//! To take a change back the engine keeps copies of the document as it was (snapshots, in memory,
//! at most [`MAX_UNDO_BYTES`] and [`MAX_UNDO_STEPS`] of them, the oldest let go first). `Restore`
//! loads one and keeps the state it leaves, so that the same step can be redone. Rotating and
//! cropping are taken back by the app with the opposite change and need no snapshot.

use std::collections::VecDeque;

use partridge_protocol::{
    ErrorCode, PageSize, PagesRequest, PagesResponse, ResponseBody, MAX_BLANK_SIDE, MAX_EDIT_PAGES,
    MAX_IMPORT_BYTES, MAX_SAVE_BYTES, MAX_SAVE_CHUNK, MAX_UNDO_BYTES, MAX_UNDO_STEPS,
    MIN_BLANK_SIDE, MIN_CROP_SIDE,
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

    fn keep(&mut self, bytes: Vec<u8>) -> u32 {
        let token = self.next_token;
        self.next_token = self.next_token.wrapping_add(1).max(1);
        self.snapshots.push_back(Snapshot { token, bytes });
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

fn usable(b: &[f32; 4]) -> bool {
    b.iter().all(|v| v.is_finite())
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

/// The document may be changed: it is open, no save or copy is holding its bytes, and it has no
/// more pages than the tools take. Returns the number of pages.
fn editable(engine: &Engine, doc: u32) -> Result<usize, Failure> {
    editable_with(engine, doc, false)
}

/// As [`editable`], and for a change to the list of pages itself (a delete, a move, a page added
/// or copied) the list must be sound: every place in it is a page that can be found. PDFium
/// crashes when it adds a page to a list that holds something that is not a page (found by the
/// fuzz target), so such a document is refused here, before PDFium is asked.
fn editable_list(engine: &Engine, doc: u32) -> Result<usize, Failure> {
    editable_with(engine, doc, true)
}

fn editable_with(engine: &Engine, doc: u32, sound: bool) -> Result<usize, Failure> {
    let document = engine.document(doc)?;
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

fn password_of(engine: &Engine, doc: u32) -> Option<String> {
    engine.passwords.get(&doc).map(|s| s.0.clone())
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
    match serialise(engine, doc, limit, progress)? {
        Some(bytes) => Ok(tools(engine, doc).keep(bytes)),
        None => Ok(0),
    }
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
    let bytes = engine
        .tools
        .get(&doc)
        .and_then(|t| t.snapshots.iter().find(|s| s.token == token))
        .map(|s| s.bytes.clone());
    match bytes {
        Some(bytes) if token != 0 => put_back(engine, pdfium, doc, bytes, password, original),
        _ => lose(engine, doc),
    }
}

fn put_back(
    engine: &mut Engine,
    pdfium: &'static Pdfium,
    doc: u32,
    bytes: Vec<u8>,
    password: Option<String>,
    original: Failure,
) -> Failure {
    match pdfium.load_pdf_from_byte_vec(bytes, password.as_deref()) {
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
/// under a parent field keeps the value its parent holds.
fn clear_form_fields(document: &PdfDocument<'_>, index: u32) -> u32 {
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
}

fn delete(
    engine: &mut Engine,
    pdfium: &'static Pdfium,
    doc: u32,
    pages: &[u32],
    progress: &mut dyn FnMut(u32),
) -> Result<PagesResponse, Failure> {
    let count = editable_list(engine, doc)?;
    let sorted = page_list(pages, count)?;
    if sorted.len() >= count {
        return fail(
            ErrorCode::InvalidRequest,
            "a document needs at least one page",
        );
    }
    let token = take_snapshot(engine, doc, progress)?;
    let fail_at = tools(engine, doc).fail_at();
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
        if document.pages().len() as usize != count - sorted.len() {
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
    let count = editable_list(engine, doc)?;
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
    // The document as it is now: the source of the pages that move, and the copy that takes the
    // move back. A document that cannot be saved cannot be reordered either.
    let Some(bytes) = serialise(engine, doc, MAX_SAVE_BYTES, progress)? else {
        return fail(
            ErrorCode::SaveTooLarge,
            "the document is too big to be reordered",
        );
    };
    let stable = stable_places(order);
    let fail_at = tools(engine, doc).fail_at();
    let password = password_of(engine, doc);
    let moved = stable.iter().filter(|s| !**s).count();
    let result = (|| {
        let source = pdfium
            .load_pdf_from_byte_slice(&bytes, password.as_deref())
            .map_err(|_| failed("a copy of the document could not be read"))?;
        let document = engine.docs.get_mut(&doc).ok_or_else(gone)?;
        // The pages that move leave first (the last ones first, so the numbers of the others
        // stay what they are) ...
        let mut leaving: Vec<u32> = (0..count)
            .filter(|i| !stable[*i])
            .map(|i| order[i])
            .collect();
        leaving.sort_unstable_by(|a, b| b.cmp(a));
        for (i, &index) in leaving.iter().enumerate() {
            if injected(fail_at, i) {
                return Err(failed("a failure made by a test"));
            }
            let page = document
                .pages()
                .get(index as PdfPageIndex)
                .map_err(|_| failed("a page could not be read"))?;
            page.delete()
                .map_err(|_| failed("PDFium could not move a page"))?;
            if i % 16 == 15 {
                progress(i as u32 + 1);
            }
        }
        // ... and come back from the copy, in their new places, from the front.
        let mut place = 0;
        while place < count {
            if stable[place] {
                place += 1;
                continue;
            }
            let mut end = place;
            while end < count && !stable[end] {
                end += 1;
            }
            document
                .pages_mut()
                .copy_pages_from_document(
                    &source,
                    &range_string(&order[place..end]),
                    place as PdfPageIndex,
                )
                .map_err(|_| failed("PDFium could not put a page in its new place"))?;
            progress(end as u32);
            place = end;
        }
        if document.pages().len() as usize != count {
            return Err(failed(
                "the document has the wrong number of pages after the move",
            ));
        }
        Ok(())
    })();
    let limit = tools(engine, doc).max_undo_bytes;
    if let Err(e) = result {
        // Put the document back from the copy.
        return Err(put_back(engine, pdfium, doc, bytes, password, e));
    }
    let token = if bytes.len() <= limit {
        tools(engine, doc).keep(bytes)
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
    let count = editable(engine, doc)?;
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
    let count = editable(engine, doc)?;
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
    let count = editable(engine, doc)?;
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
    let count = editable_list(engine, doc)?;
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
        if document.pages().len() as usize != count + 1 {
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
    editable(engine, doc)?;
    if !engine
        .tools
        .get(&doc)
        .is_some_and(|t| t.snapshots.iter().any(|s| s.token == token))
    {
        return fail(
            ErrorCode::SnapshotGone,
            "the engine no longer holds a copy of the document to go back to",
        );
    }
    // The state that is left, so that the step can be done again.
    let leaving = take_snapshot(engine, doc, progress)?;
    let password = password_of(engine, doc);
    let tools = tools(engine, doc);
    let Some(at) = tools.snapshots.iter().position(|s| s.token == token) else {
        // Making room for the copy of the state being left let go of the one we want.
        return fail(
            ErrorCode::SnapshotGone,
            "the engine no longer holds a copy of the document to go back to",
        );
    };
    let target = tools
        .snapshots
        .remove(at)
        .expect("the position was just found");
    let loaded = pdfium
        .load_pdf_from_byte_vec(target.bytes, password.as_deref())
        .map_err(|_| failed("the copy of the document could not be read"))?;
    engine.docs.insert(doc, loaded);
    changed(engine, doc, leaving, 0)
}

fn import_begin(engine: &mut Engine, doc: u32, size: u32) -> Result<PagesResponse, Failure> {
    editable(engine, doc)?;
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
    let count = editable_list(engine, doc)?;
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
    let token = take_snapshot(engine, doc, progress)?;
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
        if document.pages().len() as usize != count + added {
            return Err(failed(
                "the document has the wrong number of pages after the insert",
            ));
        }
        Ok(())
    })();
    if let Err(e) = result {
        return Err(recover(engine, pdfium, doc, token, e));
    }
    changed(engine, doc, token, added)
}

fn export_begin(
    engine: &mut Engine,
    pdfium: &'static Pdfium,
    doc: u32,
    pages: &[u32],
    progress: &mut dyn FnMut(u32),
) -> Result<PagesResponse, Failure> {
    let count = editable_list(engine, doc)?;
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
