//! The app side of the engine connection.
//!
//! [`EngineClient`] starts `partridge-engine` as a child process, speaks the protocol with it, and
//! keeps going when the engine does not: if the process crashes, hangs, exits or sends something
//! unreadable, the current request fails with [`ClientError::EngineStopped`] and the process is
//! cleaned up. The documents that engine held are gone with it (their handles are stale, and a
//! request for one fails with `UnknownDocument`); opening a file starts a fresh engine. A bad PDF
//! can therefore never take the app down.
//!
//! Document handles carry the number of the engine process that made them (the generation), so a
//! handle from an engine that has been replaced can never reach a document of the new one. Asking
//! for such a handle fails with `UnknownDocument` without bothering the engine, and closing it does
//! nothing.
//!
//! One client talks to one engine process. The app keeps one client per open document (see
//! [`EnginePool`]), so a document that crashes its engine takes down only itself.
//!
//! The engine is treated as hostile: only one request is in flight, every answer is limited to the
//! size that request can produce, a frame that nobody asked for ends the engine, and at most a
//! couple of frames wait in memory at any time.

use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use partridge_protocol::{
    read_frame_limited, write_request, ErrorCode, PageSize, Request, RequestBody, Response,
    ResponseBody, MAX_QUERY_LEN, MAX_STRING_LEN, PROTOCOL_VERSION,
};

pub use spawn::{spawn_engine, EngineProcess, SandboxPolicy, SpawnRequest, SpawnedEngine};

pub use partridge_protocol::{LinkTarget, OutlineItem, PageLink, TextMatch};

mod pool;
pub use pool::EnginePool;
mod spawn;

/// Largest answer to an open request. An `Opened` answer holds 8 bytes per page, so this allows
/// more than a million pages.
const MAX_SMALL_ANSWER: usize = 9 * 1024 * 1024;
/// The most an engine may send when nothing big was asked for: room for an error message.
const IDLE_LIMIT: usize = 70 * 1024;
/// Header of a `Rendered` answer in front of the pixels.
const RENDERED_HEADER: usize = 17;
/// Largest outline answer: 5 000 entries of at most 512 bytes of title and 9 bytes of framing.
const MAX_OUTLINE_ANSWER: usize = 3 * 1024 * 1024;
/// Largest page text answer: 100 000 characters of at most 4 bytes and 16 bytes of box each.
const MAX_TEXT_ANSWER: usize = 2_200_000;
/// Largest links answer: 500 links with an address of at most 2 048 bytes.
const MAX_LINKS_ANSWER: usize = 1_300_000;

/// How to start and supervise the engine.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Longest the engine may take to answer one request before it is considered hung and killed.
    pub timeout: Duration,
    /// Most memory the engine may use, in bytes. `None` leaves the engine's own limit (3 GiB).
    pub memory_limit: Option<u64>,
    /// Whether an engine without a working sandbox is refused. Release builds always insist.
    pub sandbox: SandboxPolicy,
}

impl EngineConfig {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        EngineConfig {
            program: program.into(),
            args: Vec::new(),
            timeout: Duration::from_secs(60),
            memory_limit: None,
            sandbox: SandboxPolicy::default_for_build(),
        }
    }
}

