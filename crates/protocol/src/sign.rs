//! The messages for signing a document with a certificate.
//!
//! The engine prepares the file and holds no key and no certificate: `Begin` makes it write the
//! document out as for a save and add a part to the end with the empty place for the signature (the
//! signature field, the signature dictionary with a byte range that is already right, and a hole
//! of the size the app asked for). The pieces of that file come out through `SaveChunk` like those
//! of a save. The app hashes the file except the hole, has the signer make the signature, and puts
//! it in the hole. The engine never sees a key, and the signer never sees the document.
//!
//! Every text and every list has a cap that the encoder and the decoder both hold, so a damaged or
//! hostile peer cannot make the other allocate or loop for ever.

use super::{
    put_f32, put_str_max, put_u32, read_mark, ProtocolError, Reader, SignatureMark, MAX_SAVE_BYTES,
    MAX_UNIX_TIME,
};

/// Most bytes of the name, the reason, the place and the name of a field.
pub const MAX_SIGN_TEXT_BYTES: usize = 256;

/// Fewest and most bytes the app may reserve for the signature. A signature with a long chain and a
/// large key takes some kilobytes; the cap keeps the hole in the file from being used to bloat it.
pub const MIN_SIGN_RESERVE: u32 = 512;
pub const MAX_SIGN_RESERVE: u32 = 48 * 1024;

/// Most signature fields the engine lists.
pub const MAX_SIGN_FIELDS: usize = 256;

/// Smallest side of the box of a visible signature, in points.
pub const MIN_SIGN_SIDE: f32 = 24.0;

pub(crate) const FIRST_REQUEST_TAG: u8 = 110;
pub(crate) const LAST_REQUEST_TAG: u8 = 112;
pub(crate) const FIRST_RESPONSE_TAG: u8 = 220;
pub(crate) const LAST_RESPONSE_TAG: u8 = 221;

const TAG_BEGIN: u8 = 110;
const TAG_FIELDS: u8 = 111;
const TAG_FINGERPRINT: u8 = 112;

const TAG_READY: u8 = 220;
const TAG_FIELD_LIST: u8 = 221;

const PLACE_INVISIBLE: u8 = 1;
const PLACE_RECT: u8 = 2;
const PLACE_FIELD: u8 = 3;

/// Where the signature goes.
#[derive(Debug, Clone, PartialEq)]
pub enum SignPlace {
    /// A signature that shows nothing on any page.
    Invisible,
    /// A new field of the box `[left, top, right, bottom]` in points from the top left of the page
    /// as it is drawn (page numbers count from 0).
    Rect { page: u32, rect: [f32; 4] },
    /// A field that the document has and nobody has signed, by its full name.
    Field { page: u32, name: String },
}

/// Everything the engine needs to prepare the file. Nothing in it is secret.
#[derive(Debug, Clone, PartialEq)]
pub struct SignSpec {
    pub doc: u32,
    pub place: SignPlace,
    /// The name of the signer, as it is written in the signature and shown in the box.
    pub name: String,
    /// The reason and the place, which may be empty.
    pub reason: String,
    pub location: String,
    /// The time to write, in seconds since 1970. It is the clock of the computer that signs, and
    /// nobody else has confirmed it.
    pub signed_at: i64,
    /// A picture to show in the box, made the way the picture of a signature is (drawn, typed or
    /// imported). It proves nothing.
    pub mark: Option<SignatureMark>,
    /// The colour of a drawn or typed picture.
    pub color: [u8; 4],
    /// How many bytes to keep free for the signature.
    pub reserve: u32,
}

/// A request about signing.
#[derive(Debug, Clone, PartialEq)]
pub enum SignRequest {
    /// Writes the document out and adds the empty signature to the end. The bytes are kept for the
    /// pieces that `SaveChunk` hands out (and `SaveEnd` frees). The document itself is not changed.
    Begin(SignSpec),
    /// Lists the signature fields that the pages have.
    Fields { doc: u32 },
    /// As `Fingerprint`, but a signature field is not counted among the annotations of its page, so
    /// that the document before signing and the file after it can be compared.
    Fingerprint { doc: u32 },
}

