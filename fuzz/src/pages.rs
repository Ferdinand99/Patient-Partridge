//! The messages of the page tools for the fuzz targets: valid messages built from fuzzer bytes,
//! the caps as assertions, and messages that are one over a cap (which the decoder must refuse).

use arbitrary::{Result, Unstructured};
use partridge_protocol::{
    PageSize, PagesRequest, PagesResponse, MAX_BOX_VALUE, MAX_EDIT_PAGES, MAX_IMPORT_BYTES,
    MAX_PAGES, MAX_SAVE_BYTES, MAX_SAVE_CHUNK,
};

use crate::{chance, count_up_to};

/// A number a box may hold: finite and inside the bound the decoder keeps.
fn bounded(u: &mut Unstructured) -> Result<f32> {
    if chance(u, 12)? {
        // Exactly on the bound, which the decoder must accept.
        return Ok(if u.arbitrary()? { MAX_BOX_VALUE } else { -MAX_BOX_VALUE });
    }
    Ok(u.int_in_range(-4000..=4000i32)? as f32 / 8.0)
}

fn a_box(u: &mut Unstructured) -> Result<[f32; 4]> {
    Ok([bounded(u)?, bounded(u)?, bounded(u)?, bounded(u)?])
}

fn page_list(u: &mut Unstructured) -> Result<Vec<u32>> {
    let count = count_up_to(u, MAX_EDIT_PAGES)?;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(u.arbitrary()?);
    }
    Ok(out)
}

/// A page tool request that the encoder writes without cutting anything.
pub fn arb_pages_request(u: &mut Unstructured) -> Result<PagesRequest> {
    let doc: u32 = u.arbitrary()?;
    Ok(match u.int_in_range(0..=11u8)? {
        0 => PagesRequest::Delete {
            doc,
            pages: page_list(u)?,
        },
        1 => PagesRequest::Reorder {
            doc,
            order: page_list(u)?,
        },
        2 => PagesRequest::Rotate {
            doc,
            pages: page_list(u)?,
            turns: u.arbitrary()?,
        },
        3 => PagesRequest::Crop {
            doc,
            pages: page_list(u)?,
            margins: a_box(u)?,
        },
        4 => {
            let pages = page_list(u)?;
            let boxes = pages
                .iter()
                .map(|_| a_box(u))
                .collect::<Result<Vec<_>>>()?;
            PagesRequest::SetBoxes { doc, pages, boxes }
        }
        5 => PagesRequest::InsertBlank {
            doc,
            at: u.arbitrary()?,
            width: bounded(u)?,
            height: bounded(u)?,
        },
        6 => PagesRequest::Restore {
            doc,
            snapshot: u.arbitrary()?,
        },
        7 => PagesRequest::ImportBegin {
            doc,
            size: if chance(u, 8)? {
                MAX_IMPORT_BYTES as u32
            } else {
                u.int_in_range(0..=MAX_IMPORT_BYTES as u32)?
            },
        },
        8 => {
            let len = if chance(u, 12)? {
                MAX_SAVE_CHUNK
            } else {
                u.int_in_range(0..=40usize)?
            };
            let mut data = vec![0u8; len];
            u.fill_buffer(&mut data)?;
            PagesRequest::ImportChunk {
                doc,
                offset: u.arbitrary()?,
                data,
            }
        }
        9 => PagesRequest::ImportEnd {
            doc,
            at: u.arbitrary()?,
        },
        10 => PagesRequest::ImportAbort { doc },
        _ => PagesRequest::ExportBegin {
            doc,
            pages: page_list(u)?,
        },
    })
}

fn sizes(u: &mut Unstructured, max: usize) -> Result<Vec<PageSize>> {
    let count = count_up_to(u, max)?;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let w: f32 = u.arbitrary()?;
        let h: f32 = u.arbitrary()?;
        out.push(PageSize {
            width: if w.is_finite() { w } else { 1.0 },
            height: if h.is_finite() { h } else { 1.0 },
        });
    }
    Ok(out)
}

