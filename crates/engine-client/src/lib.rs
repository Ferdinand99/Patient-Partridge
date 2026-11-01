//! The app side of the engine connection.
//!
//! [`EngineClient`] starts `partridge-engine` as a child process, speaks the protocol with it, and
//! keeps going when the engine does not: if the process crashes, hangs, exits or sends something
//! unreadable, the current request fails with [`ClientError::EngineStopped`], the process is
//! cleaned up, and the next request starts a fresh engine. A bad PDF can therefore never take the
//! app down.
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
use std::io::BufReader;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use partridge_protocol::{
    read_frame_limited, write_request, ErrorCode, PageSize, Request, RequestBody, Response,
    ResponseBody, MAX_QUERY_LEN, MAX_STRING_LEN, PROTOCOL_VERSION,
};

pub use partridge_protocol::{LinkTarget, OutlineItem, PageLink, TextMatch};

mod pool;
pub use pool::EnginePool;

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
}

impl EngineConfig {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        EngineConfig {
            program: program.into(),
            args: Vec::new(),
            timeout: Duration::from_secs(60),
        }
    }
}

#[derive(Debug)]
pub enum ClientError {
    /// The engine process could not be started, died, hung, or sent unreadable data. It has been
    /// stopped; the next request starts a new one.
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

struct Running {
    child: Arc<Mutex<Child>>,
    stdin: ChildStdin,
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
    killer: Mutex<Option<Arc<Mutex<Child>>>>,
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

    /// Starts the engine if needed and reports what it is.
    pub fn info(&self) -> Result<EngineInfo, ClientError> {
        let mut inner = lock(&self.inner);
        self.ensure_running(&mut inner)?;
        Ok(inner.running.as_ref().expect("just started").info.clone())
    }

    pub fn open(&self, path: &str, password: Option<&str>) -> Result<OpenedDocument, ClientError> {
        // The protocol cuts long strings. A path or password that was cut would be a different
        // path or password, so too long ones are refused here, before anything is sent.
        if path.len() > MAX_STRING_LEN || password.is_some_and(|p| p.len() > MAX_STRING_LEN) {
            return Err(ClientError::Engine {
                code: ErrorCode::InvalidRequest,
                message: "the path or the password is too long".into(),
            });
        }
        let mut inner = lock(&self.inner);
        self.ensure_running(&mut inner)?;
        let generation = inner.running.as_ref().expect("just started").generation;
        let body = RequestBody::Open {
            path: path.to_string(),
            password: password.map(str::to_string),
        };
        match self.run(&mut inner, body)? {
            ResponseBody::Opened { doc, pages } => Ok(OpenedDocument {
                doc: (generation << 32) | u64::from(doc),
                pages,
            }),
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

    /// Stops the engine process now. The next request starts a new one. This works while another
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
                Err(match lock(&running.child).try_wait() {
                    Ok(Some(status)) => format!("it exited ({status})"),
                    _ => "it exited".to_string(),
                })
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
        self.start(inner)
    }

    fn start(&self, inner: &mut Inner) -> Result<(), ClientError> {
        let mut command = Command::new(&self.config.program);
        command
            .args(&self.config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console window flashes up next to the app.
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|e| {
            ClientError::EngineStopped(format!(
                "{} could not be started ({e})",
                self.config.program.display()
            ))
        })?;
        #[cfg(windows)]
        end_with_app(&child);
        inner.spawns += 1;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
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
            }) if protocol_version == PROTOCOL_VERSION => {
                inner.running.as_mut().expect("just started").info = EngineInfo {
                    engine_version,
                    pdfium_version,
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

impl Drop for EngineClient {
    fn drop(&mut self) {
        let inner = self.inner.get_mut().unwrap_or_else(|e| e.into_inner());
        stop_running(inner);
    }
}

/// Reads frames from the engine and hands them over one at a time.
fn spawn_reader(
    stdout: std::process::ChildStdout,
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

fn stale_document() -> ClientError {
    ClientError::Engine {
        code: ErrorCode::UnknownDocument,
        message: "the document is gone: the PDF engine was restarted".into(),
    }
}

/// Puts the engine in a job object that ends all its processes when the app's last handle to the
/// job closes, which happens when the app exits or crashes. Without it a Windows engine that is
/// stuck inside PDFium would outlive the app.
#[cfg(windows)]
fn end_with_app(child: &Child) {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;

    extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            class: i32,
            info: *const c_void,
            length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
    }
    const EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;

    // One job for the whole app, kept open until the app ends.
    static JOB: OnceLock<usize> = OnceLock::new();
    let job = *JOB.get_or_init(|| {
        // JOBOBJECT_EXTENDED_LIMIT_INFORMATION is 144 bytes on 64-bit Windows; LimitFlags is the
        // u32 at offset 16.
        let mut info = [0u8; 144];
        info[16..20].copy_from_slice(&KILL_ON_JOB_CLOSE.to_le_bytes());
        // SAFETY: documented Win32 calls with a live buffer of the size the class expects.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null()
                || SetInformationJobObject(
                    job,
                    EXTENDED_LIMIT_INFORMATION,
                    info.as_ptr().cast(),
                    info.len() as u32,
                ) == 0
            {
                return 0;
            }
            job as usize
        }
    });
    if job != 0 {
        // SAFETY: both handles are valid; the child handle belongs to `child`, which is alive.
        unsafe {
            AssignProcessToJobObject(job as *mut c_void, child.as_raw_handle());
        }
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

    #[test]
    fn a_missing_program_is_reported_not_panicked() {
        let client = EngineClient::new(EngineConfig::new("/definitely/not/an/engine"));
        let err = client.open("a.pdf", None).unwrap_err();
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
        let err = client.open("a.pdf", None).unwrap_err();
        assert!(is_stopped(&err), "{err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn an_engine_that_exits_at_once_is_reported() {
        let client = EngineClient::new(EngineConfig::new("true"));
        let err = client.open("a.pdf", None).unwrap_err();
        assert!(is_stopped(&err), "{err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_hung_engine_is_killed_after_the_timeout() {
        let mut config = EngineConfig::new("sleep");
        config.args = vec!["30".into()];
        config.timeout = Duration::from_millis(300);
        let client = EngineClient::new(config);
        let started = Instant::now();
        let err = client.open("a.pdf", None).unwrap_err();
        assert!(is_stopped(&err), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(err.to_string().contains("did not answer"), "{err}");
    }

    #[test]
    fn a_path_or_password_that_does_not_fit_is_refused_before_anything_starts() {
        let client = EngineClient::new(EngineConfig::new("/definitely/not/an/engine"));
        let long = "x".repeat(MAX_STRING_LEN + 1);
        for (path, password) in [(long.as_str(), None), ("a.pdf", Some(long.as_str()))] {
            match client.open(path, password).unwrap_err() {
                ClientError::Engine { code, .. } => assert_eq!(code, ErrorCode::InvalidRequest),
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
