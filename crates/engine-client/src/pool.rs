//! One engine process per open document.
//!
//! A PDF engine that crashes, hangs or is taken over should cost the user one document, not all of
//! them, and an engine that is taken over should not be able to read the other open documents. So
//! every document gets its own [`EngineClient`], which starts its own engine process (about 4 MiB
//! when idle). The pool hands out handles that are never reused, so a handle that outlives its
//! document can never reach another one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

mod pages;
pub use pages::{Cancelled, ExtractOptions, InsertOptions, InsertOutcome, PageProgress};

use partridge_protocol::{ErrorCode, PageSize};

use crate::save::{save_document, verify_written, FileStamp};
use crate::{
    AnnotInfo, AnnotKind, Bitmap, ClientError, EngineClient, EngineConfig, EngineInfo, FontScript,
    FormField, FormKind, FormValue, OutlineItem, PageLink, PageText, SaveError, SaveOptions,
    SaveOutcome, StampKind, TextMatch,
};

/// The file a document belongs to, and how it looked when the document was opened (or last saved).
struct Source {
    path: PathBuf,
    /// The file the document was opened from, which the engine keeps reading. Never changes.
    original: PathBuf,
    stamp: Option<FileStamp>,
    /// True once a save has written this document out. The engine goes on reading the file the
    /// document was opened from, so what it can say about the signatures is about that file, not
    /// about the one that was written.
    saved: bool,
}

struct Entry {
    client: Arc<EngineClient>,
    /// The client's own handle for the document.
    doc: u64,
    source: Arc<Mutex<Source>>,
    /// The password the document was opened with, to check a saved file against it.
    password: Option<Arc<Secret>>,
    /// Held while the document is being saved: one save at a time.
    saving: Arc<Mutex<()>>,
    /// What a page tool is doing to the document, and how to stop it.
    ops: Arc<Ops>,
}

/// What a page tool is doing to a document right now.
pub(crate) struct Ops {
    /// Set by the user stopping the work; looked at between the pieces of a document.
    cancel: AtomicBool,
    /// 0 nothing, 1 a document is being handed from one engine to another, 2 the engine of the
    /// document is working on a change.
    phase: AtomicU8,
    /// The engines that read other documents for an insert. They end with the document.
    transients: Mutex<Vec<Arc<EngineClient>>>,
}

impl Ops {
    fn new() -> Ops {
        Ops {
            cancel: AtomicBool::new(false),
            phase: AtomicU8::new(0),
            transients: Mutex::new(Vec::new()),
        }
    }

    fn kill_transients(&self) {
        let all: Vec<Arc<EngineClient>> = self
            .transients
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect();
        for client in all {
            client.kill();
        }
    }
}

#[derive(Default)]
struct State {
    /// The handle the next document gets. Counted from 1, never reused.
    next: u64,
    docs: HashMap<u64, Entry>,
    /// Goes up every time all documents are let go of. A document that was still being opened when
    /// that happened belongs to a window that is gone, and is not added to the pool.
    epoch: u64,
}

pub struct EnginePool {
    config: EngineConfig,
    max_documents: usize,
    state: Mutex<State>,
    /// True once an engine has run without proving that the network is blocked (Windows).
    network_not_proved: AtomicBool,
}

/// A password held in memory: overwritten when it is let go of, never copied (it is shared), and
/// not shown by `Debug`.
struct Secret(String);

