//! Annotations through the engine: adding, changing, removing and listing them, writing them with a
//! save and reading them back from the saved file, with appearance streams that other readers
//! can draw.
//!
//! These tests run the engine in this process, without the sandbox (the sandbox and the processes
//! are covered by `tests/annots_process.rs`). PDFium wants one user at a time, so the tests take
//! turns on one engine.

mod common;

use common::*;
use partridge_protocol::{ability, AnnotKind, RequestBody, StampKind};
use partridge_testkit as kit;
use pdfium_render::prelude::*;

#[test]
fn a_highlight_is_written_with_an_appearance_and_read_back() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::features_pdf(), None).unwrap();
    let (start, len) = find(e, doc, 0, "needle");
    let b = text_box(e, doc, 0, start, len);
    let before = Shot::take(e, doc, 0, 612, 792);

    let item = annot(ask(
        e,
        RequestBody::AddMarkup {
            doc,
            page: 0,
            kind: AnnotKind::Highlight,
            color: YELLOW,
            start,
            end: start + len,
        },
    ));
    assert_eq!(item.kind, AnnotKind::Highlight);
    assert!(close(item.rect, b, 2.0), "{:?} should match {b:?}", item.rect);
    assert!(item.abilities & ability::REMOVE != 0);
    assert!(item.abilities & ability::EDIT_TEXT != 0);

    // The engine draws it at once: the box turned yellow, and was not before.
    let after = Shot::take(e, doc, 0, 612, 792);
    assert_eq!(before.near(b, [255, 235, 59], 40), 0);
    assert!(
        after.near(b, [255, 235, 59], 40) > 50,
        "the highlight is not drawn"
    );

    // The save carries a real annotation with an appearance stream.
    let saved = save_all(e, doc);
    assert!(contains(&saved, b"/Highlight"));
    assert!(contains(&saved, b"/QuadPoints"));
    assert!(contains(&saved, b"/AP"));

    // A second reader (here PDFium again, on the saved file) sees it, with an appearance.
    let again = open(e, &saved, None).unwrap();
    let items = list(e, again, 0);
    let highlight = items
        .iter()
        .find(|i| i.kind == AnnotKind::Highlight)
        .expect("the highlight is in the saved file");
    assert!(close(highlight.rect, b, 2.0));
    assert_eq!(&highlight.color[..3], &YELLOW[..3]);
    let objects = appearance_objects(e, again, 0, PdfPageAnnotationType::Highlight);
    assert_eq!(objects.len(), 1);
    assert!(objects[0] > 0, "the highlight has no appearance stream");
    let read_back = Shot::take(e, again, 0, 612, 792);
    assert!(read_back.near(b, [255, 235, 59], 40) > 50);
}

#[test]
fn underline_and_strikeout_are_written_too() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::features_pdf(), None).unwrap();
    let (start, len) = find(e, doc, 0, "haystack");
    let b = text_box(e, doc, 0, start, len);
    let red = [220, 20, 20, 255];
    let under = annot(ask(
        e,
        RequestBody::AddMarkup {
            doc,
            page: 0,
            kind: AnnotKind::Underline,
            color: red,
            start,
            end: start + len,
        },
    ));
    assert_eq!(under.kind, AnnotKind::Underline);
    let (start, len) = find(e, doc, 0, "brown fox");
    let b2 = text_box(e, doc, 0, start, len);
    let strike = annot(ask(
        e,
        RequestBody::AddMarkup {
            doc,
            page: 0,
            kind: AnnotKind::Strikeout,
            color: red,
            start,
            end: start + len,
        },
    ));
    assert_eq!(strike.kind, AnnotKind::Strikeout);
    let saved = save_all(e, doc);
    assert!(contains(&saved, b"/Underline"));
    assert!(contains(&saved, b"/StrikeOut"));
    let again = open(e, &saved, None).unwrap();
    let shot = Shot::take(e, again, 0, 612, 792);
    assert!(shot.near(b, [220, 20, 20], 60) > 10, "no underline drawn");
    assert!(shot.near(b2, [220, 20, 20], 60) > 10, "no strike-out drawn");
    for kind in [
        PdfPageAnnotationType::Underline,
        PdfPageAnnotationType::Strikeout,
    ] {
        let objects = appearance_objects(e, again, 0, kind);
        assert_eq!(objects.len(), 1);
        assert!(objects[0] > 0);
    }
    let items = list(e, again, 0);
    let kinds: Vec<AnnotKind> = items.iter().map(|i| i.kind).collect();
    assert!(kinds.contains(&AnnotKind::Underline) && kinds.contains(&AnnotKind::Strikeout));
}

#[test]
fn a_note_keeps_its_text_and_its_place() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::simple_pdf(2), None).unwrap();
    let text = "Remember this \u{2713} blåbær <b>not bold</b>\nsecond line";
    let item = annot(ask(
        e,
        RequestBody::AddNote {
            doc,
            page: 1,
            x: 100.0,
            y: 150.0,
            color: YELLOW,
            text: text.into(),
        },
    ));
    assert_eq!(item.kind, AnnotKind::Note);
    assert_eq!(item.text, text);
    assert!(close(item.rect, [100.0, 150.0, 120.0, 170.0], 1.0), "{:?}", item.rect);
    assert!(item.abilities & ability::MOVE != 0);
    assert!(list(e, doc, 0).is_empty(), "the note is on the other page");

    let saved = save_all(e, doc);
    assert!(contains(&saved, b"/Text"));
    let again = open(e, &saved, None).unwrap();
    let items = list(e, again, 1);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].text, text);
    assert!(close(items[0].rect, item.rect, 1.0));
    let objects = appearance_objects(e, again, 1, PdfPageAnnotationType::Text);
    assert!(objects[0] > 0, "the note has no appearance stream");
    let shot = Shot::take(e, again, 1, 612, 792);
    assert!(shot.inked([100.0, 150.0, 120.0, 170.0]) > 40, "no note icon");
}