#[derive(Debug)]
pub enum ClientError {
    /// The engine process could not be started, died, hung, or sent unreadable data. It has been
    /// stopped, and the documents it held are gone; opening a file starts a new one.
    EngineStopped(String),
    /// The engine is fine and refused or failed this one request.
    Engine { code: ErrorCode, message: String },
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::EngineStopped(why) => write!(f, "the PDF engine stopped: {why}"),
            ClientError::Engine { message, .. } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// What the engine reported about itself in the handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineInfo {
    pub engine_version: String,
    pub pdfium_version: String,
    /// The sandbox layers the engine reports, such as `seccomp+landlock`; `none: <reason>` without
    /// a sandbox (see [`sandbox_in_force`]).
    pub sandbox: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenedDocument {
    /// Handle of the document: the engine generation in the high 32 bits, the engine's own
    /// document number in the low 32 bits. Never reused, even after the engine was replaced.
    pub doc: u64,
    pub pages: Vec<PageSize>,
}

/// The text of one page: one entry in `text` and one box in `rects` for every character.
#[derive(Debug, Clone, PartialEq)]
pub struct PageText {
    pub text: String,
    /// `[left, top, right, bottom]` in points from the top left of the page as it is drawn.
    pub rects: Vec<[f32; 4]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

enum Event {
    Response(Response),
    /// The engine broke the rules: unreadable, too big, or not asked for.
    Failed(String),
    /// The engine closed its output: it exited.
    Closed,
}

/// The engine process, reachable from the request thread and from whoever kills it.
type SharedProcess = Arc<Mutex<Box<dyn EngineProcess>>>;

struct Running {
    child: SharedProcess,
    stdin: Box<dyn Write + Send>,
    events: Receiver<Event>,
    info: EngineInfo,
    /// Number of this engine process, counted from 1 over the life of the client.
    generation: u64,
    /// True from sending a request until its answer is taken. An answer outside that is refused.
    in_flight: Arc<AtomicBool>,
    /// Largest frame the reader thread accepts right now.
    limit: Arc<AtomicUsize>,
}

#[derive(Default)]
struct Inner {
    running: Option<Running>,
    next_id: u32,
    spawns: u64,
}

pub struct EngineClient {
    config: EngineConfig,
    inner: Mutex<Inner>,
    /// The running child, reachable without the request lock, so the engine can be killed while a
    /// request is waiting for it (when the app quits, or the user gives up).
    killer: Mutex<Option<SharedProcess>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // A panic while holding a lock leaves nothing half-done that matters: the engine is restarted
    // on the next request anyway.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl EngineClient {
    /// Creates a client. The engine is started by the first request, not here.
    pub fn new(config: EngineConfig) -> Self {
        EngineClient {
            config,
            inner: Mutex::new(Inner::default()),
            killer: Mutex::new(None),
        }
    }

    /// How many engine processes this client has started. A jump tells that the engine was replaced.
    pub fn spawn_count(&self) -> u64 {
        lock(&self.inner).spawns
    }

    /// Process id of the running engine, if there is one. Used by the measurement tools.
    pub fn pid(&self) -> Option<u32> {
        lock(&self.killer).as_ref().map(|c| lock(c).id())
    }

    /// Starts the engine if needed and reports what it is. An engine started for this is not
    /// given a document.
    pub fn info(&self) -> Result<EngineInfo, ClientError> {
        let mut inner = lock(&self.inner);
        self.ensure_running(&mut inner)?;
        Ok(inner.running.as_ref().expect("just started").info.clone())
    }

    /// Opens a file. The app opens it, and a new engine process gets a read-only handle to it
    /// when it starts: the engine never opens files by path, and it opens this one only. An engine
    /// that ran before in this client is ended, so every document has an engine of its own.
    pub fn open(&self, path: &str, password: Option<&str>) -> Result<OpenedDocument, ClientError> {
        // The protocol cuts long strings. A password that was cut would be a different password,
        // so a too long one is refused here, before anything is sent.
        if password.is_some_and(|p| p.len() > MAX_STRING_LEN) {
            return Err(ClientError::Engine {
                code: ErrorCode::InvalidRequest,
                message: "the password is too long".into(),
            });
        }
        let file = open_document(path)?;
        let mut inner = lock(&self.inner);
        self.stop(&mut inner);
        self.start(&mut inner, Some(&file))?;
        // The engine has its own handle now.
        drop(file);
        let generation = inner.running.as_ref().expect("just started").generation;
        let body = RequestBody::Open {
            password: password.map(str::to_string),
        };
        match self.run(&mut inner, body)? {
            ResponseBody::Opened { doc, pages } => {
                // A size the window cannot lay out (NaN, zero, negative, huge) is not a size. The
                // real engine never sends one, so an engine that does is not the real engine.
                if pages
                    .iter()
                    .any(|p| !usable_page_side(p.width) || !usable_page_side(p.height))
                {
                    self.stop(&mut inner);
                    return Err(ClientError::EngineStopped(
                        "it sent a page size that is not a size".into(),
                    ));
                }
                Ok(OpenedDocument {
                    doc: (generation << 32) | u64::from(doc),
                    pages,
                })
            }
            _ => self.unexpected(&mut inner, "open"),
        }
    }