/// One signature field of a page.
#[derive(Debug, Clone, PartialEq)]
pub struct SignField {
    pub page: u32,
    /// `[left, top, right, bottom]` in points from the top left of the page as it is drawn.
    pub rect: [f32; 4],
    /// The full name of the field. From the file: show it as text, nothing else.
    pub name: String,
}

/// An answer about signing.
#[derive(Debug, Clone, PartialEq)]
pub enum SignResponse {
    /// The file is ready: `size` bytes, with the hole for the signature between `hole_start` (the
    /// opening `<`) and `hole_end` (just after the closing `>`). The bytes before and after the
    /// hole are what the signature covers, and the byte range in the file says so already.
    /// `field` is the name the field was given (or has).
    Ready {
        size: u32,
        hole_start: u32,
        hole_end: u32,
        field: String,
    },
    Fields {
        fields: Vec<SignField>,
    },
}

fn put_place(out: &mut Vec<u8>, place: &SignPlace) {
    match place {
        SignPlace::Invisible => out.push(PLACE_INVISIBLE),
        SignPlace::Rect { page, rect } => {
            out.push(PLACE_RECT);
            put_u32(out, *page);
            for v in rect {
                put_f32(out, *v);
            }
        }
        SignPlace::Field { page, name } => {
            out.push(PLACE_FIELD);
            put_u32(out, *page);
            put_str_max(out, name, MAX_SIGN_TEXT_BYTES);
        }
    }
}

/// A text of the signature: plain, within the cap.
fn get_text(r: &mut Reader<'_>) -> Result<String, ProtocolError> {
    let text = r.string_max(MAX_SIGN_TEXT_BYTES)?;
    if !super::is_plain_text(&text) {
        return Err(ProtocolError::InvalidLength);
    }
    Ok(text)
}

fn get_box(r: &mut Reader<'_>) -> Result<[f32; 4], ProtocolError> {
    let mut out = [0.0f32; 4];
    for v in &mut out {
        let x = r.f32()?;
        if !x.is_finite() || x.abs() > super::MAX_COORDINATE {
            return Err(ProtocolError::InvalidLength);
        }
        *v = x;
    }
    Ok(out)
}

fn get_place(r: &mut Reader<'_>) -> Result<SignPlace, ProtocolError> {
    match r.u8()? {
        PLACE_INVISIBLE => Ok(SignPlace::Invisible),
        PLACE_RECT => Ok(SignPlace::Rect {
            page: r.u32()?,
            rect: get_box(r)?,
        }),
        PLACE_FIELD => Ok(SignPlace::Field {
            page: r.u32()?,
            name: get_text(r)?,
        }),
        other => Err(ProtocolError::UnknownValue(other)),
    }
}

impl SignRequest {
    /// Writes the request: tag, id, fields.
    pub(crate) fn encode(&self, id: u32, out: &mut Vec<u8>) {
        match self {
            SignRequest::Begin(spec) => {
                out.push(TAG_BEGIN);
                put_u32(out, id);
                put_u32(out, spec.doc);
                put_place(out, &spec.place);
                put_str_max(out, &spec.name, MAX_SIGN_TEXT_BYTES);
                put_str_max(out, &spec.reason, MAX_SIGN_TEXT_BYTES);
                put_str_max(out, &spec.location, MAX_SIGN_TEXT_BYTES);
                out.extend_from_slice(&spec.signed_at.to_le_bytes());
                match &spec.mark {
                    None => out.push(0),
                    Some(mark) => {
                        out.push(1);
                        super::put_mark(out, mark);
                    }
                }
                out.extend_from_slice(&spec.color);
                put_u32(out, spec.reserve);
            }
            SignRequest::Fields { doc } => {
                out.push(TAG_FIELDS);
                put_u32(out, id);
                put_u32(out, *doc);
            }
            SignRequest::Fingerprint { doc } => {
                out.push(TAG_FINGERPRINT);
                put_u32(out, id);
                put_u32(out, *doc);
            }
        }
    }

