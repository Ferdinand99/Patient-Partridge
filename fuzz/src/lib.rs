//! Helpers shared by the fuzz targets and the seed generator.
//!
//! `arb_request` and `arb_response` build valid protocol messages out of fuzzer bytes. They stay
//! inside the protocol limits on purpose: a message that the encoder would cut is a different
//! message, and the round-trip targets compare what comes out with what went in. The messages
//! that go past a limit on purpose are in `caps`.

mod author;
mod caps;
mod marks;
mod pages;
mod signatures;

pub use author::*;
pub use caps::*;
pub use marks::*;
pub use pages::*;
pub use signatures::*;

use std::io::{self, Read};

use arbitrary::{Result, Unstructured};
use partridge_protocol::{
    ability, field_flag, AnnotInfo, AnnotKind, ErrorCode, FontScript, FormField, FormFieldKind,
    FormKind, FormOption, FormValue, LinkTarget, OutlineItem, PageLink, PageSize, Request,
    RequestBody, Response, ResponseBody, StampKind, TextMatch, MAX_ANNOTS_PER_PAGE,
    MAX_ANNOT_AUTHOR, MAX_ANNOT_TEXT, MAX_COORDINATE, MAX_FORM_FIELDS_PER_PAGE, MAX_FORM_NAME,
    MAX_FORM_OPTIONS, MAX_FORM_OPTION_LABEL, MAX_FORM_TOOLTIP, MAX_FORM_VALUE, MAX_INK_POINTS,
    MAX_INK_STROKES, MAX_INK_WIDTH, MAX_LINKS_PER_PAGE, MAX_MARKUP_CHARS, MAX_MATCHES_PER_PAGE,
    MAX_OUTLINE_DEPTH, MAX_OUTLINE_ITEMS, MAX_OUTLINE_TITLE, MAX_QUERY_LEN, MAX_SAVE_BYTES,
    MAX_SAVE_CHUNK, MAX_SIGNATURES, MAX_STRING_LEN, MAX_UNIX_TIME, MAX_URI_LEN, MIN_INK_WIDTH,
};
/// True about one time in `n`, and never when the input has run out: `Unstructured` answers
/// `true` to every `ratio` once its bytes are used up, which would make the biggest case the
/// usual one.
pub fn chance(u: &mut Unstructured, n: u32) -> Result<bool> {
    Ok(!u.is_empty() && u.ratio(1, n)?)
}

/// A string of at most `max` bytes. Most are short and random; one in ten sits exactly on the
/// limit (or one byte under it, or ends in a two byte character), because that is where decoders
/// slip. When the input has run out the string is empty.
pub fn string_up_to(u: &mut Unstructured, max: usize) -> Result<String> {
    if u.is_empty() {
        return Ok(String::new());
    }
    Ok(match u.int_in_range(0..=39u8)? {
        0 => "a".repeat(max),
        1 => "a".repeat(max.saturating_sub(1)),
        // Two byte characters: exactly `max` bytes when `max` is even, one byte under otherwise.
        2 => "\u{e9}".repeat(max / 2),
        // Four byte characters.
        3 => "\u{1f4d5}".repeat(max / 4),
        _ => {
            let count = u.int_in_range(0..=24usize)?;
            let mut s = String::new();
            for _ in 0..count {
                let c: char = u.arbitrary()?;
                if s.len() + c.len_utf8() > max {
                    break;
                }
                s.push(c);
            }
            s
        }
    })
}

fn finite(u: &mut Unstructured) -> Result<f32> {
    let v: f32 = u.arbitrary()?;
    Ok(if v.is_finite() { v } else { 1.0 })
}

/// Any four floats, including NaN and infinity: the decoder must turn those into zeros.
fn any_rect(u: &mut Unstructured) -> Result<[f32; 4]> {
    Ok([
        u.arbitrary()?,
        u.arbitrary()?,
        u.arbitrary()?,
        u.arbitrary()?,
    ])
}

/// What the decoder gives back for a box: every number that is not finite becomes 0.
pub fn sanitized(rect: [f32; 4]) -> [f32; 4] {
    rect.map(|v| if v.is_finite() { v } else { 0.0 })
}

/// A place on a page that a request may carry: finite, and now and then exactly at the limit.
fn coordinate(u: &mut Unstructured) -> Result<f32> {
    if chance(u, 16)? {
        return Ok(if u.arbitrary()? {
            MAX_COORDINATE
        } else {
            -MAX_COORDINATE
        });
    }
    let v: f32 = u.arbitrary()?;
    Ok(if v.is_finite() && v.abs() <= MAX_COORDINATE {
        v
    } else {
        0.5
    })
}

