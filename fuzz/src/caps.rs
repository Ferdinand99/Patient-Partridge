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
    LinkTarget, Request, RequestBody, Response, ResponseBody, MAX_LINKS_PER_PAGE,
    MAX_MATCHES_PER_PAGE, MAX_OUTLINE_DEPTH, MAX_OUTLINE_ITEMS, MAX_OUTLINE_TITLE, MAX_PAGE_CHARS,
    MAX_PAGE_TEXT_BYTES, MAX_QUERY_LEN, MAX_RENDER_PIXELS, MAX_RENDER_SIDE, MAX_STRING_LEN,
    MAX_URI_LEN,
};

use crate::chance;

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
        ResponseBody::Opened { .. } | ResponseBody::Closed => {}
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
        ResponseBody::Error { message, .. } => {
            assert!(message.len() <= MAX_STRING_LEN);
        }
    }
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

/// How many cases `over_cap_request_case` has.
pub const OVER_CAP_REQUESTS: usize = 2;

/// A request that is valid except that one string is one byte over its cap. Returns a name and
/// the payload; the decoder must return an error.
pub fn over_cap_request_case(case: usize, id: u32) -> (&'static str, Vec<u8>) {
    match case % OVER_CAP_REQUESTS {
        0 => {
            let mut p = header(2, id);
            p.push(1);
            push_str(&mut p, MAX_STRING_LEN + 1);
            ("open password over the cap", p)
        }
        _ => {
            let mut p = header(8, id);
            push_u32(&mut p, 1);
            push_u32(&mut p, 0);
            push_str(&mut p, MAX_QUERY_LEN + 1);
            ("search text over the cap", p)
        }
    }
}

/// How many cases `over_cap_response_case` has.
pub const OVER_CAP_RESPONSES: usize = 13;

/// The two cases that are 1.7 MB.
const PAGE_TEXT_OVER: usize = 6;
const PAGE_TEXT_AT_CAP: usize = 11;

/// A response that is valid except for one thing, which is one over a cap. Two cases sit exactly
/// at a cap instead (the biggest page text, the widest bitmap) and must be accepted. Returns a
/// name, the payload, and whether the decoder must accept it.
pub fn over_cap_response_case(case: usize, id: u32) -> (&'static str, Vec<u8>, bool) {
    match case % OVER_CAP_RESPONSES {
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
        _ => {
            // One pixel wide and one too many rows: only the height is over.
            let height = MAX_RENDER_SIDE + 1;
            let mut p = header(131, id);
            push_u32(&mut p, 1);
            push_u32(&mut p, height);
            push_u32(&mut p, height * 4);
            p.resize(p.len() + height as usize * 4, 0);
            ("bitmap one pixel too high", p, false)
        }
    }
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
    if (case == PAGE_TEXT_OVER || case == PAGE_TEXT_AT_CAP) && !chance(u, 16)? {
        case = 0;
    }
    let id: u32 = u.arbitrary()?;
    Ok(over_cap_response_case(case, id))
}