impl Drop for Secret {
    fn drop(&mut self) {
        // SAFETY: the bytes are overwritten with zeros in place; the string is dropped right after,
        // so no one reads it as text again. The writes are volatile so that they are not removed.
        unsafe {
            for byte in self.0.as_bytes_mut() {
                std::ptr::write_volatile(byte, 0);
            }
        }
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

fn state(m: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn unknown() -> ClientError {
    ClientError::Engine {
        code: ErrorCode::UnknownDocument,
        message: "the document is not open".into(),
    }
}

impl EnginePool {
    pub fn new(config: EngineConfig, max_documents: usize) -> EnginePool {
        EnginePool {
            config,
            max_documents,
            state: Mutex::new(State::default()),
            network_not_proved: AtomicBool::new(false),
        }
    }

    /// True when an engine of this pool has run, since the app started, without being able to prove
    /// that the network is blocked. The app tells the user once.
    pub fn network_not_proved(&self) -> bool {
        self.network_not_proved.load(Ordering::Relaxed)
    }

    /// Number of documents that are open.
    pub fn len(&self) -> usize {
        state(&self.state).docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Opens a file in a new engine. The handle is valid until the document is closed. When the
    /// file cannot be opened, the engine that was started for it is ended again.
    pub fn open(
        &self,
        path: &str,
        password: Option<&str>,
    ) -> Result<(u64, Vec<PageSize>), ClientError> {
        if self.len() >= self.max_documents {
            return Err(ClientError::Engine {
                code: ErrorCode::InvalidRequest,
                message: format!(
                    "{} documents are open already; close one first",
                    self.max_documents
                ),
            });
        }
        let epoch = state(&self.state).epoch;
        // How the file looks now, to notice later that someone else changed it.
        let stamp = Some(FileStamp::read(Path::new(path)).unwrap_or_else(|_| FileStamp::unknown()));
        let client = Arc::new(EngineClient::new(self.config.clone()));
        let opened = match client.open(path, password) {
            Ok(opened) => opened,
            Err(e) => {
                client.kill();
                return Err(e);
            }
        };
        if client
            .sandbox_report()
            .is_some_and(|report| crate::network_not_proved(&report))
        {
            self.network_not_proved.store(true, Ordering::Relaxed);
        }
        let source = Source {
            path: PathBuf::from(path),
            original: PathBuf::from(path),
            stamp,
            saved: false,
        };
        let handle = self.register(
            client,
            opened.doc,
            epoch,
            source,
            password.map(|p| Arc::new(Secret(p.to_string()))),
        )?;
        Ok((handle, opened.pages))
    }

    /// Adds an opened document to the pool, unless the pool was emptied or filled up while the
    /// document was being opened. In that case the engine is ended again.
    fn register(
        &self,
        client: Arc<EngineClient>,
        doc: u64,
        epoch: u64,
        source: Source,
        password: Option<Arc<Secret>>,
    ) -> Result<u64, ClientError> {
        let mut st = state(&self.state);
        let refusal = if st.epoch != epoch {
            Some("the documents were let go of while this one was opening")
        } else if st.docs.len() >= self.max_documents {
            Some("too many documents are open")
        } else {
            None
        };
        if let Some(message) = refusal {
            drop(st);
            client.kill();
            return Err(ClientError::Engine {
                code: ErrorCode::InvalidRequest,
                message: message.into(),
            });
        }
        st.next += 1;
        let handle = st.next;
        st.docs.insert(
            handle,
            Entry {
                client,
                doc,
                source: Arc::new(Mutex::new(source)),
                password,
                saving: Arc::new(Mutex::new(())),
                ops: Arc::new(Ops::new()),
            },
        );
        Ok(handle)
    }

    /// The file a document belongs to: where it was opened from, or last saved to.
    pub fn source_path(&self, handle: u64) -> Option<PathBuf> {
        let st = state(&self.state);
        let entry = st.docs.get(&handle)?;
        let source = entry.source.lock().unwrap_or_else(|e| e.into_inner());
        Some(source.path.clone())
    }

    /// Saves the document to the file it was opened from, by way of a new file that replaces the
    /// old one in a single step (see the `save` module). The file is as it was if anything fails.
    pub fn save(&self, handle: u64, options: SaveOptions) -> Result<SaveOutcome, ClientError> {
        self.save_to_path(handle, None, options)
    }

    /// Saves the document to `target`, a file that may exist already. When it works, the document
    /// belongs to `target` from then on.
    pub fn save_as(
        &self,
        handle: u64,
        target: &Path,
        options: SaveOptions,
    ) -> Result<SaveOutcome, ClientError> {
        self.save_to_path(handle, Some(target), options)
    }

    fn save_to_path(
        &self,
        handle: u64,
        target: Option<&Path>,
        options: SaveOptions,
    ) -> Result<SaveOutcome, ClientError> {
        let (client, doc, password, source, saving) = {
            let st = state(&self.state);
            let entry = st.docs.get(&handle).ok_or_else(unknown)?;
            (
                entry.client.clone(),
                entry.doc,
                entry.password.clone(),
                entry.source.clone(),
                entry.saving.clone(),
            )
        };
        // One save at a time per document. No lock of the pool is held from here on: a document
        // that is closed during the save ends it with its engine.
        let _saving = saving
            .try_lock()
            .map_err(|_| ClientError::Save(SaveError::Busy))?;
        let (current_path, current_stamp, original) = {
            let source = source.lock().unwrap_or_else(|e| e.into_inner());
            (
                source.path.clone(),
                source.stamp.clone(),
                source.original.clone(),
            )
        };
        let (path, expected) = match target {
            None => (current_path, current_stamp),
            Some(t) if same_file(t, &current_path) => (current_path, current_stamp),
            // Another file: whatever is there now is what the user chose to replace.
            Some(t) => (t.to_path_buf(), FileStamp::read(t).ok()),
        };
        let mut outcome = save_document(
            &client,
            doc,
            &path,
            expected.as_ref(),
            options,
            &|file, info, print| {
                verify_written(
                    &self.config,
                    file,
                    password.as_ref().map(|p| p.0.as_str()),
                    info,
                    Some(print),
                )
            },
            &|| state(&self.state).docs.contains_key(&handle),
            Some(&original),
        )?;
        // The document belongs to the file it was saved to, which now looks different.
        let mut source = source.lock().unwrap_or_else(|e| e.into_inner());
        source.path = path;
        // A stamp that cannot be read is not "no stamp" (that would switch the check for a changed
        // file off): it equals no real state, so the next save asks.
        source.stamp =
            Some(FileStamp::read(&outcome.path).unwrap_or_else(|_| FileStamp::unknown()));
        source.saved = true;
        drop(source);
        outcome.replaced = self.mark_replaced(handle, &outcome.path);
        Ok(outcome)
    }

    /// Every other open document whose engine reads the file at `path` is told apart: what its
    /// engine holds is the file as it was, and the path now leads to something else. Like a document
    /// that was saved itself, it is not asked about its signatures any more. Their handles are
    /// returned so that the window can say so.
    fn mark_replaced(&self, handle: u64, path: &Path) -> Vec<u64> {
        // Comparing two paths asks the file system (it resolves both), and a share that hangs would
        // hold up every call that needs the lock of the pool for as long as that takes. So the lock
        // is held only to take a list of the other documents, and the file system is asked after it
        // is let go. The lock of a document is not held across the question either.
        let others: Vec<(u64, Arc<Mutex<Source>>)> = {
            let st = state(&self.state);
            st.docs
                .iter()
                .filter(|(other, _)| **other != handle)
                .map(|(other, entry)| (*other, entry.source.clone()))
                .collect()
        };
        let mut replaced = Vec::new();
        for (other, source) in others {
            let original = source
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .original
                .clone();
            if same_file(&original, path) {
                source.lock().unwrap_or_else(|e| e.into_inner()).saved = true;
                replaced.push(other);
            }
        }
        replaced.sort_unstable();
        replaced
    }

    fn entry(&self, handle: u64) -> Result<(Arc<EngineClient>, u64), ClientError> {
        state(&self.state)
            .docs
            .get(&handle)
            .map(|e| (e.client.clone(), e.doc))
            .ok_or_else(unknown)
    }

    pub fn render(
        &self,
        handle: u64,
        page: u32,
        width: u32,
        height: u32,
    ) -> Result<Bitmap, ClientError> {
        let (client, doc) = self.entry(handle)?;
        client.render(doc, page, width, height)
    }

    pub fn outline(&self, handle: u64) -> Result<Vec<OutlineItem>, ClientError> {
        let (client, doc) = self.entry(handle)?;
        client.outline(doc)
    }

    /// The scripts the engine of a document had no font for when PDFium asked.
    pub fn font_report(&self, handle: u64) -> Result<Vec<FontScript>, ClientError> {
        let (client, doc) = self.entry(handle)?;
        client.font_report(doc)
    }

    /// The digital signatures of the document, checked at the time `now` (seconds since 1970).
    ///
    /// The answer is about the file the document was opened from, the only one the engine reads.
    /// Once a save has written the document out, that is no longer the file the document stands
    /// for, and a verdict about it would be a verdict about other bytes: the request is refused
    /// (with `SourceChanged`) and the file that was written has to be opened to be checked.
    pub fn signatures(&self, handle: u64, now: i64) -> Result<crate::SignatureReport, ClientError> {
        let (client, doc) = self.entry(handle)?;
        let saved = {
            let st = state(&self.state);
            let entry = st.docs.get(&handle).ok_or_else(unknown)?;
            let source = entry.source.lock().unwrap_or_else(|e| e.into_inner());
            source.saved
        };
        if saved {
            return Err(ClientError::Engine {
                code: ErrorCode::SourceChanged,
                message: "the document was saved after it was opened; the signatures of the file it was opened from say nothing about the file that was written"
                    .to_string(),
            });
        }
        client.signatures(doc, now)
    }

    pub fn page_text(&self, handle: u64, page: u32) -> Result<PageText, ClientError> {
        let (client, doc) = self.entry(handle)?;
        client.page_text(doc, page)
    }

    pub fn page_links(&self, handle: u64, page: u32) -> Result<Vec<PageLink>, ClientError> {
        let (client, doc) = self.entry(handle)?;
        client.page_links(doc, page)
    }

    /// What kind of form a document has, and whether it has actions that are never run.
    pub fn form_info(&self, handle: u64) -> Result<(FormKind, bool), ClientError> {
        let (client, doc) = self.entry(handle)?;
        client.form_info(doc)
    }

    /// The fields of one page that the reader fills in.
    pub fn form_fields(&self, handle: u64, page: u32) -> Result<Vec<FormField>, ClientError> {
        let (client, doc) = self.entry(handle)?;
        client.form_fields(doc, page)
    }

    /// Sets one field; returns the fields of the page as they are then.
    pub fn set_form_field(
        &self,
        handle: u64,
        page: u32,
        index: u32,
        name: &str,
        value: &FormValue,
    ) -> Result<Vec<FormField>, ClientError> {
        let (client, doc) = self.entry(handle)?;
        client.set_form_field(doc, page, index, name, value)
    }

    /// The annotations of a page.
    pub fn list_annots(&self, handle: u64, page: u32) -> Result<Vec<AnnotInfo>, ClientError> {
        let (client, doc) = self.entry(handle)?;
        client.list_annots(doc, page)
    }

    pub fn add_markup(
        &self,
        handle: u64,
        page: u32,
        kind: AnnotKind,
        color: [u8; 4],
        start: u32,
        end: u32,
    ) -> Result<AnnotInfo, ClientError> {
        let (client, doc) = self.entry(handle)?;
        client.add_markup(doc, page, kind, color, start, end)
    }

    pub fn add_note(
        &self,
        handle: u64,
        page: u32,
        x: f32,
        y: f32,
        color: [u8; 4],
        text: &str,
    ) -> Result<AnnotInfo, ClientError> {
        let (client, doc) = self.entry(handle)?;
        client.add_note(doc, page, x, y, color, text)
    }

    pub fn add_ink(
        &self,
        handle: u64,
        page: u32,
        color: [u8; 4],
        width: f32,
        strokes: Vec<Vec<[f32; 2]>>,
    ) -> Result<AnnotInfo, ClientError> {
        let (client, doc) = self.entry(handle)?;
        client.add_ink(doc, page, color, width, strokes)
    }

    pub fn add_stamp(
        &self,
        handle: u64,
        page: u32,
        x: f32,
        y: f32,
        stamp: StampKind,
    ) -> Result<AnnotInfo, ClientError> {
        let (client, doc) = self.entry(handle)?;
        client.add_stamp(doc, page, x, y, stamp)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_annot(
        &self,
        handle: u64,
        page: u32,
        id: u32,
        text: Option<&str>,
        color: Option<[u8; 4]>,
        dx: f32,
        dy: f32,
    ) -> Result<AnnotInfo, ClientError> {
        let (client, doc) = self.entry(handle)?;
        client.update_annot(doc, page, id, text, color, dx, dy)
    }

    pub fn remove_annot(&self, handle: u64, page: u32, id: u32) -> Result<(), ClientError> {
        let (client, doc) = self.entry(handle)?;
        client.remove_annot(doc, page, id)
    }

    pub fn restore_annot(&self, handle: u64, page: u32, id: u32) -> Result<AnnotInfo, ClientError> {
        let (client, doc) = self.entry(handle)?;
        client.restore_annot(doc, page, id)
    }

    pub fn search_page(
        &self,
        handle: u64,
        page: u32,
        query: &str,
    ) -> Result<Vec<TextMatch>, ClientError> {
        let (client, doc) = self.entry(handle)?;
        client.search_page(doc, page, query)
    }

    /// What the engine of a document reports about itself.
    pub fn info(&self, handle: u64) -> Result<EngineInfo, ClientError> {
        let (client, _) = self.entry(handle)?;
        client.info()
    }

    /// Process id of the engine of a document, for the measurement tools.
    pub fn pid(&self, handle: u64) -> Option<u32> {
        self.entry(handle).ok().and_then(|(client, _)| client.pid())
    }

    /// Closes a document and ends its engine. A handle that is not open is ignored. A request that
    /// is still waiting for that engine fails with `EngineStopped`.
    pub fn close(&self, handle: u64) {
        let entry = state(&self.state).docs.remove(&handle);
        if let Some(entry) = entry {
            entry.ops.kill_transients();
            entry.client.kill();
        }
    }

    /// Ends every engine and lets go of every document. Used when the app quits, and when the
    /// window starts again (a reload): the window then knows nothing about the documents it had,
    /// so nobody could close them. A document that is still opening when this is called is ended
    /// as soon as it has opened. The pool can be used again afterwards.
    pub fn kill_all(&self) {
        let all: Vec<Entry> = {
            let mut st = state(&self.state);
            st.epoch += 1;
            st.docs.drain().map(|(_, e)| e).collect()
        };
        for entry in all {
            entry.ops.kill_transients();
            entry.client.kill();
        }
    }
}

/// True when two paths lead to the same file.
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

impl Drop for EnginePool {
    fn drop(&mut self) {
        self.kill_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> EnginePool {
        // No engine is started in these tests: a client starts its engine with its first request.
        EnginePool::new(EngineConfig::new("no-such-engine"), 4)
    }

    fn client(pool: &EnginePool) -> Arc<EngineClient> {
        Arc::new(EngineClient::new(pool.config.clone()))
    }

    fn no_source() -> Source {
        Source {
            path: PathBuf::from("a.pdf"),
            original: PathBuf::from("a.pdf"),
            stamp: None,
            saved: false,
        }
    }

    fn epoch(pool: &EnginePool) -> u64 {
        state(&pool.state).epoch
    }

    #[test]
    fn a_document_that_finishes_opening_after_a_reset_is_not_kept() {
        let pool = pool();
        let before = epoch(&pool);
        // The window starts again while a document is still opening ...
        pool.kill_all();
        // ... so when that document is ready, nobody is left to use or close it.
        let err = pool
            .register(client(&pool), 1, before, no_source(), None)
            .unwrap_err();
        assert!(
            matches!(
                err,
                ClientError::Engine {
                    code: ErrorCode::InvalidRequest,
                    ..
                }
            ),
            "{err:?}"
        );
        assert!(pool.is_empty());
    }

    #[test]
    fn page_changes_are_refused_while_the_document_is_being_saved() {
        let pool = pool();
        let handle = pool
            .register(client(&pool), 1, epoch(&pool), no_source(), None)
            .unwrap();
        // The save holds this lock for as long as it runs.
        let saving = state(&pool.state).docs.get(&handle).unwrap().saving.clone();
        let _held = saving.lock().unwrap();
        let busy = |r: Result<(), ClientError>| {
            assert!(
                matches!(r, Err(ClientError::Save(SaveError::Busy))),
                "{r:?}"
            );
        };
        busy(pool.pages_delete(handle, &[0]).map(|_| ()));
        busy(pool.pages_reorder(handle, &[0]).map(|_| ()));
        busy(pool.pages_rotate(handle, &[0], 1).map(|_| ()));
        busy(pool.pages_crop(handle, &[0], [1.0; 4]).map(|_| ()));
        busy(
            pool.pages_set_boxes(handle, &[0], &[[0.0, 0.0, 9.0, 9.0]])
                .map(|_| ()),
        );
        busy(pool.pages_insert_blank(handle, 0, 100.0, 100.0).map(|_| ()));
        busy(pool.pages_restore(handle, 1).map(|_| ()));
        busy(
            pool.insert_pdf(handle, 0, "x.pdf", None, Default::default(), &|_, _| {})
                .map(|_| ()),
        );
        busy(
            pool.extract_pages(
                handle,
                &[0],
                std::path::Path::new("y.pdf"),
                Default::default(),
                &|_, _| {},
            )
            .map(|_| ()),
        );
        // Nothing started an engine for any of them.
        assert_eq!(
            state(&pool.state)
                .docs
                .get(&handle)
                .unwrap()
                .client
                .spawn_count(),
            0
        );
    }

    #[test]
    fn a_document_opened_after_a_reset_is_kept_and_handles_go_on_counting() {
        let pool = pool();
        let now = epoch(&pool);
        let first = pool
            .register(client(&pool), 1, now, no_source(), None)
            .unwrap();
        pool.kill_all();
        assert!(pool.is_empty());
        let now = epoch(&pool);
        let second = pool
            .register(client(&pool), 1, now, no_source(), None)
            .unwrap();
        assert!(second > first, "{second} should come after {first}");
        assert_eq!(pool.len(), 1);
    }
}