/// A page tool answer that the encoder writes without cutting anything.
pub fn arb_pages_response(u: &mut Unstructured) -> Result<PagesResponse> {
    Ok(match u.int_in_range(0..=5u8)? {
        0 => PagesResponse::Changed {
            // Now and then as many pages as a document may have.
            pages: sizes(u, MAX_PAGES)?,
            snapshot: u.arbitrary()?,
            count: u.arbitrary()?,
            form_fields: u.arbitrary()?,
        },
        1 => {
            let count = count_up_to(u, MAX_EDIT_PAGES)?;
            let before = (0..count)
                .map(|_| a_box(u))
                .collect::<Result<Vec<_>>>()?;
            let after = (0..count)
                .map(|_| a_box(u))
                .collect::<Result<Vec<_>>>()?;
            PagesResponse::Cropped {
                pages: sizes(u, 8)?,
                before,
                after,
            }
        }
        2 => PagesResponse::ImportReady,
        3 => PagesResponse::ImportAck {
            received: u.arbitrary()?,
        },
        4 => PagesResponse::Exported {
            size: if chance(u, 8)? {
                MAX_SAVE_BYTES as u32
            } else {
                u.int_in_range(0..=MAX_SAVE_BYTES as u32)?
            },
            pages: if chance(u, 8)? {
                MAX_EDIT_PAGES as u32
            } else {
                u.int_in_range(0..=MAX_EDIT_PAGES as u32)?
            },
            signatures: u.arbitrary()?,
            source_security: u.arbitrary()?,
            digest: u.arbitrary()?,
        },
        _ => PagesResponse::Done,
    })
}

// ---- the caps as assertions --------------------------------------------------------------------

fn check_box(b: &[f32; 4]) {
    for v in b {
        assert!(v.is_finite() && v.abs() <= MAX_BOX_VALUE, "box number {v}");
    }
}

/// Panics when a page tool request is outside the caps the protocol promises.
pub fn check_pages_request_caps(request: &PagesRequest) {
    let list = |l: &Vec<u32>| assert!(l.len() <= MAX_EDIT_PAGES, "{} pages in a list", l.len());
    match request {
        PagesRequest::Delete { pages, .. }
        | PagesRequest::Rotate { pages, .. }
        | PagesRequest::ExportBegin { pages, .. } => list(pages),
        PagesRequest::Reorder { order, .. } => list(order),
        PagesRequest::Crop { pages, margins, .. } => {
            list(pages);
            check_box(margins);
        }
        PagesRequest::SetBoxes { pages, boxes, .. } => {
            list(pages);
            assert_eq!(pages.len(), boxes.len());
            boxes.iter().for_each(check_box);
        }
        PagesRequest::InsertBlank { width, height, .. } => {
            for v in [width, height] {
                assert!(v.is_finite() && v.abs() <= MAX_BOX_VALUE, "page side {v}");
            }
        }
        PagesRequest::ImportBegin { size, .. } => {
            assert!(*size as usize <= MAX_IMPORT_BYTES, "a document of {size} bytes");
        }
        PagesRequest::ImportChunk { data, .. } => {
            assert!(data.len() <= MAX_SAVE_CHUNK, "piece of {} bytes", data.len());
        }
        PagesRequest::Restore { .. } | PagesRequest::ImportEnd { .. } | PagesRequest::ImportAbort { .. } => {}
    }
}

/// Panics when a page tool answer is outside the caps the protocol promises.
pub fn check_pages_response_caps(response: &PagesResponse) {
    match response {
        PagesResponse::Changed { pages, .. } => {
            assert!(pages.len() <= MAX_PAGES, "{} pages", pages.len());
        }
        PagesResponse::Cropped {
            pages,
            before,
            after,
        } => {
            assert!(pages.len() <= MAX_PAGES);
            assert!(before.len() <= MAX_EDIT_PAGES, "{} boxes", before.len());
            assert_eq!(before.len(), after.len());
            before.iter().chain(after).for_each(check_box);
        }
        PagesResponse::Exported { size, pages, .. } => {
            assert!(*size as usize <= MAX_SAVE_BYTES, "a file of {size} bytes");
            assert!(*pages as usize <= MAX_EDIT_PAGES, "{pages} pages");
        }
        PagesResponse::ImportReady | PagesResponse::ImportAck { .. } | PagesResponse::Done => {}
    }
}

// ---- messages that break exactly one cap -------------------------------------------------------

fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn header(tag: u8, id: u32) -> Vec<u8> {
    let mut out = vec![tag];
    push_u32(&mut out, id);
    out
}

/// How many cases `pages_over_cap_request_case` has.
pub const PAGES_OVER_REQUESTS: usize = 8;