    pub fn render(
        &self,
        doc: u64,
        page: u32,
        width: u32,
        height: u32,
    ) -> Result<Bitmap, ClientError> {
        let mut inner = lock(&self.inner);
        let Some(engine_doc) = Self::engine_doc(&inner, doc) else {
            return Err(stale_document());
        };
        let body = RequestBody::Render {
            doc: engine_doc,
            page,
            width,
            height,
        };
        match self.run(&mut inner, body)? {
            ResponseBody::Rendered {
                width: w,
                height: h,
                rgba,
            } if w == width && h == height => Ok(Bitmap {
                width: w,
                height: h,
                rgba,
            }),
            _ => self.unexpected(&mut inner, "render"),
        }
    }

    /// Sends a request about an open document and picks the wanted kind of answer out of the
    /// reply. A handle from a replaced engine is refused without asking the engine. An answer of
    /// another kind means the two sides disagree, and the engine is stopped.
    fn call<T>(
        &self,
        doc: u64,
        what: &str,
        make: impl FnOnce(u32) -> RequestBody,
        pick: impl FnOnce(ResponseBody) -> Result<T, ResponseBody>,
    ) -> Result<T, ClientError> {
        let mut inner = lock(&self.inner);
        let Some(engine_doc) = Self::engine_doc(&inner, doc) else {
            return Err(stale_document());
        };
        let body = self.run(&mut inner, make(engine_doc))?;
        match pick(body) {
            Ok(value) => Ok(value),
            Err(_) => self.unexpected(&mut inner, what),
        }
    }

    /// The table of contents of a document.
    pub fn outline(&self, doc: u64) -> Result<Vec<OutlineItem>, ClientError> {
        self.call(
            doc,
            "outline",
            |doc| RequestBody::Outline { doc },
            |body| match body {
                ResponseBody::Outline { items } => Ok(items),
                other => Err(other),
            },
        )
    }

    /// The text of one page with the box of every character.
    pub fn page_text(&self, doc: u64, page: u32) -> Result<PageText, ClientError> {
        self.call(
            doc,
            "page text",
            |doc| RequestBody::PageText { doc, page },
            |body| match body {
                ResponseBody::PageText { text, rects } => Ok(PageText { text, rects }),
                other => Err(other),
            },
        )
    }

    /// The links of one page that lead to a web address or to a page of the document.
    pub fn page_links(&self, doc: u64, page: u32) -> Result<Vec<PageLink>, ClientError> {
        self.call(
            doc,
            "page links",
            |doc| RequestBody::PageLinks { doc, page },
            |body| match body {
                ResponseBody::PageLinks { links } => Ok(links),
                other => Err(other),
            },
        )
    }

    /// Finds `query` on one page. A search text that is too long is refused here.
    pub fn search_page(
        &self,
        doc: u64,
        page: u32,
        query: &str,
    ) -> Result<Vec<TextMatch>, ClientError> {
        if query.len() > MAX_QUERY_LEN {
            return Err(ClientError::Engine {
                code: ErrorCode::InvalidRequest,
                message: "the search text is too long".into(),
            });
        }
        self.call(
            doc,
            "search",
            |doc| RequestBody::Search {
                doc,
                page,
                query: query.to_string(),
            },
            |body| match body {
                ResponseBody::Matches { matches } => Ok(matches),
                other => Err(other),
            },
        )
    }

