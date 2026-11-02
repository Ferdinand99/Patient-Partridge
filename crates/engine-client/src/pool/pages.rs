//! The page tools of the pool: every change goes to the engine of the document, one at a time and
//! never while the document is being saved, and pages that come from another file are read by an
//! engine of their own (sandboxed like all of them) and handed over in pieces.

use std::io;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use super::{same_file, state, unknown, EnginePool, Ops};
use crate::pages::{ExportError, PagesChanged, PagesCropped};
use crate::save::{
    verify_written, write_checked, DocInfo, Fingerprint, SaveError, SaveOptions, SaveOutcome,
};
use crate::{ClientError, EngineClient, SaveStreamError};
use partridge_protocol::ErrorCode;

/// How far a transfer of pages has come: bytes done and bytes in all.
pub type PageProgress<'a> = &'a (dyn Fn(u64, u64) + Sync);

/// What the user agreed to when pages are inserted from another file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InsertOptions {
    /// The file is protected by a password, and its pages may join this document without that
    /// protection (they get this document's, if it has any).
    pub allow_protected: bool,
    /// The file is signed, and its pages may come without the signature.
    pub allow_signed: bool,
}

/// What was inserted.
#[derive(Debug, Clone, PartialEq)]
pub struct InsertOutcome {
    pub changed: PagesChanged,
    /// How many pages the other file had.
    pub inserted: u32,
}

/// What the user agreed to when pages are copied out to a new file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExtractOptions {
    /// The document is signed, and the new file will not carry the signature.
    pub allow_signed: bool,
    /// The document is protected by a password, and the new file will not be.
    pub allow_unprotected: bool,
}

/// What stopping the work did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cancelled {
    /// Nothing was under way.
    Idle,
    /// A document was being handed over: that stops, and the document is as it was.
    Handover,
    /// The engine is working on the document itself, which cannot be interrupted: stopping it
    /// would end the engine and lose every change of the document that is not saved. Nothing was
    /// done; the change finishes (an engine that stalls is stopped by the time limit of the pool,
    /// like in a save).
    Working,
}

struct Handles {
    client: Arc<EngineClient>,
    doc: u64,
    saving: Arc<Mutex<()>>,
    ops: Arc<Ops>,
}

/// Puts the pool back to rest when the work ends, however it ends: the other engine is stopped and
/// the phase is cleared.
struct Work<'a> {
    ops: &'a Ops,
    others: Vec<Arc<EngineClient>>,
}

impl Drop for Work<'_> {
    fn drop(&mut self) {
        for client in &self.others {
            client.kill();
        }
        if !self.others.is_empty() {
            let mut list = self
                .ops
                .transients
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            list.retain(|c| !self.others.iter().any(|o| Arc::ptr_eq(c, o)));
        }
        self.ops.phase.store(0, Ordering::SeqCst);
    }
}

/// An error of the engine that reads the file pages are inserted from, as the error to give about
/// that file. A password that is needed or wrong keeps its meaning (the window asks for it), and so
/// does a stop by the user; everything else (a damaged file, an engine that crashed, took too long
/// or could not start) says the file could not be used, in words that name the reason.
fn source_failed(e: ClientError) -> ClientError {
    match e {
        ClientError::Engine {
            code: ErrorCode::PasswordRequired | ErrorCode::PasswordIncorrect,
            ..
        }
        | ClientError::Save(_) => e,
        other => ClientError::Engine {
            code: ErrorCode::InsertSourceFailed,
            message: match other {
                ClientError::Engine { message, .. } => message,
                other => other.to_string(),
            },
        },
    }
}

impl EnginePool {
    fn handles(&self, handle: u64) -> Result<Handles, ClientError> {
        let st = state(&self.state);
        let entry = st.docs.get(&handle).ok_or_else(unknown)?;
        Ok(Handles {
            client: entry.client.clone(),
            doc: entry.doc,
            saving: entry.saving.clone(),
            ops: entry.ops.clone(),
        })
    }

    /// Runs one change on the engine of a document. Not while the document is being saved: the
    /// bytes of a save must be those of one state of the document.
    fn page_op<T>(
        &self,
        handle: u64,
        f: impl FnOnce(&EngineClient, u64) -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        let h = self.handles(handle)?;
        let _saving = h
            .saving
            .try_lock()
            .map_err(|_| ClientError::Save(SaveError::Busy))?;
        h.ops.cancel.store(false, Ordering::SeqCst);
        h.ops.phase.store(2, Ordering::SeqCst);
        let result = f(&h.client, h.doc);
        h.ops.phase.store(0, Ordering::SeqCst);
        result
    }

    pub fn pages_delete(&self, handle: u64, pages: &[u32]) -> Result<PagesChanged, ClientError> {
        self.page_op(handle, |c, d| c.pages_delete(d, pages))
    }

    pub fn pages_reorder(&self, handle: u64, order: &[u32]) -> Result<PagesChanged, ClientError> {
        self.page_op(handle, |c, d| c.pages_reorder(d, order))
    }

    pub fn pages_rotate(
        &self,
        handle: u64,
        pages: &[u32],
        turns: u8,
    ) -> Result<PagesChanged, ClientError> {
        self.page_op(handle, |c, d| c.pages_rotate(d, pages, turns))
    }

