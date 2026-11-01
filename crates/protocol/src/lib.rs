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
pub const PROTOCOL_VERSION: u32 = 1;

/// Largest frame (payload) either side accepts: 128 MiB, enough for the biggest allowed bitmap.
pub const MAX_FRAME_LEN: usize = 128 * 1024 * 1024;

/// Longest string either side accepts, in bytes.
pub const MAX_STRING_LEN: usize = 64 * 1024;

/// Largest bitmap the engine renders, in pixels (width times height).
pub const MAX_RENDER_PIXELS: u64 = 24_000_000;

/// Largest bitmap side the engine renders, in pixels.
pub const MAX_RENDER_SIDE: u32 = 16_384;

const TAG_REQ_HELLO: u8 = 1;
const TAG_REQ_OPEN: u8 = 2;
const TAG_REQ_RENDER: u8 = 3;
const TAG_REQ_CLOSE: u8 = 4;

const TAG_RES_HELLO: u8 = 129;
const TAG_RES_OPENED: u8 = 130;
const TAG_RES_RENDERED: u8 = 131;
const TAG_RES_CLOSED: u8 = 132;
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
    /// Open a PDF from a path on disk.
    Open {
        path: String,
        password: Option<String>,
    },
    /// Draw one page into a bitmap of exactly `width` by `height` pixels.
    Render {
        doc: u32,
        page: u32,
        width: u32,
        height: u32,
    },
    /// Forget a document and free its memory.
    Close { doc: u32 },
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
            RequestBody::Open { path, password } => {
                out.push(TAG_REQ_OPEN);
                put_u32(&mut out, self.id);
                put_str(&mut out, path);
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
                path: r.string()?,
                password: r.opt_string()?,
            },
            TAG_REQ_RENDER => RequestBody::Render {
                doc: r.u32()?,
                page: r.u32()?,
                width: r.u32()?,
                height: r.u32()?,
            },
            TAG_REQ_CLOSE => RequestBody::Close { doc: r.u32()? },
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
            } => {
                out.push(TAG_RES_HELLO);
                put_u32(&mut out, self.id);
                put_u32(&mut out, *protocol_version);
                put_str(&mut out, engine_version);
                put_str(&mut out, pdfium_version);
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

    fn string(&mut self) -> Result<String, ProtocolError> {
        let len = self.u32()? as usize;
        if len > MAX_STRING_LEN {
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
    // Do not trust the announced length for the allocation: grow while data actually arrives.
    let mut payload = Vec::with_capacity(len.min(64 * 1024));
    let read = r.take(len as u64).read_to_end(&mut payload)?;
    if read != len {
        return Err(ProtocolError::Truncated);
    }
    Ok(Some(payload))
}

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
                body: RequestBody::Open {
                    path: "C:\\Users\\Ægir\\bok 📕.pdf".into(),
                    password: None,
                },
            },
            Request {
                id: 3,
                body: RequestBody::Open {
                    path: "/tmp/a.pdf".into(),
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
            body: RequestBody::Open {
                path: String::new(),
                password: None,
            },
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
        let mut bytes = vec![TAG_REQ_OPEN, 1, 0, 0, 0];
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&[0xff, 0xfe]);
        bytes.push(0);
        assert!(matches!(
            Request::decode(&bytes),
            Err(ProtocolError::InvalidUtf8)
        ));

        let mut long = vec![TAG_REQ_OPEN, 1, 0, 0, 0];
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
                path: "é".repeat(MAX_STRING_LEN),
                password: None,
            },
        };
        let Request {
            body: RequestBody::Open { path, .. },
            ..
        } = Request::decode(&req.encode()).unwrap()
        else {
            panic!("wrong body");
        };
        assert!(path.len() <= MAX_STRING_LEN);
        assert!(path.chars().all(|c| c == 'é'));
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
                let tags = [1u8, 2, 3, 4, 129, 130, 131, 132, 255];
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
    fn error_codes_round_trip_and_have_names() {
        for v in 1..=12u8 {
            let code = ErrorCode::from_u8(v).unwrap();
            assert_eq!(code as u8, v);
            assert!(!code.name().is_empty());
        }
        assert!(ErrorCode::from_u8(0).is_none());
        assert!(ErrorCode::from_u8(13).is_none());
    }
}
