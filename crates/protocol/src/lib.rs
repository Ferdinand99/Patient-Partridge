//! The wire protocol between the Patient Partridge app and the `partridge-engine` process.
//!
//! The app (UI process) never parses PDF files. It starts the engine as a child process and
//! sends it requests on the engine's standard input; the engine answers on its standard output.
//!
//! Framing: every message is one frame, a `u32` little-endian payload length followed by the
//! payload. A payload starts with a one byte message tag and a `u32` request id, followed by
//! the fields of the message. Integers are little-endian, floats are IEEE 754 `f32`, strings
//! are a `u32` byte length followed by UTF-8 bytes, and an optional string is a presence byte
//! (0 or 1) followed by the string when present.
//!
//! The decoder treats everything it reads as untrusted: lengths are checked against the bytes
//! that are actually there before anything is allocated, and malformed input returns an error
//! instead of panicking. This protects both sides from a damaged or misbehaving peer.

use std::fmt;
use std::io::{self, Read, Write};

mod annots;
pub use annots::*;
#[cfg(test)]
mod annots_tests;
mod forms;
pub use forms::*;
mod pages;
pub use pages::*;
mod signatures;
pub use signatures::*;
#[cfg(test)]
mod signatures_tests;

/// Version of this protocol. Both sides check it in the `Hello` exchange.
pub const PROTOCOL_VERSION: u32 = 9;

/// Largest frame (payload) either side accepts: 128 MiB, enough for the biggest allowed bitmap.
pub const MAX_FRAME_LEN: usize = 128 * 1024 * 1024;

/// Longest string either side accepts, in bytes.
pub const MAX_STRING_LEN: usize = 64 * 1024;

/// Largest bitmap the engine renders, in pixels (width times height).
pub const MAX_RENDER_PIXELS: u64 = 24_000_000;

/// Largest bitmap side the engine renders, in pixels.
pub const MAX_RENDER_SIDE: u32 = 16_384;

/// Most outline entries the engine reports for one document.
pub const MAX_OUTLINE_ITEMS: usize = 5_000;

/// Deepest outline level reported (0 is the top level). Deeper entries are left out.
pub const MAX_OUTLINE_DEPTH: u8 = 15;

/// Longest outline title, in bytes. Longer titles are cut on a character boundary.
pub const MAX_OUTLINE_TITLE: usize = 512;

/// Most characters of text the engine reports for one page.
pub const MAX_PAGE_CHARS: usize = 100_000;

/// Most bytes of page text on the wire: every character is at most 4 bytes in UTF-8.
pub const MAX_PAGE_TEXT_BYTES: usize = MAX_PAGE_CHARS * 4;

/// Most links the engine reports for one page.
pub const MAX_LINKS_PER_PAGE: usize = 500;

/// Longest link address, in bytes. Links with a longer address are left out.
pub const MAX_URI_LEN: usize = 2_048;

/// Longest search text, in bytes.
pub const MAX_QUERY_LEN: usize = 256;

/// Most matches the engine reports for one page.
pub const MAX_MATCHES_PER_PAGE: usize = 2_000;

/// Largest document the engine serialises for a save, in bytes. The engine builds the whole file in
/// memory (it may not write files itself), so this bounds that buffer. A bigger document is not
/// saved at all: the engine answers `SaveTooLarge`.
pub const MAX_SAVE_BYTES: usize = 512 * 1024 * 1024;

/// Most bytes of a saved document in one `SaveData` answer. The app asks for the document piece
/// by piece, so a frame stays far below [`MAX_FRAME_LEN`] and the app never holds more than one
/// piece besides the file it writes.
pub const MAX_SAVE_CHUNK: usize = 1024 * 1024;

/// Stands for "no page" in an outline entry.
const NO_PAGE: u32 = u32::MAX;

const TAG_REQ_HELLO: u8 = 1;
const TAG_REQ_OPEN: u8 = 2;
const TAG_REQ_RENDER: u8 = 3;
const TAG_REQ_CLOSE: u8 = 4;
const TAG_REQ_OUTLINE: u8 = 5;
const TAG_REQ_PAGE_TEXT: u8 = 6;
const TAG_REQ_PAGE_LINKS: u8 = 7;
const TAG_REQ_SEARCH: u8 = 8;
const TAG_REQ_FONT_REPORT: u8 = 9;
const TAG_REQ_DOC_INFO: u8 = 10;
const TAG_REQ_SAVE_BEGIN: u8 = 11;
const TAG_REQ_SAVE_CHUNK: u8 = 12;
const TAG_REQ_SAVE_END: u8 = 13;
const TAG_REQ_FINGERPRINT: u8 = 14;
const TAG_REQ_SIGNATURES: u8 = 60;
const TAG_REQ_FORM_INFO: u8 = 40;
const TAG_REQ_LIST_FORM_FIELDS: u8 = 41;
const TAG_REQ_SET_FORM_FIELD: u8 = 42;
const TAG_REQ_LIST_ANNOTS: u8 = 15;
const TAG_REQ_ADD_MARKUP: u8 = 16;
const TAG_REQ_ADD_NOTE: u8 = 17;
const TAG_REQ_ADD_INK: u8 = 18;
const TAG_REQ_ADD_STAMP: u8 = 19;
const TAG_REQ_UPDATE_ANNOT: u8 = 20;
const TAG_REQ_REMOVE_ANNOT: u8 = 21;
const TAG_REQ_RESTORE_ANNOT: u8 = 22;

const TAG_RES_HELLO: u8 = 129;
const TAG_RES_OPENED: u8 = 130;
const TAG_RES_RENDERED: u8 = 131;
const TAG_RES_CLOSED: u8 = 132;
const TAG_RES_OUTLINE: u8 = 133;
const TAG_RES_PAGE_TEXT: u8 = 134;
const TAG_RES_PAGE_LINKS: u8 = 135;
const TAG_RES_MATCHES: u8 = 136;
const TAG_RES_FONT_REPORT: u8 = 137;
const TAG_RES_DOC_INFO: u8 = 138;
const TAG_RES_SAVE_READY: u8 = 139;
const TAG_RES_SAVE_DATA: u8 = 140;
const TAG_RES_SAVE_ENDED: u8 = 141;
const TAG_RES_PROGRESS: u8 = 142;
const TAG_RES_FINGERPRINT: u8 = 143;
const TAG_RES_SIGNATURES: u8 = 190;
const TAG_RES_FORM_INFO: u8 = 170;
const TAG_RES_FORM_FIELDS: u8 = 171;
const TAG_RES_ANNOTS: u8 = 144;
const TAG_RES_ANNOT: u8 = 145;
const TAG_RES_ANNOT_REMOVED: u8 = 146;
const TAG_RES_ERROR: u8 = 255;

/// What went wrong while reading or decoding a frame.
#[derive(Debug)]
pub enum ProtocolError {
    Io(io::Error),
    /// The peer announced a frame larger than [`MAX_FRAME_LEN`].
    FrameTooLarge(u64),
    /// The data ended before the message was complete.
    Truncated,
    /// The first byte of the payload is not a known message tag.
    UnknownTag(u8),
    /// An enum value (such as an error code) is not known.
    UnknownValue(u8),
    InvalidUtf8,
    /// A string is longer than [`MAX_STRING_LEN`].
    StringTooLong(usize),
    /// A message has bytes left over after its last field.
    TrailingBytes(usize),
    /// A length field does not match the data it describes.
    InvalidLength,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolError::Io(e) => write!(f, "I/O error: {e}"),
            ProtocolError::FrameTooLarge(n) => write!(f, "frame of {n} bytes is too large"),
            ProtocolError::Truncated => write!(f, "message is truncated"),
            ProtocolError::UnknownTag(t) => write!(f, "unknown message tag {t}"),
            ProtocolError::UnknownValue(v) => write!(f, "unknown value {v}"),
            ProtocolError::InvalidUtf8 => write!(f, "string is not valid UTF-8"),
            ProtocolError::StringTooLong(n) => write!(f, "string of {n} bytes is too long"),
            ProtocolError::TrailingBytes(n) => write!(f, "{n} unexpected bytes after the message"),
            ProtocolError::InvalidLength => write!(f, "a length field does not match the data"),
        }
    }
}

impl std::error::Error for ProtocolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProtocolError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for ProtocolError {
    fn from(e: io::Error) -> Self {
        ProtocolError::Io(e)
    }
}

/// Reasons the engine gives when it cannot do what a request asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ErrorCode {
    /// The file could not be read (missing, no permission, is a folder).
    Io = 1,
    /// The file is not a PDF or is too damaged to open.
    Corrupt = 2,
    /// The file is encrypted and no password was given.
    PasswordRequired = 3,
    /// The file is encrypted and the given password is wrong.
    PasswordIncorrect = 4,
    /// The file uses a security feature the engine does not support.
    Unsupported = 5,
    /// No open document has this id (never opened, closed, or the engine restarted).
    UnknownDocument = 6,
    /// The page index is past the last page.
    PageOutOfRange = 7,
    /// The request is malformed or asks for something outside the allowed limits.
    InvalidRequest = 8,
    /// PDFium could not draw the page.
    RenderFailed = 9,
    /// The PDFium library could not be loaded, so the engine cannot work at all.
    EngineUnavailable = 10,
    /// The two sides speak different protocol versions.
    ProtocolMismatch = 11,
    /// Anything else.
    Internal = 12,
    /// The engine could not lock itself into the operating system sandbox, so it refuses to read
    /// any document.
    SandboxUnavailable = 13,
    /// PDFium could not write the document out.
    SaveFailed = 14,
    /// The saved document would be bigger than [`MAX_SAVE_BYTES`].
    SaveTooLarge = 15,
    /// The file the document is read from changed while the document was open, so what PDFium
    /// read at different times may come from different versions of it. Nothing is serialised.
    SourceChanged = 16,
    /// Reading the file the document comes from failed at least once, so the document may be
    /// missing parts. Nothing is serialised.
    SourceUnreadable = 17,
    /// The document has more pages than the page tools (or the reader) take.
    TooManyPages = 50,
    /// The copy of the document that a page change was to go back to is gone: the engine keeps
    /// only a few, and the oldest are let go of.
    SnapshotGone = 51,
    /// The document to insert is not a usable PDF.
    ImportFailed = 52,
    /// PDFium refused a page change.
    PageChangeFailed = 53,
    /// A save, an insert or a copy of the document is under way.
    Busy = 54,
    /// The file to insert pages from could not be used: its engine stopped, took too long or could not
    /// start, or the file does not allow its pages to be copied. The document itself is not affected.
    InsertSourceFailed = 55,
    /// The permissions of the document do not allow the change (inserting, deleting, moving or
    /// turning pages, changing a page's boxes, or copying pages out).
    PagesNotAllowed = 56,
    /// The document has no form the reader can fill in (it has none, or it is an XFA form).
    FormUnsupported = 24,
    /// The field is not there any more, is not the one named, is read only, or cannot be set to
    /// what was asked.
    FormFieldRefused = 25,
    /// The annotation does not exist (any more), or the page changed under the window.
    NoSuchAnnotation = 18,
    /// The annotation exists but does not allow this change (for example, a drawing has no colour
    /// to change), or nothing is left to put back.
    AnnotationRefused = 19,
    /// The document's own permissions do not allow annotations to be added or changed.
    AnnotationsNotAllowed = 20,
}

impl ErrorCode {
    pub fn from_u8(v: u8) -> Option<ErrorCode> {
        Some(match v {
            1 => ErrorCode::Io,
            2 => ErrorCode::Corrupt,
            3 => ErrorCode::PasswordRequired,
            4 => ErrorCode::PasswordIncorrect,
            5 => ErrorCode::Unsupported,
            6 => ErrorCode::UnknownDocument,
            7 => ErrorCode::PageOutOfRange,
            8 => ErrorCode::InvalidRequest,
            9 => ErrorCode::RenderFailed,
            10 => ErrorCode::EngineUnavailable,
            11 => ErrorCode::ProtocolMismatch,
            12 => ErrorCode::Internal,
            13 => ErrorCode::SandboxUnavailable,
            14 => ErrorCode::SaveFailed,
            15 => ErrorCode::SaveTooLarge,
            16 => ErrorCode::SourceChanged,
            17 => ErrorCode::SourceUnreadable,
            50 => ErrorCode::TooManyPages,
            51 => ErrorCode::SnapshotGone,
            52 => ErrorCode::ImportFailed,
            53 => ErrorCode::PageChangeFailed,
            54 => ErrorCode::Busy,
            55 => ErrorCode::InsertSourceFailed,
            56 => ErrorCode::PagesNotAllowed,
            24 => ErrorCode::FormUnsupported,
            25 => ErrorCode::FormFieldRefused,
            18 => ErrorCode::NoSuchAnnotation,
            19 => ErrorCode::AnnotationRefused,
            20 => ErrorCode::AnnotationsNotAllowed,
            _ => return None,
        })
    }

