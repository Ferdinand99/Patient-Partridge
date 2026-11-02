//! The messages of the page tools: delete, reorder, rotate, crop, insert, and extract pages.
//!
//! Page numbers on the wire count from 0. Every list has a cap that the decoder checks before it
//! reserves anything, on both sides, so a damaged or hostile peer cannot make the other allocate or
//! loop for ever: a list of pages holds at most [`MAX_EDIT_PAGES`] entries, a document that the
//! engine reports holds at most [`MAX_PAGES`] pages, and the bytes of an inserted document come in
//! pieces of at most [`MAX_SAVE_CHUNK`] up to [`MAX_IMPORT_BYTES`] altogether.

use super::{put_f32, put_u32, PageSize, ProtocolError, Reader, MAX_SAVE_BYTES, MAX_SAVE_CHUNK};

/// Most pages a document may have for the engine to open it. A document with more is refused: the
/// answer to `Open` lists every page, and the window lays every page out.
pub const MAX_PAGES: usize = 100_000;

/// Most pages in a list that a page tool works on, and most pages a document may have for a page
/// tool to work on it. Deleting or moving a page in PDFium costs time in proportion to the number
/// of pages, so the tools stop where that time would be minutes.
pub const MAX_EDIT_PAGES: usize = 10_000;

/// Most bytes of a document that is inserted: what a save may write.
pub const MAX_IMPORT_BYTES: usize = MAX_SAVE_BYTES;

/// Most bytes of changes the engine keeps to take page changes back (copies of the document as it
/// was before a delete, a move or an insert). The oldest copies are let go first.
pub const MAX_UNDO_BYTES: usize = 256 * 1024 * 1024;

/// Most page changes the engine can take back.
pub const MAX_UNDO_STEPS: usize = 32;

/// Smallest side of a cropped page, in points. A crop that leaves less is refused.
pub const MIN_CROP_SIDE: f32 = 8.0;

/// Largest side of a blank page, in points (200 inches, what PDF allows).
pub const MAX_BLANK_SIDE: f32 = 14_400.0;

/// Smallest side of a blank page, in points.
pub const MIN_BLANK_SIDE: f32 = 8.0;

/// Largest number a page box may hold on the wire, in points. Larger numbers are refused.
pub const MAX_BOX_VALUE: f32 = 1.0e7;

pub(crate) const FIRST_REQUEST_TAG: u8 = 60;
pub(crate) const LAST_REQUEST_TAG: u8 = 71;
pub(crate) const FIRST_RESPONSE_TAG: u8 = 190;
pub(crate) const LAST_RESPONSE_TAG: u8 = 195;

const TAG_DELETE: u8 = 60;
const TAG_REORDER: u8 = 61;
const TAG_ROTATE: u8 = 62;
const TAG_CROP: u8 = 63;
const TAG_SET_BOXES: u8 = 64;
const TAG_INSERT_BLANK: u8 = 65;
const TAG_RESTORE: u8 = 66;
const TAG_IMPORT_BEGIN: u8 = 67;
const TAG_IMPORT_CHUNK: u8 = 68;
const TAG_IMPORT_END: u8 = 69;
const TAG_IMPORT_ABORT: u8 = 70;
const TAG_EXPORT_BEGIN: u8 = 71;

const TAG_CHANGED: u8 = 190;
const TAG_CROPPED: u8 = 191;
const TAG_IMPORT_READY: u8 = 192;
const TAG_IMPORT_ACK: u8 = 193;
const TAG_EXPORTED: u8 = 194;
const TAG_DONE: u8 = 195;

