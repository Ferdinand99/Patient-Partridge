//! The caps of the protocol as assertions, and messages that break exactly one of them.
//!
//! `over_cap_request` and `over_cap_response` write the bytes of a message that is valid in every
//! way except that one list or string is one over its cap, so the decoder must refuse it. The
//! encoder cannot write such a message (it cuts lists and strings), so the bytes are written here.
//! Most caps share their error with a second check (a count that does not fit the bytes that are
//! there), so only a message that really holds the extra entry can show that the cap itself is
//! checked.

use arbitrary::{Result, Unstructured};
use partridge_protocol::{
    ability, FontScript, FormValue, LinkTarget, Request, RequestBody, Response, ResponseBody,
    MAX_ANNOTS_PER_PAGE, MAX_ANNOT_AUTHOR, MAX_ANNOT_TEXT, MAX_BYTE_RANGE, MAX_COORDINATE,
    MAX_FORM_ANSWER_BYTES, MAX_FORM_FIELDS_PER_PAGE, MAX_FORM_NAME, MAX_FORM_OPTIONS,
    MAX_FORM_OPTION_LABEL, MAX_FORM_TOOLTIP, MAX_FORM_VALUE, MAX_INK_POINTS, MAX_INK_STROKES,
    MAX_INK_WIDTH, MAX_LINKS_PER_PAGE, MAX_MARKUP_CHARS, MAX_MATCHES_PER_PAGE, MAX_OUTLINE_DEPTH,
    MAX_OUTLINE_ITEMS, MAX_OUTLINE_TITLE, MAX_PAGES, MAX_PAGE_CHARS, MAX_PAGE_TEXT_BYTES,
    MAX_QUERY_LEN, MAX_RENDER_PIXELS, MAX_RENDER_SIDE, MAX_SAVE_BYTES, MAX_SAVE_CHUNK,
    MAX_SIGNATURES, MAX_SIG_NAME, MAX_STRING_LEN, MAX_UNIX_TIME, MAX_URI_LEN, MIN_INK_WIDTH,
};

use crate::{chance, check_signature_report};

// ---- the caps as assertions ----------------------------------------------------------

/// Panics when a request is outside the caps the protocol promises.
pub fn check_request_caps(request: &Request) {
    match &request.body {
        RequestBody::Open { password } => {
            if let Some(p) = password {
                assert!(p.len() <= MAX_STRING_LEN, "password of {} bytes", p.len());
            }
        }
        RequestBody::Search { query, .. } => {
            assert!(
                query.len() <= MAX_QUERY_LEN,
                "query of {} bytes",
                query.len()
            );
        }
        RequestBody::SaveChunk { len, .. } => {
            assert!(*len as usize <= MAX_SAVE_CHUNK, "piece of {len} bytes");
        }
        RequestBody::Pages(request) => crate::pages::check_pages_request_caps(request),
        RequestBody::Author(request) => crate::author::check_author_request_caps(request),
        RequestBody::Sign(request) => crate::sign::check_sign_request_caps(request),
        RequestBody::SetFormField { name, value, .. } => {
            assert!(
                name.len() <= MAX_FORM_NAME,
                "field name of {} bytes",
                name.len()
            );
            match value {
                FormValue::Text(text) => {
                    assert!(
                        text.len() <= MAX_FORM_VALUE,
                        "value of {} bytes",
                        text.len()
                    )
                }
                FormValue::Choice(picked) => {
                    assert!(picked.len() <= MAX_FORM_OPTIONS, "{} choices", picked.len())
                }
                FormValue::Checked(_) => {}
            }
        }
        RequestBody::AddMarkup { start, end, .. } => {
            assert!(end > start, "an empty range of text");
            assert!(
                (end - start) as usize <= MAX_MARKUP_CHARS,
                "{} characters marked",
                end - start
            );
        }
        RequestBody::AddNote { x, y, text, .. } => {
            assert!(text.len() <= MAX_ANNOT_TEXT, "note of {} bytes", text.len());
            assert!(x.abs() <= MAX_COORDINATE && y.abs() <= MAX_COORDINATE);
        }
        RequestBody::AddInk { width, strokes, .. } => {
            assert!(
                (MIN_INK_WIDTH..=MAX_INK_WIDTH).contains(width),
                "width {width}"
            );
            assert!(!strokes.is_empty() && strokes.len() <= MAX_INK_STROKES);
            assert!(strokes.iter().all(|s| !s.is_empty()));
            assert!(strokes.iter().map(Vec::len).sum::<usize>() <= MAX_INK_POINTS);
            assert!(strokes
                .iter()
                .flatten()
                .all(|p| p[0].abs() <= MAX_COORDINATE && p[1].abs() <= MAX_COORDINATE));
        }
        RequestBody::AddStamp { x, y, .. } => {
            assert!(x.abs() <= MAX_COORDINATE && y.abs() <= MAX_COORDINATE);
        }
        RequestBody::AddSignature { rect, mark, .. } => {
            assert!(rect.iter().all(|v| v.abs() <= MAX_COORDINATE));
            crate::marks::check_mark_caps(mark);
        }
        RequestBody::PlaceSignature { rect, .. } => {
            assert!(rect.iter().all(|v| v.abs() <= MAX_COORDINATE));
        }
        RequestBody::UpdateAnnot { text, dx, dy, .. } => {
            if let Some(text) = text {
                assert!(text.len() <= MAX_ANNOT_TEXT, "text of {} bytes", text.len());
            }
            assert!(dx.abs() <= MAX_COORDINATE && dy.abs() <= MAX_COORDINATE);
        }
        _ => {}
    }
}