    /// A short stable name, used as the error code the UI sees.
    pub fn name(self) -> &'static str {
        match self {
            ErrorCode::Io => "io",
            ErrorCode::Corrupt => "corrupt",
            ErrorCode::PasswordRequired => "password_required",
            ErrorCode::PasswordIncorrect => "password_incorrect",
            ErrorCode::Unsupported => "unsupported",
            ErrorCode::UnknownDocument => "unknown_document",
            ErrorCode::PageOutOfRange => "page_out_of_range",
            ErrorCode::InvalidRequest => "invalid_request",
            ErrorCode::RenderFailed => "render_failed",
            ErrorCode::EngineUnavailable => "engine_unavailable",
            ErrorCode::ProtocolMismatch => "protocol_mismatch",
            ErrorCode::Internal => "internal",
            ErrorCode::SandboxUnavailable => "sandbox_unavailable",
            ErrorCode::SaveFailed => "save_failed",
            ErrorCode::SaveTooLarge => "save_too_large",
            ErrorCode::SourceChanged => "source_changed",
            ErrorCode::SourceUnreadable => "source_unreadable",
            ErrorCode::TooManyPages => "too_many_pages",
            ErrorCode::SnapshotGone => "snapshot_gone",
            ErrorCode::ImportFailed => "import_failed",
            ErrorCode::PageChangeFailed => "page_change_failed",
            ErrorCode::Busy => "busy",
            ErrorCode::InsertSourceFailed => "insert_source_failed",
            ErrorCode::PagesNotAllowed => "pages_not_allowed",
            ErrorCode::FormUnsupported => "form_unsupported",
            ErrorCode::FormFieldRefused => "form_field_refused",
            ErrorCode::NoSuchAnnotation => "no_such_annotation",
            ErrorCode::AnnotationRefused => "annotation_refused",
            ErrorCode::AnnotationsNotAllowed => "annotations_not_allowed",
        }
    }
}

/// Size of one page in PDF points (1/72 inch), after the page's own rotation is applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RequestBody {
    /// First message on a connection.
    Hello { protocol_version: u32 },
    /// Open the one document the app handed to the engine when it started it (an inherited,
    /// read-only file handle). The engine never opens files by path. It opens that document
    /// once; a second `Open` is refused.
    Open { password: Option<String> },
    /// Draw one page into a bitmap of exactly `width` by `height` pixels.
    Render {
        doc: u32,
        page: u32,
        width: u32,
        height: u32,
    },
    /// Forget a document and free its memory.
    Close { doc: u32 },
    /// The table of contents of a document (its bookmarks).
    Outline { doc: u32 },
    /// The text of one page, character by character, with the place of every character.
    PageText { doc: u32, page: u32 },
    /// The links of one page that point to a web address or to a page of the same document.
    PageLinks { doc: u32, page: u32 },
    /// Find `query` on one page, ignoring case and runs of white space.
    Search { doc: u32, page: u32, query: String },
    /// What the app needs to know before it saves: the page count, whether the document is
    /// encrypted, and how many signatures it holds.
    DocInfo { doc: u32 },
    /// Serialise the document (with everything changed in it) into a buffer in the engine and
    /// say how big it is. The engine writes nothing to disk: the app fetches the bytes with
    /// `SaveChunk` and writes the file itself. A second `SaveBegin` replaces the buffer.
    SaveBegin { doc: u32 },
    /// `len` bytes of the buffer made by `SaveBegin`, starting at `offset`. The engine answers
    /// with exactly that many bytes, or with an error. `len` is at most [`MAX_SAVE_CHUNK`].
    SaveChunk { doc: u32, offset: u32, len: u32 },
    /// Free the buffer made by `SaveBegin`.
    SaveEnd { doc: u32 },
    /// A digest of everything a reader sees of the document: the size of every page, the text,
    /// the number of objects and annotations. The app asks it of the engine that holds the
    /// document and of the engine that reads the saved file, and the two must agree.
    Fingerprint { doc: u32 },
    /// Describe and check the digital signatures of the file the document was opened from.
    /// `now` is the time of the check in seconds since 1970 (the engine does not read the clock):
    /// the validity of certificates is judged against it. The check reads the whole file, so it
    /// reports progress like `DocInfo`. `now` is at most [`MAX_UNIX_TIME`].
    Signatures { doc: u32, now: i64 },
    /// What kind of form the document has, and whether it has scripts, which never run, or actions
    /// on fields.
    FormInfo { doc: u32 },
    /// The fields of one page that the reader fills in, with their values.
    ListFormFields { doc: u32, page: u32 },
    /// Sets one field. `index` and `name` say which: the place of the widget on its page, and the full
    /// name the window saw there. The answer is the fields of the page as they are now.
    SetFormField {
        doc: u32,
        page: u32,
        index: u32,
        name: String,
        value: FormValue,
    },
    /// The scripts for which PDFium asked the engine for a font and got none (no system font for
    /// the script, a font that did not pass the check, or the budget of font data used up).
    /// Text in those scripts that the PDF does not carry a font for is drawn as blanks.
    FontReport,
    /// A page tool: delete, reorder, rotate, crop, insert, extract (see the `pages` module).
    Pages(PagesRequest),
    /// The annotations of one page that the reader can show and change: markup (highlight,
    /// underline, strike-out), notes, drawings and stamps, and a few kinds that can only be removed.
    /// Links and form fields are not in the list.
    ListAnnots { doc: u32, page: u32 },
    /// Marks the characters `start..end` of the page text (the same numbering as `PageText`) with a
    /// highlight, an underline or a strike-out. The engine finds the boxes of the characters
    /// itself, so the window never says where the marks go.
    AddMarkup {
        doc: u32,
        page: u32,
        kind: AnnotKind,
        color: [u8; 4],
        start: u32,
        end: u32,
    },
    /// A sticky note with `text` at a point of the page, in points from the top left of the page
    /// as it is drawn.
    AddNote {
        doc: u32,
        page: u32,
        x: f32,
        y: f32,
        color: [u8; 4],
        text: String,
    },
    /// A free drawing: one or more strokes of points in points from the top left of the page as it
    /// is drawn, with a line `width` in points.
    AddInk {
        doc: u32,
        page: u32,
        color: [u8; 4],
        width: f32,
        strokes: Vec<Vec<[f32; 2]>>,
    },
    /// A stamp with a fixed wording, its top left corner at a point of the page.
    AddStamp {
        doc: u32,
        page: u32,
        x: f32,
        y: f32,
        stamp: StampKind,
    },
    /// Changes an annotation: its text, its colour and its place (`dx` and `dy` in points on the
    /// drawn page). What the annotation does not allow is refused.
    UpdateAnnot {
        doc: u32,
        page: u32,
        id: u32,
        text: Option<String>,
        color: Option<[u8; 4]>,
        dx: f32,
        dy: f32,
    },
    /// Takes an annotation off the page.
    RemoveAnnot { doc: u32, page: u32, id: u32 },
    /// Puts back an annotation that `RemoveAnnot` took off (`id` is the id it had). It gets a new id.
    RestoreAnnot { doc: u32, page: u32, id: u32 },
}

/// One entry of the table of contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineItem {
    /// Nesting level: 0 for the top level.
    pub depth: u8,
    /// Page the entry leads to (counted from 0), when it leads to a page of this document.
    pub page: Option<u32>,
    pub title: String,
}

/// Where a link on a page leads. Links that do anything else (open a file, run a program, go to
/// another document) are never reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// A web address, as written in the document. It can be any scheme: the app decides.
    Uri(String),
    /// A page of the same document, counted from 0.
    Page(u32),
}

/// A link on a page. The box is `[left, top, right, bottom]` in points from the top left of the
/// page as it is drawn (after the page's own rotation).
#[derive(Debug, Clone, PartialEq)]
pub struct PageLink {
    pub rect: [f32; 4],
    pub target: LinkTarget,
}