fn markup_kind(u: &mut Unstructured) -> Result<AnnotKind> {
    Ok([
        AnnotKind::Highlight,
        AnnotKind::Underline,
        AnnotKind::Strikeout,
    ][u.int_in_range(0..=2usize)?])
}

/// The strokes of a drawing inside the caps: a few short ones mostly, and now and then the most
/// strokes, or the most points, the protocol allows (made of one repeated point, so that they cost
/// the fuzzer's input nothing).
fn arb_strokes(u: &mut Unstructured) -> Result<Vec<Vec<[f32; 2]>>> {
    if chance(u, 24)? {
        return Ok(if u.arbitrary()? {
            vec![vec![[1.0, 1.0]]; MAX_INK_STROKES]
        } else {
            vec![vec![[2.0, 3.0]; MAX_INK_POINTS]]
        });
    }
    let count = u.int_in_range(1..=4usize)?;
    let mut strokes = Vec::new();
    for _ in 0..count {
        let points = u.int_in_range(1..=6usize)?;
        let mut stroke = Vec::new();
        for _ in 0..points {
            stroke.push([coordinate(u)?, coordinate(u)?]);
        }
        strokes.push(stroke);
    }
    Ok(strokes)
}

/// A request that the encoder writes without cutting anything.
pub fn arb_request(u: &mut Unstructured) -> Result<Request> {
    let id: u32 = u.arbitrary()?;
    let body = match u.int_in_range(0..=29u8)? {
        0 => RequestBody::Hello {
            protocol_version: u.arbitrary()?,
        },
        1 => RequestBody::Open {
            password: if u.arbitrary()? {
                Some(string_up_to(u, MAX_STRING_LEN)?)
            } else {
                None
            },
        },
        2 => RequestBody::Render {
            doc: u.arbitrary()?,
            page: u.arbitrary()?,
            width: u.arbitrary()?,
            height: u.arbitrary()?,
        },
        3 => RequestBody::Close {
            doc: u.arbitrary()?,
        },
        4 => RequestBody::Outline {
            doc: u.arbitrary()?,
        },
        5 => RequestBody::PageText {
            doc: u.arbitrary()?,
            page: u.arbitrary()?,
        },
        6 => RequestBody::PageLinks {
            doc: u.arbitrary()?,
            page: u.arbitrary()?,
        },
        7 => RequestBody::Search {
            doc: u.arbitrary()?,
            page: u.arbitrary()?,
            query: string_up_to(u, MAX_QUERY_LEN)?,
        },
        8 => RequestBody::DocInfo {
            doc: u.arbitrary()?,
        },
        9 => RequestBody::SaveBegin {
            doc: u.arbitrary()?,
        },
        10 => RequestBody::SaveChunk {
            doc: u.arbitrary()?,
            offset: u.arbitrary()?,
            // Now and then exactly the biggest piece, which the decoder must accept.
            len: if chance(u, 8)? {
                MAX_SAVE_CHUNK as u32
            } else {
                u.int_in_range(0..=MAX_SAVE_CHUNK as u32)?
            },
        },
        11 => RequestBody::SaveEnd {
            doc: u.arbitrary()?,
        },
        12 => RequestBody::Fingerprint {
            doc: u.arbitrary()?,
        },
        13 => RequestBody::ListAnnots {
            doc: u.arbitrary()?,
            page: u.arbitrary()?,
        },
        14 => {
            // At least one character, at most a page of them, and the end still a u32.
            let start = u.int_in_range(0..=u32::MAX - MAX_MARKUP_CHARS as u32)?;
            let len = if chance(u, 8)? {
                MAX_MARKUP_CHARS as u32
            } else {
                u.int_in_range(1..=64u32)?
            };
            RequestBody::AddMarkup {
                doc: u.arbitrary()?,
                page: u.arbitrary()?,
                kind: markup_kind(u)?,
                color: u.arbitrary()?,
                start,
                end: start + len,
            }
        }
        15 => RequestBody::AddNote {
            doc: u.arbitrary()?,
            page: u.arbitrary()?,
            x: coordinate(u)?,
            y: coordinate(u)?,
            color: u.arbitrary()?,
            text: string_up_to(u, MAX_ANNOT_TEXT)?,
        },
        16 => RequestBody::AddInk {
            doc: u.arbitrary()?,
            page: u.arbitrary()?,
            color: u.arbitrary()?,
            // Quarter points from the thinnest to the thickest line, edges included.
            width: if chance(u, 8)? {
                [MIN_INK_WIDTH, MAX_INK_WIDTH][u.int_in_range(0..=1usize)?]
            } else {
                u.int_in_range(1..=288u32)? as f32 * 0.25
            },
            strokes: arb_strokes(u)?,
        },
        17 => RequestBody::AddStamp {
            doc: u.arbitrary()?,
            page: u.arbitrary()?,
            x: coordinate(u)?,
            y: coordinate(u)?,
            stamp: StampKind::ALL[u.int_in_range(0..=StampKind::ALL.len() - 1)?],
        },
        18 => RequestBody::UpdateAnnot {
            doc: u.arbitrary()?,
            page: u.arbitrary()?,
            id: u.arbitrary()?,
            text: if u.arbitrary()? {
                Some(string_up_to(u, MAX_ANNOT_TEXT)?)
            } else {
                None
            },
            color: if u.arbitrary()? {
                Some(u.arbitrary()?)
            } else {
                None
            },
            dx: coordinate(u)?,
            dy: coordinate(u)?,
        },
        19 => RequestBody::RemoveAnnot {
            doc: u.arbitrary()?,
            page: u.arbitrary()?,
            id: u.arbitrary()?,
        },
        20 => RequestBody::RestoreAnnot {
            doc: u.arbitrary()?,
            page: u.arbitrary()?,
            id: u.arbitrary()?,
        },
        21 => RequestBody::FontReport,
        22 => RequestBody::FormInfo {
            doc: u.arbitrary()?,
        },
        23 => RequestBody::ListFormFields {
            doc: u.arbitrary()?,
            page: u.arbitrary()?,
        },
        24 => RequestBody::SetFormField {
            doc: u.arbitrary()?,
            page: u.arbitrary()?,
            index: u.arbitrary()?,
            name: string_up_to(u, MAX_FORM_NAME)?,
            value: arb_form_value(u)?,
        },
        25 => RequestBody::Signatures {
            doc: u.arbitrary()?,
            now: if chance(u, 8)? {
                MAX_UNIX_TIME
            } else {
                u.int_in_range(0..=MAX_UNIX_TIME)?
            },
        },
        26 => RequestBody::Pages(arb_pages_request(u)?),
        27 => RequestBody::AddSignature {
            doc: u.arbitrary()?,
            page: u.arbitrary()?,
            rect: [
                coordinate(u)?,
                coordinate(u)?,
                coordinate(u)?,
                coordinate(u)?,
            ],
            color: u.arbitrary()?,
            mark: arb_mark(u)?,
        },
        28 => RequestBody::PlaceSignature {
            doc: u.arbitrary()?,
            page: u.arbitrary()?,
            id: u.arbitrary()?,
            rect: [
                coordinate(u)?,
                coordinate(u)?,
                coordinate(u)?,
                coordinate(u)?,
            ],
        },
        _ => RequestBody::Author(arb_author_request(u)?),
    };
    Ok(Request { id, body })
}

