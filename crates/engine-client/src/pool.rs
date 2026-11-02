//! One engine process per open document.
//!
//! A PDF engine that crashes, hangs or is taken over should cost the user one document, not all of
//! them, and an engine that is taken over should not be able to read the other open documents. So
//! every document gets its own [`EngineClient`], which starts its own engine process (about 4 MiB
//! when idle). The pool hands out handles that are never reused, so a handle that outlives its
//! document can never reach another one.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use partridge_protocol::{ErrorCode, PageSize};

use crate::{
    Bitmap, ClientError, EngineClient, EngineConfig, EngineInfo, FontScript, OutlineItem, PageLink,
    PageText, TextMatch,
};

struct Entry {
    client: Arc<EngineClient>,
    /// The client's own handle for the document.
    doc: u64,
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
        }
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
        let client = Arc::new(EngineClient::new(self.config.clone()));
        let opened = match client.open(path, password) {
            Ok(opened) => opened,
            Err(e) => {
                client.kill();
                return Err(e);
            }
        };
        let handle = self.register(client, opened.doc, epoch)?;
        Ok((handle, opened.pages))
    }

    /// Adds an opened document to the pool, unless the pool was emptied or filled up while the
    /// document was being opened. In that case the engine is ended again.
    fn register(
        &self,
        client: Arc<EngineClient>,
        doc: u64,
        epoch: u64,
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
        st.docs.insert(handle, Entry { client, doc });
        Ok(handle)
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

    pub fn page_text(&self, handle: u64, page: u32) -> Result<PageText, ClientError> {
        let (client, doc) = self.entry(handle)?;
        client.page_text(doc, page)
    }

    pub fn page_links(&self, handle: u64, page: u32) -> Result<Vec<PageLink>, ClientError> {
        let (client, doc) = self.entry(handle)?;
        client.page_links(doc, page)
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
            entry.client.kill();
        }
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
        let err = pool.register(client(&pool), 1, before).unwrap_err();
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
    fn a_document_opened_after_a_reset_is_kept_and_handles_go_on_counting() {
        let pool = pool();
        let now = epoch(&pool);
        let first = pool.register(client(&pool), 1, now).unwrap();
        pool.kill_all();
        assert!(pool.is_empty());
        let now = epoch(&pool);
        let second = pool.register(client(&pool), 1, now).unwrap();
        assert!(second > first, "{second} should come after {first}");
        assert_eq!(pool.len(), 1);
    }
}
