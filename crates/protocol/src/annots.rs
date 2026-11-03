//! Annotations on the wire: the kinds, the limits and the pieces of the messages that are more than
//! a few numbers.
//!
//! Everything an annotation carries that came from a PDF (its text, its author, its box) is
//! untrusted. The caps below are held by the encoder and by the decoder, on both sides of the
//! connection, so that neither a damaged file nor a damaged engine can make the app hold, or draw,
//! more than a fixed amount.

use super::{put_f32, put_str_max, put_u32, ProtocolError, Reader, MAX_PAGE_CHARS};

/// Most annotations the engine lists for one page. Pages with more show the first ones.
pub const MAX_ANNOTS_PER_PAGE: usize = 500;

/// Longest text of an annotation (a note or a comment), in bytes. Longer text is cut on a
/// character boundary when listed; a request with longer text is refused.
pub const MAX_ANNOT_TEXT: usize = 4 * 1024;

/// Longest author name of an annotation, in bytes.
pub const MAX_ANNOT_AUTHOR: usize = 256;

/// Most characters one markup (highlight, underline, strike-out) may cover.
pub const MAX_MARKUP_CHARS: usize = MAX_PAGE_CHARS;

/// Most strokes in one drawing.
pub const MAX_INK_STROKES: usize = 256;

/// Most points in one drawing, all strokes together.
pub const MAX_INK_POINTS: usize = 20_000;

/// Thinnest and thickest line of a drawing, in points.
pub const MIN_INK_WIDTH: f32 = 0.25;
pub const MAX_INK_WIDTH: f32 = 72.0;

/// Largest value of a coordinate in a request, in points. A page is at most 14 400 points wide.
pub const MAX_COORDINATE: f32 = 1_000_000.0;

/// The fewest bytes one listed annotation takes: id, kind, abilities, box, colour and the length
/// of the two strings.
pub const ANNOT_INFO_MIN: usize = 4 + 1 + 1 + 16 + 4 + 4 + 4;

/// What an annotation is, as far as the reader is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AnnotKind {
    Highlight = 1,
    Underline = 2,
    Strikeout = 3,
    /// A sticky note.
    Note = 4,
    /// A free drawing.
    Ink = 5,
    Stamp = 6,
    /// Something else the reader shows and can only remove: a squiggly underline, a text box, a
    /// square or a circle.
    Other = 7,
    /// A picture of a signature that this reader placed in the session: a stamp that can be
    /// moved and resized. Once the file is saved and opened again it is an ordinary stamp.
    Signature = 8,
}

impl AnnotKind {
    pub fn from_u8(v: u8) -> Result<AnnotKind, ProtocolError> {
        Ok(match v {
            1 => AnnotKind::Highlight,
            2 => AnnotKind::Underline,
            3 => AnnotKind::Strikeout,
            4 => AnnotKind::Note,
            5 => AnnotKind::Ink,
            6 => AnnotKind::Stamp,
            7 => AnnotKind::Other,
            8 => AnnotKind::Signature,
            other => return Err(ProtocolError::UnknownValue(other)),
        })
    }

    /// The kind of a request for markup: only highlight, underline and strike-out are markup.
    pub fn markup_from(v: u8) -> Result<AnnotKind, ProtocolError> {
        match AnnotKind::from_u8(v)? {
            kind @ (AnnotKind::Highlight | AnnotKind::Underline | AnnotKind::Strikeout) => Ok(kind),
            _ => Err(ProtocolError::UnknownValue(v)),
        }
    }

    /// A short stable name.
    pub fn name(self) -> &'static str {
        match self {
            AnnotKind::Highlight => "highlight",
            AnnotKind::Underline => "underline",
            AnnotKind::Strikeout => "strikeout",
            AnnotKind::Note => "note",
            AnnotKind::Ink => "ink",
            AnnotKind::Stamp => "stamp",
            AnnotKind::Other => "other",
            AnnotKind::Signature => "signature",
        }
    }
}

/// The stamps the reader can put on a page. The wording is fixed, so a stamp carries no text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StampKind {
    Approved = 1,
    Rejected = 2,
    Draft = 3,
    Final = 4,
    Confidential = 5,
    Reviewed = 6,
}

impl StampKind {
    pub const ALL: [StampKind; 6] = [
        StampKind::Approved,
        StampKind::Rejected,
        StampKind::Draft,
        StampKind::Final,
        StampKind::Confidential,
        StampKind::Reviewed,
    ];

    pub fn from_u8(v: u8) -> Result<StampKind, ProtocolError> {
        StampKind::ALL
            .iter()
            .copied()
            .find(|s| *s as u8 == v)
            .ok_or(ProtocolError::UnknownValue(v))
    }

    /// The words on the stamp. Plain ASCII, drawn in a standard font.
    pub fn label(self) -> &'static str {
        match self {
            StampKind::Approved => "APPROVED",
            StampKind::Rejected => "REJECTED",
            StampKind::Draft => "DRAFT",
            StampKind::Final => "FINAL",
            StampKind::Confidential => "CONFIDENTIAL",
            StampKind::Reviewed => "REVIEWED",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            StampKind::Approved => "approved",
            StampKind::Rejected => "rejected",
            StampKind::Draft => "draft",
            StampKind::Final => "final",
            StampKind::Confidential => "confidential",
            StampKind::Reviewed => "reviewed",
        }
    }
}