    /// Closes a document. A handle from an engine that is gone is already closed: nothing happens,
    /// and no engine is started for it.
    pub fn close(&self, doc: u64) -> Result<(), ClientError> {
        let mut inner = lock(&self.inner);
        let Some(engine_doc) = Self::engine_doc(&inner, doc) else {
            return Ok(());
        };
        match self.run(&mut inner, RequestBody::Close { doc: engine_doc })? {
            ResponseBody::Closed => Ok(()),
            _ => self.unexpected(&mut inner, "close"),
        }
    }

    /// Stops the engine process now. The documents it held are gone; opening a file starts a new
    /// engine. This works while another
    /// thread is waiting for an answer: that request then fails with `EngineStopped`.
    pub fn kill(&self) {
        // First the quick way that needs no request lock: a hung engine must not keep this out.
        if let Some(child) = lock(&self.killer).take() {
            let _ = lock(&child).kill();
        }
        // When no request is running, tidy up at once (reap the process, drop the pipes).
        if let Ok(mut inner) = self.inner.try_lock() {
            self.stop(&mut inner);
        }
    }

    /// The engine's own number for a handle, or None when the handle is not from the running engine.
    fn engine_doc(inner: &Inner, handle: u64) -> Option<u32> {
        let running = inner.running.as_ref()?;
        (handle >> 32 == running.generation).then_some(handle as u32)
    }

    /// An answer of the wrong kind means the two sides disagree: treat the engine as broken.
    fn unexpected<T>(&self, inner: &mut Inner, what: &str) -> Result<T, ClientError> {
        self.stop(inner);
        Err(ClientError::EngineStopped(format!(
            "it gave an unexpected answer to {what}"
        )))
    }

    /// Sends a request to the running engine and returns its answer. Failures stop the engine.
    fn run(&self, inner: &mut Inner, body: RequestBody) -> Result<ResponseBody, ClientError> {
        match self.exchange(inner, body) {
            Ok(ResponseBody::Error { code, message }) => Err(ClientError::Engine { code, message }),
            Ok(other) => Ok(other),
            Err(why) => {
                self.stop(inner);
                Err(ClientError::EngineStopped(why))
            }
        }
    }

    /// The biggest answer the given request can legitimately get.
    fn answer_limit(body: &RequestBody) -> usize {
        match body {
            RequestBody::Render { width, height, .. } => {
                let pixels = (*width as usize)
                    .saturating_mul(*height as usize)
                    .saturating_mul(4);
                pixels.saturating_add(RENDERED_HEADER).max(IDLE_LIMIT)
            }
            RequestBody::Open { .. } => MAX_SMALL_ANSWER,
            RequestBody::Outline { .. } => MAX_OUTLINE_ANSWER,
            RequestBody::PageText { .. } => MAX_TEXT_ANSWER,
            RequestBody::PageLinks { .. } => MAX_LINKS_ANSWER,
            // A search answer holds at most 2 000 matches of 8 bytes: it fits the idle limit.
            _ => IDLE_LIMIT,
        }
    }

    /// Sends one request and waits for its answer. An `Err` means the connection is unusable.
    fn exchange(&self, inner: &mut Inner, body: RequestBody) -> Result<ResponseBody, String> {
        inner.next_id = inner.next_id.wrapping_add(1);
        let id = inner.next_id;
        let running = inner.running.as_mut().ok_or("it is not running")?;
        // Anything waiting now was not asked for in this request.
        match running.events.try_recv() {
            Err(TryRecvError::Empty) => {}
            Ok(Event::Closed) | Err(TryRecvError::Disconnected) => return Err("it exited".into()),
            Ok(Event::Failed(why)) => return Err(why),
            Ok(Event::Response(_)) => return Err("it sent an answer nobody asked for".into()),
        }
        running
            .limit
            .store(Self::answer_limit(&body), Ordering::SeqCst);
        running.in_flight.store(true, Ordering::SeqCst);
        let result = self.wait_for_answer(running, id, body);
        running.in_flight.store(false, Ordering::SeqCst);
        running.limit.store(IDLE_LIMIT, Ordering::SeqCst);
        result
    }