/// How many entries a list gets: mostly a few, now and then the most the protocol allows.
fn count_up_to(u: &mut Unstructured, max: usize) -> Result<usize> {
    if chance(u, 24)? {
        Ok(max)
    } else {
        Ok(u.int_in_range(0..=6usize)?.min(max))
    }
}

/// A response in the form the decoder gives it back: boxes already cleaned, an outline depth
/// already clamped. `sanitize_rects` is false for answers that carry no boxes.
pub struct GeneratedResponse {
    /// What the engine side would hold.
    pub sent: Response,
    /// What the decoder must return for it.
    pub expected: Response,
}

/// One listed annotation inside the caps: any defined abilities, a box that may hold numbers that
/// are not numbers (the decoder turns those into zeros), text and an author up to the caps. Returns
/// what is sent and what must come back.
fn arb_annot(u: &mut Unstructured) -> Result<(AnnotInfo, AnnotInfo)> {
    let sent = AnnotInfo {
        id: u.arbitrary()?,
        kind: AnnotKind::from_u8(u.int_in_range(1..=7u8)?).expect("1 to 7 are kinds"),
        abilities: u.int_in_range(0..=ability::ALL)?,
        rect: any_rect(u)?,
        color: u.arbitrary()?,
        text: string_up_to(u, MAX_ANNOT_TEXT)?,
        author: string_up_to(u, MAX_ANNOT_AUTHOR)?,
    };
    let expected = AnnotInfo {
        rect: sanitized(sent.rect),
        ..sent.clone()
    };
    Ok((sent, expected))
}

