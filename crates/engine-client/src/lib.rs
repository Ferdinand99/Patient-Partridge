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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use partridge_protocol::{
    read_frame_limited, write_request, ErrorCode, PageSize, Request, RequestBody, Response,
    ResponseBody, ANNOT_INFO_MIN, MAX_ANNOTS_PER_PAGE, MAX_ANNOT_AUTHOR, MAX_ANNOT_TEXT,
    MAX_COORDINATE, MAX_FORM_ANSWER_BYTES, MAX_FORM_NAME, MAX_FORM_OPTIONS, MAX_FORM_VALUE,
    MAX_INK_POINTS, MAX_INK_STROKES, MAX_INK_WIDTH, MAX_MARKUP_CHARS, MAX_QUERY_LEN,
    MAX_SAVE_BYTES, MAX_SAVE_CHUNK, MAX_SIGNATURE_REPORT, MAX_STRING_LEN, MIN_INK_WIDTH,
    PROTOCOL_VERSION, SAVE_DATA_HEADER_LEN,
};

#[cfg(all(windows, debug_assertions))]
pub use spawn::allow_stand_in_to_use;
pub use spawn::{spawn_engine, EngineProcess, SandboxPolicy, SpawnRequest, SpawnedEngine};

pub use partridge_protocol::{
    ability, field_flag, AnnotInfo, AnnotKind, CertificateTime, FontScript, FormField,
    FormFieldKind, FormKind, FormOption, FormValue, LinkTarget, OutlineItem, PageLink,
    SignatureInfo, SignatureReason, SignatureReport, SignatureState, StampKind, TextMatch,
};

mod pages;
pub use pages::{ExportError, Exported, PagesChanged, PagesCropped};
mod pool;
pub use pool::{Cancelled, EnginePool, ExtractOptions, InsertOptions, InsertOutcome, PageProgress};
mod save;
pub use save::{verify_written, DocInfo, Fingerprint, SaveError, SaveOptions, SaveOutcome};
mod spawn;
pub mod system_fonts;

/// Largest answer to an open request. An `Opened` answer holds 8 bytes per page, so this allows
/// more than a million pages.
const MAX_SMALL_ANSWER: usize = 9 * 1024 * 1024;
/// The most an engine may send when nothing big was asked for: room for an error message.
const IDLE_LIMIT: usize = 70 * 1024;
/// Header of a `Rendered` answer in front of the pixels.
const RENDERED_HEADER: usize = 17;
/// How many times a long request (`DocInfo`, `FormInfo`, `Fingerprint`, `SaveBegin`) may have to
/// read the whole document: the check of the file before and after, and the serialising itself.
const SAVE_PASSES: u64 = 4;
/// How many times the engine may have to read the whole document to check its signatures: the
/// check of the file before and after, the scan for signature marks, and one pass for each of the
/// 64 signatures it describes (the engine hashes at most 2 GiB in all).
const SIGNATURE_PASSES: u64 = 70;

/// What the error of an engine that was stopped for taking too long starts with. The app turns it
/// into its own error code, so that the window can say what happened.
pub const TOOK_TOO_LONG: &str = "it was still working when the overall time limit ended";

/// What the error of an engine that was stopped in the middle of a change to the pages starts
/// with, because it went on for too long or said nothing for too long. The app turns it into its
/// own error code, so that the window says that the change took too long and not that the file
/// may be damaged.
pub const CHANGE_TOOK_TOO_LONG: &str = "a change to the pages took too long";

/// Largest answer to a page tool: 100 000 page sizes of 8 bytes, or 10 000 pairs of boxes.
const PAGES_ANSWER_LIMIT: usize = 2 * 1024 * 1024;