    pub fn pages_crop(
        &self,
        handle: u64,
        pages: &[u32],
        margins: [f32; 4],
    ) -> Result<PagesCropped, ClientError> {
        self.page_op(handle, |c, d| c.pages_crop(d, pages, margins))
    }

    pub fn pages_set_boxes(
        &self,
        handle: u64,
        pages: &[u32],
        boxes: &[[f32; 4]],
    ) -> Result<PagesChanged, ClientError> {
        self.page_op(handle, |c, d| c.pages_set_boxes(d, pages, boxes))
    }

    pub fn pages_insert_blank(
        &self,
        handle: u64,
        at: u32,
        width: f32,
        height: f32,
    ) -> Result<PagesChanged, ClientError> {
        self.page_op(handle, |c, d| c.pages_insert_blank(d, at, width, height))
    }

    pub fn pages_restore(&self, handle: u64, snapshot: u32) -> Result<PagesChanged, ClientError> {
        self.page_op(handle, |c, d| c.pages_restore(d, snapshot))
    }

    /// Inserts all the pages of another file so that the first becomes page `at` (counted from
    /// 0). The file is opened by an engine of its own, with a read-only handle like any document;
    /// the pages travel from that engine to the one of this document through the app, in pieces,
    /// and neither engine ever names a file. A file that needs a password fails with
    /// `password_required` or `password_incorrect` like opening does; one that is protected or
    /// signed fails with an error of its own until `options` says its pages may come without
    /// that. `progress` is told how many bytes have gone.
    pub fn insert_pdf(
        &self,
        handle: u64,
        at: u32,
        path: &str,
        password: Option<&str>,
        options: InsertOptions,
        progress: PageProgress<'_>,
    ) -> Result<InsertOutcome, ClientError> {
        let h = self.handles(handle)?;
        let _saving = h
            .saving
            .try_lock()
            .map_err(|_| ClientError::Save(SaveError::Busy))?;
        h.ops.cancel.store(false, Ordering::SeqCst);
        let cancelled = || h.ops.cancel.load(Ordering::SeqCst);
        let other = Arc::new(EngineClient::new(self.config.clone()));
        h.ops
            .transients
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(other.clone());
        h.ops.phase.store(1, Ordering::SeqCst);
        let _work = Work {
            ops: &h.ops,
            others: vec![other.clone()],
        };
        let stopped = |e: ClientError| {
            if cancelled() {
                ClientError::Save(SaveError::Cancelled)
            } else {
                e
            }
        };

        // Whatever goes wrong with the other file is about that file: the document the pages were
        // to join is not affected, and must not be taken for lost (a file that crashes its
        // engine, takes too long or cannot start one is not a reason to close the document).
        let opened = other
            .open(path, password)
            .map_err(|e| stopped(source_failed(e)))?;
        let info = other
            .doc_info(opened.doc)
            .map_err(|e| stopped(source_failed(e)))?;
        if cancelled() {
            return Err(SaveError::Cancelled.into());
        }
        if info.security != 0 && !options.allow_protected {
            return Err(SaveError::InsertProtected.into());
        }
        if info.is_signed() && !options.allow_signed {
            return Err(SaveError::InsertSigned {
                signatures: info.signatures.max(1),
            }
            .into());
        }
        let all: Vec<u32> = (0..opened.pages.len() as u32).collect();
        if all.is_empty() {
            return Err(ClientError::Engine {
                code: ErrorCode::InsertSourceFailed,
                message: "the file has no pages".into(),
            });
        }

        // The target's side of the transfer fails with a `ClientError`, which cannot travel
        // through the `io::Error` the other engine's loop knows; it waits here.
        let target_error: std::cell::RefCell<Option<ClientError>> = std::cell::RefCell::new(None);
        let total = std::cell::Cell::new(0u64);
        let mut offset: u64 = 0;
        let exported = other.export_to(
            opened.doc,
            &all,
            &mut |header| {
                total.set(u64::from(header.size));
                h.client.import_begin(h.doc, total.get()).map_err(|e| {
                    *target_error.borrow_mut() = Some(e);
                    io::Error::other("the document could not take the pages")
                })
            },
            &mut |piece| {
                if cancelled() {
                    return Err(io::Error::other("stopped"));
                }
                h.client.import_piece(h.doc, offset, piece).map_err(|e| {
                    *target_error.borrow_mut() = Some(e);
                    io::Error::other("the document could not take the pages")
                })?;
                offset += piece.len() as u64;
                progress(offset, total.get());
                Ok(())
            },
        );
        match exported {
            Ok(_) => {}
            Err(failure) => {
                h.client.import_abort(h.doc);
                if cancelled() {
                    return Err(SaveError::Cancelled.into());
                }
                return Err(match failure {
                    ExportError::Client(e) => source_failed(e),
                    ExportError::Sink(io) => target_error
                        .borrow_mut()
                        .take()
                        .unwrap_or_else(|| SaveError::Write(io.to_string()).into()),
                });
            }
        }
        // The other file is read; its engine is not needed any more.
        other.kill();
        if cancelled() {
            h.client.import_abort(h.doc);
            return Err(SaveError::Cancelled.into());
        }
        h.ops.phase.store(2, Ordering::SeqCst);
        let changed = match h.client.import_end(h.doc, at) {
            Ok(changed) => changed,
            Err(e) => {
                h.client.import_abort(h.doc);
                return Err(e);
            }
        };
        Ok(InsertOutcome {
            changed,
            inserted: all.len() as u32,
        })
    }