/// A page tool request that is valid except for one thing, which is one over a cap (or not a
/// number). The decoder must return an error.
pub fn pages_over_cap_request_case(case: usize, id: u32) -> (&'static str, Vec<u8>) {
    let list_over = |tag: u8| {
        let mut p = header(tag, id);
        push_u32(&mut p, 1);
        push_u32(&mut p, MAX_EDIT_PAGES as u32 + 1);
        for i in 0..=MAX_EDIT_PAGES as u32 {
            push_u32(&mut p, i);
        }
        p
    };
    match case % PAGES_OVER_REQUESTS {
        0 => ("delete with one page too many in the list", list_over(80)),
        1 => ("reorder with one page too many in the list", list_over(81)),
        2 => ("export with one page too many in the list", list_over(91)),
        3 => {
            let mut p = header(88, id);
            push_u32(&mut p, 1);
            push_u32(&mut p, 0);
            push_u32(&mut p, MAX_SAVE_CHUNK as u32 + 1);
            p.resize(p.len() + MAX_SAVE_CHUNK + 1, b'a');
            ("insert piece one byte too long", p)
        }
        4 => {
            let mut p = header(87, id);
            push_u32(&mut p, 1);
            push_u32(&mut p, MAX_IMPORT_BYTES as u32 + 1);
            ("insert of a document one byte over the biggest file", p)
        }
        5 => {
            let mut p = header(83, id);
            push_u32(&mut p, 1);
            push_u32(&mut p, 1);
            push_u32(&mut p, 0);
            for v in [f32::NAN, 1.0, 2.0, 3.0] {
                p.extend_from_slice(&v.to_le_bytes());
            }
            ("crop margins that are not a number", p)
        }
        6 => {
            let mut p = header(85, id);
            push_u32(&mut p, 1);
            push_u32(&mut p, 0);
            for v in [100.0f32, MAX_BOX_VALUE * 2.0] {
                p.extend_from_slice(&v.to_le_bytes());
            }
            ("blank page of an absurd height", p)
        }
        _ => {
            // One page named, and its box left out.
            let mut p = header(84, id);
            push_u32(&mut p, 1);
            push_u32(&mut p, 1);
            push_u32(&mut p, 0);
            ("set boxes with a box missing", p)
        }
    }
}

/// How many cases `pages_over_cap_response_case` has.
pub const PAGES_OVER_RESPONSES: usize = 7;

/// A page tool answer that is valid except for one thing, which is one over a cap. Some cases sit
/// exactly at a cap and must be accepted. Returns a name, the payload, and whether the decoder
/// must accept it. The cases that list pages are 80 KB to 800 KB.
pub fn pages_over_cap_response_case(case: usize, id: u32) -> (&'static str, Vec<u8>, bool) {
    let changed = |pages: usize| {
        let mut p = header(200, id);
        push_u32(&mut p, pages as u32);
        p.resize(p.len() + pages * 8, 0);
        // The sizes are zeros: valid for the decoder, which leaves the check of sizes to the app.
        push_u32(&mut p, 1);
        push_u32(&mut p, 1);
        push_u32(&mut p, 0);
        p
    };
    let exported = |size: u32, pages: u32| {
        let mut p = header(204, id);
        push_u32(&mut p, size);
        push_u32(&mut p, pages);
        push_u32(&mut p, 0);
        p.push(0);
        p.extend_from_slice(&7u64.to_le_bytes());
        p
    };
    match case % PAGES_OVER_RESPONSES {
        0 => ("changed document with one page too many", changed(MAX_PAGES + 1), false),
        1 => ("changed document with the most pages", changed(MAX_PAGES), true),
        2 => {
            let mut p = header(201, id);
            push_u32(&mut p, 0);
            push_u32(&mut p, MAX_EDIT_PAGES as u32 + 1);
            p.resize(p.len() + (MAX_EDIT_PAGES + 1) * 32, 0);
            ("crop answer with one box too many", p, false)
        }
        3 => (
            "copied document one byte over the biggest file",
            exported(MAX_SAVE_BYTES as u32 + 1, 1),
            false,
        ),
        4 => (
            "copied document of the biggest size",
            exported(MAX_SAVE_BYTES as u32, MAX_EDIT_PAGES as u32),
            true,
        ),
        5 => (
            "copied document with one page too many",
            exported(1, MAX_EDIT_PAGES as u32 + 1),
            false,
        ),
        _ => {
            // Says 1000 pages and holds 3.
            let mut p = header(200, id);
            push_u32(&mut p, 1000);
            p.resize(p.len() + 24, 0);
            ("changed document that says more pages than it holds", p, false)
        }
    }
}
