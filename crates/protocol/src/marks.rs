//! The picture of a signature on the wire: the three ways to make one, and the caps on each.
//!
//! A picture is drawn lines, typed text or an imported image. The window makes it and sends
//! numbers, a short text or raw pixels; the engine draws it on the page as a stamp. The caps below
//! are held by the encoder and by the decoder, on both sides of the connection.
//!
//! A picture of a signature is only a picture. It says nothing about who placed it.

use super::{put_f32, put_str_max, put_u32, ProtocolError, Reader};

/// Most strokes in the drawn picture of a signature.
pub const MAX_MARK_STROKES: usize = 64;

/// Most points in the drawn picture of a signature, all strokes together.
pub const MAX_MARK_POINTS: usize = 4_096;

/// Largest side of the box a drawn picture is made in, in the units of the box. A point may be
/// at most twice that far from the origin.
pub const MAX_MARK_SPACE: f32 = 4_096.0;

/// Thinnest and thickest line of a drawn picture, in the units of its box.
pub const MIN_MARK_LINE: f32 = 0.1;
pub const MAX_MARK_LINE: f32 = 256.0;

/// Most characters of a typed signature.
pub const MAX_MARK_TEXT_CHARS: usize = 40;

/// Most bytes of a typed signature (a character is at most four bytes).
pub const MAX_MARK_TEXT_BYTES: usize = MAX_MARK_TEXT_CHARS * 4;

/// Largest side of an imported picture, in pixels, and most pixels in all. The window scales a
/// bigger picture down before it sends it.
pub const MAX_MARK_IMAGE_SIDE: u32 = 1_024;
pub const MAX_MARK_IMAGE_PIXELS: u32 = 1_024 * 512;

/// Smallest side of a signature picture on a page, in points. The engine refuses a smaller box.
pub const MIN_SIGNATURE_SIDE: f32 = 8.0;

/// The fonts a typed signature can be set in. They are bundled with the engine, so the window
/// names one and never sends font data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SignatureFont {
    GreatVibes = 1,
    Allura = 2,
    Kristi = 3,
}

impl SignatureFont {
    pub const ALL: [SignatureFont; 3] = [
        SignatureFont::GreatVibes,
        SignatureFont::Allura,
        SignatureFont::Kristi,
    ];

    pub fn from_u8(v: u8) -> Result<SignatureFont, ProtocolError> {
        SignatureFont::ALL
            .iter()
            .copied()
            .find(|f| *f as u8 == v)
            .ok_or(ProtocolError::UnknownValue(v))
    }

    /// A short stable name.
    pub fn name(self) -> &'static str {
        match self {
            SignatureFont::GreatVibes => "great-vibes",
            SignatureFont::Allura => "allura",
            SignatureFont::Kristi => "kristi",
        }
    }

    /// The font a name stands for.
    pub fn from_name(name: &str) -> Option<SignatureFont> {
        SignatureFont::ALL
            .iter()
            .copied()
            .find(|f| f.name() == name)
    }
}

/// The picture of a signature, made one of three ways.
#[derive(Debug, Clone, PartialEq)]
pub enum SignatureMark {
    /// A drawn signature: lines in a box of `width` by `height` units, points counted from the
    /// top left, drawn with a line of `line` units.
    Strokes {
        width: f32,
        height: f32,
        line: f32,
        strokes: Vec<Vec<[f32; 2]>>,
    },
    /// A typed signature: `text` set in one of the bundled fonts.
    Typed { font: SignatureFont, text: String },
    /// An imported picture: raw pixels, red, green, blue and alpha, row by row from the top.
    Image {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
}

const MARK_STROKES: u8 = 1;
const MARK_TYPED: u8 = 2;
const MARK_IMAGE: u8 = 3;

impl SignatureMark {
    /// Near enough the bytes this picture takes in memory, to keep what is remembered in bounds.
    pub fn weight(&self) -> usize {
        match self {
            SignatureMark::Strokes { strokes, .. } => {
                strokes.iter().map(|s| s.len() * 8 + 24).sum::<usize>() + 64
            }
            SignatureMark::Typed { text, .. } => text.len() + 64,
            SignatureMark::Image { rgba, .. } => rgba.len() + 64,
        }
    }