/// Panics when a response is outside the caps the protocol promises: list lengths, string
/// lengths, outline depth, bitmap size. Used on what the decoder returns and on what the engine
/// builds before it is encoded (the encoder cuts lists, which would hide a missing bound).
pub fn check_response_caps(response: &Response) {
    match &response.body {
        ResponseBody::Hello {
            engine_version,
            pdfium_version,
            sandbox,
            ..
        } => {
            assert!(engine_version.len() <= MAX_STRING_LEN);
            assert!(pdfium_version.len() <= MAX_STRING_LEN);
            assert!(sandbox.len() <= MAX_STRING_LEN);
        }
        ResponseBody::Annots { items } => {
            assert!(
                items.len() <= MAX_ANNOTS_PER_PAGE,
                "{} annotations",
                items.len()
            );
            for item in items {
                check_annot(item);
            }
        }
        ResponseBody::Annot { item } => check_annot(item),
        ResponseBody::AnnotRemoved => {}
        ResponseBody::Opened { pages, .. } => {
            assert!(pages.len() <= MAX_PAGES, "{} pages", pages.len());
        }
        ResponseBody::Pages(response) => crate::pages::check_pages_response_caps(response),
        ResponseBody::Author(response) => crate::author::check_author_response_caps(response),
        ResponseBody::Sign(response) => crate::sign::check_sign_response_caps(response),
        ResponseBody::Closed
        | ResponseBody::DocInfo { .. }
        | ResponseBody::Progress { .. }
        | ResponseBody::Fingerprint { .. }
        | ResponseBody::SaveEnded => {}
        ResponseBody::SaveReady { size } => {
            assert!(*size as usize <= MAX_SAVE_BYTES, "a file of {size} bytes");
        }
        ResponseBody::SaveData { offset, data } => {
            assert!(
                data.len() <= MAX_SAVE_CHUNK,
                "piece of {} bytes",
                data.len()
            );
            assert!(
                *offset as u64 + data.len() as u64 <= MAX_SAVE_BYTES as u64,
                "a piece that ends past the biggest file"
            );
        }
        ResponseBody::Rendered {
            width,
            height,
            rgba,
        } => {
            assert!(*width <= MAX_RENDER_SIDE && *height <= MAX_RENDER_SIDE);
            assert!(*width as u64 * *height as u64 <= MAX_RENDER_PIXELS);
            assert_eq!(rgba.len() as u64, *width as u64 * *height as u64 * 4);
        }
        ResponseBody::Outline { items } => {
            assert!(
                items.len() <= MAX_OUTLINE_ITEMS,
                "{} outline items",
                items.len()
            );
            for item in items {
                assert!(item.depth <= MAX_OUTLINE_DEPTH, "depth {}", item.depth);
                assert!(
                    item.title.len() <= MAX_OUTLINE_TITLE,
                    "title of {} bytes",
                    item.title.len()
                );
            }
        }
        ResponseBody::PageText { text, rects } => {
            assert!(rects.len() <= MAX_PAGE_CHARS, "{} characters", rects.len());
            assert!(
                text.len() <= MAX_PAGE_TEXT_BYTES,
                "{} bytes of text",
                text.len()
            );
            assert_eq!(text.chars().count(), rects.len());
        }
        ResponseBody::PageLinks { links } => {
            assert!(links.len() <= MAX_LINKS_PER_PAGE, "{} links", links.len());
            for link in links {
                if let LinkTarget::Uri(u) = &link.target {
                    assert!(u.len() <= MAX_URI_LEN, "address of {} bytes", u.len());
                }
            }
        }
        ResponseBody::Matches { matches } => {
            assert!(
                matches.len() <= MAX_MATCHES_PER_PAGE,
                "{} matches",
                matches.len()
            );
        }
        ResponseBody::Signatures { report } => check_signature_report(report),
        ResponseBody::FormInfo { .. } => {}
        ResponseBody::FormFields { fields } => {
            assert!(
                fields.len() <= MAX_FORM_FIELDS_PER_PAGE,
                "{} fields",
                fields.len()
            );
            for field in fields {
                assert!(field.name.len() <= MAX_FORM_NAME);
                assert!(field.tooltip.len() <= MAX_FORM_TOOLTIP);
                assert!(field.value.len() <= MAX_FORM_VALUE);
                assert!(field.options.len() <= MAX_FORM_OPTIONS);
                for option in &field.options {
                    assert!(option.label.len() <= MAX_FORM_OPTION_LABEL);
                }
            }
        }
        ResponseBody::FontReport { missed } => {
            assert!(
                missed.len() <= FontScript::ALL.len(),
                "{} scripts",
                missed.len()
            );
        }
        ResponseBody::Error { message, .. } => {
            assert!(message.len() <= MAX_STRING_LEN);
        }
    }
}