    /// Reads the request whose tag has been read already, from the reader positioned after the id.
    pub(crate) fn decode(tag: u8, r: &mut Reader<'_>) -> Result<SignRequest, ProtocolError> {
        let request = match tag {
            TAG_BEGIN => {
                let doc = r.u32()?;
                let place = get_place(r)?;
                let name = get_text(r)?;
                let reason = get_text(r)?;
                let location = get_text(r)?;
                let t = r.take(8)?;
                let signed_at =
                    i64::from_le_bytes([t[0], t[1], t[2], t[3], t[4], t[5], t[6], t[7]]);
                if !(0..=MAX_UNIX_TIME).contains(&signed_at) {
                    return Err(ProtocolError::InvalidLength);
                }
                let mark = match r.u8()? {
                    0 => None,
                    1 => Some(read_mark(r)?),
                    other => return Err(ProtocolError::UnknownValue(other)),
                };
                let color = r.color()?;
                let reserve = r.u32()?;
                if !(MIN_SIGN_RESERVE..=MAX_SIGN_RESERVE).contains(&reserve) {
                    return Err(ProtocolError::InvalidLength);
                }
                SignRequest::Begin(SignSpec {
                    doc,
                    place,
                    name,
                    reason,
                    location,
                    signed_at,
                    mark,
                    color,
                    reserve,
                })
            }
            TAG_FIELDS => SignRequest::Fields { doc: r.u32()? },
            TAG_FINGERPRINT => SignRequest::Fingerprint { doc: r.u32()? },
            other => return Err(ProtocolError::UnknownValue(other)),
        };
        Ok(request)
    }
}

impl SignResponse {
    /// Writes the answer: tag, id, fields.
    pub(crate) fn encode(&self, id: u32, out: &mut Vec<u8>) {
        match self {
            SignResponse::Ready {
                size,
                hole_start,
                hole_end,
                field,
            } => {
                out.push(TAG_READY);
                put_u32(out, id);
                put_u32(out, *size);
                put_u32(out, *hole_start);
                put_u32(out, *hole_end);
                put_str_max(out, field, MAX_SIGN_TEXT_BYTES);
            }
            SignResponse::Fields { fields } => {
                out.push(TAG_FIELD_LIST);
                put_u32(out, id);
                let fields = &fields[..fields.len().min(MAX_SIGN_FIELDS)];
                put_u32(out, fields.len() as u32);
                for f in fields {
                    put_u32(out, f.page);
                    for v in &f.rect {
                        put_f32(out, *v);
                    }
                    put_str_max(out, &f.name, MAX_SIGN_TEXT_BYTES);
                }
            }
        }
    }

    pub(crate) fn decode(tag: u8, r: &mut Reader<'_>) -> Result<SignResponse, ProtocolError> {
        match tag {
            TAG_READY => {
                let size = r.u32()?;
                let hole_start = r.u32()?;
                let hole_end = r.u32()?;
                let field = get_text(r)?;
                // The hole is `<`, an even number of hexadecimal digits and `>`, inside the file.
                let digits = hole_end
                    .checked_sub(hole_start)
                    .and_then(|n| n.checked_sub(2));
                let sound = size as usize <= MAX_SAVE_BYTES
                    && hole_end <= size
                    && digits.is_some_and(|n| {
                        n % 2 == 0 && (MIN_SIGN_RESERVE * 2..=MAX_SIGN_RESERVE * 2).contains(&n)
                    });
                if !sound {
                    return Err(ProtocolError::InvalidLength);
                }
                Ok(SignResponse::Ready {
                    size,
                    hole_start,
                    hole_end,
                    field,
                })
            }
            TAG_FIELD_LIST => {
                let count = r.u32()? as usize;
                // A field takes at least 24 bytes: its page, its box and the length of its name.
                if count > MAX_SIGN_FIELDS || count > r.remaining() / 24 {
                    return Err(ProtocolError::InvalidLength);
                }
                let mut fields = Vec::with_capacity(count);
                for _ in 0..count {
                    let page = r.u32()?;
                    let rect = get_box(r)?;
                    let name = get_text(r)?;
                    fields.push(SignField { page, rect, name });
                }
                Ok(SignResponse::Fields { fields })
            }
            other => Err(ProtocolError::UnknownValue(other)),
        }
    }
}