    /// True when the picture is within every cap, that is, when the decoder would take it.
    pub fn within_caps(&self) -> bool {
        match self {
            SignatureMark::Strokes {
                width,
                height,
                line,
                strokes,
            } => {
                let points: usize = strokes.iter().map(Vec::len).sum();
                valid_box(*width, *height, *line)
                    && !strokes.is_empty()
                    && strokes.len() <= MAX_MARK_STROKES
                    && points <= MAX_MARK_POINTS
                    && strokes.iter().all(|s| {
                        !s.is_empty() && s.iter().all(|p| in_space(p[0]) && in_space(p[1]))
                    })
            }
            SignatureMark::Typed { text, .. } => text_ok(text),
            SignatureMark::Image {
                width,
                height,
                rgba,
            } => image_ok(*width, *height) && rgba.len() == *width as usize * *height as usize * 4,
        }
    }
}

/// True for a character a typed signature may hold: not a control character, and nothing that
/// changes the direction of the text or hides.
pub fn mark_char_ok(c: char) -> bool {
    !c.is_control()
        && !matches!(
            c,
            '\u{200E}'
                | '\u{200F}'
                | '\u{061C}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
                | '\u{200B}'
                | '\u{FEFF}'
                | '\u{2028}'
                | '\u{2029}'
        )
}

/// True for the text of a typed signature: one to forty characters, none of them a control or
/// direction character, and not only spaces.
pub fn mark_text_ok(text: &str) -> bool {
    text_ok(text)
}

/// True for the size of an imported picture, in pixels.
pub fn mark_image_size_ok(width: u32, height: u32) -> bool {
    image_ok(width, height)
}

fn text_ok(text: &str) -> bool {
    let chars = text.chars().count();
    chars > 0
        && chars <= MAX_MARK_TEXT_CHARS
        && text.len() <= MAX_MARK_TEXT_BYTES
        && text.chars().all(mark_char_ok)
        && text.chars().any(|c| !c.is_whitespace())
}

fn image_ok(width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && width <= MAX_MARK_IMAGE_SIDE
        && height <= MAX_MARK_IMAGE_SIDE
        && width * height <= MAX_MARK_IMAGE_PIXELS
}

fn valid_box(width: f32, height: f32, line: f32) -> bool {
    width.is_finite()
        && width > 0.0
        && width <= MAX_MARK_SPACE
        && height.is_finite()
        && height > 0.0
        && height <= MAX_MARK_SPACE
        && (MIN_MARK_LINE..=MAX_MARK_LINE).contains(&line)
}

fn in_space(v: f32) -> bool {
    v.is_finite() && v.abs() <= MAX_MARK_SPACE * 2.0
}

pub(crate) fn put_mark(out: &mut Vec<u8>, mark: &SignatureMark) {
    match mark {
        SignatureMark::Strokes {
            width,
            height,
            line,
            strokes,
        } => {
            out.push(MARK_STROKES);
            put_f32(out, *width);
            put_f32(out, *height);
            put_f32(out, *line);
            // Whatever is over the caps is left out, so that what is written always decodes.
            let mut kept: Vec<&Vec<[f32; 2]>> = Vec::new();
            let mut points = 0usize;
            for stroke in strokes.iter().filter(|s| !s.is_empty()) {
                if kept.len() >= MAX_MARK_STROKES || points + stroke.len() > MAX_MARK_POINTS {
                    break;
                }
                points += stroke.len();
                kept.push(stroke);
            }
            put_u32(out, kept.len() as u32);
            for stroke in kept {
                put_u32(out, stroke.len() as u32);
                for point in stroke {
                    put_f32(out, point[0]);
                    put_f32(out, point[1]);
                }
            }
        }
        SignatureMark::Typed { font, text } => {
            out.push(MARK_TYPED);
            out.push(*font as u8);
            put_str_max(out, text, MAX_MARK_TEXT_BYTES);
        }
        SignatureMark::Image {
            width,
            height,
            rgba,
        } => {
            out.push(MARK_IMAGE);
            put_u32(out, *width);
            put_u32(out, *height);
            out.extend_from_slice(rgba);
        }
    }
}

pub(crate) fn read_mark(r: &mut Reader<'_>) -> Result<SignatureMark, ProtocolError> {
    match r.u8()? {
        MARK_STROKES => {
            let width = r.f32()?;
            let height = r.f32()?;
            let line = r.f32()?;
            if !valid_box(width, height, line) {
                return Err(ProtocolError::InvalidLength);
            }
            let count = r.u32()? as usize;
            // A stroke takes at least 12 bytes: its length and one point.
            if count == 0 || count > MAX_MARK_STROKES || count > r.remaining() / 12 {
                return Err(ProtocolError::InvalidLength);
            }
            let mut strokes = Vec::with_capacity(count);
            let mut total = 0usize;
            for _ in 0..count {
                let n = r.u32()? as usize;
                if n == 0
                    || n > MAX_MARK_POINTS - total.min(MAX_MARK_POINTS)
                    || n > r.remaining() / 8
                {
                    return Err(ProtocolError::InvalidLength);
                }
                total += n;
                let mut stroke = Vec::with_capacity(n);
                for _ in 0..n {
                    let (x, y) = (r.f32()?, r.f32()?);
                    if !in_space(x) || !in_space(y) {
                        return Err(ProtocolError::InvalidLength);
                    }
                    stroke.push([x, y]);
                }
                strokes.push(stroke);
            }
            Ok(SignatureMark::Strokes {
                width,
                height,
                line,
                strokes,
            })
        }
        MARK_TYPED => {
            let font = SignatureFont::from_u8(r.u8()?)?;
            let text = r.string_max(MAX_MARK_TEXT_BYTES)?;
            if !text_ok(&text) {
                return Err(ProtocolError::InvalidLength);
            }
            Ok(SignatureMark::Typed { font, text })
        }
        MARK_IMAGE => {
            let width = r.u32()?;
            let height = r.u32()?;
            if !image_ok(width, height) {
                return Err(ProtocolError::InvalidLength);
            }
            // The size is checked against the bytes that are there before anything is copied.
            let rgba = r.take(width as usize * height as usize * 4)?.to_vec();
            Ok(SignatureMark::Image {
                width,
                height,
                rgba,
            })
        }
        other => Err(ProtocolError::UnknownValue(other)),
    }
}