/// One match of a search on a page. `start` and `len` count characters of the page text as
/// reported by `PageText`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextMatch {
    pub start: u32,
    pub len: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: u32,
    pub body: RequestBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResponseBody {
    Hello {
        protocol_version: u32,
        engine_version: String,
        pdfium_version: String,
        /// The layers of the operating system sandbox the engine runs in, for example
        /// `seccomp+landlock`. When the engine is not sandboxed (debug builds only) it starts
        /// with `none`, followed by the reason.
        sandbox: String,
    },
    Opened {
        doc: u32,
        pages: Vec<PageSize>,
    },
    /// `rgba` holds `width * height * 4` bytes, rows top to bottom.
    Rendered {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Closed,
    Outline {
        items: Vec<OutlineItem>,
    },
    /// `text` holds one entry per character; `rects` holds `[left, top, right, bottom]` for each
    /// of them, in points from the top left of the page as it is drawn. A character without a
    /// place on the page (a line break) has an empty box of zeros.
    PageText {
        text: String,
        rects: Vec<[f32; 4]>,
    },
    PageLinks {
        links: Vec<PageLink>,
    },
    Matches {
        matches: Vec<TextMatch>,
    },
    /// Answer to `DocInfo`. `security` is the revision of the standard security handler (2 to 4,
    /// 255 for a newer one) and 0 for a document that is not encrypted. `signatures` is how many
    /// signatures PDFium finds, looking at the field list and at every widget on every page.
    /// `maybe_signed` is true when the bytes of the file mention anything that belongs to a
    /// signature (`/ByteRange`, `/DocMDP`, `/SigFlags`), whether PDFium found one or not: a
    /// document is treated as signed when either says so. `touched` is true when the time of
    /// change of the file moved while parts of it had never been read: nothing seen so far differs,
    /// but a part that was not read cannot be compared with what it was, so the engine's copy of
    /// the document may not be the file's. It may be written to a new file, not over the original.
    DocInfo {
        pages: u32,
        security: u8,
        signatures: u32,
        maybe_signed: bool,
        touched: bool,
    },
    /// Sent while a request takes long (`SaveBegin`, `DocInfo`, `Fingerprint`) to show the engine
    /// is working: `done` counts bytes written or pages looked at. Only the request in flight can
    /// get them, and the app waits for the answer as long as they keep coming.
    Progress {
        done: u32,
    },
    /// Answer to `Fingerprint`: how many pages, how many of them could not be read, and a digest
    /// of what was read.
    Fingerprint {
        pages: u32,
        failed: u32,
        digest: u64,
    },
    /// Answer to `Signatures`.
    Signatures {
        report: SignatureReport,
    },
    /// Answer to `FormInfo`. `has_actions` is true when the document has scripts or actions on its
    /// fields, which the reader never runs: fields that add up or format values are not updated.
    FormInfo {
        kind: FormKind,
        has_actions: bool,
    },
    /// Answer to `ListFormFields` and `SetFormField`, at most [`MAX_FORM_FIELDS_PER_PAGE`] fields.
    FormFields {
        fields: Vec<FormField>,
    },
    /// Answer to `SaveBegin`: the size of the serialised document, at most [`MAX_SAVE_BYTES`].
    SaveReady {
        size: u32,
    },
    /// Answer to `SaveChunk`: the bytes at `offset`, at most [`MAX_SAVE_CHUNK`] of them.
    SaveData {
        offset: u32,
        data: Vec<u8>,
    },
    SaveEnded,
    /// The answer to `FontReport`, in the fixed order of [`FontScript::ALL`].
    FontReport {
        missed: Vec<FontScript>,
    },
    /// The answer to a page tool.
    Pages(PagesResponse),
    /// Answer to `ListAnnots`, at most [`MAX_ANNOTS_PER_PAGE`] entries.
    Annots {
        items: Vec<AnnotInfo>,
    },
    /// Answer to an `Add*`, `UpdateAnnot` or `RestoreAnnot`: the annotation as it is now.
    Annot {
        item: AnnotInfo,
    },
    /// Answer to `RemoveAnnot`.
    AnnotRemoved,
    Error {
        code: ErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub id: u32,
    pub body: ResponseBody,
}

// ---- encoding -------------------------------------------------------------

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Writes a string, cut to at most [`MAX_STRING_LEN`] bytes on a character boundary, so that
/// everything the encoder writes can always be decoded. A cut value is a different value, so
/// callers must refuse a path or password that is too long before encoding (the engine client
/// does); the cut is only a safety net, and fine for error messages.
fn put_str(out: &mut Vec<u8>, s: &str) {
    let mut end = s.len().min(MAX_STRING_LEN);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    put_u32(out, end as u32);
    out.extend_from_slice(&s.as_bytes()[..end]);
}

/// Writes a string cut to at most `max` bytes on a character boundary.
fn put_str_max(out: &mut Vec<u8>, s: &str, max: usize) {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    put_u32(out, end as u32);
    out.extend_from_slice(&s.as_bytes()[..end]);
}

fn put_rect(out: &mut Vec<u8>, rect: &[f32; 4]) {
    for v in rect {
        put_f32(out, *v);
    }
}

fn put_opt_str(out: &mut Vec<u8>, s: Option<&str>) {
    match s {
        Some(s) => {
            out.push(1);
            put_str(out, s);
        }
        None => out.push(0),
    }
}

impl Request {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32);
        match &self.body {
            RequestBody::Hello { protocol_version } => {
                out.push(TAG_REQ_HELLO);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *protocol_version);
            }
            RequestBody::Open { password } => {
                out.push(TAG_REQ_OPEN);
                put_u32(&mut out, self.id);
                put_opt_str(&mut out, password.as_deref());
            }
            RequestBody::Render {
                doc,
                page,
                width,
                height,
            } => {
                out.push(TAG_REQ_RENDER);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
                put_u32(&mut out, *page);
                put_u32(&mut out, *width);
                put_u32(&mut out, *height);
            }
            RequestBody::Close { doc } => {
                out.push(TAG_REQ_CLOSE);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
            }
            RequestBody::Outline { doc } => {
                out.push(TAG_REQ_OUTLINE);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
            }
            RequestBody::PageText { doc, page } => {
                out.push(TAG_REQ_PAGE_TEXT);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
                put_u32(&mut out, *page);
            }
            RequestBody::PageLinks { doc, page } => {
                out.push(TAG_REQ_PAGE_LINKS);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
                put_u32(&mut out, *page);
            }
            RequestBody::Search { doc, page, query } => {
                out.push(TAG_REQ_SEARCH);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
                put_u32(&mut out, *page);
                put_str_max(&mut out, query, MAX_QUERY_LEN);
            }
            RequestBody::DocInfo { doc } => {
                out.push(TAG_REQ_DOC_INFO);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
            }
            RequestBody::SaveBegin { doc } => {
                out.push(TAG_REQ_SAVE_BEGIN);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
            }
            RequestBody::SaveChunk { doc, offset, len } => {
                out.push(TAG_REQ_SAVE_CHUNK);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
                put_u32(&mut out, *offset);
                put_u32(&mut out, *len);
            }
            RequestBody::SaveEnd { doc } => {
                out.push(TAG_REQ_SAVE_END);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
            }
            RequestBody::Fingerprint { doc } => {
                out.push(TAG_REQ_FINGERPRINT);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
            }
            RequestBody::Signatures { doc, now } => {
                out.push(TAG_REQ_SIGNATURES);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
                put_u32(&mut out, *now as u32);
                put_u32(&mut out, (*now >> 32) as u32);
            }
            RequestBody::FormInfo { doc } => {
                out.push(TAG_REQ_FORM_INFO);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
            }
            RequestBody::ListFormFields { doc, page } => {
                out.push(TAG_REQ_LIST_FORM_FIELDS);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
                put_u32(&mut out, *page);
            }
            RequestBody::SetFormField {
                doc,
                page,
                index,
                name,
                value,
            } => {
                out.push(TAG_REQ_SET_FORM_FIELD);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
                put_u32(&mut out, *page);
                put_u32(&mut out, *index);
                put_str_max(&mut out, name, MAX_FORM_NAME);
                put_form_value(&mut out, value);
            }
            RequestBody::FontReport => {
                out.push(TAG_REQ_FONT_REPORT);
                put_u32(&mut out, self.id);
            }
            RequestBody::Pages(request) => request.encode(self.id, &mut out),
            RequestBody::ListAnnots { doc, page } => {
                out.push(TAG_REQ_LIST_ANNOTS);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
                put_u32(&mut out, *page);
            }
            RequestBody::AddMarkup {
                doc,
                page,
                kind,
                color,
                start,
                end,
            } => {
                out.push(TAG_REQ_ADD_MARKUP);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
                put_u32(&mut out, *page);
                out.push(*kind as u8);
                out.extend_from_slice(color);
                put_u32(&mut out, *start);
                put_u32(&mut out, *end);
            }
            RequestBody::AddNote {
                doc,
                page,
                x,
                y,
                color,
                text,
            } => {
                out.push(TAG_REQ_ADD_NOTE);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
                put_u32(&mut out, *page);
                put_f32(&mut out, *x);
                put_f32(&mut out, *y);
                out.extend_from_slice(color);
                put_str_max(&mut out, text, MAX_ANNOT_TEXT);
            }
            RequestBody::AddInk {
                doc,
                page,
                color,
                width,
                strokes,
            } => {
                out.push(TAG_REQ_ADD_INK);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
                put_u32(&mut out, *page);
                out.extend_from_slice(color);
                put_f32(&mut out, *width);
                annots::put_strokes(&mut out, strokes);
            }
            RequestBody::AddStamp {
                doc,
                page,
                x,
                y,
                stamp,
            } => {
                out.push(TAG_REQ_ADD_STAMP);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
                put_u32(&mut out, *page);
                put_f32(&mut out, *x);
                put_f32(&mut out, *y);
                out.push(*stamp as u8);
            }
            RequestBody::UpdateAnnot {
                doc,
                page,
                id,
                text,
                color,
                dx,
                dy,
            } => {
                out.push(TAG_REQ_UPDATE_ANNOT);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
                put_u32(&mut out, *page);
                put_u32(&mut out, *id);
                annots::put_update(&mut out, text.as_deref(), color.as_ref(), *dx, *dy);
            }
            RequestBody::RemoveAnnot { doc, page, id } => {
                out.push(TAG_REQ_REMOVE_ANNOT);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
                put_u32(&mut out, *page);
                put_u32(&mut out, *id);
            }
            RequestBody::RestoreAnnot { doc, page, id } => {
                out.push(TAG_REQ_RESTORE_ANNOT);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
                put_u32(&mut out, *page);
                put_u32(&mut out, *id);
            }
        }
        out
    }

    pub fn decode(payload: &[u8]) -> Result<Request, ProtocolError> {
        let mut r = Reader::new(payload);
        let tag = r.u8()?;
        let id = r.u32()?;
        let body = match tag {
            TAG_REQ_HELLO => RequestBody::Hello {
                protocol_version: r.u32()?,
            },
            TAG_REQ_OPEN => RequestBody::Open {
                password: r.opt_string()?,
            },
            TAG_REQ_RENDER => RequestBody::Render {
                doc: r.u32()?,
                page: r.u32()?,
                width: r.u32()?,
                height: r.u32()?,
            },
            TAG_REQ_CLOSE => RequestBody::Close { doc: r.u32()? },
            TAG_REQ_OUTLINE => RequestBody::Outline { doc: r.u32()? },
            TAG_REQ_PAGE_TEXT => RequestBody::PageText {
                doc: r.u32()?,
                page: r.u32()?,
            },
            TAG_REQ_PAGE_LINKS => RequestBody::PageLinks {
                doc: r.u32()?,
                page: r.u32()?,
            },
            TAG_REQ_SEARCH => RequestBody::Search {
                doc: r.u32()?,
                page: r.u32()?,
                query: r.string_max(MAX_QUERY_LEN)?,
            },
            TAG_REQ_DOC_INFO => RequestBody::DocInfo { doc: r.u32()? },
            TAG_REQ_SAVE_BEGIN => RequestBody::SaveBegin { doc: r.u32()? },
            TAG_REQ_SAVE_CHUNK => {
                let doc = r.u32()?;
                let offset = r.u32()?;
                let len = r.u32()?;
                if len as usize > MAX_SAVE_CHUNK {
                    return Err(ProtocolError::InvalidLength);
                }
                RequestBody::SaveChunk { doc, offset, len }
            }
            TAG_REQ_SAVE_END => RequestBody::SaveEnd { doc: r.u32()? },
            TAG_REQ_FINGERPRINT => RequestBody::Fingerprint { doc: r.u32()? },
            TAG_REQ_SIGNATURES => {
                let doc = r.u32()?;
                let now = i64::from(r.u32()?) | (i64::from(r.u32()?) << 32);
                if !(0..=MAX_UNIX_TIME).contains(&now) {
                    return Err(ProtocolError::InvalidLength);
                }
                RequestBody::Signatures { doc, now }
            }
            t if (pages::FIRST_REQUEST_TAG..=pages::LAST_REQUEST_TAG).contains(&t) => {
                RequestBody::Pages(PagesRequest::decode(t, &mut r)?)
            }
            TAG_REQ_FORM_INFO => RequestBody::FormInfo { doc: r.u32()? },
            TAG_REQ_LIST_FORM_FIELDS => RequestBody::ListFormFields {
                doc: r.u32()?,
                page: r.u32()?,
            },
            TAG_REQ_SET_FORM_FIELD => RequestBody::SetFormField {
                doc: r.u32()?,
                page: r.u32()?,
                index: r.u32()?,
                name: r.string_max(MAX_FORM_NAME)?,
                value: read_form_value(&mut r)?,
            },
            TAG_REQ_FONT_REPORT => RequestBody::FontReport,
            TAG_REQ_LIST_ANNOTS => RequestBody::ListAnnots {
                doc: r.u32()?,
                page: r.u32()?,
            },
            TAG_REQ_ADD_MARKUP => {
                let doc = r.u32()?;
                let page = r.u32()?;
                let kind = AnnotKind::markup_from(r.u8()?)?;
                let color = r.color()?;
                let start = r.u32()?;
                let end = r.u32()?;
                // At least one character, and no more than a page holds.
                if end <= start || (end - start) as usize > MAX_MARKUP_CHARS {
                    return Err(ProtocolError::InvalidLength);
                }
                RequestBody::AddMarkup {
                    doc,
                    page,
                    kind,
                    color,
                    start,
                    end,
                }
            }
            TAG_REQ_ADD_NOTE => RequestBody::AddNote {
                doc: r.u32()?,
                page: r.u32()?,
                x: r.coordinate()?,
                y: r.coordinate()?,
                color: r.color()?,
                text: r.string_max(MAX_ANNOT_TEXT)?,
            },
            TAG_REQ_ADD_INK => {
                let doc = r.u32()?;
                let page = r.u32()?;
                let color = r.color()?;
                let width = r.f32()?;
                if !(MIN_INK_WIDTH..=MAX_INK_WIDTH).contains(&width) {
                    return Err(ProtocolError::InvalidLength);
                }
                RequestBody::AddInk {
                    doc,
                    page,
                    color,
                    width,
                    strokes: annots::read_strokes(&mut r)?,
                }
            }
            TAG_REQ_ADD_STAMP => RequestBody::AddStamp {
                doc: r.u32()?,
                page: r.u32()?,
                x: r.coordinate()?,
                y: r.coordinate()?,
                stamp: StampKind::from_u8(r.u8()?)?,
            },
            TAG_REQ_UPDATE_ANNOT => {
                let doc = r.u32()?;
                let page = r.u32()?;
                let id = r.u32()?;
                let (text, color, dx, dy) = annots::read_update(&mut r)?;
                RequestBody::UpdateAnnot {
                    doc,
                    page,
                    id,
                    text,
                    color,
                    dx,
                    dy,
                }
            }
            TAG_REQ_REMOVE_ANNOT => RequestBody::RemoveAnnot {
                doc: r.u32()?,
                page: r.u32()?,
                id: r.u32()?,
            },
            TAG_REQ_RESTORE_ANNOT => RequestBody::RestoreAnnot {
                doc: r.u32()?,
                page: r.u32()?,
                id: r.u32()?,
            },
            other => return Err(ProtocolError::UnknownTag(other)),
        };
        r.finish()?;
        Ok(Request { id, body })
    }
}

/// Number of bytes of a `Rendered` payload before the pixels: tag, id, width, height, pixel length.
const RENDERED_HEADER_LEN: usize = 1 + 4 + 4 + 4 + 4;

/// Number of bytes of a `SaveData` payload before the data: tag, id, offset, length.
pub const SAVE_DATA_HEADER_LEN: usize = 1 + 4 + 4 + 4;