pub fn arb_response(u: &mut Unstructured) -> Result<GeneratedResponse> {
    let id: u32 = u.arbitrary()?;
    let (sent, expected) = match u.int_in_range(0..=23u8)? {
        0 => {
            let b = ResponseBody::Hello {
                protocol_version: u.arbitrary()?,
                engine_version: string_up_to(u, MAX_STRING_LEN)?,
                pdfium_version: string_up_to(u, MAX_STRING_LEN)?,
                sandbox: string_up_to(u, MAX_STRING_LEN)?,
            };
            (b.clone(), b)
        }
        1 => {
            let count = u.int_in_range(0..=12usize)?;
            let mut pages = Vec::new();
            for _ in 0..count {
                pages.push(PageSize {
                    width: finite(u)?,
                    height: finite(u)?,
                });
            }
            let b = ResponseBody::Opened {
                doc: u.arbitrary()?,
                pages,
            };
            (b.clone(), b)
        }
        2 => {
            let width = u.int_in_range(0..=8u32)?;
            let height = u.int_in_range(0..=8u32)?;
            let mut rgba = vec![0u8; (width * height * 4) as usize];
            u.fill_buffer(&mut rgba)?;
            let b = ResponseBody::Rendered {
                width,
                height,
                rgba,
            };
            (b.clone(), b)
        }
        3 => (ResponseBody::Closed, ResponseBody::Closed),
        4 => {
            let count = count_up_to(u, MAX_OUTLINE_ITEMS)?;
            let mut sent = Vec::new();
            let mut expected = Vec::new();
            for _ in 0..count {
                // Deeper than allowed is clamped by the encoder, and a page that equals the
                // "no page" marker reads back as no page.
                let depth: u8 = if u.ratio(1, 8)? {
                    u.arbitrary()?
                } else {
                    u.int_in_range(0..=MAX_OUTLINE_DEPTH)?
                };
                let page: Option<u32> = u.arbitrary()?;
                let title = string_up_to(u, MAX_OUTLINE_TITLE)?;
                sent.push(OutlineItem {
                    depth,
                    page,
                    title: title.clone(),
                });
                expected.push(OutlineItem {
                    depth: depth.min(MAX_OUTLINE_DEPTH),
                    page: page.filter(|p| *p != u32::MAX),
                    title,
                });
            }
            (
                ResponseBody::Outline { items: sent },
                ResponseBody::Outline { items: expected },
            )
        }
        5 => {
            let count = u.int_in_range(0..=40usize)?;
            let mut text = String::new();
            let mut rects = Vec::new();
            let mut expected_rects = Vec::new();
            for _ in 0..count {
                text.push(u.arbitrary::<char>()?);
                let r = any_rect(u)?;
                rects.push(r);
                expected_rects.push(sanitized(r));
            }
            (
                ResponseBody::PageText {
                    text: text.clone(),
                    rects,
                },
                ResponseBody::PageText {
                    text,
                    rects: expected_rects,
                },
            )
        }
        6 => {
            let count = count_up_to(u, MAX_LINKS_PER_PAGE)?;
            let mut sent = Vec::new();
            let mut expected = Vec::new();
            for _ in 0..count {
                let rect = any_rect(u)?;
                let target = if u.arbitrary()? {
                    LinkTarget::Uri(string_up_to(u, MAX_URI_LEN)?)
                } else {
                    LinkTarget::Page(u.arbitrary()?)
                };
                sent.push(PageLink {
                    rect,
                    target: target.clone(),
                });
                expected.push(PageLink {
                    rect: sanitized(rect),
                    target,
                });
            }
            (
                ResponseBody::PageLinks { links: sent },
                ResponseBody::PageLinks { links: expected },
            )
        }
        7 => {
            let count = count_up_to(u, MAX_MATCHES_PER_PAGE)?;
            let mut matches = Vec::new();
            for _ in 0..count {
                matches.push(TextMatch {
                    start: u.arbitrary()?,
                    len: u.arbitrary()?,
                });
            }
            let b = ResponseBody::Matches { matches };
            (b.clone(), b)
        }
        8 => {
            // The codes of the reader, the save, the annotations and the forms (1 to 20, then 24 and
            // 25), and now and then those of the page tools (50 to 56).
            let code = if chance(u, 6)? {
                ErrorCode::from_u8(u.int_in_range(50..=56u8)?).expect("50 to 56 are codes")
            } else {
                let n = u.int_in_range(1..=22u8)?;
                ErrorCode::from_u8(if n > 20 { n + 3 } else { n }).expect("a known code")
            };
            let b = ResponseBody::Error {
                code,
                message: string_up_to(u, MAX_STRING_LEN)?,
            };
            (b.clone(), b)
        }
        9 => {
            let b = ResponseBody::DocInfo {
                pages: u.arbitrary()?,
                security: u.arbitrary()?,
                signatures: u.arbitrary()?,
                maybe_signed: u.arbitrary()?,
                touched: u.arbitrary()?,
            };
            (b.clone(), b)
        }
        10 => {
            // Now and then the biggest file there is.
            let size = if chance(u, 8)? {
                MAX_SAVE_BYTES as u32
            } else {
                u.int_in_range(0..=MAX_SAVE_BYTES as u32)?
            };
            let b = ResponseBody::SaveReady { size };
            (b.clone(), b)
        }
        11 => {
            // A piece of the file: a few bytes mostly, and now and then the biggest piece, at the
            // offset where the last byte of the biggest file is.
            let (offset, len) = if chance(u, 12)? {
                ((MAX_SAVE_BYTES - MAX_SAVE_CHUNK) as u32, MAX_SAVE_CHUNK)
            } else {
                let len = u.int_in_range(0..=40usize)?;
                (u.int_in_range(0..=(MAX_SAVE_BYTES - len) as u32)?, len)
            };
            let mut data = vec![0u8; len];
            u.fill_buffer(&mut data)?;
            let b = ResponseBody::SaveData { offset, data };
            (b.clone(), b)
        }
        12 => (ResponseBody::SaveEnded, ResponseBody::SaveEnded),
        13 => {
            let b = ResponseBody::Progress {
                done: u.arbitrary()?,
            };
            (b.clone(), b)
        }
        14 => {
            let b = ResponseBody::Fingerprint {
                pages: u.arbitrary()?,
                failed: u.arbitrary()?,
                digest: u.arbitrary()?,
            };
            (b.clone(), b)
        }
        15 => {
            let count = count_up_to(u, MAX_ANNOTS_PER_PAGE)?;
            let mut sent = Vec::new();
            let mut expected = Vec::new();
            for _ in 0..count {
                let (s, e) = arb_annot(u)?;
                sent.push(s);
                expected.push(e);
            }
            (
                ResponseBody::Annots { items: sent },
                ResponseBody::Annots { items: expected },
            )
        }
        16 => {
            let (s, e) = arb_annot(u)?;
            (
                ResponseBody::Annot { item: s },
                ResponseBody::Annot { item: e },
            )
        }
        17 => (ResponseBody::AnnotRemoved, ResponseBody::AnnotRemoved),
        18 => {
            // Any scripts, in the order of the list, as the engine reports them.
            let mut missed = Vec::new();
            for script in FontScript::ALL {
                if u.arbitrary()? {
                    missed.push(script);
                }
            }
            let b = ResponseBody::FontReport { missed };
            (b.clone(), b)
        }
        19 => {
            let b = ResponseBody::FormInfo {
                kind: match u.int_in_range(0..=2u8)? {
                    0 => FormKind::None,
                    1 => FormKind::Acro,
                    _ => FormKind::Xfa,
                },
                has_actions: u.arbitrary()?,
            };
            (b.clone(), b)
        }
        20 => {
            // The fields of a page. A long list gets short strings, so that the whole answer is
            // under the limit the encoder keeps to (it leaves out what does not fit).
            let count = count_up_to(u, MAX_FORM_FIELDS_PER_PAGE)?;
            let heavy = count <= 8;
            let mut sent = Vec::new();
            let mut expected = Vec::new();
            for index in 0..count {
                let (s, e) = arb_form_field(u, index as u32, heavy)?;
                sent.push(s);
                expected.push(e);
            }
            (
                ResponseBody::FormFields { fields: sent },
                ResponseBody::FormFields { fields: expected },
            )
        }
        21 => {
            let count = count_up_to(u, MAX_SIGNATURES)?;
            let b = ResponseBody::Signatures {
                report: arb_signature_report(u, count)?,
            };
            (b.clone(), b)
        }
        22 => {
            let b = ResponseBody::Pages(arb_pages_response(u)?);
            (b.clone(), b)
        }
        _ => {
            let b = ResponseBody::Author(arb_author_response(u)?);
            (b.clone(), b)
        }
    };
    Ok(GeneratedResponse {
        sent: Response { id, body: sent },
        expected: Response { id, body: expected },
    })
}

