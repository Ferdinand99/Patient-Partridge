//! What a saved file holds of things that were made in the session and taken off again.
//!
//! PDFium writes every object made in the session, also when nothing refers to it any more. A save
//! therefore writes the bytes, reads them in again and writes them a second time (see
//! `save::serialise_clean`). These tests save after a picture of a signature, a drawing, a stamp or a
//! note was put on a page and taken off again, and look at the streams of the file itself.

mod common;

use common::*;
use partridge_engine::Engine;
use partridge_protocol::{
    ErrorCode, PagesRequest, RequestBody, ResponseBody, SignatureFont, SignatureMark, StampKind,
};
use partridge_testkit as kit;

const INK: [u8; 4] = [10, 40, 120, 255];

/// Pixels of a colour that nothing else in a file has.
const MARKER: [u8; 3] = [0x11, 0x22, 0x33];

fn marked_image(width: u32, height: u32) -> SignatureMark {
    let mut rgba = Vec::new();
    for _ in 0..width * height {
        rgba.extend_from_slice(&[MARKER[0], MARKER[1], MARKER[2], 255]);
    }
    SignatureMark::Image {
        width,
        height,
        rgba,
    }
}

/// Noise that a compressor cannot shrink, so that the size of a saved file follows the pictures in it.
fn noise_image(width: u32, height: u32) -> SignatureMark {
    let mut seed: u32 = 12345;
    let mut rgba = Vec::new();
    for _ in 0..width * height {
        for _ in 0..3 {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            rgba.push((seed >> 16) as u8);
        }
        rgba.push(255);
    }
    SignatureMark::Image {
        width,
        height,
        rgba,
    }
}

fn scribble() -> SignatureMark {
    let mut line = Vec::new();
    for i in 0..=60 {
        let t = i as f32 / 60.0;
        line.push([
            10.0 + t * 200.0,
            50.0 + 30.0 * (t * 17.0).sin() * (1.0 - t * 0.5),
        ]);
    }
    SignatureMark::Strokes {
        width: 240.0,
        height: 100.0,
        line: 5.0,
        strokes: vec![line],
    }
}

fn typed() -> SignatureMark {
    SignatureMark::Typed {
        font: SignatureFont::GreatVibes,
        text: "Anna Alvorlig".into(),
    }
}

fn place(e: &mut Engine, doc: u32, mark: SignatureMark) -> u32 {
    annot(ask(
        e,
        RequestBody::AddSignature {
            doc,
            page: 0,
            rect: [100.0, 100.0, 400.0, 300.0],
            color: INK,
            mark,
        },
    ))
    .id
}

fn take_off(e: &mut Engine, doc: u32, id: u32) {
    let answer = ask(e, RequestBody::RemoveAnnot { doc, page: 0, id });
    assert!(matches!(answer, ResponseBody::AnnotRemoved), "{answer:?}");
}

fn put_back(e: &mut Engine, doc: u32, id: u32) {
    annot(ask(e, RequestBody::RestoreAnnot { doc, page: 0, id }));
}

fn blank_file(e: &mut Engine) -> Vec<u8> {
    let doc = open(e, &kit::blank_pages_pdf(1), None).unwrap();
    save_all(e, doc)
}

fn holds_marker(file: &[u8]) -> bool {
    let run: Vec<u8> = MARKER.iter().copied().cycle().take(60).collect();
    let has = |h: &[u8]| h.windows(run.len()).any(|w| w == run.as_slice());
    has(file) || kit::inflated_streams(file).iter().any(|s| has(s))
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}

#[test]
fn a_picture_that_is_on_the_page_is_found_by_the_check() {
    // The control for the tests below: they look for the pixels of the picture in the file, and this
    // shows the look finds them when the picture is there.
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::blank_pages_pdf(1), None).unwrap();
    place(e, doc, marked_image(200, 80));
    let saved = save_all(e, doc);
    assert!(holds_marker(&saved));
    assert!(count(&saved, b"/Image") >= 1);
}