impl Response {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match &self.body {
            ResponseBody::Rendered { rgba, .. } => {
                out.reserve(RENDERED_HEADER_LEN + rgba.len());
                out.extend_from_slice(&self.rendered_header());
                out.extend_from_slice(rgba);
            }
            ResponseBody::Hello {
                protocol_version,
                engine_version,
                pdfium_version,
                sandbox,
            } => {
                out.push(TAG_RES_HELLO);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *protocol_version);
                put_str(&mut out, engine_version);
                put_str(&mut out, pdfium_version);
                put_str(&mut out, sandbox);
            }
            ResponseBody::Opened { doc, pages } => {
                out.push(TAG_RES_OPENED);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *doc);
                let pages = &pages[..pages.len().min(MAX_PAGES)];
                put_u32(&mut out, pages.len() as u32);
                for p in pages {
                    put_f32(&mut out, p.width);
                    put_f32(&mut out, p.height);
                }
            }
            ResponseBody::Closed => {
                out.push(TAG_RES_CLOSED);
                put_u32(&mut out, self.id);
            }
            ResponseBody::Outline { items } => {
                out.push(TAG_RES_OUTLINE);
                put_u32(&mut out, self.id);
                let items = &items[..items.len().min(MAX_OUTLINE_ITEMS)];
                put_u32(&mut out, items.len() as u32);
                for item in items {
                    out.push(item.depth.min(MAX_OUTLINE_DEPTH));
                    put_u32(&mut out, item.page.unwrap_or(NO_PAGE));
                    put_str_max(&mut out, &item.title, MAX_OUTLINE_TITLE);
                }
            }
            ResponseBody::PageText { text, rects } => {
                out.push(TAG_RES_PAGE_TEXT);
                put_u32(&mut out, self.id);
                // Whole characters only, so the text and the boxes always agree on the count.
                let chars = text.chars().count().min(rects.len()).min(MAX_PAGE_CHARS);
                let end = text
                    .char_indices()
                    .nth(chars)
                    .map(|(i, _)| i)
                    .unwrap_or(text.len());
                put_u32(&mut out, chars as u32);
                put_u32(&mut out, end as u32);
                out.extend_from_slice(&text.as_bytes()[..end]);
                for rect in &rects[..chars] {
                    put_rect(&mut out, rect);
                }
            }
            ResponseBody::PageLinks { links } => {
                out.push(TAG_RES_PAGE_LINKS);
                put_u32(&mut out, self.id);
                let links: Vec<&PageLink> = links
                    .iter()
                    .filter(|l| match &l.target {
                        LinkTarget::Uri(u) => u.len() <= MAX_URI_LEN,
                        LinkTarget::Page(_) => true,
                    })
                    .take(MAX_LINKS_PER_PAGE)
                    .collect();
                put_u32(&mut out, links.len() as u32);
                for link in links {
                    put_rect(&mut out, &link.rect);
                    match &link.target {
                        LinkTarget::Uri(u) => {
                            out.push(0);
                            put_str_max(&mut out, u, MAX_URI_LEN);
                        }
                        LinkTarget::Page(p) => {
                            out.push(1);
                            put_u32(&mut out, *p);
                        }
                    }
                }
            }
            ResponseBody::Matches { matches } => {
                out.push(TAG_RES_MATCHES);
                put_u32(&mut out, self.id);
                let matches = &matches[..matches.len().min(MAX_MATCHES_PER_PAGE)];
                put_u32(&mut out, matches.len() as u32);
                for m in matches {
                    put_u32(&mut out, m.start);
                    put_u32(&mut out, m.len);
                }
            }
            ResponseBody::DocInfo {
                pages,
                security,
                signatures,
                maybe_signed,
                touched,
            } => {
                out.push(TAG_RES_DOC_INFO);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *pages);
                out.push(*security);
                put_u32(&mut out, *signatures);
                out.push(u8::from(*maybe_signed));
                out.push(u8::from(*touched));
            }
            ResponseBody::Signatures { report } => {
                out.push(TAG_RES_SIGNATURES);
                put_u32(&mut out, self.id);
                report.encode(&mut out);
            }
            ResponseBody::Progress { done } => {
                out.push(TAG_RES_PROGRESS);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *done);
            }
            ResponseBody::Fingerprint {
                pages,
                failed,
                digest,
            } => {
                out.push(TAG_RES_FINGERPRINT);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *pages);
                put_u32(&mut out, *failed);
                out.extend_from_slice(&digest.to_le_bytes());
            }
            ResponseBody::SaveReady { size } => {
                out.push(TAG_RES_SAVE_READY);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *size);
            }
            ResponseBody::SaveData { offset, data } => {
                // Never more than the cap: a longer piece would not decode.
                let data = &data[..data.len().min(MAX_SAVE_CHUNK)];
                out.reserve(SAVE_DATA_HEADER_LEN + data.len());
                out.push(TAG_RES_SAVE_DATA);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *offset);
                put_u32(&mut out, data.len() as u32);
                out.extend_from_slice(data);
            }
            ResponseBody::SaveEnded => {
                out.push(TAG_RES_SAVE_ENDED);
                put_u32(&mut out, self.id);
            }
            ResponseBody::FormInfo { kind, has_actions } => {
                out.push(TAG_RES_FORM_INFO);
                put_u32(&mut out, self.id);
                out.push(*kind as u8);
                out.push(u8::from(*has_actions));
            }
            ResponseBody::FormFields { fields } => {
                out.push(TAG_RES_FORM_FIELDS);
                put_u32(&mut out, self.id);
                put_fields(&mut out, fields);
            }
            ResponseBody::FontReport { missed } => {
                out.push(TAG_RES_FONT_REPORT);
                put_u32(&mut out, self.id);
                let missed = &missed[..missed.len().min(FontScript::ALL.len())];
                out.push(missed.len() as u8);
                for script in missed {
                    out.push(script.code());
                }
            }
            ResponseBody::Pages(response) => response.encode(self.id, &mut out),
            ResponseBody::Annots { items } => {
                out.push(TAG_RES_ANNOTS);
                put_u32(&mut out, self.id);
                let items = &items[..items.len().min(MAX_ANNOTS_PER_PAGE)];
                put_u32(&mut out, items.len() as u32);
                for item in items {
                    annots::put_info(&mut out, item);
                }
            }
            ResponseBody::Annot { item } => {
                out.push(TAG_RES_ANNOT);
                put_u32(&mut out, self.id);
                annots::put_info(&mut out, item);
            }
            ResponseBody::AnnotRemoved => {
                out.push(TAG_RES_ANNOT_REMOVED);
                put_u32(&mut out, self.id);
            }
            ResponseBody::Error { code, message } => {
                out.push(TAG_RES_ERROR);
                put_u32(&mut out, self.id);
                out.push(*code as u8);
                put_str(&mut out, message);
            }
        }
        out
    }

    /// The part of a `Rendered` payload that comes before the pixels.
    fn rendered_header(&self) -> [u8; RENDERED_HEADER_LEN] {
        let ResponseBody::Rendered {
            width,
            height,
            rgba,
        } = &self.body
        else {
            unreachable!("only called for Rendered");
        };
        let mut h = [0u8; RENDERED_HEADER_LEN];
        h[0] = TAG_RES_RENDERED;
        h[1..5].copy_from_slice(&self.id.to_le_bytes());
        h[5..9].copy_from_slice(&width.to_le_bytes());
        h[9..13].copy_from_slice(&height.to_le_bytes());
        h[13..17].copy_from_slice(&(rgba.len() as u32).to_le_bytes());
        h
    }

    /// Decodes a response. Takes the frame by value so the bitmap can reuse its buffer: the 17 header
    /// bytes are cut off the front (one move inside the buffer, no new allocation).
    pub fn decode(mut payload: Vec<u8>) -> Result<Response, ProtocolError> {
        let mut r = Reader::new(&payload);
        let tag = r.u8()?;
        let id = r.u32()?;
        let body = match tag {
            TAG_RES_HELLO => ResponseBody::Hello {
                protocol_version: r.u32()?,
                engine_version: r.string()?,
                pdfium_version: r.string()?,
                sandbox: r.string()?,
            },
            TAG_RES_OPENED => {
                let doc = r.u32()?;
                let count = r.u32()? as usize;
                // Each page takes 8 bytes: check before reserving anything.
                if count > r.remaining() / 8 {
                    return Err(ProtocolError::Truncated);
                }
                if count > MAX_PAGES {
                    return Err(ProtocolError::InvalidLength);
                }
                let mut pages = Vec::with_capacity(count);
                for _ in 0..count {
                    pages.push(PageSize {
                        width: r.f32()?,
                        height: r.f32()?,
                    });
                }
                ResponseBody::Opened { doc, pages }
            }
            TAG_RES_RENDERED => {
                let width = r.u32()?;
                let height = r.u32()?;
                let len = r.u32()? as usize;
                let pixels = (width as u64) * (height as u64);
                // The product of two u32 fits in a u64; times 4 is checked anyway.
                let expected = pixels.checked_mul(4).ok_or(ProtocolError::InvalidLength)?;
                if len != r.remaining()
                    || expected != len as u64
                    || width > MAX_RENDER_SIDE
                    || height > MAX_RENDER_SIDE
                    || pixels > MAX_RENDER_PIXELS
                {
                    return Err(ProtocolError::InvalidLength);
                }
                payload.drain(..RENDERED_HEADER_LEN);
                return Ok(Response {
                    id,
                    body: ResponseBody::Rendered {
                        width,
                        height,
                        rgba: payload,
                    },
                });
            }
            TAG_RES_CLOSED => ResponseBody::Closed,
            TAG_RES_OUTLINE => {
                let count = r.u32()? as usize;
                // An entry takes at least 9 bytes: depth, page, and the length of the title.
                if count > MAX_OUTLINE_ITEMS || count > r.remaining() / 9 {
                    return Err(ProtocolError::InvalidLength);
                }
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    let depth = r.u8()?;
                    if depth > MAX_OUTLINE_DEPTH {
                        return Err(ProtocolError::UnknownValue(depth));
                    }
                    let page = match r.u32()? {
                        NO_PAGE => None,
                        p => Some(p),
                    };
                    items.push(OutlineItem {
                        depth,
                        page,
                        title: r.string_max(MAX_OUTLINE_TITLE)?,
                    });
                }
                ResponseBody::Outline { items }
            }
            TAG_RES_PAGE_TEXT => {
                let count = r.u32()? as usize;
                let text_len = r.u32()? as usize;
                if count > MAX_PAGE_CHARS || text_len > MAX_PAGE_TEXT_BYTES {
                    return Err(ProtocolError::InvalidLength);
                }
                let text_bytes = r.take(text_len)?;
                let text = std::str::from_utf8(text_bytes)
                    .map_err(|_| ProtocolError::InvalidUtf8)?
                    .to_string();
                if text.chars().count() != count || count > r.remaining() / 16 {
                    return Err(ProtocolError::InvalidLength);
                }
                let mut rects = Vec::with_capacity(count);
                for _ in 0..count {
                    rects.push(r.rect()?);
                }
                ResponseBody::PageText { text, rects }
            }
            TAG_RES_PAGE_LINKS => {
                let count = r.u32()? as usize;
                // A link takes at least 21 bytes: the box, the kind and a page or a length.
                if count > MAX_LINKS_PER_PAGE || count > r.remaining() / 21 {
                    return Err(ProtocolError::InvalidLength);
                }
                let mut links = Vec::with_capacity(count);
                for _ in 0..count {
                    let rect = r.rect()?;
                    let target = match r.u8()? {
                        0 => LinkTarget::Uri(r.string_max(MAX_URI_LEN)?),
                        1 => LinkTarget::Page(r.u32()?),
                        other => return Err(ProtocolError::UnknownValue(other)),
                    };
                    links.push(PageLink { rect, target });
                }
                ResponseBody::PageLinks { links }
            }
            TAG_RES_MATCHES => {
                let count = r.u32()? as usize;
                if count > MAX_MATCHES_PER_PAGE || count > r.remaining() / 8 {
                    return Err(ProtocolError::InvalidLength);
                }
                let mut matches = Vec::with_capacity(count);
                for _ in 0..count {
                    matches.push(TextMatch {
                        start: r.u32()?,
                        len: r.u32()?,
                    });
                }
                ResponseBody::Matches { matches }
            }
            TAG_RES_DOC_INFO => ResponseBody::DocInfo {
                pages: r.u32()?,
                security: r.u8()?,
                signatures: r.u32()?,
                maybe_signed: match r.u8()? {
                    0 => false,
                    1 => true,
                    other => return Err(ProtocolError::UnknownValue(other)),
                },
                touched: match r.u8()? {
                    0 => false,
                    1 => true,
                    other => return Err(ProtocolError::UnknownValue(other)),
                },
            },
            TAG_RES_PROGRESS => ResponseBody::Progress { done: r.u32()? },
            TAG_RES_FINGERPRINT => {
                let pages = r.u32()?;
                let failed = r.u32()?;
                let digest = {
                    let low = u64::from(r.u32()?);
                    let high = u64::from(r.u32()?);
                    low | (high << 32)
                };
                ResponseBody::Fingerprint {
                    pages,
                    failed,
                    digest,
                }
            }
            TAG_RES_SAVE_READY => {
                let size = r.u32()?;
                if size as usize > MAX_SAVE_BYTES {
                    return Err(ProtocolError::InvalidLength);
                }
                ResponseBody::SaveReady { size }
            }
            TAG_RES_SAVE_DATA => {
                let offset = r.u32()?;
                let len = r.u32()? as usize;
                if len > MAX_SAVE_CHUNK
                    || len != r.remaining()
                    || offset as u64 + len as u64 > MAX_SAVE_BYTES as u64
                {
                    return Err(ProtocolError::InvalidLength);
                }
                payload.drain(..SAVE_DATA_HEADER_LEN);
                return Ok(Response {
                    id,
                    body: ResponseBody::SaveData {
                        offset,
                        data: payload,
                    },
                });
            }
            TAG_RES_SIGNATURES => ResponseBody::Signatures {
                report: SignatureReport::decode(&mut r)?,
            },
            TAG_RES_SAVE_ENDED => ResponseBody::SaveEnded,
            TAG_RES_FORM_INFO => ResponseBody::FormInfo {
                kind: FormKind::from_u8(r.u8()?)?,
                has_actions: match r.u8()? {
                    0 => false,
                    1 => true,
                    other => return Err(ProtocolError::UnknownValue(other)),
                },
            },
            TAG_RES_FORM_FIELDS => ResponseBody::FormFields {
                fields: read_fields(&mut r)?,
            },
            TAG_RES_FONT_REPORT => {
                let count = r.u8()? as usize;
                if count > FontScript::ALL.len() {
                    return Err(ProtocolError::InvalidLength);
                }
                let mut missed = Vec::with_capacity(count);
                for _ in 0..count {
                    let raw = r.u8()?;
                    missed
                        .push(FontScript::from_code(raw).ok_or(ProtocolError::UnknownValue(raw))?);
                }
                ResponseBody::FontReport { missed }
            }
            t if (pages::FIRST_RESPONSE_TAG..=pages::LAST_RESPONSE_TAG).contains(&t) => {
                ResponseBody::Pages(PagesResponse::decode(t, &mut r)?)
            }
            TAG_RES_ANNOTS => {
                let count = r.u32()? as usize;
                if count > MAX_ANNOTS_PER_PAGE || count > r.remaining() / ANNOT_INFO_MIN {
                    return Err(ProtocolError::InvalidLength);
                }
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    items.push(annots::read_info(&mut r)?);
                }
                ResponseBody::Annots { items }
            }
            TAG_RES_ANNOT => ResponseBody::Annot {
                item: annots::read_info(&mut r)?,
            },
            TAG_RES_ANNOT_REMOVED => ResponseBody::AnnotRemoved,
            TAG_RES_ERROR => {
                let raw = r.u8()?;
                let code = ErrorCode::from_u8(raw).ok_or(ProtocolError::UnknownValue(raw))?;
                ResponseBody::Error {
                    code,
                    message: r.string()?,
                }
            }
            other => return Err(ProtocolError::UnknownTag(other)),
        };
        r.finish()?;
        Ok(Response { id, body })
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ProtocolError> {
        if n > self.remaining() {
            return Err(ProtocolError::Truncated);
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, ProtocolError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn f32(&mut self) -> Result<f32, ProtocolError> {
        let b = self.take(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// A box of four numbers. A number that is not finite becomes 0, so nothing downstream has to
    /// deal with NaN or infinity.
    fn rect(&mut self) -> Result<[f32; 4], ProtocolError> {
        let mut out = [0.0f32; 4];
        for v in &mut out {
            let x = self.f32()?;
            *v = if x.is_finite() { x } else { 0.0 };
        }
        Ok(out)
    }

    /// A colour as red, green, blue and alpha.
    fn color(&mut self) -> Result<[u8; 4], ProtocolError> {
        let b = self.take(4)?;
        Ok([b[0], b[1], b[2], b[3]])
    }

    /// A place on a page: a number that is finite and not absurdly large. A request that carries
    /// anything else is refused, not repaired.
    fn coordinate(&mut self) -> Result<f32, ProtocolError> {
        let x = self.f32()?;
        if x.is_finite() && x.abs() <= annots::MAX_COORDINATE {
            Ok(x)
        } else {
            Err(ProtocolError::InvalidLength)
        }
    }

    fn string(&mut self) -> Result<String, ProtocolError> {
        self.string_max(MAX_STRING_LEN)
    }

    fn string_max(&mut self, max: usize) -> Result<String, ProtocolError> {
        let len = self.u32()? as usize;
        if len > max {
            return Err(ProtocolError::StringTooLong(len));
        }
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| ProtocolError::InvalidUtf8)
    }

    fn opt_string(&mut self) -> Result<Option<String>, ProtocolError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.string()?)),
            other => Err(ProtocolError::UnknownValue(other)),
        }
    }

    fn finish(self) -> Result<(), ProtocolError> {
        match self.remaining() {
            0 => Ok(()),
            n => Err(ProtocolError::TrailingBytes(n)),
        }
    }
}