fn check_annot(item: &partridge_protocol::AnnotInfo) {
    assert!(
        item.text.len() <= MAX_ANNOT_TEXT,
        "text of {} bytes",
        item.text.len()
    );
    assert!(
        item.author.len() <= MAX_ANNOT_AUTHOR,
        "author of {} bytes",
        item.author.len()
    );
    assert_eq!(
        item.abilities & !ability::ALL,
        0,
        "abilities {}",
        item.abilities
    );
    assert!(item.rect.iter().all(|v| v.is_finite()));
}

// ---- messages that break exactly one cap ------------------------------------------------

fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// A string field: the length, then that many bytes.
fn push_str(out: &mut Vec<u8>, len: usize) {
    push_u32(out, len as u32);
    out.resize(out.len() + len, b'a');
}

fn push_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn push_rect(out: &mut Vec<u8>) {
    for v in [1.0f32, 2.0, 3.0, 4.0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

fn header(tag: u8, id: u32) -> Vec<u8> {
    let mut out = vec![tag];
    push_u32(&mut out, id);
    out
}

/// One form field with a name and a value of the given sizes and the given number of choices (each
/// with a label of `label` bytes), the way the engine writes it.
fn raw_form_field(name: usize, value: usize, options: usize, label: usize) -> Vec<u8> {
    let mut p = Vec::new();
    push_u32(&mut p, 0);
    p.push(1);
    p.extend_from_slice(&0u16.to_le_bytes());
    p.push(0);
    push_rect(&mut p);
    p.extend_from_slice(&0.0f32.to_le_bytes());
    push_str(&mut p, name);
    push_str(&mut p, 0);
    push_str(&mut p, value);
    push_u32(&mut p, 0);
    push_u32(&mut p, options as u32);
    for _ in 0..options {
        p.push(0);
        push_str(&mut p, label);
    }
    p
}

/// An answer with the fields of a page: one field.
fn form_answer(id: u32, name: usize, value: usize, options: usize, label: usize) -> Vec<u8> {
    let mut p = header(171, id);
    push_u32(&mut p, 1);
    p.extend_from_slice(&raw_form_field(name, value, options, label));
    assert!(p.len() <= MAX_FORM_ANSWER_BYTES);
    p
}

/// How many cases `over_cap_request_case` has.
pub const OVER_CAP_REQUESTS: usize =
    15 + crate::marks::MARK_OVER_REQUESTS + crate::pages::PAGES_OVER_REQUESTS;

/// A request that is valid except that one string is one byte over its cap. Returns a name and
/// the payload; the decoder must return an error.
pub fn over_cap_request_case(case: usize, id: u32) -> (&'static str, Vec<u8>) {
    match case % OVER_CAP_REQUESTS {
        n if n >= 15 + crate::marks::MARK_OVER_REQUESTS => {
            crate::pages::pages_over_cap_request_case(n - 15 - crate::marks::MARK_OVER_REQUESTS, id)
        }
        n if n >= 15 => crate::marks::mark_over_cap_request_case(n - 15, id),
        0 => {
            let mut p = header(2, id);
            p.push(1);
            push_str(&mut p, MAX_STRING_LEN + 1);
            ("open password over the cap", p)
        }
        1 => {
            let mut p = header(8, id);
            push_u32(&mut p, 1);
            push_u32(&mut p, 0);
            push_str(&mut p, MAX_QUERY_LEN + 1);
            ("search text over the cap", p)
        }
        2 => {
            // A request for a piece of the saved file that is one byte longer than a piece may be.
            let mut p = header(12, id);
            push_u32(&mut p, 1);
            push_u32(&mut p, 0);
            push_u32(&mut p, MAX_SAVE_CHUNK as u32 + 1);
            ("save piece one byte too long", p)
        }
        3 => {
            // Markup of one character more than a page holds.
            let mut p = header(16, id);
            push_u32(&mut p, 1);
            push_u32(&mut p, 0);
            p.push(1);
            p.extend_from_slice(&[0, 0, 0, 255]);
            push_u32(&mut p, 0);
            push_u32(&mut p, MAX_MARKUP_CHARS as u32 + 1);
            ("markup of one character too many", p)
        }
        4 => {
            // Markup with no characters.
            let mut p = header(16, id);
            push_u32(&mut p, 1);
            push_u32(&mut p, 0);
            p.push(1);
            p.extend_from_slice(&[0, 0, 0, 255]);
            push_u32(&mut p, 7);
            push_u32(&mut p, 7);
            ("markup of no characters", p)
        }
        5 => {
            // Markup that is a note.
            let mut p = header(16, id);
            push_u32(&mut p, 1);
            push_u32(&mut p, 0);
            p.push(4);
            p.extend_from_slice(&[0, 0, 0, 255]);
            push_u32(&mut p, 0);
            push_u32(&mut p, 1);
            ("markup of a kind that is not markup", p)
        }
        6 => {
            // A note whose text is one byte over the cap.
            let mut p = header(17, id);
            push_u32(&mut p, 1);
            push_u32(&mut p, 0);
            push_f32(&mut p, 1.0);
            push_f32(&mut p, 2.0);
            p.extend_from_slice(&[0, 0, 0, 255]);
            push_str(&mut p, MAX_ANNOT_TEXT + 1);
            ("note text one byte too long", p)
        }
        7 => {
            // A note at a place that is not a number.
            let mut p = header(17, id);
            push_u32(&mut p, 1);
            push_u32(&mut p, 0);
            push_f32(&mut p, f32::NAN);
            push_f32(&mut p, 2.0);
            p.extend_from_slice(&[0, 0, 0, 255]);
            push_str(&mut p, 3);
            ("note at a place that is not a number", p)
        }
        8 => {
            // A drawing of one stroke more than the cap, each stroke one point.
            let mut p = header(18, id);
            push_u32(&mut p, 1);
            push_u32(&mut p, 0);
            p.extend_from_slice(&[0, 0, 0, 255]);
            push_f32(&mut p, 2.0);
            push_u32(&mut p, MAX_INK_STROKES as u32 + 1);
            for _ in 0..=MAX_INK_STROKES {
                push_u32(&mut p, 1);
                push_f32(&mut p, 1.0);
                push_f32(&mut p, 1.0);
            }
            ("drawing of one stroke too many", p)
        }
        9 => {
            // A drawing of one point more than the cap, in two strokes.
            let mut p = header(18, id);
            push_u32(&mut p, 1);
            push_u32(&mut p, 0);
            p.extend_from_slice(&[0, 0, 0, 255]);
            push_f32(&mut p, 2.0);
            push_u32(&mut p, 2);
            for n in [MAX_INK_POINTS - 1, 2] {
                push_u32(&mut p, n as u32);
                for _ in 0..n {
                    push_f32(&mut p, 1.0);
                    push_f32(&mut p, 1.0);
                }
            }
            ("drawing of one point too many", p)
        }
        10 => {
            // A line a little thicker than the thickest.
            let mut p = header(18, id);
            push_u32(&mut p, 1);
            push_u32(&mut p, 0);
            p.extend_from_slice(&[0, 0, 0, 255]);
            push_f32(&mut p, MAX_INK_WIDTH + 0.25);
            push_u32(&mut p, 1);
            push_u32(&mut p, 1);
            push_f32(&mut p, 1.0);
            push_f32(&mut p, 1.0);
            ("drawing with a line too thick", p)
        }
        11 => {
            // A change with text one byte over the cap.
            let mut p = header(20, id);
            push_u32(&mut p, 1);
            push_u32(&mut p, 0);
            push_u32(&mut p, 3);
            p.push(1);
            push_str(&mut p, MAX_ANNOT_TEXT + 1);
            push_f32(&mut p, 0.0);
            push_f32(&mut p, 0.0);
            ("annotation text one byte too long", p)
        }
        12 => {
            // A field named with one byte more than a name may have.
            let mut p = header(42, id);
            for v in [1u32, 0, 0] {
                push_u32(&mut p, v);
            }
            push_str(&mut p, MAX_FORM_NAME + 1);
            p.extend_from_slice(&[2, 1]);
            ("form field name over the cap", p)
        }
        13 => {
            // A text for a field that is one byte longer than a value may be.
            let mut p = header(42, id);
            for v in [1u32, 0, 0] {
                push_u32(&mut p, v);
            }
            push_str(&mut p, 1);
            p.push(1);
            push_str(&mut p, MAX_FORM_VALUE + 1);
            ("form field value over the cap", p)
        }
        _ => {
            // The time of the check is later than the end of the year 9999.
            let mut p = header(60, id);
            push_u32(&mut p, 1);
            let now = MAX_UNIX_TIME as u64 + 1;
            push_u32(&mut p, now as u32);
            push_u32(&mut p, (now >> 32) as u32);
            ("signature check time past the year 9999", p)
        }
    }
}

/// How many cases `over_cap_response_case` has.
pub const OVER_CAP_RESPONSES: usize = 37 + crate::pages::PAGES_OVER_RESPONSES;

/// The two cases that are 1.7 MB.
const PAGE_TEXT_OVER: usize = 6;
const PAGE_TEXT_AT_CAP: usize = 11;
/// The two cases that are 1 MiB.
const SAVE_PIECE_OVER: usize = 17;
const SAVE_PIECE_AT_CAP: usize = 18;
/// The three cases of the page tools that are 800 KB, 320 KB and 800 KB.
const PAGES_BIG: [usize; 3] = [37, 38, 39];

/// A response that is valid except for one thing, which is one over a cap. Two cases sit exactly
/// at a cap instead (the biggest page text, the widest bitmap) and must be accepted. Returns a
/// name, the payload, and whether the decoder must accept it.
pub fn over_cap_response_case(case: usize, id: u32) -> (&'static str, Vec<u8>, bool) {
    match case % OVER_CAP_RESPONSES {
        n if n >= 37 => crate::pages::pages_over_cap_response_case(n - 37, id),
        0 => {
            let mut p = header(133, id);
            push_u32(&mut p, MAX_OUTLINE_ITEMS as u32 + 1);
            for _ in 0..=MAX_OUTLINE_ITEMS {
                p.push(0);
                push_u32(&mut p, u32::MAX);
                push_u32(&mut p, 0);
            }
            ("outline with one entry too many", p, false)
        }
        1 => {
            let mut p = header(133, id);
            push_u32(&mut p, 1);
            p.push(MAX_OUTLINE_DEPTH + 1);
            push_u32(&mut p, 0);
            push_u32(&mut p, 0);
            ("outline entry one level too deep", p, false)
        }
        2 => {
            let mut p = header(133, id);
            push_u32(&mut p, 1);
            p.push(0);
            push_u32(&mut p, 0);
            push_str(&mut p, MAX_OUTLINE_TITLE + 1);
            ("outline title one byte too long", p, false)
        }
        3 => {
            let mut p = header(135, id);
            push_u32(&mut p, MAX_LINKS_PER_PAGE as u32 + 1);
            for _ in 0..=MAX_LINKS_PER_PAGE {
                push_rect(&mut p);
                p.push(1);
                push_u32(&mut p, 0);
            }
            ("page with one link too many", p, false)
        }
        4 => {
            let mut p = header(135, id);
            push_u32(&mut p, 1);
            push_rect(&mut p);
            p.push(0);
            push_str(&mut p, MAX_URI_LEN + 1);
            ("link address one byte too long", p, false)
        }
        5 => {
            let mut p = header(136, id);
            push_u32(&mut p, MAX_MATCHES_PER_PAGE as u32 + 1);
            for i in 0..=MAX_MATCHES_PER_PAGE as u32 {
                push_u32(&mut p, i * 2);
                push_u32(&mut p, 1);
            }
            ("page with one match too many", p, false)
        }
        n @ (PAGE_TEXT_OVER | PAGE_TEXT_AT_CAP) => {
            let chars = if n == PAGE_TEXT_OVER {
                MAX_PAGE_CHARS + 1
            } else {
                MAX_PAGE_CHARS
            };
            let mut p = header(134, id);
            push_u32(&mut p, chars as u32);
            push_u32(&mut p, chars as u32);
            p.resize(p.len() + chars, b'a');
            for _ in 0..chars {
                push_rect(&mut p);
            }
            if n == PAGE_TEXT_OVER {
                ("page text with one character too many", p, false)
            } else {
                ("page text of the biggest size", p, true)
            }
        }
        7 => {
            let mut p = header(129, id);
            push_u32(&mut p, 2);
            push_str(&mut p, MAX_STRING_LEN + 1);
            push_str(&mut p, 0);
            ("hello string one byte too long", p, false)
        }
        8 => {
            let mut p = header(255, id);
            p.push(2);
            push_str(&mut p, MAX_STRING_LEN + 1);
            ("error text one byte too long", p, false)
        }
        9 => {
            let width = MAX_RENDER_SIDE + 1;
            let mut p = header(131, id);
            push_u32(&mut p, width);
            push_u32(&mut p, 1);
            push_u32(&mut p, width * 4);
            p.resize(p.len() + width as usize * 4, 0);
            ("bitmap one pixel too wide", p, false)
        }
        10 => {
            let width = MAX_RENDER_SIDE;
            let mut p = header(131, id);
            push_u32(&mut p, width);
            push_u32(&mut p, 1);
            push_u32(&mut p, width * 4);
            p.resize(p.len() + width as usize * 4, 0);
            ("bitmap of the widest size", p, true)
        }
        12 => {
            // One pixel wide and one too many rows: only the height is over.
            let height = MAX_RENDER_SIDE + 1;
            let mut p = header(131, id);
            push_u32(&mut p, 1);
            push_u32(&mut p, height);
            push_u32(&mut p, height * 4);
            p.resize(p.len() + height as usize * 4, 0);
            ("bitmap one pixel too high", p, false)
        }
        13 => {
            let mut p = header(139, id);
            push_u32(&mut p, MAX_SAVE_BYTES as u32 + 1);
            ("save size one byte over the biggest file", p, false)
        }
        14 => {
            let mut p = header(139, id);
            push_u32(&mut p, MAX_SAVE_BYTES as u32);
            ("save size of the biggest file", p, true)
        }
        15 => {
            // The last byte of the biggest file is the last byte this piece may reach.
            let mut p = header(140, id);
            push_u32(&mut p, MAX_SAVE_BYTES as u32 - 4);
            push_u32(&mut p, 4);
            p.extend_from_slice(b"tail");
            (
                "save piece that ends on the last byte of the biggest file",
                p,
                true,
            )
        }
        16 => {
            let mut p = header(140, id);
            push_u32(&mut p, MAX_SAVE_BYTES as u32 - 4);
            push_u32(&mut p, 5);
            p.extend_from_slice(b"tail!");
            (
                "save piece that ends one byte past the biggest file",
                p,
                false,
            )
        }
        n @ (SAVE_PIECE_OVER | SAVE_PIECE_AT_CAP) => {
            let len = if n == SAVE_PIECE_OVER {
                MAX_SAVE_CHUNK + 1
            } else {
                MAX_SAVE_CHUNK
            };
            let mut p = header(140, id);
            push_u32(&mut p, 0);
            push_u32(&mut p, len as u32);
            p.resize(p.len() + len, b'a');
            if n == SAVE_PIECE_OVER {
                ("save piece one byte too long", p, false)
            } else {
                ("save piece of the biggest size", p, true)
            }
        }
        19 => {
            // The length says 10, the piece holds 9.
            let mut p = header(140, id);
            push_u32(&mut p, 0);
            push_u32(&mut p, 10);
            p.extend_from_slice(b"123456789");
            ("save piece shorter than it says", p, false)
        }
        20 => {
            let mut p = header(144, id);
            push_u32(&mut p, MAX_ANNOTS_PER_PAGE as u32 + 1);
            for _ in 0..=MAX_ANNOTS_PER_PAGE {
                push_annot(&mut p, 0, 0);
            }
            ("annotation list with one entry too many", p, false)
        }
        21 => {
            let mut p = header(145, id);
            push_annot(&mut p, MAX_ANNOT_TEXT + 1, 0);
            ("annotation text one byte too long", p, false)
        }
        22 => {
            let mut p = header(145, id);
            push_annot(&mut p, 0, MAX_ANNOT_AUTHOR + 1);
            ("annotation author one byte too long", p, false)
        }
        23 => {
            let mut p = header(145, id);
            push_annot(&mut p, 0, 0);
            // The ability byte sits after the id and the kind.
            p[1 + 4 + 4 + 1] = ability::ALL + 1;
            ("annotation with an ability nobody defined", p, false)
        }
        24 => {
            let mut p = header(144, id);
            push_u32(&mut p, MAX_ANNOTS_PER_PAGE as u32);
            for _ in 0..MAX_ANNOTS_PER_PAGE {
                push_annot(&mut p, 0, 0);
            }
            ("annotation list of the longest size", p, true)
        }
        25 => {
            let mut p = header(145, id);
            push_annot(&mut p, MAX_ANNOT_TEXT, MAX_ANNOT_AUTHOR);
            ("annotation with the longest text and author", p, true)
        }
        26 => (
            "form field name over the cap",
            form_answer(id, MAX_FORM_NAME + 1, 1, 0, 0),
            false,
        ),
        27 => (
            "form field value over the cap",
            form_answer(id, 1, MAX_FORM_VALUE + 1, 0, 0),
            false,
        ),
        28 => (
            "form field with one choice too many",
            form_answer(id, 1, 1, MAX_FORM_OPTIONS + 1, 0),
            false,
        ),
        29 => (
            "form field at every cap",
            form_answer(id, MAX_FORM_NAME, MAX_FORM_VALUE, MAX_FORM_OPTIONS, 4),
            true,
        ),
        30 => {
            let mut p = header(171, id);
            push_u32(&mut p, MAX_FORM_FIELDS_PER_PAGE as u32 + 1);
            for _ in 0..=MAX_FORM_FIELDS_PER_PAGE {
                p.extend_from_slice(&raw_form_field(1, 1, 0, 0));
            }
            ("page with one form field too many", p, false)
        }
        31 => {
            let mut p = header(190, id);
            for v in [MAX_SIGNATURES as u32 + 1, 0, 0, MAX_SIGNATURES as u32 + 1] {
                push_u32(&mut p, v);
            }
            for index in 0..=MAX_SIGNATURES as u32 {
                push_signature(&mut p, index, 0, 0);
            }
            ("signature report with one signature too many", p, false)
        }
        32 => {
            let mut p = signature_report(id);
            push_signature(&mut p, 0, MAX_SIG_NAME + 1, 0);
            ("signer name one byte too long", p, false)
        }
        33 => {
            let mut p = signature_report(id);
            push_signature(&mut p, 0, MAX_SIG_NAME, 0);
            ("signer name of the most bytes", p, true)
        }
        34 => {
            let mut p = signature_report(id);
            push_signature(&mut p, 0, 0, MAX_BYTE_RANGE + 1);
            ("signature byte range of one number too many", p, false)
        }
        35 => {
            // A tick for a signature whose reason is a broken one.
            let mut p = signature_report(id);
            push_signature(&mut p, 0, 0, 0);
            p[1 + 4 + 12 + 4 + 4] = 1;
            (
                "trusted state with the reason of a broken signature",
                p,
                false,
            )
        }
        36 => {
            let mut p = signature_report(id);
            push_signature(&mut p, 0, 0, 0);
            // Make the signer a text with a line break in it.
            let at = p.len() - 13;
            p[at] = 1;
            p.splice(at + 1..at + 1, [2, 0, 0, 0, b'a', 10]);
            ("signer name with a line break", p, false)
        }
        _ => unreachable!("the case number is taken modulo the number of cases"),
    }
}

/// The start of a report with one signature in it.
fn signature_report(id: u32) -> Vec<u8> {
    let mut p = header(190, id);
    for v in [1u32, 0, 0, 1] {
        push_u32(&mut p, v);
    }
    p
}

/// One described signature (unknown signer, no trust list) whose signer name has `name` bytes and
/// whose byte range has `ranges` numbers; the rest is empty.
fn push_signature(out: &mut Vec<u8>, index: u32, name: usize, ranges: usize) {
    push_u32(out, index);
    out.extend_from_slice(&[2, 50, 1, 0, 1]);
    push_u32(out, 0);
    out.push(ranges as u8);
    for _ in 0..ranges {
        out.extend_from_slice(&0u64.to_le_bytes());
    }
    out.extend_from_slice(&0u64.to_le_bytes());
    // The signer, then twelve more optional texts, all absent.
    if name > 0 {
        out.push(1);
        push_str(out, name);
    } else {
        out.push(0);
    }
    out.extend_from_slice(&[0; 12]);
}

/// One listed annotation: a note with the given lengths of text and author. Bytes of an entry:
/// the id, the kind, the abilities, the box, the colour, the text and the author.
fn push_annot(out: &mut Vec<u8>, text: usize, author: usize) {
    push_u32(out, 1);
    out.push(4);
    out.push(ability::EDIT_TEXT);
    push_rect(out);
    out.extend_from_slice(&[255, 235, 59, 255]);
    push_str(out, text);
    push_str(out, author);
}

/// A request case picked by the fuzzer's bytes.
pub fn over_cap_request(u: &mut Unstructured) -> Result<(&'static str, Vec<u8>)> {
    let case = u.int_in_range(0..=OVER_CAP_REQUESTS - 1)?;
    let id: u32 = u.arbitrary()?;
    Ok(over_cap_request_case(case, id))
}

/// A response case picked by the fuzzer's bytes. The page text cases are 1.7 MB each, so they
/// are picked less often than the rest.
pub fn over_cap_response(u: &mut Unstructured) -> Result<(&'static str, Vec<u8>, bool)> {
    let mut case = u.int_in_range(0..=OVER_CAP_RESPONSES - 1)?;
    let big = matches!(
        case,
        PAGE_TEXT_OVER | PAGE_TEXT_AT_CAP | SAVE_PIECE_OVER | SAVE_PIECE_AT_CAP
    ) || PAGES_BIG.contains(&case);
    if big && !chance(u, 16)? {
        case = 0;
    }
    let id: u32 = u.arbitrary()?;
    Ok(over_cap_response_case(case, id))
}