/// A reader that hands out the data in small pieces and now and then fails with `Interrupted`,
/// like a pipe does. `read_frame` must give the same answer as with all the data at once.
pub struct ChunkReader<'a> {
    data: &'a [u8],
    sizes: &'a [u8],
    next: usize,
    interrupted: bool,
}

impl<'a> ChunkReader<'a> {
    pub fn new(data: &'a [u8], sizes: &'a [u8]) -> Self {
        ChunkReader {
            data,
            sizes,
            next: 0,
            interrupted: false,
        }
    }
}

impl Read for ChunkReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let step = if self.sizes.is_empty() {
            usize::MAX
        } else {
            let s = self.sizes[self.next % self.sizes.len()];
            self.next += 1;
            // 0 means "interrupted"; anything else is the number of bytes to give. Two
            // interruptions never follow each other, so every caller that retries makes progress.
            if s == 0 && !self.interrupted {
                self.interrupted = true;
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            self.interrupted = false;
            (s as usize).max(1)
        };
        let n = step.min(buf.len()).min(self.data.len());
        buf[..n].copy_from_slice(&self.data[..n]);
        self.data = &self.data[n..];
        Ok(n)
    }
}

/// What a request sets a field to: a text at most as long as the cap, a switch, or choices.
fn arb_form_value(u: &mut Unstructured) -> Result<FormValue> {
    Ok(match u.int_in_range(0..=2u8)? {
        0 => FormValue::Text(string_up_to(u, MAX_FORM_VALUE)?),
        1 => FormValue::Checked(u.arbitrary()?),
        _ => {
            let count = count_up_to(u, MAX_FORM_OPTIONS)?;
            let mut picked = Vec::with_capacity(count);
            for _ in 0..count {
                picked.push(u.arbitrary()?);
            }
            FormValue::Choice(picked)
        }
    })
}

