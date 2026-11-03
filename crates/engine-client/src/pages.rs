//! The page tools, from the app's side: asking the engine to delete, move, turn, crop, insert and
//! copy out pages, and holding it to the rules while it does.
//!
//! Every answer is checked before it is passed on: the sizes of the pages must be sizes, a piece
//! of a document must start where the last one ended and be as long as was asked, a document must
//! not be bigger than a save may be. An engine that breaks a rule is stopped, like in a save.

use std::io;

use partridge_protocol::{
    ErrorCode, PageSize, PagesRequest, PagesResponse, RequestBody, ResponseBody, MAX_BLANK_SIDE,
    MAX_BOX_VALUE, MAX_EDIT_PAGES, MAX_IMPORT_BYTES, MAX_SAVE_BYTES, MAX_SAVE_CHUNK,
    MIN_BLANK_SIDE,
};

use crate::{lock, stale_document, usable_page_side, ClientError, EngineClient};

/// The document after a change.
#[derive(Debug, Clone, PartialEq)]
pub struct PagesChanged {
    /// The size of every page now, as it is drawn.
    pub pages: Vec<PageSize>,
    /// The number of the copy of the document before the change, to go back to it; 0 when there
    /// is none (a turn needs none, and a document too big to keep a copy of has none).
    pub snapshot: u32,
    /// How many pages the change touched.
    pub count: u32,
    /// How many form fields sat on the pages that were deleted.
    pub form_fields: u32,
}

/// The document after a crop, with the crop box of each cropped page before and after.
#[derive(Debug, Clone, PartialEq)]
pub struct PagesCropped {
    pub pages: Vec<PageSize>,
    pub before: Vec<[f32; 4]>,
    pub after: Vec<[f32; 4]>,
}

/// A new document the engine built out of some pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exported {
    pub size: u32,
    pub pages: u32,
    pub signatures: u32,
    /// The kind of protection of the document the pages come from; the new document has none.
    pub source_security: u8,
    pub digest: u64,
}

fn refuse(message: &str) -> ClientError {
    ClientError::Engine {
        code: ErrorCode::InvalidRequest,
        message: message.into(),
    }
}

/// An answer that must be `Changed`, with page sizes the window can lay out. A document of no
/// pages is never a result: the engine refuses to delete the last page, and a document that reads
/// back with none is not the one that was open.
pub(crate) fn accept_changed(body: ResponseBody) -> Result<PagesChanged, ResponseBody> {
    match body {
        ResponseBody::Pages(PagesResponse::Changed {
            pages,
            snapshot,
            count,
            form_fields,
        }) if !pages.is_empty()
            && usable_sizes(&pages)
            && count as usize <= partridge_protocol::MAX_PAGES =>
        {
            Ok(PagesChanged {
                pages,
                snapshot,
                count,
                form_fields,
            })
        }
        other => Err(other),
    }
}

fn usable_sizes(pages: &[PageSize]) -> bool {
    pages
        .iter()
        .all(|p| usable_page_side(p.width) && usable_page_side(p.height))
}

impl EngineClient {
    fn pages_call(
        &self,
        doc: u64,
        what: &str,
        op: impl FnOnce(u32) -> PagesRequest,
    ) -> Result<PagesResponse, ClientError> {
        self.call(
            doc,
            what,
            |doc| RequestBody::Pages(op(doc)),
            |body| match body {
                ResponseBody::Pages(response) => Ok(response),
                other => Err(other),
            },
        )
    }

    /// An answer that must be `Changed`, with page sizes the window can lay out.
    fn pages_changed(
        &self,
        doc: u64,
        what: &str,
        op: impl FnOnce(u32) -> PagesRequest,
    ) -> Result<PagesChanged, ClientError> {
        self.call(doc, what, |doc| RequestBody::Pages(op(doc)), accept_changed)
    }

    /// Removes pages (counted from 0).
    pub fn pages_delete(&self, doc: u64, pages: &[u32]) -> Result<PagesChanged, ClientError> {
        list_ok(pages)?;
        self.pages_changed(doc, "delete", |doc| PagesRequest::Delete {
            doc,
            pages: pages.to_vec(),
        })
    }

    /// Puts the pages in a new order: `order[i]` is the page that goes to place `i`.
    pub fn pages_reorder(&self, doc: u64, order: &[u32]) -> Result<PagesChanged, ClientError> {
        list_ok(order)?;
        self.pages_changed(doc, "reorder", |doc| PagesRequest::Reorder {
            doc,
            order: order.to_vec(),
        })
    }