#[test]
fn a_picture_that_was_taken_off_is_not_in_the_saved_file() {
    let mut guard = engine();
    let e = &mut guard.0;
    let blank = blank_file(e);
    let blank_streams = kit::inflated_streams(&blank).len();
    for (name, mark) in [
        ("image", marked_image(200, 80)),
        ("drawn", scribble()),
        ("typed", typed()),
    ] {
        let doc = open(e, &kit::blank_pages_pdf(1), None).unwrap();
        let id = place(e, doc, mark);
        take_off(e, doc, id);
        let saved = save_all(e, doc);
        assert!(!holds_marker(&saved), "{name}: the pixels are in the file");
        assert_eq!(
            count(&saved, b"/Image"),
            0,
            "{name}: an image is in the file"
        );
        assert_eq!(
            count(&saved, b"/Stamp"),
            0,
            "{name}: the stamp is in the file"
        );
        assert_eq!(
            kit::inflated_streams(&saved).len(),
            blank_streams,
            "{name}: a stream is left in the file"
        );
        assert!(
            saved.len() <= blank.len() + 64,
            "{name}: {} bytes, a file with nothing on it has {}",
            saved.len(),
            blank.len()
        );
    }
}

#[test]
fn a_picture_that_was_put_back_and_taken_off_again_is_not_in_the_saved_file() {
    let mut guard = engine();
    let e = &mut guard.0;
    let blank = blank_file(e);
    let doc = open(e, &kit::blank_pages_pdf(1), None).unwrap();
    let id = place(e, doc, marked_image(200, 80));
    take_off(e, doc, id);
    put_back(e, doc, id);
    take_off(e, doc, id);
    let saved = save_all(e, doc);
    assert!(!holds_marker(&saved));
    assert_eq!(count(&saved, b"/Image"), 0);
    assert!(saved.len() <= blank.len() + 64);
}

#[test]
fn undo_and_redo_do_not_make_the_saved_file_grow() {
    let mut guard = engine();
    let e = &mut guard.0;
    let blank = blank_file(e);
    let mut sizes = Vec::new();
    for cycles in [0usize, 3, 6] {
        let doc = open(e, &kit::blank_pages_pdf(1), None).unwrap();
        let id = place(e, doc, noise_image(512, 256));
        for _ in 0..cycles {
            take_off(e, doc, id);
            put_back(e, doc, id);
        }
        let saved = save_all(e, doc);
        // The picture and the soft mask that holds its transparency.
        assert_eq!(count(&saved, b"/Subtype/Image"), 2, "{cycles} cycles");
        sizes.push(saved.len());
    }
    assert!(
        sizes[0] > blank.len() + 100_000,
        "the picture is in the file: {sizes:?}"
    );
    assert!(
        sizes[1] <= sizes[0] + 256 && sizes[2] <= sizes[0] + 256,
        "the file grew with the cycles: {sizes:?}"
    );
    // Taken off after all those cycles, nothing is left of it.
    let doc = open(e, &kit::blank_pages_pdf(1), None).unwrap();
    let id = place(e, doc, noise_image(512, 256));
    for _ in 0..6 {
        take_off(e, doc, id);
        put_back(e, doc, id);
    }
    take_off(e, doc, id);
    let saved = save_all(e, doc);
    assert!(saved.len() <= blank.len() + 64, "{} bytes", saved.len());
}