#[test]
fn a_drawing_is_written_as_an_ink_annotation_with_an_appearance() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::simple_pdf(1), None).unwrap();
    let blue = [0, 0, 255, 255];
    let item = annot(ask(
        e,
        RequestBody::AddInk {
            doc,
            page: 0,
            color: blue,
            width: 4.0,
            strokes: vec![
                vec![[100.0, 600.0], [150.0, 620.0], [200.0, 600.0], [250.0, 640.0]],
                vec![[100.0, 700.0]],
            ],
        },
    ));
    assert_eq!(item.kind, AnnotKind::Ink);
    assert!(item.rect[0] <= 98.0 && item.rect[2] >= 252.0, "{:?}", item.rect);
    assert!(item.rect[1] <= 598.0 && item.rect[3] >= 702.0, "{:?}", item.rect);
    let shot = Shot::take(e, doc, 0, 612, 792);
    assert!(shot.near([100.0, 600.0, 250.0, 640.0], [0, 0, 255], 40) > 100);
    // The single point is a dot.
    assert!(shot.near([97.0, 697.0, 103.0, 703.0], [0, 0, 255], 40) > 3);

    let saved = save_all(e, doc);
    assert!(contains(&saved, b"/Ink"));
    let again = open(e, &saved, None).unwrap();
    let objects = appearance_objects(e, again, 0, PdfPageAnnotationType::Ink);
    assert_eq!(objects.len(), 1);
    assert!(objects[0] >= 1, "the drawing has no appearance stream");
    let read_back = Shot::take(e, again, 0, 612, 792);
    assert!(read_back.near([100.0, 600.0, 250.0, 640.0], [0, 0, 255], 40) > 100);
    let items = list(e, again, 0);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, AnnotKind::Ink);
}

#[test]
fn every_stamp_is_drawn_and_written() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::simple_pdf(1), None).unwrap();
    for (i, stamp) in StampKind::ALL.iter().enumerate() {
        let item = annot(ask(
            e,
            RequestBody::AddStamp {
                doc,
                page: 0,
                x: 40.0,
                y: 20.0 + i as f32 * 60.0,
                stamp: *stamp,
            },
        ));
        assert_eq!(item.kind, AnnotKind::Stamp);
        assert_eq!(item.text, stamp.label());
        assert!(item.rect[2] - item.rect[0] > 60.0);
    }
    let shot = Shot::take(e, doc, 0, 612, 792);
    let saved = save_all(e, doc);
    let again = open(e, &saved, None).unwrap();
    let read_back = Shot::take(e, again, 0, 612, 792);
    for (i, _) in StampKind::ALL.iter().enumerate() {
        let b = [40.0, 20.0 + i as f32 * 60.0, 240.0, 20.0 + i as f32 * 60.0 + 44.0];
        assert!(shot.inked(b) > 200, "stamp {i} is not drawn");
        assert!(read_back.inked(b) > 200, "stamp {i} is not drawn after a save");
    }
    let objects = appearance_objects(e, again, 0, PdfPageAnnotationType::Stamp);
    assert_eq!(objects.len(), 6);
    assert!(objects.iter().all(|n| *n >= 2));
    assert!(contains(&saved, b"/Stamp"));
}

#[test]
fn the_raw_handle_of_a_document_can_be_read() {
    // The drawing code reaches PDFium's own document handle through the text `pdfium-render` gives
    // for `Debug`. If that text ever changes, this fails and the code must be looked at again.
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::simple_pdf(3), None).unwrap();
    let pages = e
        .edit_for_tests(doc, |document| {
            let handle = partridge_engine::annots::raw_document(document).expect("no handle");
            // The handle is the document's: PDFium counts its pages through it.
            let counted = unsafe { document.bindings().FPDF_GetPageCount(handle) };
            (counted, document.pages().len())
        })
        .unwrap();
    assert_eq!(pages.0, 3);
    assert_eq!(pages.1, 3);
}

#[test]
fn a_drawing_has_a_list_of_strokes_as_other_readers_expect() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::simple_pdf(1), None).unwrap();
    annot(ask(
        e,
        RequestBody::AddInk {
            doc,
            page: 0,
            color: [0, 0, 255, 255],
            width: 3.0,
            strokes: vec![vec![[100.0, 100.0], [150.0, 120.0]], vec![[200.0, 100.0], [250.0, 130.0], [300.0, 100.0]]],
        },
    ));
    let saved = save_all(e, doc);
    assert!(contains(&saved, b"/InkList"), "the drawing has no InkList");
    assert!(contains(&saved, b"/Subtype/Ink") || contains(&saved, b"/Subtype /Ink"));
}