/// Largest outline answer: 5 000 entries of at most 512 bytes of title and 9 bytes of framing.
const MAX_OUTLINE_ANSWER: usize = 3 * 1024 * 1024;
/// Largest page text answer: 100 000 characters of at most 4 bytes and 16 bytes of box each.
const MAX_TEXT_ANSWER: usize = 2_200_000;
/// Largest links answer: 500 links with an address of at most 2 048 bytes.
const MAX_LINKS_ANSWER: usize = 1_300_000;
/// Largest answer about the fields of a page. The engine keeps to `MAX_FORM_ANSWER_BYTES`; the rest is
/// the room for the header.
const MAX_FORMS_ANSWER: usize = MAX_FORM_ANSWER_BYTES + 64;
/// Largest annotation list: 500 annotations with the longest text, author and framing they may have.
const MAX_ANNOTS_ANSWER: usize =
    MAX_ANNOTS_PER_PAGE * (ANNOT_INFO_MIN + MAX_ANNOT_TEXT + MAX_ANNOT_AUTHOR) + 64;

/// How to start and supervise the engine.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Longest the engine may take to answer one request before it is considered hung and killed.
    pub timeout: Duration,
    /// Longest the engine may go without a sign of life while it describes or serialises a
    /// document (`DocInfo`, `FormInfo`, `Fingerprint`, `SaveBegin`). These requests can take minutes for a
    /// large document on a slow disk, so the engine reports progress while it works (about every
    /// MiB written, 32 pages looked at or MiB of the file read), and every report starts this
    /// wait afresh. An engine that makes progress is not taken for a hung one for being slow;
    /// one that says nothing for this long is. Only `save_floor` and `save_slowest` set an end to
    /// an engine that keeps reporting.
    pub save_stall: Duration,
    /// The signs of life renew `save_stall` and so could go on for ever (an engine that is hostile,
    /// or a document with pages that take minutes each). One of these requests therefore also has
    /// an overall limit: `save_floor` plus the time that the document would take to be read
    /// `SAVE_PASSES` times at `save_slowest`. An engine that is still at it when the limit is up
    /// is stopped, and the error says that it took too long.
    pub save_floor: Duration,
    /// The slowest speed, in bytes a second, at which a long request still has to get through the
    /// document (see `save_floor`). A slow drive or a share is allowed to be this slow.
    pub save_slowest: u64,
    /// The overall limit of a change to the pages, longer than `save_floor`: a change to a long
    /// document of pages with a lot on them can take many minutes and still be healthy, and it tells
    /// that it is working (the engine reports progress after each page it has done, and at most four
    /// times a second), so it is only ended by `save_stall` when it says nothing, and by this when
    /// it goes on and on. There is no way to stop such a change short of ending the engine, and the
    /// window holds the document for as long as it lasts, so the limit is 20 minutes and no more.
    /// `save_slowest` adds to it as for a save.
    pub pages_floor: Duration,
    /// Most memory the engine may use, in bytes. `None` leaves the engine's own limit (3 GiB).
    pub memory_limit: Option<u64>,
    /// Whether an engine without a working sandbox is refused. Release builds always insist.
    pub sandbox: SandboxPolicy,
    /// Whether the app hands the engine system fonts for the scripts that PDFium's own fonts do not
    /// cover (see `system_fonts`). On by default; a test can turn it off.
    pub system_fonts: bool,
    /// Font files to hand over instead of the ones the table finds (for tests of damaged and
    /// odd fonts). The app never sets this; nothing a PDF says reaches it.
    pub system_font_files: Option<Vec<(partridge_protocol::FontScript, std::path::PathBuf)>>,
}