    /// Copies some pages (counted from 0, in the order given) into a new file `target`, by way of
    /// the same steps as a save: a new file next to it, flushed, read back, opened by a second
    /// sandboxed engine that must find the pages the first said it made, and only then put in
    /// place. The document itself is not changed. The new file has no password and carries no
    /// signature, and `options` must say that is all right when the document has either.
    pub fn extract_pages(
        &self,
        handle: u64,
        pages: &[u32],
        target: &Path,
        options: ExtractOptions,
        progress: PageProgress<'_>,
    ) -> Result<SaveOutcome, ClientError> {
        let h = self.handles(handle)?;
        let _saving = h
            .saving
            .try_lock()
            .map_err(|_| ClientError::Save(SaveError::Busy))?;
        h.ops.cancel.store(false, Ordering::SeqCst);
        h.ops.phase.store(1, Ordering::SeqCst);
        let _work = Work {
            ops: &h.ops,
            others: Vec::new(),
        };
        let cancelled = || h.ops.cancel.load(Ordering::SeqCst);

        let (current, original) = {
            let st = state(&self.state);
            let entry = st.docs.get(&handle).ok_or_else(unknown)?;
            let source = entry.source.lock().unwrap_or_else(|e| e.into_inner());
            (source.path.clone(), source.original.clone())
        };
        if same_file(target, &current) || same_file(target, &original) {
            return Err(SaveError::OverSource.into());
        }
        let info = h.client.doc_info(h.doc)?;
        if info.is_signed() && !options.allow_signed {
            return Err(SaveError::Signed {
                signatures: info.signatures.max(1),
            }
            .into());
        }
        if info.security != 0 && !options.allow_unprotected {
            return Err(SaveError::Unprotected.into());
        }

        // What the engine says it made, learned when it begins and used to check the file.
        let header = std::cell::Cell::new(None::<crate::pages::Exported>);
        let total = std::cell::Cell::new(0u64);
        let mut written: u64 = 0;
        let placeholder_info = DocInfo {
            pages: 0,
            security: 0,
            signatures: 0,
            maybe_signed: false,
            touched: false,
        };
        let placeholder_print = Fingerprint {
            pages: 0,
            failed: 0,
            digest: 0,
        };
        let config = self.config.clone();
        let outcome = write_checked(
            target,
            None,
            SaveOptions::default(),
            placeholder_info,
            placeholder_print,
            &mut |sink| {
                h.client
                    .export_to(
                        h.doc,
                        pages,
                        &mut |ex| {
                            total.set(u64::from(ex.size));
                            header.set(Some(*ex));
                            Ok(())
                        },
                        &mut |piece| {
                            if cancelled() {
                                return Err(io::Error::other("stopped"));
                            }
                            sink(piece)?;
                            written += piece.len() as u64;
                            progress(written, total.get());
                            Ok(())
                        },
                    )
                    .map(|ex| u64::from(ex.size))
                    .map_err(|e| match e {
                        ExportError::Client(c) => SaveStreamError::Client(c),
                        ExportError::Sink(io) => SaveStreamError::Sink(io),
                    })
            },
            &|path, _, _| {
                let Some(ex) = header.get() else {
                    return Err(SaveError::Verify(
                        "the engine did not describe the pages".into(),
                    ));
                };
                verify_written(
                    &config,
                    path,
                    None,
                    DocInfo {
                        pages: ex.pages,
                        security: 0,
                        signatures: ex.signatures,
                        maybe_signed: false,
                        touched: false,
                    },
                    Some(&Fingerprint {
                        pages: ex.pages,
                        failed: 0,
                        digest: ex.digest,
                    }),
                )
            },
            &|| state(&self.state).docs.contains_key(&handle),
        );
        match outcome {
            Err(_) if cancelled() => Err(SaveError::Cancelled.into()),
            other => other,
        }
    }

    /// Stops the work a page tool is doing on a document: the transfer of pages ends and the
    /// document is as it was. When the engine itself is in the middle of a change, which cannot be
    /// interrupted, nothing is done and `Working` is the answer: ending the engine would lose
    /// every change of the document that has not been saved.
    pub fn cancel_pages(&self, handle: u64) -> Cancelled {
        let ops = {
            let st = state(&self.state);
            let Some(entry) = st.docs.get(&handle) else {
                return Cancelled::Idle;
            };
            entry.ops.clone()
        };
        match ops.phase.load(Ordering::SeqCst) {
            1 => {
                ops.cancel.store(true, Ordering::SeqCst);
                ops.kill_transients();
                Cancelled::Handover
            }
            2 => Cancelled::Working,
            _ => Cancelled::Idle,
        }
    }
}