/// What the reader may do with an annotation (the bits of [`AnnotInfo::abilities`]).
pub mod ability {
    /// The text (the note, or the comment of a markup) can be changed.
    pub const EDIT_TEXT: u8 = 1;
    /// The colour can be changed.
    pub const RECOLOUR: u8 = 2;
    /// The annotation can be moved.
    pub const MOVE: u8 = 4;
    /// The annotation can be removed.
    pub const REMOVE: u8 = 8;
    /// The annotation can be put back after it was removed.
    pub const RESTORE: u8 = 16;
    /// The file locks the annotation (its flags say read-only or locked), or the document does not
    /// allow annotations to be changed. No other bit is set then, except possibly none.
    pub const LOCKED: u8 = 32;
    /// The annotation can be made bigger or smaller (a signature picture).
    pub const RESIZE: u8 = 64;
    /// All the bits that mean something.
    pub const ALL: u8 = EDIT_TEXT | RECOLOUR | MOVE | REMOVE | RESTORE | LOCKED | RESIZE;
}

/// One annotation of a page.
#[derive(Debug, Clone, PartialEq)]
pub struct AnnotInfo {
    /// Names the annotation on its page for as long as the document is open. Never reused.
    pub id: u32,
    pub kind: AnnotKind,
    /// A sum of the bits of [`ability`].
    pub abilities: u8,
    /// `[left, top, right, bottom]` in points from the top left of the page as it is drawn.
    pub rect: [f32; 4],
    /// Red, green, blue and alpha.
    pub color: [u8; 4],
    /// The text, if any. It came from the file: show it as text, nothing else.
    pub text: String,
    /// The author, if the file names one. Also from the file.
    pub author: String,
}

pub(crate) fn put_info(out: &mut Vec<u8>, item: &AnnotInfo) {
    put_u32(out, item.id);
    out.push(item.kind as u8);
    out.push(item.abilities & ability::ALL);
    for v in item.rect {
        put_f32(out, v);
    }
    out.extend_from_slice(&item.color);
    put_str_max(out, &item.text, MAX_ANNOT_TEXT);
    put_str_max(out, &item.author, MAX_ANNOT_AUTHOR);
}

pub(crate) fn read_info(r: &mut Reader<'_>) -> Result<AnnotInfo, ProtocolError> {
    let id = r.u32()?;
    let kind = AnnotKind::from_u8(r.u8()?)?;
    let abilities = r.u8()?;
    if abilities & !ability::ALL != 0 {
        return Err(ProtocolError::UnknownValue(abilities));
    }
    let rect = r.rect()?;
    let color = r.color()?;
    let text = r.string_max(MAX_ANNOT_TEXT)?;
    let author = r.string_max(MAX_ANNOT_AUTHOR)?;
    Ok(AnnotInfo {
        id,
        kind,
        abilities,
        rect,
        color,
        text,
        author,
    })
}

/// Writes the strokes of a drawing: the number of strokes, then for each the number of points and
/// the points. Whatever is over the caps is left out, so the result always decodes.
pub(crate) fn put_strokes(out: &mut Vec<u8>, strokes: &[Vec<[f32; 2]>]) {
    let mut kept: Vec<&Vec<[f32; 2]>> = Vec::new();
    let mut points = 0usize;
    for stroke in strokes.iter().filter(|s| !s.is_empty()) {
        if kept.len() >= MAX_INK_STROKES || points + stroke.len() > MAX_INK_POINTS {
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

pub(crate) fn read_strokes(r: &mut Reader<'_>) -> Result<Vec<Vec<[f32; 2]>>, ProtocolError> {
    let count = r.u32()? as usize;
    // A stroke takes at least 12 bytes: its length and one point.
    if count == 0 || count > MAX_INK_STROKES || count > r.remaining() / 12 {
        return Err(ProtocolError::InvalidLength);
    }
    let mut strokes = Vec::with_capacity(count);
    let mut total = 0usize;
    for _ in 0..count {
        let n = r.u32()? as usize;
        if n == 0 || n > MAX_INK_POINTS - total.min(MAX_INK_POINTS) || n > r.remaining() / 8 {
            return Err(ProtocolError::InvalidLength);
        }
        total += n;
        let mut stroke = Vec::with_capacity(n);
        for _ in 0..n {
            stroke.push([r.coordinate()?, r.coordinate()?]);
        }
        strokes.push(stroke);
    }
    Ok(strokes)
}

const UPDATE_TEXT: u8 = 1;
const UPDATE_COLOR: u8 = 2;

pub(crate) fn put_update(
    out: &mut Vec<u8>,
    text: Option<&str>,
    color: Option<&[u8; 4]>,
    dx: f32,
    dy: f32,
) {
    let mut flags = 0u8;
    if text.is_some() {
        flags |= UPDATE_TEXT;
    }
    if color.is_some() {
        flags |= UPDATE_COLOR;
    }
    out.push(flags);
    if let Some(text) = text {
        put_str_max(out, text, MAX_ANNOT_TEXT);
    }
    if let Some(color) = color {
        out.extend_from_slice(color);
    }
    put_f32(out, dx);
    put_f32(out, dy);
}

#[allow(clippy::type_complexity)]
pub(crate) fn read_update(
    r: &mut Reader<'_>,
) -> Result<(Option<String>, Option<[u8; 4]>, f32, f32), ProtocolError> {
    let flags = r.u8()?;
    if flags & !(UPDATE_TEXT | UPDATE_COLOR) != 0 {
        return Err(ProtocolError::UnknownValue(flags));
    }
    let text = if flags & UPDATE_TEXT != 0 {
        Some(r.string_max(MAX_ANNOT_TEXT)?)
    } else {
        None
    };
    let color = if flags & UPDATE_COLOR != 0 {
        Some(r.color()?)
    } else {
        None
    };
    Ok((text, color, r.coordinate()?, r.coordinate()?))
}