/// A request of the page tools. A box is `[left, bottom, right, top]` in the page's own
/// coordinates (PDF user space, before the page's rotation).
#[derive(Debug, Clone, PartialEq)]
pub enum PagesRequest {
    /// Remove pages. The engine keeps a copy of the document as it was, and says its number.
    /// Removing every page is refused.
    Delete { doc: u32, pages: Vec<u32> },
    /// Put the pages in a new order. `order` lists, for every place in the new document, the page
    /// that goes there: it holds every page number once.
    Reorder { doc: u32, order: Vec<u32> },
    /// Turn pages clockwise by `turns` quarter turns (1 to 3).
    Rotate {
        doc: u32,
        pages: Vec<u32>,
        turns: u8,
    },
    /// Set the crop box of pages so that the visible page is smaller by `margins`, which are
    /// `[left, top, right, bottom]` in points on the page as it is drawn. This hides the edges;
    /// it removes nothing from the page.
    Crop {
        doc: u32,
        pages: Vec<u32>,
        margins: [f32; 4],
    },
    /// Set the crop box of each page to the box given (to take a crop back or make it again).
    SetBoxes {
        doc: u32,
        pages: Vec<u32>,
        boxes: Vec<[f32; 4]>,
    },
    /// Add an empty page of the given size so that it becomes page `at`.
    InsertBlank {
        doc: u32,
        at: u32,
        width: f32,
        height: f32,
    },
    /// Go back to the copy of the document with this number (taken by a delete, a reorder or an
    /// insert). The engine keeps the state it leaves under a new number, so that this can be
    /// undone again.
    Restore { doc: u32, snapshot: u32 },
    /// Begin to hand over a document whose pages are to be inserted: `size` bytes follow in pieces.
    ImportBegin { doc: u32, size: u32 },
    /// The next piece of the document, which must start at `offset`.
    ImportChunk {
        doc: u32,
        offset: u32,
        data: Vec<u8>,
    },
    /// The document is complete: insert all of its pages so that the first becomes page `at`.
    ImportEnd { doc: u32, at: u32 },
    /// Drop what `ImportBegin` and `ImportChunk` have gathered.
    ImportAbort { doc: u32 },
    /// Build a new document from some pages of this one and keep its bytes for the pieces that
    /// `SaveChunk` hands out (and `SaveEnd` frees). The document itself is not changed.
    ExportBegin { doc: u32, pages: Vec<u32> },
}

/// An answer of the page tools.
#[derive(Debug, Clone, PartialEq)]
pub enum PagesResponse {
    /// The document after a change: the size of every page, the number of the copy of the
    /// document before the change (0 when there is none to go back to), how many pages the
    /// change touched, and (for a delete) how many form fields sat on the deleted pages: PDFium
    /// keeps the list of fields of a form, so those fields stay in the file, with the text typed
    /// into text fields cleared.
    Changed {
        pages: Vec<PageSize>,
        snapshot: u32,
        count: u32,
        form_fields: u32,
    },
    /// The document after a crop, with the crop box of each page before and after.
    Cropped {
        pages: Vec<PageSize>,
        before: Vec<[f32; 4]>,
        after: Vec<[f32; 4]>,
    },
    ImportReady,
    /// How many bytes of the document to insert the engine holds now.
    ImportAck {
        received: u32,
    },
    /// A document built by `ExportBegin`, `size` bytes, with what the check of the written file
    /// needs: its pages, its signatures, the kind of protection of the source (the new document
    /// has none), and a digest of what a reader sees of the pages.
    Exported {
        size: u32,
        pages: u32,
        signatures: u32,
        source_security: u8,
        digest: u64,
    },
    Done,
}

fn put_list(out: &mut Vec<u8>, list: &[u32]) {
    let list = &list[..list.len().min(MAX_EDIT_PAGES)];
    put_u32(out, list.len() as u32);
    for v in list {
        put_u32(out, *v);
    }
}

fn get_list(r: &mut Reader<'_>) -> Result<Vec<u32>, ProtocolError> {
    let count = r.u32()? as usize;
    if count > MAX_EDIT_PAGES {
        return Err(ProtocolError::InvalidLength);
    }
    if count > r.remaining() / 4 {
        return Err(ProtocolError::Truncated);
    }
    (0..count).map(|_| r.u32()).collect()
}

/// A number of a box: finite and not absurd.
fn get_number(r: &mut Reader<'_>) -> Result<f32, ProtocolError> {
    let v = r.f32()?;
    if !v.is_finite() || v.abs() > MAX_BOX_VALUE {
        return Err(ProtocolError::InvalidLength);
    }
    Ok(v)
}

fn get_box(r: &mut Reader<'_>) -> Result<[f32; 4], ProtocolError> {
    Ok([
        get_number(r)?,
        get_number(r)?,
        get_number(r)?,
        get_number(r)?,
    ])
}

fn put_box(out: &mut Vec<u8>, b: &[f32; 4]) {
    for v in b {
        put_f32(out, *v);
    }
}

fn put_sizes(out: &mut Vec<u8>, pages: &[PageSize]) {
    let pages = &pages[..pages.len().min(MAX_PAGES)];
    put_u32(out, pages.len() as u32);
    for p in pages {
        put_f32(out, p.width);
        put_f32(out, p.height);
    }
}

fn get_sizes(r: &mut Reader<'_>) -> Result<Vec<PageSize>, ProtocolError> {
    let count = r.u32()? as usize;
    if count > MAX_PAGES {
        return Err(ProtocolError::InvalidLength);
    }
    if count > r.remaining() / 8 {
        return Err(ProtocolError::Truncated);
    }
    (0..count)
        .map(|_| {
            Ok(PageSize {
                width: r.f32()?,
                height: r.f32()?,
            })
        })
        .collect()
}