    /// Turns pages by `turns` quarter turns clockwise (1 to 3).
    pub fn pages_rotate(
        &self,
        doc: u64,
        pages: &[u32],
        turns: u8,
    ) -> Result<PagesChanged, ClientError> {
        list_ok(pages)?;
        self.pages_changed(doc, "rotate", |doc| PagesRequest::Rotate {
            doc,
            pages: pages.to_vec(),
            turns,
        })
    }

    /// Crops pages: the margins are `[left, top, right, bottom]` in points on the page as it is
    /// drawn. This hides the edges; it removes nothing.
    pub fn pages_crop(
        &self,
        doc: u64,
        pages: &[u32],
        margins: [f32; 4],
    ) -> Result<PagesCropped, ClientError> {
        list_ok(pages)?;
        // The limits the engine and the protocol have: a number outside them would end the engine
        // (the protocol refuses to carry it) instead of being refused.
        if margins
            .iter()
            .any(|m| !m.is_finite() || *m < 0.0 || *m > MAX_BLANK_SIDE)
        {
            return Err(refuse("the margins are not usable"));
        }
        let wanted = pages.len();
        self.call(
            doc,
            "crop",
            |doc| {
                RequestBody::Pages(PagesRequest::Crop {
                    doc,
                    pages: pages.to_vec(),
                    margins,
                })
            },
            |body| match body {
                ResponseBody::Pages(PagesResponse::Cropped {
                    pages,
                    before,
                    after,
                }) if usable_sizes(&pages)
                    && before.len() <= wanted
                    && after.len() == before.len() =>
                {
                    Ok(PagesCropped {
                        pages,
                        before,
                        after,
                    })
                }
                other => Err(other),
            },
        )
    }