impl EngineConfig {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        EngineConfig {
            program: program.into(),
            args: Vec::new(),
            timeout: Duration::from_secs(60),
            save_stall: Duration::from_secs(300),
            save_floor: Duration::from_secs(600),
            save_slowest: 1024 * 1024,
            pages_floor: Duration::from_secs(20 * 60),
            memory_limit: None,
            sandbox: SandboxPolicy::default_for_build(),
            system_fonts: true,
            system_font_files: None,
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
    /// A save did not happen, and the file it was meant for is as it was.
    Save(SaveError),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::EngineStopped(why) => write!(f, "the PDF engine stopped: {why}"),
            ClientError::Engine { message, .. } => write!(f, "{message}"),
            ClientError::Save(why) => write!(f, "{why}"),
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

/// Why [`EngineClient::save_to`] stopped.
#[derive(Debug)]
pub enum SaveStreamError {
    /// The engine failed, broke the rules, or the document is gone.
    Client(ClientError),
    /// The caller's sink failed: the bytes could not be written.
    Sink(std::io::Error),
}

impl From<ClientError> for SaveStreamError {
    fn from(e: ClientError) -> Self {
        SaveStreamError::Client(e)
    }
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
    /// Size of the file the document was opened from, in bytes (0 before there is a document).
    document_len: u64,
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
    /// How many times a start was tried again (see `start`). A start that works the first time does
    /// not count.
    start_retries: AtomicU64,
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
            start_retries: AtomicU64::new(0),
        }
    }

    /// How many starts of this client failed the first time and were tried again. The tests that
    /// start the engine many times require zero: the new attempt hides a start that fails now and
    /// then, which these tests exist to find.
    pub fn start_retries(&self) -> u64 {
        self.start_retries.load(Ordering::Relaxed)
    }

    /// How many engine processes this client has started. A jump tells that the engine was replaced.
    pub fn spawn_count(&self) -> u64 {
        lock(&self.inner).spawns
    }