impl PagesRequest {
    /// Writes the request: tag, id, fields.
    pub(crate) fn encode(&self, id: u32, out: &mut Vec<u8>) {
        match self {
            PagesRequest::Delete { doc, pages } => {
                out.push(TAG_DELETE);
                put_u32(out, id);
                put_u32(out, *doc);
                put_list(out, pages);
            }
            PagesRequest::Reorder { doc, order } => {
                out.push(TAG_REORDER);
                put_u32(out, id);
                put_u32(out, *doc);
                put_list(out, order);
            }
            PagesRequest::Rotate { doc, pages, turns } => {
                out.push(TAG_ROTATE);
                put_u32(out, id);
                put_u32(out, *doc);
                put_list(out, pages);
                out.push(*turns);
            }
            PagesRequest::Crop {
                doc,
                pages,
                margins,
            } => {
                out.push(TAG_CROP);
                put_u32(out, id);
                put_u32(out, *doc);
                put_list(out, pages);
                put_box(out, margins);
            }
            PagesRequest::SetBoxes { doc, pages, boxes } => {
                out.push(TAG_SET_BOXES);
                put_u32(out, id);
                put_u32(out, *doc);
                // One box for every page; a list that does not match is cut to the shorter.
                let n = pages.len().min(boxes.len()).min(MAX_EDIT_PAGES);
                put_list(out, &pages[..n]);
                for b in &boxes[..n] {
                    put_box(out, b);
                }
            }
            PagesRequest::InsertBlank {
                doc,
                at,
                width,
                height,
            } => {
                out.push(TAG_INSERT_BLANK);
                put_u32(out, id);
                put_u32(out, *doc);
                put_u32(out, *at);
                put_f32(out, *width);
                put_f32(out, *height);
            }
            PagesRequest::Restore { doc, snapshot } => {
                out.push(TAG_RESTORE);
                put_u32(out, id);
                put_u32(out, *doc);
                put_u32(out, *snapshot);
            }
            PagesRequest::ImportBegin { doc, size } => {
                out.push(TAG_IMPORT_BEGIN);
                put_u32(out, id);
                put_u32(out, *doc);
                put_u32(out, *size);
            }
            PagesRequest::ImportChunk { doc, offset, data } => {
                let data = &data[..data.len().min(MAX_SAVE_CHUNK)];
                out.reserve(data.len() + 16);
                out.push(TAG_IMPORT_CHUNK);
                put_u32(out, id);
                put_u32(out, *doc);
                put_u32(out, *offset);
                put_u32(out, data.len() as u32);
                out.extend_from_slice(data);
            }
            PagesRequest::ImportEnd { doc, at } => {
                out.push(TAG_IMPORT_END);
                put_u32(out, id);
                put_u32(out, *doc);
                put_u32(out, *at);
            }
            PagesRequest::ImportAbort { doc } => {
                out.push(TAG_IMPORT_ABORT);
                put_u32(out, id);
                put_u32(out, *doc);
            }
            PagesRequest::ExportBegin { doc, pages } => {
                out.push(TAG_EXPORT_BEGIN);
                put_u32(out, id);
                put_u32(out, *doc);
                put_list(out, pages);
            }
        }
    }

    /// Reads the fields of the request with this tag (the tag and the id are read already).
    pub(crate) fn decode(tag: u8, r: &mut Reader<'_>) -> Result<PagesRequest, ProtocolError> {
        Ok(match tag {
            TAG_DELETE => PagesRequest::Delete {
                doc: r.u32()?,
                pages: get_list(r)?,
            },
            TAG_REORDER => PagesRequest::Reorder {
                doc: r.u32()?,
                order: get_list(r)?,
            },
            TAG_ROTATE => PagesRequest::Rotate {
                doc: r.u32()?,
                pages: get_list(r)?,
                turns: r.u8()?,
            },
            TAG_CROP => PagesRequest::Crop {
                doc: r.u32()?,
                pages: get_list(r)?,
                margins: get_box(r)?,
            },
            TAG_SET_BOXES => {
                let doc = r.u32()?;
                let pages = get_list(r)?;
                if pages.len() > r.remaining() / 16 {
                    return Err(ProtocolError::Truncated);
                }
                let boxes = (0..pages.len())
                    .map(|_| get_box(r))
                    .collect::<Result<Vec<_>, _>>()?;
                PagesRequest::SetBoxes { doc, pages, boxes }
            }
            TAG_INSERT_BLANK => PagesRequest::InsertBlank {
                doc: r.u32()?,
                at: r.u32()?,
                width: get_number(r)?,
                height: get_number(r)?,
            },
            TAG_RESTORE => PagesRequest::Restore {
                doc: r.u32()?,
                snapshot: r.u32()?,
            },
            TAG_IMPORT_BEGIN => {
                let doc = r.u32()?;
                let size = r.u32()?;
                if size as usize > MAX_IMPORT_BYTES {
                    return Err(ProtocolError::InvalidLength);
                }
                PagesRequest::ImportBegin { doc, size }
            }
            TAG_IMPORT_CHUNK => {
                let doc = r.u32()?;
                let offset = r.u32()?;
                let len = r.u32()? as usize;
                if len > MAX_SAVE_CHUNK {
                    return Err(ProtocolError::InvalidLength);
                }
                PagesRequest::ImportChunk {
                    doc,
                    offset,
                    data: r.take(len)?.to_vec(),
                }
            }
            TAG_IMPORT_END => PagesRequest::ImportEnd {
                doc: r.u32()?,
                at: r.u32()?,
            },
            TAG_IMPORT_ABORT => PagesRequest::ImportAbort { doc: r.u32()? },
            TAG_EXPORT_BEGIN => PagesRequest::ExportBegin {
                doc: r.u32()?,
                pages: get_list(r)?,
            },
            other => return Err(ProtocolError::UnknownTag(other)),
        })
    }
}