// ---- framing --------------------------------------------------------------

/// Writes one frame: the payload length, then the payload.
pub fn write_frame<W: Write>(w: &mut W, payload: &[u8]) -> Result<(), ProtocolError> {
    if payload.len() > MAX_FRAME_LEN {
        return Err(ProtocolError::FrameTooLarge(payload.len() as u64));
    }
    w.write_all(&(payload.len() as u32).to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()?;
    Ok(())
}

/// Writes a request as one frame.
pub fn write_request<W: Write>(w: &mut W, request: &Request) -> Result<(), ProtocolError> {
    write_frame(w, &request.encode())
}

/// Writes a response as one frame. A bitmap goes straight to the writer without being copied
/// into an intermediate buffer first. The bytes on the wire are the same as `write_frame(encode())`.
pub fn write_response<W: Write>(w: &mut W, response: &Response) -> Result<(), ProtocolError> {
    match &response.body {
        ResponseBody::Rendered { rgba, .. } => {
            let total = RENDERED_HEADER_LEN + rgba.len();
            if total > MAX_FRAME_LEN {
                return Err(ProtocolError::FrameTooLarge(total as u64));
            }
            w.write_all(&(total as u32).to_le_bytes())?;
            w.write_all(&response.rendered_header())?;
            w.write_all(rgba)?;
            w.flush()?;
            Ok(())
        }
        _ => write_frame(w, &response.encode()),
    }
}

/// Reads one frame of at most [`MAX_FRAME_LEN`] bytes. Returns `Ok(None)` when the stream ends
/// cleanly between two frames.
pub fn read_frame<R: Read>(r: &mut R) -> Result<Option<Vec<u8>>, ProtocolError> {
    read_frame_limited(r, || MAX_FRAME_LEN)
}

/// Like [`read_frame`], but the largest accepted frame is asked for after the length has arrived
/// (`limit` is called once per frame). A reader that knows what it expects can refuse anything
/// bigger before a single byte of it is stored.
pub fn read_frame_limited<R: Read>(
    r: &mut R,
    limit: impl FnOnce() -> usize,
) -> Result<Option<Vec<u8>>, ProtocolError> {
    let mut len_bytes = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match r.read(&mut len_bytes[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(ProtocolError::Truncated),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    let len = u32::from_le_bytes(len_bytes) as usize;
    if len > limit().min(MAX_FRAME_LEN) {
        return Err(ProtocolError::FrameTooLarge(len as u64));
    }
    // Do not trust the announced length for the allocation. Room is made for one chunk at a time,
    // and only after the chunk before it has arrived in full, so a peer that announces 128 MiB
    // and sends nothing costs one chunk, and memory use follows the bytes that really came.
    let mut payload: Vec<u8> = Vec::new();
    let mut left = len;
    while left > 0 {
        let chunk = left.min(READ_CHUNK);
        payload.reserve(chunk);
        let read = r.by_ref().take(chunk as u64).read_to_end(&mut payload)?;
        if read != chunk {
            return Err(ProtocolError::Truncated);
        }
        left -= chunk;
    }
    Ok(Some(payload))
}

/// How much room `read_frame` makes at a time while a frame arrives.
const READ_CHUNK: usize = 64 * 1024;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn requests() -> Vec<Request> {
        vec![
            Request {
                id: 1,
                body: RequestBody::Hello {
                    protocol_version: PROTOCOL_VERSION,
                },
            },
            Request {
                id: 2,
                body: RequestBody::Open { password: None },
            },
            Request {
                id: 3,
                body: RequestBody::Open {
                    password: Some("hemmelig ✓".into()),
                },
            },
            Request {
                id: u32::MAX,
                body: RequestBody::Render {
                    doc: 7,
                    page: 499,
                    width: 1200,
                    height: 1600,
                },
            },
            Request {
                id: 5,
                body: RequestBody::Close { doc: 7 },
            },
            Request {
                id: 6,
                body: RequestBody::Outline { doc: 7 },
            },
            Request {
                id: 40,
                body: RequestBody::FontReport,
            },
            Request {
                id: 7,
                body: RequestBody::PageText { doc: 7, page: 12 },
            },
            Request {
                id: 8,
                body: RequestBody::PageLinks { doc: 7, page: 0 },
            },
            Request {
                id: 9,
                body: RequestBody::Search {
                    doc: 7,
                    page: 3,
                    query: "blåbær ✓".into(),
                },
            },
        ]
    }

    fn responses() -> Vec<Response> {
        vec![
            Response {
                id: 1,
                body: ResponseBody::Hello {
                    protocol_version: PROTOCOL_VERSION,
                    engine_version: "0.1.0".into(),
                    pdfium_version: "PDFium 8125".into(),
                    sandbox: "seccomp+landlock".into(),
                },
            },
            Response {
                id: 2,
                body: ResponseBody::Opened {
                    doc: 3,
                    pages: vec![
                        PageSize {
                            width: 612.0,
                            height: 792.0,
                        },
                        PageSize {
                            width: 595.276,
                            height: 841.89,
                        },
                    ],
                },
            },
            Response {
                id: 2,
                body: ResponseBody::Opened {
                    doc: 3,
                    pages: vec![],
                },
            },
            Response {
                id: 4,
                body: ResponseBody::Rendered {
                    width: 2,
                    height: 3,
                    rgba: (0..24).collect(),
                },
            },
            Response {
                id: 5,
                body: ResponseBody::Closed,
            },
            Response {
                id: 41,
                body: ResponseBody::FontReport {
                    missed: vec![FontScript::Korean, FontScript::Thai],
                },
            },
            Response {
                id: 6,
                body: ResponseBody::Error {
                    code: ErrorCode::PasswordRequired,
                    message: "This file is protected by a password.".into(),
                },
            },
            Response {
                id: 7,
                body: ResponseBody::Outline {
                    items: vec![
                        OutlineItem {
                            depth: 0,
                            page: Some(0),
                            title: "Kapittel 1 ✓".into(),
                        },
                        OutlineItem {
                            depth: 1,
                            page: None,
                            title: String::new(),
                        },
                    ],
                },
            },
            Response {
                id: 8,
                body: ResponseBody::PageText {
                    text: "Hé\n📕".into(),
                    rects: vec![
                        [1.0, 2.0, 3.0, 4.0],
                        [5.0, 6.0, 7.0, 8.0],
                        [0.0; 4],
                        [9.0, 10.0, 11.0, 12.0],
                    ],
                },
            },
            Response {
                id: 9,
                body: ResponseBody::PageText {
                    text: String::new(),
                    rects: vec![],
                },
            },
            Response {
                id: 10,
                body: ResponseBody::PageLinks {
                    links: vec![
                        PageLink {
                            rect: [10.0, 20.0, 30.0, 40.0],
                            target: LinkTarget::Uri("https://example.com/a?b=c".into()),
                        },
                        PageLink {
                            rect: [1.5, 2.5, 3.5, 4.5],
                            target: LinkTarget::Page(41),
                        },
                    ],
                },
            },
            Response {
                id: 11,
                body: ResponseBody::Matches {
                    matches: vec![
                        TextMatch { start: 3, len: 5 },
                        TextMatch { start: 90, len: 1 },
                    ],
                },
            },
            Response {
                id: 12,
                body: ResponseBody::DocInfo {
                    pages: 500,
                    security: 3,
                    signatures: 2,
                    maybe_signed: true,
                    touched: true,
                },
            },
            Response {
                id: 18,
                body: ResponseBody::Progress { done: u32::MAX },
            },
            Response {
                id: 19,
                body: ResponseBody::Fingerprint {
                    pages: 12,
                    failed: 1,
                    digest: 0xDEAD_BEEF_0123_4567,
                },
            },
            Response {
                id: 13,
                body: ResponseBody::SaveReady { size: 123_456 },
            },
            Response {
                id: 14,
                body: ResponseBody::SaveData {
                    offset: 1_048_576,
                    data: (0..=255).collect(),
                },
            },
            Response {
                id: 15,
                body: ResponseBody::SaveData {
                    offset: 0,
                    data: Vec::new(),
                },
            },
            Response {
                id: 16,
                body: ResponseBody::SaveEnded,
            },
            Response {
                id: 17,
                body: ResponseBody::Error {
                    code: ErrorCode::SaveTooLarge,
                    message: "too big".into(),
                },
            },
        ]
    }

    #[test]
    fn save_requests_round_trip_and_have_one_encoding() {
        for body in [
            RequestBody::DocInfo { doc: 7 },
            RequestBody::SaveBegin { doc: 7 },
            RequestBody::SaveChunk {
                doc: 7,
                offset: u32::MAX,
                len: MAX_SAVE_CHUNK as u32,
            },
            RequestBody::SaveChunk {
                doc: 7,
                offset: 0,
                len: 0,
            },
            RequestBody::SaveEnd { doc: 7 },
            RequestBody::Fingerprint { doc: 7 },
        ] {
            let req = Request { id: 99, body };
            let bytes = req.encode();
            assert_eq!(Request::decode(&bytes).unwrap(), req);
            // One byte too many or too few is never a valid message.
            assert!(Request::decode(&bytes[..bytes.len() - 1]).is_err());
            let mut longer = bytes.clone();
            longer.push(0);
            assert!(matches!(
                Request::decode(&longer),
                Err(ProtocolError::TrailingBytes(1))
            ));
        }
    }

    #[test]
    fn a_save_request_for_more_than_one_chunk_is_refused() {
        let req = Request {
            id: 1,
            body: RequestBody::SaveChunk {
                doc: 1,
                offset: 0,
                len: MAX_SAVE_CHUNK as u32 + 1,
            },
        };
        assert!(matches!(
            Request::decode(&req.encode()),
            Err(ProtocolError::InvalidLength)
        ));
    }

    fn save_data_bytes(offset: u32, declared: u32, actual: usize) -> Vec<u8> {
        let mut p = vec![TAG_RES_SAVE_DATA];
        p.extend_from_slice(&1u32.to_le_bytes());
        p.extend_from_slice(&offset.to_le_bytes());
        p.extend_from_slice(&declared.to_le_bytes());
        p.resize(p.len() + actual, 7);
        p
    }

    #[test]
    fn save_data_is_held_to_the_chunk_cap_and_to_its_own_length() {
        // The cap itself is fine, one over is not.
        let ok = save_data_bytes(0, MAX_SAVE_CHUNK as u32, MAX_SAVE_CHUNK);
        match Response::decode(ok).unwrap().body {
            ResponseBody::SaveData { offset, data } => {
                assert_eq!((offset, data.len()), (0, MAX_SAVE_CHUNK));
            }
            other => panic!("{other:?}"),
        }
        let over = save_data_bytes(0, MAX_SAVE_CHUNK as u32 + 1, MAX_SAVE_CHUNK + 1);
        assert!(matches!(
            Response::decode(over),
            Err(ProtocolError::InvalidLength)
        ));
        // A length that does not match the bytes that are there, both ways.
        assert!(Response::decode(save_data_bytes(0, 10, 9)).is_err());
        assert!(Response::decode(save_data_bytes(0, 9, 10)).is_err());
        // A piece that reaches past the biggest file is refused.
        assert!(Response::decode(save_data_bytes(MAX_SAVE_BYTES as u32 - 4, 5, 5)).is_err());
        assert!(Response::decode(save_data_bytes(MAX_SAVE_BYTES as u32 - 5, 5, 5)).is_ok());
        // The encoder never writes more than the cap.
        let long = Response {
            id: 1,
            body: ResponseBody::SaveData {
                offset: 0,
                data: vec![1; MAX_SAVE_CHUNK + 10],
            },
        };
        match Response::decode(long.encode()).unwrap().body {
            ResponseBody::SaveData { data, .. } => assert_eq!(data.len(), MAX_SAVE_CHUNK),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_flag_of_a_possibly_signed_document_is_a_flag() {
        let mut bytes = Response {
            id: 1,
            body: ResponseBody::DocInfo {
                pages: 1,
                security: 0,
                signatures: 0,
                maybe_signed: true,
                touched: true,
            },
        }
        .encode();
        let end = bytes.len();
        assert_eq!(bytes[end - 2..], [1, 1]);
        bytes[end - 2] = 2;
        assert!(matches!(
            Response::decode(bytes.clone()),
            Err(ProtocolError::UnknownValue(2))
        ));
        bytes[end - 2] = 1;
        bytes[end - 1] = 2;
        assert!(matches!(
            Response::decode(bytes),
            Err(ProtocolError::UnknownValue(2))
        ));
    }

    #[test]
    fn a_save_size_over_the_cap_is_refused() {
        let ready = |size: u32| {
            Response {
                id: 1,
                body: ResponseBody::SaveReady { size },
            }
            .encode()
        };
        assert!(Response::decode(ready(MAX_SAVE_BYTES as u32)).is_ok());
        assert!(matches!(
            Response::decode(ready(MAX_SAVE_BYTES as u32 + 1)),
            Err(ProtocolError::InvalidLength)
        ));
    }

    #[test]
    fn requests_round_trip() {
        for req in requests() {
            let decoded = Request::decode(&req.encode()).unwrap();
            assert_eq!(decoded, req);
        }
    }

    #[test]
    fn responses_round_trip() {
        for res in responses() {
            let decoded = Response::decode(res.encode()).unwrap();
            assert_eq!(decoded, res);
        }
    }

    /// Several features add messages to this protocol on branches of their own. Their numbers
    /// must not meet when the branches are put together: the compiler cannot see it, because the
    /// page tools keep their tags in a module of their own and `decode` reaches them through a
    /// range. The ranges below are those the other features use (annotations, forms, signatures);
    /// no tag of the page tools may fall in them, and no tag of the reader may fall in the ranges of
    /// the page tools.
    #[test]
    fn the_numbers_of_the_page_tools_meet_no_other_feature() {
        let pages_requests = pages::FIRST_REQUEST_TAG..=pages::LAST_REQUEST_TAG;
        let pages_answers = pages::FIRST_RESPONSE_TAG..=pages::LAST_RESPONSE_TAG;
        let others_requests: [std::ops::RangeInclusive<u8>; 3] = [15..=22, 40..=42, 60..=60];
        let others_answers: [std::ops::RangeInclusive<u8>; 3] = [144..=146, 170..=171, 190..=190];
        for other in &others_requests {
            assert!(
                other.end() < pages_requests.start() || other.start() > pages_requests.end(),
                "{other:?}"
            );
        }
        for other in &others_answers {
            assert!(
                other.end() < pages_answers.start() || other.start() > pages_answers.end(),
                "{other:?}"
            );
        }
        // The error codes of the other features: annotations 18 to 20, forms 24 and 25.
        for code in [18u8, 19, 20, 24, 25] {
            assert!(!(50..=56).contains(&code));
        }
        for req in requests() {
            assert!(!pages_requests.contains(&req.encode()[0]));
        }
        for res in responses() {
            assert!(!pages_answers.contains(&res.encode()[0]));
        }
        // Every code of the reader and the save is below the ones of the page tools, and every
        // code that has a number is known by name.
        for raw in 0..=u8::MAX {
            if let Some(code) = ErrorCode::from_u8(raw) {
                assert_eq!(code as u8, raw);
                assert!(!code.name().is_empty());
            }
        }
    }

    #[test]
    fn frames_round_trip_through_a_stream() {
        let mut wire = Vec::new();
        for req in requests() {
            write_request(&mut wire, &req).unwrap();
        }
        let mut cursor = Cursor::new(wire);
        for req in requests() {
            let frame = read_frame(&mut cursor).unwrap().unwrap();
            assert_eq!(Request::decode(&frame).unwrap(), req);
        }
        assert!(read_frame(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn write_response_matches_encode() {
        for res in responses() {
            let mut a = Vec::new();
            write_response(&mut a, &res).unwrap();
            let mut b = Vec::new();
            write_frame(&mut b, &res.encode()).unwrap();
            assert_eq!(a, b);
        }
    }

    /// A reader that returns one byte per call, the worst case for pipes.
    struct OneByte<R: Read>(R);

    impl<R: Read> Read for OneByte<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            self.0.read(&mut buf[..1])
        }
    }

    #[test]
    fn frames_survive_one_byte_reads() {
        let res = responses().remove(3);
        let mut wire = Vec::new();
        write_response(&mut wire, &res).unwrap();
        let mut r = OneByte(Cursor::new(wire));
        let frame = read_frame(&mut r).unwrap().unwrap();
        assert_eq!(Response::decode(frame).unwrap(), res);
    }

    #[test]
    fn clean_end_of_stream_is_none_but_a_cut_header_is_an_error() {
        assert!(read_frame(&mut Cursor::new(Vec::new())).unwrap().is_none());
        for cut in 1..4 {
            let err = read_frame(&mut Cursor::new(vec![9u8; cut])).unwrap_err();
            assert!(matches!(err, ProtocolError::Truncated), "{err:?}");
        }
    }

    #[test]
    fn a_cut_payload_is_an_error() {
        let mut wire = Vec::new();
        write_request(&mut wire, &requests()[1]).unwrap();
        wire.truncate(wire.len() - 1);
        let err = read_frame(&mut Cursor::new(wire)).unwrap_err();
        assert!(matches!(err, ProtocolError::Truncated), "{err:?}");
    }

    /// Counts the biggest single allocation (or growth) of the thread that asks, to show how much
    /// room a read makes.
    mod tracking {
        use std::alloc::{GlobalAlloc, Layout, System};
        use std::cell::Cell;

        thread_local! {
            static BIGGEST: Cell<usize> = const { Cell::new(0) };
        }

        fn note(size: usize) {
            let _ = BIGGEST.try_with(|b| {
                if size > b.get() {
                    b.set(size)
                }
            });
        }

        pub struct Tracking;

        // SAFETY: every call is passed on unchanged to the system allocator.
        unsafe impl GlobalAlloc for Tracking {
            unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
                note(layout.size());
                System.alloc(layout)
            }
            unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
                note(layout.size());
                System.alloc_zeroed(layout)
            }
            unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
                System.dealloc(ptr, layout)
            }
            unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
                note(new_size);
                System.realloc(ptr, layout, new_size)
            }
        }

        pub fn biggest_allocation_during<T>(f: impl FnOnce() -> T) -> (T, usize) {
            BIGGEST.with(|b| b.set(0));
            let result = f();
            (result, BIGGEST.with(|b| b.get()))
        }
    }

    #[global_allocator]
    static ALLOCATOR: tracking::Tracking = tracking::Tracking;

    #[test]
    fn a_frame_that_announces_much_and_sends_little_costs_one_chunk() {
        // Announces the largest frame there is and sends three bytes.
        let mut wire = (MAX_FRAME_LEN as u32).to_le_bytes().to_vec();
        wire.extend_from_slice(&[1, 2, 3]);
        let (result, biggest) =
            tracking::biggest_allocation_during(|| read_frame(&mut Cursor::new(wire)));
        assert!(
            matches!(result, Err(ProtocolError::Truncated)),
            "{result:?}"
        );
        assert!(
            biggest <= 2 * READ_CHUNK,
            "a 128 MiB announcement made an allocation of {biggest} bytes"
        );
    }

    #[test]
    fn a_frame_grows_the_buffer_as_data_arrives() {
        for len in [
            0,
            1,
            READ_CHUNK - 1,
            READ_CHUNK,
            READ_CHUNK + 1,
            2 * READ_CHUNK,
            300_000,
        ] {
            let payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let mut wire = (len as u32).to_le_bytes().to_vec();
            wire.extend_from_slice(&payload);
            // A second frame behind it must be left alone.
            wire.extend_from_slice(&[7, 0, 0, 0, 9, 9, 9, 9, 9, 9, 9]);
            let mut cursor = Cursor::new(wire);
            let (frame, biggest) =
                tracking::biggest_allocation_during(|| read_frame(&mut cursor).unwrap().unwrap());
            assert_eq!(frame, payload, "length {len}");
            assert!(
                biggest <= 2 * len.max(READ_CHUNK),
                "length {len}: allocation of {biggest} bytes"
            );
            assert_eq!(
                read_frame(&mut cursor).unwrap().unwrap(),
                vec![9u8; 7],
                "length {len}"
            );
        }
    }

    #[test]
    fn an_oversized_frame_is_refused_before_allocating() {
        let wire = u32::MAX.to_le_bytes().to_vec();
        let err = read_frame(&mut Cursor::new(wire)).unwrap_err();
        assert!(matches!(err, ProtocolError::FrameTooLarge(_)), "{err:?}");
        let too_big = vec![0u8; MAX_FRAME_LEN + 1];
        assert!(matches!(
            write_frame(&mut Vec::new(), &too_big),
            Err(ProtocolError::FrameTooLarge(_))
        ));
    }

    #[test]
    fn unknown_tags_and_values_are_refused() {
        assert!(matches!(
            Request::decode(&[200, 0, 0, 0, 0]),
            Err(ProtocolError::UnknownTag(200))
        ));
        assert!(matches!(
            Response::decode(vec![3, 0, 0, 0, 0]),
            Err(ProtocolError::UnknownTag(3))
        ));
        let mut bad_code = Response {
            id: 1,
            body: ResponseBody::Error {
                code: ErrorCode::Io,
                message: String::new(),
            },
        }
        .encode();
        bad_code[5] = 99;
        assert!(matches!(
            Response::decode(bad_code),
            Err(ProtocolError::UnknownValue(99))
        ));
        let mut bad_flag = Request {
            id: 1,
            body: RequestBody::Open { password: None },
        }
        .encode();
        let last = bad_flag.len() - 1;
        bad_flag[last] = 2;
        assert!(matches!(
            Request::decode(&bad_flag),
            Err(ProtocolError::UnknownValue(2))
        ));
    }

    #[test]
    fn trailing_bytes_are_refused() {
        let mut bytes = requests()[4].encode();
        bytes.push(0);
        assert!(matches!(
            Request::decode(&bytes),
            Err(ProtocolError::TrailingBytes(1))
        ));
    }

    #[test]
    fn invalid_utf8_and_long_strings_are_refused() {
        let mut bytes = vec![TAG_REQ_OPEN, 1, 0, 0, 0, 1];
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&[0xff, 0xfe]);
        assert!(matches!(
            Request::decode(&bytes),
            Err(ProtocolError::InvalidUtf8)
        ));

        let mut long = vec![TAG_REQ_OPEN, 1, 0, 0, 0, 1];
        long.extend_from_slice(&((MAX_STRING_LEN + 1) as u32).to_le_bytes());
        assert!(matches!(
            Request::decode(&long),
            Err(ProtocolError::StringTooLong(_))
        ));
    }

    #[test]
    fn the_encoder_cuts_strings_so_they_can_still_be_decoded() {
        let req = Request {
            id: 1,
            body: RequestBody::Open {
                password: Some("é".repeat(MAX_STRING_LEN)),
            },
        };
        let Request {
            body: RequestBody::Open {
                password: Some(password),
            },
            ..
        } = Request::decode(&req.encode()).unwrap()
        else {
            panic!("wrong body");
        };
        assert!(password.len() <= MAX_STRING_LEN);
        assert!(password.chars().all(|c| c == 'é'));
    }

    #[test]
    fn a_page_count_that_does_not_fit_is_refused_without_allocating() {
        let mut bytes = vec![TAG_RES_OPENED, 1, 0, 0, 0];
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            Response::decode(bytes),
            Err(ProtocolError::Truncated)
        ));
    }

    #[test]
    fn a_bitmap_with_the_wrong_size_is_refused() {
        let good = Response {
            id: 1,
            body: ResponseBody::Rendered {
                width: 2,
                height: 2,
                rgba: vec![0; 16],
            },
        }
        .encode();
        // Claim a width of 3 while the data stays 2x2.
        let mut wrong_width = good.clone();
        wrong_width[5] = 3;
        assert!(matches!(
            Response::decode(wrong_width),
            Err(ProtocolError::InvalidLength)
        ));
        let mut short = good;
        short.pop();
        assert!(matches!(
            Response::decode(short),
            Err(ProtocolError::InvalidLength)
        ));
    }

    /// A small deterministic generator, so the test needs no extra crate and never flakes.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u8 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 33) as u8
        }
    }

    #[test]
    fn random_bytes_never_panic_the_decoders() {
        let mut rng = Lcg(42);
        for round in 0..20_000 {
            let len = (rng.next() as usize) % 64;
            let mut bytes: Vec<u8> = (0..len).map(|_| rng.next()).collect();
            // Make most inputs start with a valid tag so the field readers get exercised.
            if round % 4 != 0 && !bytes.is_empty() {
                let tags = [
                    1u8, 2, 3, 4, 5, 6, 7, 8, 129, 130, 131, 132, 133, 134, 135, 136, 255,
                ];
                bytes[0] = tags[(rng.next() as usize) % tags.len()];
            }
            let _ = Request::decode(&bytes);
            let _ = Response::decode(bytes.clone());
            let _ = read_frame(&mut Cursor::new(bytes));
        }
    }

    #[test]
    fn every_truncation_of_a_valid_message_is_an_error_not_a_panic() {
        for req in requests() {
            let bytes = req.encode();
            for cut in 0..bytes.len() {
                assert!(Request::decode(&bytes[..cut]).is_err());
            }
        }
        for res in responses() {
            let bytes = res.encode();
            for cut in 0..bytes.len() {
                assert!(Response::decode(bytes[..cut].to_vec()).is_err());
            }
        }
    }

    #[test]
    fn a_reader_can_refuse_frames_bigger_than_it_expects() {
        let mut wire = Vec::new();
        write_frame(&mut wire, &[7u8; 100]).unwrap();
        let ok = read_frame_limited(&mut Cursor::new(wire.clone()), || 100).unwrap();
        assert_eq!(ok.unwrap().len(), 100);
        let err = read_frame_limited(&mut Cursor::new(wire), || 99).unwrap_err();
        assert!(matches!(err, ProtocolError::FrameTooLarge(100)), "{err:?}");
    }

    #[test]
    fn a_bitmap_larger_than_the_limits_is_refused_when_decoding() {
        // Huge sides with no data: must be an error, not an overflow or a panic.
        let mut bytes = vec![TAG_RES_RENDERED, 1, 0, 0, 0];
        bytes.extend_from_slice(&(1u32 << 31).to_le_bytes());
        bytes.extend_from_slice(&(1u32 << 31).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        assert!(matches!(
            Response::decode(bytes),
            Err(ProtocolError::InvalidLength)
        ));
        // Consistent but over the side limit.
        let side = MAX_RENDER_SIDE + 1;
        let mut bytes = vec![TAG_RES_RENDERED, 1, 0, 0, 0];
        bytes.extend_from_slice(&side.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&(side * 4).to_le_bytes());
        bytes.extend(std::iter::repeat_n(0u8, (side * 4) as usize));
        assert!(matches!(
            Response::decode(bytes),
            Err(ProtocolError::InvalidLength)
        ));
    }

    #[test]
    fn a_bitmap_over_one_limit_only_is_refused_when_decoding() {
        let bitmap = |width: u32, height: u32| {
            let len = width as usize * height as usize * 4;
            let mut bytes = vec![TAG_RES_RENDERED, 1, 0, 0, 0];
            bytes.extend_from_slice(&width.to_le_bytes());
            bytes.extend_from_slice(&height.to_le_bytes());
            bytes.extend_from_slice(&(len as u32).to_le_bytes());
            bytes.resize(bytes.len() + len, 0);
            bytes
        };
        // Too high, but narrow: only the side limit is broken.
        assert!(matches!(
            Response::decode(bitmap(1, MAX_RENDER_SIDE + 1)),
            Err(ProtocolError::InvalidLength)
        ));
        // Both sides are allowed, but there are too many pixels: only the pixel limit is broken.
        // (This message holds 96 MB of pixels.)
        let width = MAX_RENDER_SIDE;
        let height = (MAX_RENDER_PIXELS / width as u64) as u32 + 1;
        assert!(height <= MAX_RENDER_SIDE && width as u64 * height as u64 > MAX_RENDER_PIXELS);
        assert!(matches!(
            Response::decode(bitmap(width, height)),
            Err(ProtocolError::InvalidLength)
        ));
        // One row less is fine.
        assert!(Response::decode(bitmap(width, height - 1)).is_ok());
    }

    #[test]
    fn the_encoder_cuts_what_is_too_long_and_the_result_still_decodes() {
        let long_title = "é".repeat(MAX_OUTLINE_TITLE);
        let res = Response {
            id: 1,
            body: ResponseBody::Outline {
                items: vec![
                    OutlineItem {
                        depth: 200,
                        page: Some(1),
                        title: long_title,
                    };
                    MAX_OUTLINE_ITEMS + 10
                ],
            },
        };
        let ResponseBody::Outline { items } = Response::decode(res.encode()).unwrap().body else {
            panic!("wrong body");
        };
        assert_eq!(items.len(), MAX_OUTLINE_ITEMS);
        assert!(items.iter().all(|i| i.depth == MAX_OUTLINE_DEPTH));
        assert!(items.iter().all(|i| i.title.len() <= MAX_OUTLINE_TITLE));
        assert!(items.iter().all(|i| i.title.chars().all(|c| c == 'é')));
    }

    #[test]
    fn page_text_with_a_wrong_count_is_refused() {
        let good = Response {
            id: 1,
            body: ResponseBody::PageText {
                text: "ab".into(),
                rects: vec![[0.0; 4]; 2],
            },
        }
        .encode();
        // Claim three characters while the text holds two (and there are only two boxes).
        let mut wrong = good.clone();
        wrong[5] = 3;
        assert!(matches!(
            Response::decode(wrong),
            Err(ProtocolError::InvalidLength)
        ));
        // Claim a huge count.
        let mut huge = good.clone();
        huge[5..9].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            Response::decode(huge),
            Err(ProtocolError::InvalidLength)
        ));
        // A text length that runs past the frame.
        let mut long_text = good;
        long_text[9..13].copy_from_slice(&1000u32.to_le_bytes());
        assert!(Response::decode(long_text).is_err());
    }

    #[test]
    fn page_text_is_cut_to_whole_characters_that_have_a_box() {
        let res = Response {
            id: 1,
            body: ResponseBody::PageText {
                text: "abcd".into(),
                rects: vec![[1.0; 4]; 3],
            },
        };
        let ResponseBody::PageText { text, rects } = Response::decode(res.encode()).unwrap().body
        else {
            panic!("wrong body");
        };
        assert_eq!(text, "abc");
        assert_eq!(rects.len(), 3);
    }

    #[test]
    fn numbers_that_are_not_finite_become_zero() {
        let res = Response {
            id: 1,
            body: ResponseBody::PageLinks {
                links: vec![PageLink {
                    rect: [f32::NAN, f32::INFINITY, 1.0, f32::NEG_INFINITY],
                    target: LinkTarget::Page(0),
                }],
            },
        };
        let ResponseBody::PageLinks { links } = Response::decode(res.encode()).unwrap().body else {
            panic!("wrong body");
        };
        assert_eq!(links[0].rect, [0.0, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn links_with_a_too_long_address_are_left_out() {
        let res = Response {
            id: 1,
            body: ResponseBody::PageLinks {
                links: vec![
                    PageLink {
                        rect: [0.0; 4],
                        target: LinkTarget::Uri("a".repeat(MAX_URI_LEN + 1)),
                    },
                    PageLink {
                        rect: [0.0; 4],
                        target: LinkTarget::Uri("https://example.com".into()),
                    },
                ],
            },
        };
        let ResponseBody::PageLinks { links } = Response::decode(res.encode()).unwrap().body else {
            panic!("wrong body");
        };
        assert_eq!(links.len(), 1);
    }

    #[test]
    fn counts_that_do_not_fit_the_frame_are_refused_without_allocating() {
        for tag in [
            TAG_RES_OUTLINE,
            TAG_RES_PAGE_LINKS,
            TAG_RES_MATCHES,
            TAG_RES_PAGE_TEXT,
        ] {
            let mut bytes = vec![tag, 1, 0, 0, 0];
            bytes.extend_from_slice(&1_000_000u32.to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
            assert!(Response::decode(bytes).is_err(), "tag {tag}");
        }
    }

    #[test]
    fn a_too_long_query_is_refused_and_an_unknown_link_kind_too() {
        let mut long = vec![TAG_REQ_SEARCH, 1, 0, 0, 0];
        long.extend_from_slice(&[0u8; 8]);
        long.extend_from_slice(&((MAX_QUERY_LEN + 1) as u32).to_le_bytes());
        assert!(matches!(
            Request::decode(&long),
            Err(ProtocolError::StringTooLong(_))
        ));
        let mut bad_kind = vec![TAG_RES_PAGE_LINKS, 1, 0, 0, 0];
        bad_kind.extend_from_slice(&1u32.to_le_bytes());
        bad_kind.extend_from_slice(&[0u8; 16]);
        bad_kind.push(9);
        bad_kind.extend_from_slice(&[0u8; 8]);
        assert!(matches!(
            Response::decode(bad_kind),
            Err(ProtocolError::UnknownValue(9))
        ));
    }

    #[test]
    fn error_codes_round_trip_and_have_names() {
        for v in 1..=20u8 {
            let code = ErrorCode::from_u8(v).unwrap();
            assert_eq!(code as u8, v);
            assert!(!code.name().is_empty());
        }
        assert!(ErrorCode::from_u8(0).is_none());
        assert!(ErrorCode::from_u8(21).is_none());
        assert_eq!(ErrorCode::from_u8(16).unwrap().name(), "source_changed");
        assert_eq!(ErrorCode::from_u8(17).unwrap().name(), "source_unreadable");
        assert_eq!(ErrorCode::from_u8(14).unwrap().name(), "save_failed");
        assert_eq!(ErrorCode::from_u8(15).unwrap().name(), "save_too_large");
    }
}

/// The scripts for which the app can hand the engine a system font. PDFium asks for a font by
/// character set, and these are the character sets it can ask for that the fonts built into PDFium
/// do not cover. The app chooses the files; the PDF only decides which script it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FontScript {
    Japanese,
    Korean,
    ChineseSimplified,
    ChineseTraditional,
    Arabic,
    Hebrew,
    Thai,
}

impl FontScript {
    /// Every script, in a fixed order.
    pub const ALL: [FontScript; 7] = [
        FontScript::Japanese,
        FontScript::Korean,
        FontScript::ChineseSimplified,
        FontScript::ChineseTraditional,
        FontScript::Arabic,
        FontScript::Hebrew,
        FontScript::Thai,
    ];

    /// A short stable name, used on the engine's command line and in the window.
    pub fn name(self) -> &'static str {
        match self {
            FontScript::Japanese => "japanese",
            FontScript::Korean => "korean",
            FontScript::ChineseSimplified => "chinese-simplified",
            FontScript::ChineseTraditional => "chinese-traditional",
            FontScript::Arabic => "arabic",
            FontScript::Hebrew => "hebrew",
            FontScript::Thai => "thai",
        }
    }

    pub fn from_name(name: &str) -> Option<FontScript> {
        FontScript::ALL.into_iter().find(|s| s.name() == name)
    }

    /// The number of the script on the wire: its place in [`FontScript::ALL`].
    pub fn code(self) -> u8 {
        FontScript::ALL
            .iter()
            .position(|s| *s == self)
            .expect("every script is in the list") as u8
    }

    pub fn from_code(code: u8) -> Option<FontScript> {
        FontScript::ALL.get(code as usize).copied()
    }
}

/// How many bytes of a font file the check needs to see: the table directory of a font with up to
/// 128 tables, or of a collection whose first font is near the start.
pub const FONT_HEAD_BYTES: usize = 8 * 1024;

/// Most tables in the directory of a font that the check accepts. Real fonts have 10 to 30.
const MAX_FONT_TABLES: usize = 128;

/// Checks that `head` (the first [`FONT_HEAD_BYTES`] bytes, or the whole file if it is shorter) and
/// the length of the file are those of an intact TrueType or OpenType font, or of a collection
/// whose first font is intact: the table directory is there, every table lies inside the file, and
/// the tables a font needs to draw text (`cmap` and outlines) are present. This is cheap and
/// catches the damage that is common in practice (a file cut short, a download that stopped, a
/// file that is not a font at all); it does not make a damaged file safe, which only the sandbox
/// does.
pub fn check_font(head: &[u8], file_len: u64) -> Result<(), &'static str> {
    fn u16_at(data: &[u8], at: usize) -> Option<usize> {
        data.get(at..at + 2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]) as usize)
    }
    fn u32_at(data: &[u8], at: usize) -> Option<u64> {
        data.get(at..at + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64)
    }
    let tag = head
        .get(..4)
        .ok_or("the file is shorter than a font header")?;
    let start = if tag == b"ttcf" {
        let fonts = u32_at(head, 8).ok_or("the collection header is cut off")?;
        if fonts == 0 {
            return Err("the collection has no font");
        }
        let first = u32_at(head, 12).ok_or("the collection header is cut off")?;
        if first + 12 > file_len {
            return Err("the first font of the collection lies outside the file");
        }
        first as usize
    } else if tag == [0, 1, 0, 0] || tag == b"OTTO" || tag == b"true" {
        0
    } else {
        return Err("the file is not a TrueType or OpenType font");
    };
    let tables = u16_at(head, start + 4).ok_or("the table directory is cut off")?;
    if tables == 0 || tables > MAX_FONT_TABLES {
        return Err("the number of tables is not that of a font");
    }
    let (mut cmap, mut outlines, mut head_table) = (false, false, false);
    for i in 0..tables {
        let at = start + 12 + 16 * i;
        let record = head
            .get(at..at + 16)
            .ok_or("the table directory is cut off")?;
        let offset = u32::from_be_bytes([record[8], record[9], record[10], record[11]]) as u64;
        let length = u32::from_be_bytes([record[12], record[13], record[14], record[15]]) as u64;
        if offset + length > file_len {
            return Err("a table lies outside the file (the file is cut short)");
        }
        match &record[..4] {
            b"cmap" => cmap = true,
            b"head" => head_table = true,
            b"glyf" | b"CFF " | b"CFF2" => outlines = true,
            _ => {}
        }
    }
    if !cmap || !outlines || !head_table {
        return Err("the font has no character map, no outlines or no header table");
    }
    Ok(())
}