#[test]
fn drawings_stamps_and_notes_that_were_deleted_leave_nothing_in_the_saved_file() {
    let mut guard = engine();
    let e = &mut guard.0;
    let blank = blank_file(e);
    let doc = open(e, &kit::blank_pages_pdf(1), None).unwrap();
    let note = annot(ask(
        e,
        RequestBody::AddNote {
            doc,
            page: 0,
            x: 100.0,
            y: 150.0,
            color: [255, 220, 0, 255],
            text: "Zz secret note text".into(),
        },
    ))
    .id;
    let ink = annot(ask(
        e,
        RequestBody::AddInk {
            doc,
            page: 0,
            color: [0, 0, 255, 255],
            width: 4.0,
            strokes: vec![vec![[100.0, 200.0], [150.0, 220.0], [200.0, 200.0]]],
        },
    ))
    .id;
    let stamp = annot(ask(
        e,
        RequestBody::AddStamp {
            doc,
            page: 0,
            x: 300.0,
            y: 300.0,
            stamp: StampKind::ALL[0],
        },
    ))
    .id;
    // Saved with them on the page, they are in the file.
    let with = save_all(e, doc);
    assert!(kit::text_in_file(&with, "Zz secret note text"));
    for id in [note, ink, stamp] {
        take_off(e, doc, id);
    }
    let saved = save_all(e, doc);
    assert!(!kit::text_in_file(&saved, "Zz secret note text"));
    assert_eq!(count(&saved, b"/Subtype/Ink"), 0);
    assert_eq!(count(&saved, b"/Subtype/Stamp"), 0);
    assert_eq!(
        kit::inflated_streams(&saved).len(),
        kit::inflated_streams(&blank).len()
    );
    assert!(
        saved.len() <= blank.len() + 64,
        "{} bytes, a file with nothing on it has {}",
        saved.len(),
        blank.len()
    );
}

#[test]
fn a_protected_document_is_saved_the_same_way_and_still_needs_its_password() {
    let mut guard = engine();
    let e = &mut guard.0;
    let bytes = kit::encrypted_pdf("secret", 1);
    let doc = open(e, &bytes, Some("secret")).unwrap();
    let id = place(e, doc, marked_image(100, 40));
    take_off(e, doc, id);
    let saved = save_all(e, doc);
    assert!(count(&saved, b"/Encrypt") >= 1);
    assert_eq!(
        open(e, &saved, None).err(),
        Some(ErrorCode::PasswordRequired)
    );
    let again = open(e, &saved, Some("secret")).unwrap();
    assert!(list(e, again, 0).is_empty());
}

#[test]
fn putting_a_big_picture_back_again_and_again_is_stopped_before_the_engine_runs_out_of_memory() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::blank_pages_pdf(1), None).unwrap();
    let id = place(e, doc, noise_image(1_024, 512));
    let mut cycles = 0;
    let mut stopped = None;
    for _ in 0..400 {
        take_off(e, doc, id);
        match ask(e, RequestBody::RestoreAnnot { doc, page: 0, id }) {
            ResponseBody::Annot { .. } => cycles += 1,
            other => {
                stopped = Some(error_code(other));
                break;
            }
        }
    }
    assert_eq!(
        stopped,
        Some(ErrorCode::AnnotationRefused),
        "{cycles} cycles"
    );
    assert!((50..=200).contains(&cycles), "{cycles} cycles");
    // The picture stays off the page, and the document still saves.
    assert!(list(e, doc, 0).is_empty());
    let saved = save_all(e, doc);
    assert_eq!(count(&saved, b"/Subtype/Image"), 0);
}

#[test]
fn the_limit_on_pictures_made_goes_on_across_a_change_to_the_pages() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::blank_pages_pdf(1), None).unwrap();
    let id = place(e, doc, noise_image(1_024, 512));
    let mut stopped = false;
    for _ in 0..400 {
        take_off(e, doc, id);
        if !matches!(
            ask(e, RequestBody::RestoreAnnot { doc, page: 0, id }),
            ResponseBody::Annot { .. }
        ) {
            stopped = true;
            break;
        }
    }
    assert!(stopped);
    // PDFium keeps what it made, and the document is changed in place: a new page does not give
    // the allowance back.
    let added = ask(
        e,
        RequestBody::Pages(PagesRequest::InsertBlank {
            doc,
            at: 1,
            width: 300.0,
            height: 200.0,
        }),
    );
    assert!(matches!(added, ResponseBody::Pages(_)), "{added:?}");
    let more = ask(
        e,
        RequestBody::AddSignature {
            doc,
            page: 1,
            rect: [100.0, 100.0, 200.0, 150.0],
            color: INK,
            mark: noise_image(1_024, 512),
        },
    );
    assert_eq!(error_code(more), ErrorCode::AnnotationRefused);
}