impl PagesResponse {
    pub(crate) fn encode(&self, id: u32, out: &mut Vec<u8>) {
        match self {
            PagesResponse::Changed {
                pages,
                snapshot,
                count,
                form_fields,
            } => {
                out.push(TAG_CHANGED);
                put_u32(out, id);
                put_sizes(out, pages);
                put_u32(out, *snapshot);
                put_u32(out, *count);
                put_u32(out, *form_fields);
            }
            PagesResponse::Cropped {
                pages,
                before,
                after,
            } => {
                out.push(TAG_CROPPED);
                put_u32(out, id);
                put_sizes(out, pages);
                let n = before.len().min(after.len()).min(MAX_EDIT_PAGES);
                put_u32(out, n as u32);
                for b in &before[..n] {
                    put_box(out, b);
                }
                for b in &after[..n] {
                    put_box(out, b);
                }
            }
            PagesResponse::ImportReady => {
                out.push(TAG_IMPORT_READY);
                put_u32(out, id);
            }
            PagesResponse::ImportAck { received } => {
                out.push(TAG_IMPORT_ACK);
                put_u32(out, id);
                put_u32(out, *received);
            }
            PagesResponse::Exported {
                size,
                pages,
                signatures,
                source_security,
                digest,
            } => {
                out.push(TAG_EXPORTED);
                put_u32(out, id);
                put_u32(out, *size);
                put_u32(out, *pages);
                put_u32(out, *signatures);
                out.push(*source_security);
                out.extend_from_slice(&digest.to_le_bytes());
            }
            PagesResponse::Done => {
                out.push(TAG_DONE);
                put_u32(out, id);
            }
        }
    }

    pub(crate) fn decode(tag: u8, r: &mut Reader<'_>) -> Result<PagesResponse, ProtocolError> {
        Ok(match tag {
            TAG_CHANGED => PagesResponse::Changed {
                pages: get_sizes(r)?,
                snapshot: r.u32()?,
                count: r.u32()?,
                form_fields: r.u32()?,
            },
            TAG_CROPPED => {
                let pages = get_sizes(r)?;
                let n = r.u32()? as usize;
                if n > MAX_EDIT_PAGES {
                    return Err(ProtocolError::InvalidLength);
                }
                if n > r.remaining() / 32 {
                    return Err(ProtocolError::Truncated);
                }
                let before = (0..n).map(|_| get_box(r)).collect::<Result<Vec<_>, _>>()?;
                let after = (0..n).map(|_| get_box(r)).collect::<Result<Vec<_>, _>>()?;
                PagesResponse::Cropped {
                    pages,
                    before,
                    after,
                }
            }
            TAG_IMPORT_READY => PagesResponse::ImportReady,
            TAG_IMPORT_ACK => PagesResponse::ImportAck { received: r.u32()? },
            TAG_EXPORTED => {
                let size = r.u32()?;
                if size as usize > MAX_SAVE_BYTES {
                    return Err(ProtocolError::InvalidLength);
                }
                let pages = r.u32()?;
                if pages as usize > MAX_EDIT_PAGES {
                    return Err(ProtocolError::InvalidLength);
                }
                let signatures = r.u32()?;
                let source_security = r.u8()?;
                let d = r.take(8)?;
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(d);
                PagesResponse::Exported {
                    size,
                    pages,
                    signatures,
                    source_security,
                    digest: u64::from_le_bytes(bytes),
                }
            }
            TAG_DONE => PagesResponse::Done,
            other => return Err(ProtocolError::UnknownTag(other)),
        })
    }
}

#[cfg(test)]
mod tests;