/// Where a table of the first font lies, as (offset, length) from the start of the file, read from
/// the table directory in `head`. `None` when the directory is not there or has no such table.
pub fn font_table(head: &[u8], tag: &[u8; 4]) -> Option<(u64, u64)> {
    let start = if head.get(..4)? == b"ttcf" {
        u32::from_be_bytes(head.get(12..16)?.try_into().ok()?) as usize
    } else {
        0
    };
    let tables = u16::from_be_bytes(head.get(start + 4..start + 6)?.try_into().ok()?) as usize;
    (0..tables.min(MAX_FONT_TABLES)).find_map(|i| {
        let at = start + 12 + 16 * i;
        let record = head.get(at..at + 16)?;
        (&record[..4] == tag).then(|| {
            (
                u32::from_be_bytes([record[8], record[9], record[10], record[11]]) as u64,
                u32::from_be_bytes([record[12], record[13], record[14], record[15]]) as u64,
            )
        })
    })
}

/// How many bytes of the `head` table [`check_head_table`] needs.
pub const FONT_HEAD_TABLE_BYTES: usize = 54;

/// Checks the `head` table of a font: its length, the magic number every intact one has
/// (0x5F0F3CF5) and a sane units-per-em. A font whose tables were overwritten fails here, which
/// the table directory alone does not show, and FreeType refuses it too.
pub fn check_head_table(table: &[u8]) -> Result<(), &'static str> {
    if table.len() < FONT_HEAD_TABLE_BYTES {
        return Err("the header table is too short");
    }
    if table[12..16] != [0x5F, 0x0F, 0x3C, 0xF5] {
        return Err("the header table has the wrong magic number");
    }
    let units_per_em = u16::from_be_bytes([table[18], table[19]]);
    if !(16..=16384).contains(&units_per_em) {
        return Err("the header table has a units-per-em that is not that of a font");
    }
    Ok(())
}