    fn wait_for_answer(
        &self,
        running: &mut Running,
        id: u32,
        body: RequestBody,
    ) -> Result<ResponseBody, String> {
        write_request(&mut running.stdin, &Request { id, body })
            .map_err(|e| format!("the request could not be sent ({e})"))?;
        // One request is in flight at a time, so the next event is its answer or a failure.
        match running.events.recv_timeout(self.config.timeout) {
            Ok(Event::Response(r)) if r.id == id => Ok(r.body),
            Ok(Event::Response(_)) => Err("it answered a different request".into()),
            Ok(Event::Failed(why)) => Err(format!("its answer was unreadable ({why})")),
            Ok(Event::Closed) | Err(RecvTimeoutError::Disconnected) => {
                Err(exit_text(&running.child))
            }
            Err(RecvTimeoutError::Timeout) => Err(format!(
                "it did not answer within {} seconds",
                self.config.timeout.as_secs_f32().ceil()
            )),
        }
    }

    /// Makes sure an engine runs. An engine that ended by itself while idle is replaced quietly;
    /// one that broke the rules while idle is reported once, so it is not hidden.
    fn ensure_running(&self, inner: &mut Inner) -> Result<(), ClientError> {
        if let Some(running) = inner.running.as_ref() {
            match running.events.try_recv() {
                Err(TryRecvError::Empty) => return Ok(()),
                Ok(Event::Closed) | Err(TryRecvError::Disconnected) => self.stop(inner),
                Ok(Event::Failed(why)) => {
                    self.stop(inner);
                    return Err(ClientError::EngineStopped(why));
                }
                Ok(Event::Response(_)) => {
                    self.stop(inner);
                    return Err(ClientError::EngineStopped(
                        "it sent an answer nobody asked for".into(),
                    ));
                }
            }
        }
        self.start(inner, None)
    }

    fn start(&self, inner: &mut Inner, document: Option<&File>) -> Result<(), ClientError> {
        let spawned = spawn_engine(&SpawnRequest {
            program: &self.config.program,
            args: &self.config.args,
            document,
            memory_limit: self.config.memory_limit,
            policy: self.config.sandbox,
        })
        .map_err(|e| {
            ClientError::EngineStopped(format!(
                "{} could not be started ({e})",
                self.config.program.display()
            ))
        })?;
        inner.spawns += 1;
        let SpawnedEngine {
            process: child,
            stdin,
            stdout,
        } = spawned;
        let in_flight = Arc::new(AtomicBool::new(false));
        let limit = Arc::new(AtomicUsize::new(IDLE_LIMIT));
        // Room for one frame: the reader waits when the app has not taken the last one yet, so a
        // chatty engine cannot make the app's memory grow.
        let (tx, events) = mpsc::sync_channel(1);
        spawn_reader(stdout, tx, in_flight.clone(), limit.clone());
        let child = Arc::new(Mutex::new(child));
        *lock(&self.killer) = Some(child.clone());
        inner.running = Some(Running {
            child,
            stdin,
            events,
            info: EngineInfo {
                engine_version: String::new(),
                pdfium_version: String::new(),
                sandbox: String::new(),
            },
            generation: inner.spawns,
            in_flight,
            limit,
        });

        match self.exchange(
            inner,
            RequestBody::Hello {
                protocol_version: PROTOCOL_VERSION,
            },
        ) {
            Ok(ResponseBody::Hello {
                protocol_version,
                engine_version,
                pdfium_version,
                sandbox,
            }) if protocol_version == PROTOCOL_VERSION => {
                // An engine that is not locked in does not get to read documents, unless this is a
                // development build that said it is fine with that.
                if !sandbox_in_force(&sandbox) && self.config.sandbox == SandboxPolicy::Required {
                    self.stop(inner);
                    return Err(ClientError::Engine {
                        code: ErrorCode::SandboxUnavailable,
                        message: format!(
                            "the PDF engine did not report a sandbox, so it was not used ({})",
                            if sandbox.is_empty() {
                                "no reason given"
                            } else {
                                &sandbox
                            }
                        ),
                    });
                }
                inner.running.as_mut().expect("just started").info = EngineInfo {
                    engine_version,
                    pdfium_version,
                    sandbox,
                };
                Ok(())
            }
            Ok(ResponseBody::Error { code, message }) => {
                self.stop(inner);
                Err(ClientError::Engine { code, message })
            }
            Ok(_) => {
                self.stop(inner);
                Err(ClientError::EngineStopped(
                    "it did not greet the app correctly".into(),
                ))
            }
            Err(why) => {
                self.stop(inner);
                Err(ClientError::EngineStopped(why))
            }
        }
    }