    /// Sets the crop box of each page to the box given (`[left, bottom, right, top]` in the page's
    /// own coordinates), to take a crop back or make it again.
    pub fn pages_set_boxes(
        &self,
        doc: u64,
        pages: &[u32],
        boxes: &[[f32; 4]],
    ) -> Result<PagesChanged, ClientError> {
        list_ok(pages)?;
        if pages.len() != boxes.len() {
            return Err(refuse("every page needs a box"));
        }
        if boxes
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || v.abs() > MAX_BOX_VALUE)
        {
            return Err(refuse("a box is not usable"));
        }
        self.pages_changed(doc, "set boxes", |doc| PagesRequest::SetBoxes {
            doc,
            pages: pages.to_vec(),
            boxes: boxes.to_vec(),
        })
    }

    /// Adds an empty page so that it becomes page `at`.
    pub fn pages_insert_blank(
        &self,
        doc: u64,
        at: u32,
        width: f32,
        height: f32,
    ) -> Result<PagesChanged, ClientError> {
        let side = |v: f32| v.is_finite() && (MIN_BLANK_SIDE..=MAX_BLANK_SIDE).contains(&v);
        if !side(width) || !side(height) {
            return Err(refuse("the size of the page is not usable"));
        }
        self.pages_changed(doc, "insert a blank page", |doc| {
            PagesRequest::InsertBlank {
                doc,
                at,
                width,
                height,
            }
        })
    }

    /// Goes back to the copy of the document with this number.
    pub fn pages_restore(&self, doc: u64, snapshot: u32) -> Result<PagesChanged, ClientError> {
        self.pages_changed(doc, "restore", |doc| PagesRequest::Restore {
            doc,
            snapshot,
        })
    }

    /// Begins to hand over a document whose pages are to be inserted. `size` is its length.
    pub fn import_begin(&self, doc: u64, size: u64) -> Result<(), ClientError> {
        if size == 0 || size as usize > MAX_IMPORT_BYTES {
            return Err(refuse("the document to insert is too big"));
        }
        self.pages_call(doc, "insert", |doc| PagesRequest::ImportBegin {
            doc,
            size: size as u32,
        })
        .and_then(|r| match r {
            PagesResponse::ImportReady => Ok(()),
            _ => Err(refuse("the engine did not accept the document")),
        })
    }

    /// Hands over the next piece of the document, which starts at `offset`.
    pub fn import_piece(&self, doc: u64, offset: u64, data: &[u8]) -> Result<(), ClientError> {
        if data.len() > MAX_SAVE_CHUNK {
            return Err(refuse("the piece is too long"));
        }
        let expect = offset + data.len() as u64;
        self.call(
            doc,
            "insert",
            |doc| {
                RequestBody::Pages(PagesRequest::ImportChunk {
                    doc,
                    offset: offset as u32,
                    data: data.to_vec(),
                })
            },
            |body| match body {
                ResponseBody::Pages(PagesResponse::ImportAck { received })
                    if u64::from(received) == expect =>
                {
                    Ok(())
                }
                other => Err(other),
            },
        )
    }

    /// The document is complete: inserts all its pages so that the first becomes page `at`.
    pub fn import_end(&self, doc: u64, at: u32) -> Result<PagesChanged, ClientError> {
        self.pages_changed(doc, "insert", |doc| PagesRequest::ImportEnd { doc, at })
    }

    /// Drops what was handed over so far. Does nothing for a document that is gone.
    pub fn import_abort(&self, doc: u64) {
        let _ = self.pages_call(doc, "insert", |doc| PagesRequest::ImportAbort { doc });
    }

    /// Has the engine build a new document out of some pages, and hands the bytes to `piece`
    /// as they arrive, after `begin` has been told what is coming. The engine is held to the
    /// same rules as in a save: the size is at most what a save may be, every piece is at the
    /// offset asked for and as long as asked, and together they are the size announced. An error
    /// of `begin` or `piece` ends the work (the engine drops what it holds).
    pub fn export_to(
        &self,
        doc: u64,
        pages: &[u32],
        begin: &mut dyn FnMut(&Exported) -> io::Result<()>,
        piece: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> Result<Exported, ExportError> {
        list_ok(pages).map_err(ExportError::Client)?;
        let mut inner = lock(&self.inner);
        let Some(engine_doc) = Self::engine_doc(&inner, doc) else {
            return Err(ExportError::Client(stale_document()));
        };
        let body = RequestBody::Pages(PagesRequest::ExportBegin {
            doc: engine_doc,
            pages: pages.to_vec(),
        });
        let exported = match self.run(&mut inner, body)? {
            ResponseBody::Pages(PagesResponse::Exported {
                size,
                pages: count,
                signatures,
                source_security,
                digest,
            }) if size as usize <= MAX_SAVE_BYTES && count as usize == pages.len() => Exported {
                size,
                pages: count,
                signatures,
                source_security,
                digest,
            },
            _ => {
                return Err(ExportError::Client(
                    self.unexpected::<()>(&mut inner, "copy pages").unwrap_err(),
                ))
            }
        };
        let result = (|| {
            begin(&exported).map_err(ExportError::Sink)?;
            let mut done: u32 = 0;
            while done < exported.size {
                let len = (exported.size - done).min(MAX_SAVE_CHUNK as u32);
                let body = RequestBody::SaveChunk {
                    doc: engine_doc,
                    offset: done,
                    len,
                };
                match self.run(&mut inner, body) {
                    Ok(ResponseBody::SaveData { offset, data })
                        if offset == done && data.len() == len as usize =>
                    {
                        piece(&data).map_err(ExportError::Sink)?;
                        done += len;
                    }
                    Ok(_) => {
                        return Err(ExportError::Client(
                            self.unexpected::<()>(&mut inner, "copy pages").unwrap_err(),
                        ))
                    }
                    Err(e) => return Err(ExportError::Client(e)),
                }
            }
            Ok(())
        })();
        if inner.running.is_some() {
            let _ = self.run(&mut inner, RequestBody::SaveEnd { doc: engine_doc });
        }
        result.map(|()| exported)
    }
}

/// Why [`EngineClient::export_to`] stopped.
#[derive(Debug)]
pub enum ExportError {
    Client(ClientError),
    /// The caller's side failed (the file could not be written, the user stopped the work).
    Sink(io::Error),
}

impl From<ClientError> for ExportError {
    fn from(e: ClientError) -> Self {
        ExportError::Client(e)
    }
}

/// A list of pages that fits the protocol.
fn list_ok(pages: &[u32]) -> Result<(), ClientError> {
    if pages.is_empty() {
        return Err(refuse("no pages were named"));
    }
    if pages.len() > MAX_EDIT_PAGES {
        return Err(ClientError::Engine {
            code: ErrorCode::TooManyPages,
            message: format!("at most {MAX_EDIT_PAGES} pages can be worked on at a time"),
        });
    }
    Ok(())
}