/// [`check_font`] and [`check_head_table`] on a font file held whole in memory.
pub fn check_font_data(data: &[u8]) -> Result<(), &'static str> {
    let head = &data[..data.len().min(FONT_HEAD_BYTES)];
    check_font(head, data.len() as u64)?;
    let (offset, length) = font_table(head, b"head").ok_or("the font has no header table")?;
    let table = usize::try_from(offset)
        .ok()
        .and_then(|o| data.get(o..o.checked_add(usize::try_from(length).ok()?)?))
        .ok_or("the header table lies outside the file")?;
    check_head_table(table)
}

#[cfg(test)]
mod font_script_tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for script in FontScript::ALL {
            assert_eq!(FontScript::from_code(script.code()), Some(script));
        }
        assert_eq!(FontScript::from_code(7), None);
    }

    /// A font header with the given tables, each (tag, offset, length). With `start` above zero it
    /// is the first font of a collection and sits at that offset.
    fn header(start: usize, tables: &[(&[u8; 4], u32, u32)]) -> Vec<u8> {
        let mut out = vec![0u8; start];
        if start > 0 {
            out[..4].copy_from_slice(b"ttcf");
            out[8..12].copy_from_slice(&1u32.to_be_bytes());
            out[12..16].copy_from_slice(&(start as u32).to_be_bytes());
        }
        out.extend_from_slice(&[0, 1, 0, 0]);
        out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
        out.extend_from_slice(&[0; 6]);
        for (tag, offset, length) in tables {
            out.extend_from_slice(*tag);
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&offset.to_be_bytes());
            out.extend_from_slice(&length.to_be_bytes());
        }
        out
    }

    #[test]
    fn an_intact_font_header_passes_and_a_cut_file_does_not() {
        let tables: [(&[u8; 4], u32, u32); 3] =
            [(b"cmap", 100, 50), (b"glyf", 150, 800), (b"head", 950, 50)];
        let ok = header(0, &tables);
        assert_eq!(check_font(&ok, 1000), Ok(()));
        // The same header on a file that was cut short.
        assert!(check_font(&ok, 600).is_err());
        // A collection whose first font is near the start.
        assert_eq!(check_font(&header(16, &tables), 1000), Ok(()));
        assert!(check_font(&header(16, &tables), 400).is_err());
    }

    #[test]
    fn the_header_table_must_be_a_real_one() {
        let mut head = vec![0u8; 54];
        head[12..16].copy_from_slice(&[0x5F, 0x0F, 0x3C, 0xF5]);
        head[18..20].copy_from_slice(&1000u16.to_be_bytes());
        assert_eq!(check_head_table(&head), Ok(()));
        assert!(check_head_table(&head[..53]).is_err());
        head[12] = 0;
        assert!(check_head_table(&head).is_err());
        head[12] = 0x5F;
        head[18..20].copy_from_slice(&0u16.to_be_bytes());
        assert!(check_head_table(&head).is_err());
    }

    #[test]
    fn a_whole_font_is_checked_down_to_its_header_table() {
        // cmap and glyf at 100 and 104, head at 108 (54 bytes), 162 bytes in all.
        let tables: [(&[u8; 4], u32, u32); 3] =
            [(b"cmap", 100, 4), (b"glyf", 104, 4), (b"head", 108, 54)];
        let mut font = header(0, &tables);
        font.resize(162, 0);
        font[108 + 12..108 + 16].copy_from_slice(&[0x5F, 0x0F, 0x3C, 0xF5]);
        font[108 + 18..108 + 20].copy_from_slice(&2048u16.to_be_bytes());
        assert_eq!(check_font_data(&font), Ok(()));
        assert_eq!(font_table(&font, b"head"), Some((108, 54)));
        assert_eq!(font_table(&font, b"nope"), None);
        // The directory is the same, the contents are not.
        font[108..162].fill(0xA5);
        assert!(check_font_data(&font).is_err());
        assert!(check_font_data(&font[..120]).is_err());
    }

    #[test]
    fn what_is_not_a_font_does_not_pass() {
        assert!(check_font(b"", 0).is_err());
        assert!(check_font(b"%PDF-1.7 not a font at all......", 100).is_err());
        assert!(check_font(&[0; 64], 64).is_err());
        // No outlines, or no character map.
        assert!(check_font(&header(0, &[(b"cmap", 50, 10), (b"head", 60, 54)]), 1000).is_err());
        assert!(check_font(&header(0, &[(b"glyf", 50, 10), (b"head", 60, 54)]), 1000).is_err());
        assert!(check_font(&header(0, &[(b"cmap", 50, 10), (b"glyf", 60, 10)]), 1000).is_err());
        // Wildly many tables.
        let mut many = header(0, &[]);
        many[4..6].copy_from_slice(&60_000u16.to_be_bytes());
        assert!(check_font(&many, 1000).is_err());
    }

    #[test]
    fn names_round_trip() {
        for script in FontScript::ALL {
            assert_eq!(FontScript::from_name(script.name()), Some(script));
        }
        assert_eq!(FontScript::from_name("latin"), None);
    }
}