    /// What the running engine said about its sandbox in the greeting, if there is an engine.
    pub fn sandbox_report(&self) -> Option<String> {
        lock(&self.inner)
            .running
            .as_ref()
            .map(|r| r.info.sandbox.clone())
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

    /// The scripts for which PDFium asked the engine of this document for a font and the engine had
    /// none to give: text in them, in a font the PDF does not carry, is drawn as blanks. Empty when
    /// every font that was asked for was found (or none was asked for).
    pub fn font_report(&self, doc: u64) -> Result<Vec<FontScript>, ClientError> {
        self.call(
            doc,
            "font report",
            |_| RequestBody::FontReport,
            |body| match body {
                ResponseBody::FontReport { missed } => Ok(missed),
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

    /// What kind of form the document has, and whether it has scripts or actions that are never run.
    pub fn form_info(&self, doc: u64) -> Result<(FormKind, bool), ClientError> {
        self.call(
            doc,
            "form info",
            |doc| RequestBody::FormInfo { doc },
            |body| match body {
                ResponseBody::FormInfo { kind, has_actions } => Ok((kind, has_actions)),
                other => Err(other),
            },
        )
    }

    /// The fields of one page that the reader fills in.
    pub fn form_fields(&self, doc: u64, page: u32) -> Result<Vec<FormField>, ClientError> {
        self.call(
            doc,
            "form fields",
            |doc| RequestBody::ListFormFields { doc, page },
            |body| match body {
                ResponseBody::FormFields { fields } => Ok(fields),
                other => Err(other),
            },
        )
    }

    /// Sets one field and returns the fields of the page as they are then. A name or a value over
    /// the caps is refused here, before it is sent.
    pub fn set_form_field(
        &self,
        doc: u64,
        page: u32,
        index: u32,
        name: &str,
        value: &FormValue,
    ) -> Result<Vec<FormField>, ClientError> {
        let too_long = name.len() > MAX_FORM_NAME
            || match value {
                FormValue::Text(text) => text.len() > MAX_FORM_VALUE,
                FormValue::Choice(picked) => picked.len() > MAX_FORM_OPTIONS,
                FormValue::Checked(_) => false,
            };
        if too_long {
            return Err(ClientError::Engine {
                code: ErrorCode::InvalidRequest,
                message: "the name or the value is too long".into(),
            });
        }
        self.call(
            doc,
            "set form field",
            |doc| RequestBody::SetFormField {
                doc,
                page,
                index,
                name: name.to_string(),
                value: value.clone(),
            },
            |body| match body {
                ResponseBody::FormFields { fields } => Ok(fields),
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

    /// What the app needs to know before it saves: pages, protection, signatures.
    pub fn doc_info(&self, doc: u64) -> Result<DocInfo, ClientError> {
        self.call(
            doc,
            "document info",
            |doc| RequestBody::DocInfo { doc },
            |body| match body {
                ResponseBody::DocInfo {
                    pages,
                    security,
                    signatures,
                    maybe_signed,
                    touched,
                } => Ok(DocInfo {
                    pages,
                    security,
                    signatures,
                    maybe_signed,
                    touched,
                }),
                other => Err(other),
            },
        )
    }

    /// A digest of what a reader sees of every page of the document.
    pub fn fingerprint(&self, doc: u64) -> Result<Fingerprint, ClientError> {
        self.call(
            doc,
            "fingerprint",
            |doc| RequestBody::Fingerprint { doc },
            |body| match body {
                ResponseBody::Fingerprint {
                    pages,
                    failed,
                    digest,
                } => Ok(Fingerprint {
                    pages,
                    failed,
                    digest,
                }),
                other => Err(other),
            },
        )
    }

    /// The digital signatures of the file the document was opened from, described and checked by
    /// the engine at the time `now` (seconds since 1970; certificates are judged against it).
    ///
    /// The engine is treated as hostile here too: an answer that cannot be right is not passed on
    /// and stops the engine. In particular no signature may be called trusted when there is no
    /// trust list, none may be called trusted without naming the certificate it leads to, and the
    /// signatures must come in the order of their numbers.
    pub fn signatures(&self, doc: u64, now: i64) -> Result<SignatureReport, ClientError> {
        let now = now.clamp(0, partridge_protocol::MAX_UNIX_TIME);
        self.call(
            doc,
            "signatures",
            |doc| RequestBody::Signatures { doc, now },
            |body| match body {
                ResponseBody::Signatures { report } if report_is_sound(&report) => Ok(report),
                other => Err(other),
            },
        )
    }

    /// The annotations of one page that the reader can show and change.
    pub fn list_annots(&self, doc: u64, page: u32) -> Result<Vec<AnnotInfo>, ClientError> {
        self.call(
            doc,
            "annotations",
            |doc| RequestBody::ListAnnots { doc, page },
            |body| match body {
                ResponseBody::Annots { items } => Ok(items),
                other => Err(other),
            },
        )
    }

    /// Sends a request that adds, changes or puts back an annotation and takes the annotation out
    /// of the answer. `expect` says whether the annotation is the one the request was about: an
    /// engine that answers with another one is not the engine we know.
    fn annot_call(
        &self,
        doc: u64,
        what: &str,
        make: impl FnOnce(u32) -> RequestBody,
        expect: impl FnOnce(&AnnotInfo) -> bool,
    ) -> Result<AnnotInfo, ClientError> {
        self.call(doc, what, make, |body| match body {
            ResponseBody::Annot { item } if expect(&item) => Ok(item),
            other => Err(other),
        })
    }

    /// Marks the characters `start..end` of the page text (numbered as `page_text` numbers them).
    pub fn add_markup(
        &self,
        doc: u64,
        page: u32,
        kind: AnnotKind,
        color: [u8; 4],
        start: u32,
        end: u32,
    ) -> Result<AnnotInfo, ClientError> {
        if !matches!(
            kind,
            AnnotKind::Highlight | AnnotKind::Underline | AnnotKind::Strikeout
        ) || end <= start
            || (end - start) as usize > MAX_MARKUP_CHARS
        {
            return Err(refused_request("that is not a range of text to mark"));
        }
        self.annot_call(
            doc,
            "markup",
            |doc| RequestBody::AddMarkup {
                doc,
                page,
                kind,
                color,
                start,
                end,
            },
            |item| item.kind == kind,
        )
    }

    /// A sticky note at a point of the page, in points from its top left as it is drawn.
    pub fn add_note(
        &self,
        doc: u64,
        page: u32,
        x: f32,
        y: f32,
        color: [u8; 4],
        text: &str,
    ) -> Result<AnnotInfo, ClientError> {
        check_place(x, y)?;
        check_text(text)?;
        self.annot_call(
            doc,
            "note",
            |doc| RequestBody::AddNote {
                doc,
                page,
                x,
                y,
                color,
                text: text.to_string(),
            },
            |item| item.kind == AnnotKind::Note,
        )
    }

    /// A free drawing. The strokes are held to the limits of the protocol here, because the
    /// protocol would leave out what is over them without saying so.
    pub fn add_ink(
        &self,
        doc: u64,
        page: u32,
        color: [u8; 4],
        width: f32,
        strokes: Vec<Vec<[f32; 2]>>,
    ) -> Result<AnnotInfo, ClientError> {
        let points: usize = strokes.iter().map(Vec::len).sum();
        if strokes.is_empty()
            || strokes.len() > MAX_INK_STROKES
            || strokes.iter().any(Vec::is_empty)
            || points > MAX_INK_POINTS
            || !(MIN_INK_WIDTH..=MAX_INK_WIDTH).contains(&width)
        {
            return Err(refused_request("that drawing is outside the limits"));
        }
        for p in strokes.iter().flatten() {
            check_place(p[0], p[1])?;
        }
        self.annot_call(
            doc,
            "drawing",
            |doc| RequestBody::AddInk {
                doc,
                page,
                color,
                width,
                strokes,
            },
            |item| item.kind == AnnotKind::Ink,
        )
    }

    /// A stamp with its top left corner at a point of the page.
    pub fn add_stamp(
        &self,
        doc: u64,
        page: u32,
        x: f32,
        y: f32,
        stamp: StampKind,
    ) -> Result<AnnotInfo, ClientError> {
        check_place(x, y)?;
        self.annot_call(
            doc,
            "stamp",
            |doc| RequestBody::AddStamp {
                doc,
                page,
                x,
                y,
                stamp,
            },
            |item| item.kind == AnnotKind::Stamp,
        )
    }

    /// Changes the text, the colour or the place of an annotation.
    #[allow(clippy::too_many_arguments)]
    pub fn update_annot(
        &self,
        doc: u64,
        page: u32,
        id: u32,
        text: Option<&str>,
        color: Option<[u8; 4]>,
        dx: f32,
        dy: f32,
    ) -> Result<AnnotInfo, ClientError> {
        check_place(dx, dy)?;
        if let Some(text) = text {
            check_text(text)?;
        }
        self.annot_call(
            doc,
            "annotation change",
            |doc| RequestBody::UpdateAnnot {
                doc,
                page,
                id,
                text: text.map(str::to_string),
                color,
                dx,
                dy,
            },
            |item| item.id == id,
        )
    }

    /// Takes an annotation off the page.
    pub fn remove_annot(&self, doc: u64, page: u32, id: u32) -> Result<(), ClientError> {
        self.call(
            doc,
            "annotation removal",
            |doc| RequestBody::RemoveAnnot { doc, page, id },
            |body| match body {
                ResponseBody::AnnotRemoved => Ok(()),
                other => Err(other),
            },
        )
    }

    /// Puts back an annotation that was taken off; it keeps its id.
    pub fn restore_annot(&self, doc: u64, page: u32, id: u32) -> Result<AnnotInfo, ClientError> {
        self.annot_call(
            doc,
            "annotation restore",
            |doc| RequestBody::RestoreAnnot { doc, page, id },
            |item| item.id == id,
        )
    }

    /// Has the engine serialise the document, and hands every piece to `sink` as it arrives; the
    /// pieces together are the file. Returns how many bytes there were.
    ///
    /// The engine is treated as hostile here too: the size it announces is held to
    /// [`MAX_SAVE_BYTES`], every piece must be at the offset asked for and exactly as long as
    /// asked, and the pieces must add up to the size announced. An engine that breaks one of
    /// these is stopped, and the pieces already handed to `sink` are not a file. An error of the
    /// sink (the disk is full) ends the save; the engine is told to drop its buffer.
    pub fn save_to(
        &self,
        doc: u64,
        sink: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
    ) -> Result<u64, SaveStreamError> {
        let mut inner = lock(&self.inner);
        let Some(engine_doc) = Self::engine_doc(&inner, doc) else {
            return Err(stale_document().into());
        };
        let size = match self.run(&mut inner, RequestBody::SaveBegin { doc: engine_doc })? {
            ResponseBody::SaveReady { size } if size as usize <= MAX_SAVE_BYTES => size,
            _ => {
                return Err(self
                    .unexpected::<()>(&mut inner, "save")
                    .unwrap_err()
                    .into())
            }
        };
        let mut done: u32 = 0;
        let result = loop {
            if done == size {
                break Ok(());
            }
            let len = (size - done).min(MAX_SAVE_CHUNK as u32);
            let body = RequestBody::SaveChunk {
                doc: engine_doc,
                offset: done,
                len,
            };
            match self.run(&mut inner, body) {
                Ok(ResponseBody::SaveData { offset, data })
                    if offset == done && data.len() == len as usize =>
                {
                    if let Err(e) = sink(&data) {
                        break Err(SaveStreamError::Sink(e));
                    }
                    done += len;
                }
                Ok(_) => {
                    break Err(self
                        .unexpected::<()>(&mut inner, "save")
                        .unwrap_err()
                        .into());
                }
                Err(e) => break Err(e.into()),
            }
        };
        // Freeing the buffer is housekeeping, and an engine that was stopped has none to free.
        if inner.running.is_some() {
            let _ = self.run(&mut inner, RequestBody::SaveEnd { doc: engine_doc });
        }
        result.map(|()| u64::from(size))
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
            RequestBody::Signatures { .. } => MAX_SIGNATURE_REPORT.max(IDLE_LIMIT),
            RequestBody::ListFormFields { .. } | RequestBody::SetFormField { .. } => {
                MAX_FORMS_ANSWER
            }
            RequestBody::ListAnnots { .. } => MAX_ANNOTS_ANSWER,
            RequestBody::SaveChunk { len, .. } => {
                (SAVE_DATA_HEADER_LEN + (*len as usize).min(MAX_SAVE_CHUNK)).max(IDLE_LIMIT)
            }
            // The size of every page, for up to MAX_PAGES pages, and a list of boxes.
            RequestBody::Pages(_) => PAGES_ANSWER_LIMIT,
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
            // With the exit code: a start that ends before the greeting is told from the code.
            Ok(Event::Closed) | Err(TryRecvError::Disconnected) => {
                return Err(exit_text(&running.child))
            }
            Ok(Event::Failed(why)) => return Err(why),
            Ok(Event::Response(_)) => return Err("it sent an answer nobody asked for".into()),
        }
        running
            .limit
            .store(Self::answer_limit(&body), Ordering::SeqCst);
        running.in_flight.store(true, Ordering::SeqCst);
        // A request that can take long gets the longer wait, renewed by every sign of life.
        let patience = match body {
            RequestBody::DocInfo { .. }
            | RequestBody::FormInfo { .. }
            | RequestBody::Fingerprint { .. }
            | RequestBody::Signatures { .. }
            | RequestBody::SaveBegin { .. }
            | RequestBody::Pages(_) => self.config.save_stall,
            _ => self.config.timeout,
        };
        let result = self.wait_for_answer(running, id, body, patience);
        running.in_flight.store(false, Ordering::SeqCst);
        running.limit.store(IDLE_LIMIT, Ordering::SeqCst);
        result
    }

    fn wait_for_answer(
        &self,
        running: &mut Running,
        id: u32,
        body: RequestBody,
        patience: Duration,
    ) -> Result<ResponseBody, String> {
        let passes = if matches!(body, RequestBody::Signatures { .. }) {
            SIGNATURE_PASSES
        } else {
            SAVE_PASSES
        };
        let is_change = matches!(body, RequestBody::Pages(_));
        write_request(&mut running.stdin, &Request { id, body }).map_err(|e| {
            format!(
                "the request could not be sent ({e}); {}",
                exit_text(&running.child)
            )
        })?;
        // One request is in flight at a time, so the next event is its answer, a sign of life
        // (only from a request that can take long, and each one has to show more work done than
        // the one before) or a failure.
        let may_report = patience != self.config.timeout;
        let mut last_done = 0u32;
        // The signs of life renew the patience, but not the overall time.
        let started = Instant::now();
        let overall = if may_report {
            self.overall_limit(running.document_len, passes, is_change)
        } else {
            Duration::MAX
        };
        let too_long = |limit: Duration| {
            if is_change {
                change_too_long(limit)
            } else {
                too_long(limit)
            }
        };
        loop {
            let left = overall.saturating_sub(started.elapsed());
            if left.is_zero() {
                return Err(too_long(overall));
            }
            match running.events.recv_timeout(patience.min(left)) {
                Ok(Event::Response(Response {
                    id: reported,
                    body: ResponseBody::Progress { done },
                })) if reported == id && may_report && done > last_done => {
                    last_done = done;
                }
                Ok(Event::Response(r)) if r.id == id => {
                    if matches!(r.body, ResponseBody::Progress { .. }) {
                        return Err("it reported progress that was not progress".into());
                    }
                    return Ok(r.body);
                }
                Ok(Event::Response(_)) => return Err("it answered a different request".into()),
                Ok(Event::Failed(why)) => return Err(format!("its answer was unreadable ({why})")),
                Ok(Event::Closed) | Err(RecvTimeoutError::Disconnected) => {
                    return Err(exit_text(&running.child))
                }
                Err(RecvTimeoutError::Timeout) => {
                    if started.elapsed() >= overall {
                        return Err(too_long(overall));
                    }
                    let seconds = patience.as_secs_f32().ceil();
                    return Err(if is_change {
                        format!(
                            "{CHANGE_TOOK_TOO_LONG}: it did not answer within {seconds} seconds"
                        )
                    } else {
                        format!("it did not answer within {seconds} seconds")
                    });
                }
            }
        }
    }

    /// The longest a long request may take altogether, for a document of `len` bytes.
    fn overall_limit(&self, len: u64, passes: u64, is_change: bool) -> Duration {
        let slowest = self.config.save_slowest.max(1);
        let reading = Duration::from_secs(len.saturating_mul(passes) / slowest);
        let floor = if is_change {
            self.config.pages_floor
        } else {
            self.config.save_floor
        };
        floor.saturating_add(reading)
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

    /// Starts an engine. On Windows a start that ends before the greeting because the system's
    /// loader could not find a library (exit code `0xC0000135`) is tried again, up to three times
    /// in all: the loader hands library loading to helper threads that have no rights in the
    /// engine's locked-down token, and although the engine links the C runtime statically so that
    /// nothing but the system's own libraries is left to load, a system that keeps one of them
    /// outside its list of known libraries could still lose that race now and then. A start that
    /// fails for another reason, or every time, is reported as it was. Nothing is relaxed by trying
    /// again: every attempt starts the engine with all its restrictions.
    fn start(&self, inner: &mut Inner, document: Option<&File>) -> Result<(), ClientError> {
        let mut attempt = 1;
        loop {
            match self.start_once(inner, document) {
                Err(ClientError::EngineStopped(why))
                    if attempt < 3 && cfg!(windows) && why.contains("exit code: 3221225781") =>
                {
                    attempt += 1;
                    self.start_retries.fetch_add(1, Ordering::Relaxed);
                }
                other => return other,
            }
        }
    }

    fn start_once(&self, inner: &mut Inner, document: Option<&File>) -> Result<(), ClientError> {
        // The fonts are opened for this engine only and closed again when it has them.
        let fonts = if document.is_none() {
            // An engine that only says what it is has no document and needs no fonts.
            Vec::new()
        } else if let Some(files) = &self.config.system_font_files {
            files
                .iter()
                .filter_map(|(script, path)| Some((*script, File::open(path).ok()?)))
                .collect()
        } else if self.config.system_fonts {
            system_fonts::open_all()
        } else {
            Vec::new()
        };
        let spawned = spawn_engine(&SpawnRequest {
            program: &self.config.program,
            args: &self.config.args,
            document,
            memory_limit: self.config.memory_limit,
            policy: self.config.sandbox,
            fonts: &fonts,
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
            document_len: document
                .and_then(|file| file.metadata().ok())
                .map_or(0, |meta| meta.len()),
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

fn refused_request(message: &str) -> ClientError {
    ClientError::Engine {
        code: ErrorCode::InvalidRequest,
        message: message.into(),
    }
}

/// A place that is a number and not absurd. The protocol would refuse the request; this says so
/// before the engine is asked.
fn check_place(x: f32, y: f32) -> Result<(), ClientError> {
    if x.is_finite() && y.is_finite() && x.abs() <= MAX_COORDINATE && y.abs() <= MAX_COORDINATE {
        Ok(())
    } else {
        Err(refused_request("that place is not on any page"))
    }
}

/// Text for a note or a comment. The protocol would cut it, which would change what the person wrote.
fn check_text(text: &str) -> Result<(), ClientError> {
    if text.len() > MAX_ANNOT_TEXT {
        Err(refused_request("the text is too long"))
    } else {
        Ok(())
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

/// True when the greeting says that the engine runs, but that it could not prove that the network
/// is blocked (Windows: `network-not-proved`, followed by what the system answered). Elsewhere the
/// block is the system call filter or the profile, which the engine does not probe, so this is
/// never true there.
pub fn network_not_proved(report: &str) -> bool {
    report.contains("network-not-proved")
}

/// True when the engine's greeting names a sandbox. An engine without one says `none` and why.
pub fn sandbox_in_force(report: &str) -> bool {
    !report.is_empty() && !report.starts_with("none")
}

/// Says how the engine ended. The pipe closes a moment before the system reports the exit, so this
/// waits a little for the exit status.
/// The error of a long request that was still going when its overall limit ended.
fn change_too_long(limit: Duration) -> String {
    format!(
        "{CHANGE_TOOK_TOO_LONG}: it was still working after {} minutes",
        limit.as_secs().div_ceil(60).max(1)
    )
}

fn too_long(limit: Duration) -> String {
    format!(
        "{TOOK_TOO_LONG} ({} minutes)",
        limit.as_secs().div_ceil(60).max(1)
    )
}

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

/// Whether a report of signatures can be what an honest engine sends. The decoder has already
/// checked the sizes, the text and that every reason goes with its state.
fn report_is_sound(report: &SignatureReport) -> bool {
    use partridge_protocol::MAX_SIGNATURES;
    if report.items.len() != (report.total as usize).min(MAX_SIGNATURES) {
        return false;
    }
    report.items.iter().enumerate().all(|(place, item)| {
        let trusted = item.state == SignatureState::Trusted;
        item.index as usize == place
            // A signature is trusted only through a certificate on a list that exists, and that
            // certificate is named, and the signer's own certificate is valid.
            && trusted == item.trust_anchor.is_some()
            && (!trusted || (report.trust_anchors > 0 && item.certificate_time == CertificateTime::Valid))
            // "No trust list" is a statement about an empty list, and only about it.
            && ((item.reason == SignatureReason::NoTrustList) == (report.trust_anchors == 0 && item.state == SignatureState::UnknownSigner))
    })
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