    /// Kills the engine process and waits for it so no zombie is left behind.
    fn stop(&self, inner: &mut Inner) {
        lock(&self.killer).take();
        stop_running(inner);
    }
}

fn stop_running(inner: &mut Inner) {
    if let Some(running) = inner.running.take() {
        let mut child = lock(&running.child);
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Largest page side PDF allows, in points (200 inches). The engine cuts sizes to this.
const MAX_PAGE_SIDE: f32 = 14_400.0;

fn usable_page_side(side: f32) -> bool {
    side.is_finite() && side > 0.0 && side <= MAX_PAGE_SIDE
}

impl Drop for EngineClient {
    fn drop(&mut self) {
        let inner = self.inner.get_mut().unwrap_or_else(|e| e.into_inner());
        stop_running(inner);
    }
}

/// Reads frames from the engine and hands them over one at a time.
fn spawn_reader(
    stdout: Box<dyn Read + Send>,
    tx: SyncSender<Event>,
    in_flight: Arc<AtomicBool>,
    limit: Arc<AtomicUsize>,
) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let frame = read_frame_limited(&mut reader, || limit.load(Ordering::SeqCst));
            let event = match frame {
                Ok(Some(_)) if !in_flight.load(Ordering::SeqCst) => {
                    Event::Failed("it sent data nobody asked for".into())
                }
                Ok(Some(frame)) => match Response::decode(frame) {
                    Ok(response) => Event::Response(response),
                    Err(e) => Event::Failed(e.to_string()),
                },
                Ok(None) => Event::Closed,
                Err(e) => Event::Failed(e.to_string()),
            };
            let last = !matches!(event, Event::Response(_));
            // `send` waits while the last frame has not been taken, and fails when the app has
            // stopped listening.
            if tx.send(event).is_err() || last {
                return;
            }
        }
    });
}

/// True when the engine's greeting names a sandbox. An engine without one says `none` and why.
pub fn sandbox_in_force(report: &str) -> bool {
    !report.is_empty() && !report.starts_with("none")
}

/// Says how the engine ended. The pipe closes a moment before the system reports the exit, so this
/// waits a little for the exit status.
fn exit_text(child: &SharedProcess) -> String {
    for _ in 0..40 {
        if let Ok(Some(status)) = lock(child).try_wait() {
            return format!("it exited ({status})");
        }
        thread::sleep(Duration::from_millis(25));
    }
    "it exited".to_string()
}

/// Opens the file the user chose, for reading. A file system object that is not a plain file (a
/// folder, a pipe, a device) is refused here, before any engine is started.
fn open_document(path: &str) -> Result<File, ClientError> {
    let refuse = |message: String| ClientError::Engine {
        code: ErrorCode::Io,
        message,
    };
    let metadata = std::fs::metadata(path)
        .map_err(|e| refuse(format!("the file could not be opened: {e}")))?;
    if !metadata.is_file() {
        return Err(refuse("the path is not a file".into()));
    }
    let file =
        File::open(path).map_err(|e| refuse(format!("the file could not be opened: {e}")))?;
    // The path may have been swapped for something else between the two calls.
    if !file.metadata().map(|m| m.is_file()).unwrap_or(false) {
        return Err(refuse("the path is not a file".into()));
    }
    Ok(file)
}