#[cfg(test)]
mod tags {
    use super::*;

    /// The two lines of development that were merged each gave out tags and error codes, and no
    /// tag may stand for two messages: the decoders tell messages apart by it alone.
    #[test]
    fn no_tag_or_error_code_stands_for_two_things() {
        let requests: Vec<u8> = vec![
            TAG_REQ_HELLO,
            TAG_REQ_OPEN,
            TAG_REQ_RENDER,
            TAG_REQ_CLOSE,
            TAG_REQ_OUTLINE,
            TAG_REQ_PAGE_TEXT,
            TAG_REQ_PAGE_LINKS,
            TAG_REQ_SEARCH,
            TAG_REQ_FONT_REPORT,
            TAG_REQ_DOC_INFO,
            TAG_REQ_SAVE_BEGIN,
            TAG_REQ_SAVE_CHUNK,
            TAG_REQ_SAVE_END,
            TAG_REQ_FINGERPRINT,
            TAG_REQ_SIGNATURES,
            TAG_REQ_FORM_INFO,
            TAG_REQ_LIST_FORM_FIELDS,
            TAG_REQ_SET_FORM_FIELD,
            TAG_REQ_LIST_ANNOTS,
            TAG_REQ_ADD_MARKUP,
            TAG_REQ_ADD_NOTE,
            TAG_REQ_ADD_INK,
            TAG_REQ_ADD_STAMP,
            TAG_REQ_UPDATE_ANNOT,
            TAG_REQ_REMOVE_ANNOT,
            TAG_REQ_RESTORE_ANNOT,
        ];
        let responses: Vec<u8> = vec![
            TAG_RES_HELLO,
            TAG_RES_OPENED,
            TAG_RES_RENDERED,
            TAG_RES_CLOSED,
            TAG_RES_OUTLINE,
            TAG_RES_PAGE_TEXT,
            TAG_RES_PAGE_LINKS,
            TAG_RES_MATCHES,
            TAG_RES_FONT_REPORT,
            TAG_RES_DOC_INFO,
            TAG_RES_SAVE_READY,
            TAG_RES_SAVE_DATA,
            TAG_RES_SAVE_ENDED,
            TAG_RES_PROGRESS,
            TAG_RES_FINGERPRINT,
            TAG_RES_SIGNATURES,
            TAG_RES_FORM_INFO,
            TAG_RES_FORM_FIELDS,
            TAG_RES_ANNOTS,
            TAG_RES_ANNOT,
            TAG_RES_ANNOT_REMOVED,
            TAG_RES_ERROR,
        ];
        let mut seen = std::collections::HashSet::new();
        for tag in &requests {
            assert!(seen.insert(*tag), "request tag {tag} is used twice");
            assert!(
                !(pages::FIRST_REQUEST_TAG..=pages::LAST_REQUEST_TAG).contains(tag),
                "request tag {tag} is in the range of the page tools"
            );
        }
        let mut seen = std::collections::HashSet::new();
        for tag in &responses {
            assert!(seen.insert(*tag), "answer tag {tag} is used twice");
            assert!(
                !(pages::FIRST_RESPONSE_TAG..=pages::LAST_RESPONSE_TAG).contains(tag),
                "answer tag {tag} is in the range of the page tools"
            );
        }
        // Every error code reads back as itself and has a name of its own.
        let mut names = std::collections::HashSet::new();
        for raw in 0..=255u8 {
            if let Some(code) = ErrorCode::from_u8(raw) {
                assert_eq!(code as u8, raw);
                assert!(names.insert(code.name()), "two error codes are called {}", code.name());
            }
        }
        // The codes of the annotations and forms, and those of the page tools, are all there.
        for raw in [18u8, 19, 20, 24, 25, 50, 51, 52, 53, 54, 55, 56] {
            assert!(ErrorCode::from_u8(raw).is_some(), "error code {raw} is missing");
        }
        assert_eq!(PROTOCOL_VERSION, 9);
    }
}