/// A string for a field of a long list: short, so that the answer fits.
fn form_string(u: &mut Unstructured, max: usize, heavy: bool) -> Result<String> {
    if heavy {
        string_up_to(u, max)
    } else {
        let count = u.int_in_range(0..=12usize)?;
        let mut s = String::new();
        for _ in 0..count {
            let c: char = u.arbitrary()?;
            if s.len() + c.len_utf8() > max {
                break;
            }
            s.push(c);
        }
        Ok(s)
    }
}

/// A field, as the engine sends it, and as the decoder gives it back (a box that is not a number
/// is zeros, a size of text that is not a size is zero).
fn arb_form_field(u: &mut Unstructured, index: u32, heavy: bool) -> Result<(FormField, FormField)> {
    let kind = match u.int_in_range(0..=4u8)? {
        0 => FormFieldKind::Text,
        1 => FormFieldKind::Checkbox,
        2 => FormFieldKind::Radio,
        3 => FormFieldKind::Combo,
        _ => FormFieldKind::List,
    };
    let flags = u.arbitrary::<u16>()? & field_flag::ALL;
    let options = if heavy {
        let count = count_up_to(u, MAX_FORM_OPTIONS)?;
        let mut options = Vec::with_capacity(count);
        for _ in 0..count {
            options.push(FormOption {
                selected: u.arbitrary()?,
                label: string_up_to(u, MAX_FORM_OPTION_LABEL)?,
            });
        }
        options
    } else {
        Vec::new()
    };
    let rect = any_rect(u)?;
    let font_size: f32 = if chance(u, 8)? {
        f32::NAN
    } else {
        finite(u)?.abs()
    };
    let field = FormField {
        index,
        kind,
        flags,
        turns: u.int_in_range(0..=3u8)?,
        rect,
        font_size,
        name: form_string(u, MAX_FORM_NAME, heavy)?,
        tooltip: form_string(u, MAX_FORM_TOOLTIP, heavy)?,
        value: form_string(u, MAX_FORM_VALUE, heavy)?,
        max_len: u.arbitrary()?,
        options,
    };
    let mut expected = field.clone();
    expected.rect = sanitized(rect);
    expected.font_size = if font_size.is_finite() {
        font_size
    } else {
        0.0
    };
    Ok((field, expected))
}