fn stale_document() -> ClientError {
    ClientError::Engine {
        code: ErrorCode::UnknownDocument,
        message: "the document is gone: the PDF engine was restarted".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::time::Instant;

    fn is_stopped(err: &ClientError) -> bool {
        matches!(err, ClientError::EngineStopped(_))
    }

    /// A file for the client to open. The stand-in engines in these tests never read it.
    fn some_file() -> partridge_testkit::TempFile {
        partridge_testkit::TempFile::new(
            "a.pdf",
            b"%PDF-1.7
",
        )
    }

    #[test]
    fn a_missing_program_is_reported_not_panicked() {
        let client = EngineClient::new(EngineConfig::new("/definitely/not/an/engine"));
        let file = some_file();
        let err = client.open(&file.path_str(), None).unwrap_err();
        assert!(is_stopped(&err), "{err:?}");
        // And again: the failure leaves no half-started state behind.
        assert!(is_stopped(&client.info().unwrap_err()));
    }

    #[test]
    fn unreadable_output_stops_the_engine() {
        // The test program itself prints text when asked for help: not a valid frame.
        let mut config = EngineConfig::new(std::env::current_exe().unwrap());
        config.args = vec!["--help".into()];
        let client = EngineClient::new(config);
        let file = some_file();
        let err = client.open(&file.path_str(), None).unwrap_err();
        assert!(is_stopped(&err), "{err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn an_engine_that_exits_at_once_is_reported() {
        let client = EngineClient::new(EngineConfig::new("true"));
        let file = some_file();
        let err = client.open(&file.path_str(), None).unwrap_err();
        assert!(is_stopped(&err), "{err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_hung_engine_is_killed_after_the_timeout() {
        // The client adds its own options after these; the shell keeps them as positional words.
        let mut config = EngineConfig::new("sh");
        config.args = vec!["-c".into(), "sleep 30".into()];
        config.timeout = Duration::from_millis(300);
        let client = EngineClient::new(config);
        let file = some_file();
        let started = Instant::now();
        let err = client.open(&file.path_str(), None).unwrap_err();
        assert!(is_stopped(&err), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(err.to_string().contains("did not answer"), "{err}");
    }

    #[test]
    fn a_password_that_does_not_fit_is_refused_before_anything_starts() {
        let client = EngineClient::new(EngineConfig::new("/definitely/not/an/engine"));
        let long = "x".repeat(MAX_STRING_LEN + 1);
        let file = some_file();
        match client.open(&file.path_str(), Some(&long)).unwrap_err() {
            ClientError::Engine { code, .. } => assert_eq!(code, ErrorCode::InvalidRequest),
            other => panic!("{other:?}"),
        }
        assert_eq!(client.spawn_count(), 0);
    }

    #[test]
    fn a_path_that_is_not_a_file_is_refused_before_an_engine_starts() {
        let client = EngineClient::new(EngineConfig::new("/definitely/not/an/engine"));
        let dir = std::env::temp_dir();
        for path in [
            dir.to_string_lossy().into_owned(),
            "/no/such/file.pdf".to_string(),
        ] {
            match client.open(&path, None).unwrap_err() {
                ClientError::Engine { code, .. } => assert_eq!(code, ErrorCode::Io, "{path}"),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(client.spawn_count(), 0);
    }

    #[test]
    fn handles_of_an_engine_that_is_not_running_are_stale() {
        let client = EngineClient::new(EngineConfig::new("/definitely/not/an/engine"));
        let handle = (1u64 << 32) | 1;
        match client.render(handle, 0, 10, 10).unwrap_err() {
            ClientError::Engine { code, .. } => assert_eq!(code, ErrorCode::UnknownDocument),
            other => panic!("{other:?}"),
        }
        // Closing a stale handle is a no-op and does not try to start an engine.
        client.close(handle).unwrap();
        assert_eq!(client.spawn_count(), 0);
    }
}
