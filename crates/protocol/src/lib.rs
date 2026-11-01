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

/// Version of this protocol. Both sides check it in the `Hello` exchange.
pub const PROTOCOL_VERSION: u32 = 3;

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

const TAG_RES_HELLO: u8 = 129;
const TAG_RES_OPENED: u8 = 130;
const TAG_RES_RENDERED: u8 = 131;
const TAG_RES_CLOSED: u8 = 132;
const TAG_RES_OUTLINE: u8 = 133;
const TAG_RES_PAGE_TEXT: u8 = 134;
const TAG_RES_PAGE_LINKS: u8 = 135;
const TAG_RES_MATCHES: u8 = 136;
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
            other => return Err(ProtocolError::UnknownTag(other)),
        };
        r.finish()?;
        Ok(Request { id, body })
    }
}

/// Number of bytes of a `Rendered` payload before the pixels: tag, id, width, height, pixel length.
const RENDERED_HEADER_LEN: usize = 1 + 4 + 4 + 4 + 4;

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
        ]
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
        for v in 1..=13u8 {
            let code = ErrorCode::from_u8(v).unwrap();
            assert_eq!(code as u8, v);
            assert!(!code.name().is_empty());
        }
        assert!(ErrorCode::from_u8(0).is_none());
        assert!(ErrorCode::from_u8(14).is_none());
    }
}
