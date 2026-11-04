//! The visible signature: a picture of a signature placed on a page as a stamp annotation that
//! prints. Everything goes through the real engine and the real PDFium: the picture is made three
//! ways (drawn lines, typed text, an imported image), moved, resized, taken off and put back, saved
//! and opened again.
//!
//! A picture of a signature is only a picture. None of these tests, and nothing in the engine,
//! makes a digital signature.

mod common;

use common::*;
use partridge_engine::Engine;
use partridge_protocol::{
    ability, AnnotInfo, AnnotKind, ErrorCode, RequestBody, ResponseBody, SignatureFont,
    SignatureMark, MAX_MARK_IMAGE_PIXELS, MAX_MARK_POINTS, MAX_MARK_STROKES, MAX_MARK_TEXT_CHARS,
};
use partridge_testkit as kit;
use pdfium_render::prelude::*;

const INK: [u8; 4] = [10, 40, 120, 255];

fn scribble() -> SignatureMark {
    let mut a = Vec::new();
    for i in 0..=60 {
        let t = i as f32 / 60.0;
        a.push([
            10.0 + t * 200.0,
            50.0 + 30.0 * (t * 17.0).sin() * (1.0 - t * 0.5),
        ]);
    }
    SignatureMark::Strokes {
        width: 240.0,
        height: 100.0,
        line: 5.0,
        strokes: vec![a, vec![[225.0, 80.0]]],
    }
}

fn typed(font: SignatureFont, text: &str) -> SignatureMark {
    SignatureMark::Typed {
        font,
        text: text.to_string(),
    }
}

/// Red on the left half, nothing on the right half.
fn half_red(width: u32, height: u32) -> SignatureMark {
    let mut rgba = Vec::new();
    for _y in 0..height {
        for x in 0..width {
            if x < width / 2 {
                rgba.extend_from_slice(&[255, 0, 0, 255]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    SignatureMark::Image {
        width,
        height,
        rgba,
    }
}

fn add(e: &mut Engine, doc: u32, page: u32, rect: [f32; 4], mark: SignatureMark) -> ResponseBody {
    ask(
        e,
        RequestBody::AddSignature {
            doc,
            page,
            rect,
            color: INK,
            mark,
        },
    )
}

fn added(e: &mut Engine, doc: u32, rect: [f32; 4], mark: SignatureMark) -> AnnotInfo {
    annot(add(e, doc, 0, rect, mark))
}

/// A blank Letter page: whatever ink there is on it, the picture put it there.
fn open_simple(e: &mut Engine) -> u32 {
    open(e, &kit::blank_pages_pdf(1), None).unwrap()
}

/// A blank page, turned, and cropped when `crop` says so.
fn turned_blank(rotate: u32, crop: Option<[i32; 4]>) -> Vec<u8> {
    let crop = crop
        .map(|c| format!(" /CropBox [{} {} {} {}]", c[0], c[1], c[2], c[3]))
        .unwrap_or_default();
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Rotate {rotate}{crop} >>"),
    ];
    let mut out = b"%PDF-1.4
"
    .to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(
            format!(
                "{} 0 obj
{object}
endobj
",
                i + 1
            )
            .as_bytes(),
        );
    }
    let xref = out.len();
    out.extend(
        format!(
            "xref
0 {}
0000000000 65535 f 
",
            objects.len() + 1
        )
        .as_bytes(),
    );
    for offset in offsets {
        out.extend(
            format!(
                "{offset:010} 00000 n 
"
            )
            .as_bytes(),
        );
    }
    out.extend(
        format!(
            "trailer
<< /Size {} /Root 1 0 R >>
startxref
{xref}
%%EOF
",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

fn place(e: &mut Engine, doc: u32, id: u32, rect: [f32; 4]) -> ResponseBody {
    ask(
        e,
        RequestBody::PlaceSignature {
            doc,
            page: 0,
            id,
            rect,
        },
    )
}

fn page_shot(e: &mut Engine, doc: u32) -> Shot {
    Shot::take(e, doc, 0, 612, 792)
}

/// The flags (`/F`) of every annotation of a page, read through PDFium.
fn flags(e: &mut Engine, doc: u32, page: u32) -> Vec<i32> {
    e.edit_for_tests(doc, |document| {
        let handle = partridge_engine::annots::raw_document(document).expect("a raw handle");
        let b = document.bindings();
        // SAFETY: the handle is the live document's; page and annotations are closed again.
        unsafe {
            let p = b.FPDF_LoadPage(handle, page as i32);
            assert!(!p.is_null());
            let n = b.FPDFPage_GetAnnotCount(p);
            let out = (0..n)
                .map(|i| {
                    let a = b.FPDFPage_GetAnnot(p, i);
                    let f = b.FPDFAnnot_GetFlags(a);
                    b.FPDFPage_CloseAnnot(a);
                    f
                })
                .collect();
            b.FPDF_ClosePage(p);
            out
        }
    })
    .expect("the document is open")
}

fn grown(rect: [f32; 4], by: f32) -> [f32; 4] {
    [rect[0] - by, rect[1] - by, rect[2] + by, rect[3] + by]
}

#[test]
fn every_way_of_making_the_picture_puts_a_signature_on_the_page() {
    let mut guard = engine();
    let e = &mut guard.0;
    let want = [100.0, 100.0, 340.0, 200.0];
    for (name, mark) in [
        ("drawn", scribble()),
        ("typed", typed(SignatureFont::GreatVibes, "Anna Alvorlig")),
        ("image", half_red(120, 40)),
    ] {
        let doc = open_simple(e);
        let before = page_shot(e, doc);
        assert_eq!(
            before.inked(grown(want, 4.0)),
            0,
            "{name}: the page is blank"
        );
        let item = added(e, doc, want, mark);
        assert_eq!(item.kind, AnnotKind::Signature, "{name}");
        // It can be moved, resized, taken off and put back, and nothing else.
        assert_eq!(
            item.abilities,
            ability::MOVE | ability::RESIZE | ability::REMOVE | ability::RESTORE,
            "{name}"
        );
        // The picture is fitted whole into the box, with its own proportions, on the page.
        assert!(
            item.rect[0] >= want[0] - 0.5
                && item.rect[1] >= want[1] - 0.5
                && item.rect[2] <= want[2] + 0.5
                && item.rect[3] <= want[3] + 0.5,
            "{name}: {:?}",
            item.rect
        );
        let after = page_shot(e, doc);
        let inked = after.inked(grown(item.rect, 2.0));
        assert!(inked > 80, "{name}: only {inked} pixels drawn");
        // Nothing is drawn outside the box it reports.
        let all = after.inked([0.0, 0.0, 612.0, 792.0]);
        assert_eq!(all, inked, "{name}: ink outside the box");
        assert_eq!(list(e, doc, 0).len(), 1);
        assert_eq!(list(e, doc, 0)[0].kind, AnnotKind::Signature);
    }
}

#[test]
fn the_stamp_has_the_print_flag_and_keeps_it_after_a_save() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open_simple(e);
    added(e, doc, [100.0, 100.0, 340.0, 200.0], scribble());
    // The flag is on the annotation now.
    assert_eq!(flags(e, doc, 0), vec![4]);
    // And it is in the file: the saved copy has it too, as a stamp.
    let bytes = save_all(e, doc);
    let again = open(e, &bytes, None).unwrap();
    assert_eq!(flags(e, again, 0), vec![4]);
    let items = list(e, again, 0);
    assert_eq!(items.len(), 1);
    // A file does not know what made a stamp: after a reopen it is an ordinary stamp.
    assert_eq!(items[0].kind, AnnotKind::Stamp);
    assert!(contains(&bytes, b"/Stamp"));
}

#[test]
fn a_stamp_made_by_the_fixed_stamps_has_no_flags_so_the_flag_is_the_pictures_own() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open_simple(e);
    annot(ask(
        e,
        RequestBody::AddStamp {
            doc,
            page: 0,
            x: 100.0,
            y: 400.0,
            stamp: partridge_protocol::StampKind::Draft,
        },
    ));
    added(e, doc, [100.0, 100.0, 340.0, 200.0], scribble());
    assert_eq!(flags(e, doc, 0), vec![0, 4]);
}

#[test]
fn the_picture_is_drawn_the_same_after_a_save_and_a_reopen() {
    let mut guard = engine();
    let e = &mut guard.0;
    for (name, mark) in [
        ("drawn", scribble()),
        ("typed", typed(SignatureFont::Allura, "Hanne \u{c5}s")),
        ("image", half_red(80, 30)),
    ] {
        let doc = open_simple(e);
        let item = added(e, doc, [120.0, 300.0, 380.0, 400.0], mark);
        let box_ = grown(item.rect, 3.0);
        let live = page_shot(e, doc);
        let bytes = save_all(e, doc);
        let again = open(e, &bytes, None).unwrap();
        let reopened = page_shot(e, again);
        let (a, b) = (live.inked(box_), reopened.inked(box_));
        assert!(a > 80, "{name}: {a}");
        assert!(
            (a as i64 - b as i64).unsigned_abs() as f32 <= a as f32 * 0.03,
            "{name}: {a} pixels before the save, {b} after the reopen"
        );
        // Nothing appeared elsewhere on the page.
        assert_eq!(reopened.inked([0.0, 0.0, 612.0, 792.0]), b, "{name}");
        // The appearance holds the same number of objects: the lines, the letters, the image.
        let objects = appearance_objects(e, again, 0, PdfPageAnnotationType::Stamp);
        assert_eq!(objects.len(), 1, "{name}");
        let live_objects = appearance_objects(e, doc, 0, PdfPageAnnotationType::Stamp);
        assert_eq!(objects, live_objects, "{name}");
        // The colour of a drawn picture is the colour asked for.
        if name != "image" {
            let dark = reopened.near(box_, [10, 40, 120], 30);
            assert!(dark > 40, "{name}: {dark} pixels of the colour");
        }
    }
}

#[test]
fn a_typed_signature_carries_curves_and_no_font() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open_simple(e);
    let base = save_all(e, doc).len();
    let doc = open_simple(e);
    added(
        e,
        doc,
        [100.0, 100.0, 400.0, 200.0],
        typed(SignatureFont::GreatVibes, "Anna Alvorlig"),
    );
    let bytes = save_all(e, doc);
    // Curves cost about 15 000 bytes; the whole font of PDFium's own way would cost 220 000.
    assert!(
        bytes.len() < base + 40_000,
        "the file grew by {} bytes",
        bytes.len() - base
    );
    assert!(!contains(&bytes, b"FontFile"));
    assert!(!contains(&bytes, b"/TrueType"));
}

#[test]
fn every_font_draws_and_the_fonts_differ() {
    let mut guard = engine();
    let e = &mut guard.0;
    let mut counts = Vec::new();
    for font in SignatureFont::ALL {
        let doc = open_simple(e);
        let item = added(
            e,
            doc,
            [100.0, 100.0, 400.0, 200.0],
            typed(font, "Nora Berg"),
        );
        let n = page_shot(e, doc).inked(grown(item.rect, 2.0));
        assert!(n > 100, "{font:?}: {n}");
        counts.push(n);
    }
    counts.sort_unstable();
    counts.dedup();
    assert_eq!(counts.len(), 3, "the three fonts drew the same ink");
}

#[test]
fn text_the_font_cannot_set_is_refused_and_leaves_the_page_alone() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open_simple(e);
    for text in ["\u{65e5}\u{672c}", "A\u{1F600}", "   "] {
        let code = error_code(add(
            e,
            doc,
            0,
            [100.0, 100.0, 400.0, 200.0],
            typed(SignatureFont::Kristi, text),
        ));
        assert!(
            matches!(
                code,
                ErrorCode::InvalidRequest | ErrorCode::AnnotationRefused
            ),
            "{text:?}: {code:?}"
        );
    }
    assert!(list(e, doc, 0).is_empty());
    assert_eq!(page_shot(e, doc).inked([0.0, 0.0, 612.0, 792.0]), 0);
}

#[test]
fn the_engine_holds_the_caps_even_when_the_protocol_is_not_in_the_way() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open_simple(e);
    let rect = [100.0, 100.0, 400.0, 200.0];
    let strokes = |n: usize, points: usize| SignatureMark::Strokes {
        width: 100.0,
        height: 50.0,
        line: 2.0,
        strokes: vec![vec![[1.0, 1.0]; points]; n],
    };
    let image = |w: u32, h: u32| SignatureMark::Image {
        width: w,
        height: h,
        rgba: vec![0; w as usize * h as usize * 4],
    };
    for (what, mark) in [
        ("one stroke too many", strokes(MAX_MARK_STROKES + 1, 1)),
        ("one point too many", strokes(1, MAX_MARK_POINTS + 1)),
        (
            "text one character too long",
            typed(SignatureFont::Allura, &"a".repeat(MAX_MARK_TEXT_CHARS + 1)),
        ),
        ("an image one pixel too wide", image(1_025, 1)),
        (
            "an image with too many pixels",
            image(1_024, MAX_MARK_IMAGE_PIXELS / 1_024 + 1),
        ),
        (
            "pixels that do not fill the image",
            SignatureMark::Image {
                width: 4,
                height: 4,
                rgba: vec![0; 10],
            },
        ),
    ] {
        assert_eq!(
            error_code(add(e, doc, 0, rect, mark)),
            ErrorCode::InvalidRequest,
            "{what}"
        );
    }
    // At the caps it works.
    annot(add(
        e,
        doc,
        0,
        rect,
        strokes(MAX_MARK_STROKES, MAX_MARK_POINTS / MAX_MARK_STROKES),
    ));
    annot(add(
        e,
        doc,
        0,
        [10.0, 100.0, 600.0, 300.0],
        typed(SignatureFont::Allura, &"a".repeat(MAX_MARK_TEXT_CHARS)),
    ));
    annot(add(e, doc, 0, rect, image(1_024, 512)));
    assert_eq!(list(e, doc, 0).len(), 3);
}

#[test]
fn boxes_that_cannot_hold_a_picture_are_refused() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open_simple(e);
    for rect in [
        [100.0, 100.0, 104.0, 104.0],
        [100.0, 100.0, 100.0, 200.0],
        [300.0, 300.0, 200.0, 200.0],
    ] {
        assert_eq!(
            error_code(add(e, doc, 0, rect, scribble())),
            ErrorCode::InvalidRequest,
            "{rect:?}"
        );
    }
    // A page that is not there.
    assert_eq!(
        error_code(add(e, doc, 9, [100.0, 100.0, 300.0, 200.0], scribble())),
        ErrorCode::PageOutOfRange
    );
    assert!(list(e, doc, 0).is_empty());
}

#[test]
fn a_box_off_the_page_is_pulled_back_onto_it() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open_simple(e);
    let item = added(e, doc, [500.0, 700.0, 900.0, 1000.0], scribble());
    assert!(
        item.rect[2] <= 612.01 && item.rect[3] <= 792.01,
        "{:?}",
        item.rect
    );
    assert!(item.rect[0] >= 0.0 && item.rect[1] >= 0.0);
}

#[test]
fn the_picture_moves_with_the_move_of_an_annotation() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open_simple(e);
    let item = added(e, doc, [100.0, 100.0, 340.0, 200.0], scribble());
    let ink_before = page_shot(e, doc).inked(grown(item.rect, 2.0));
    let moved = annot(ask(
        e,
        RequestBody::UpdateAnnot {
            doc,
            page: 0,
            id: item.id,
            text: None,
            color: None,
            dx: 150.0,
            dy: 200.0,
        },
    ));
    assert!(close(
        moved.rect,
        [
            item.rect[0] + 150.0,
            item.rect[1] + 200.0,
            item.rect[2] + 150.0,
            item.rect[3] + 200.0
        ],
        0.2
    ));
    let shot = page_shot(e, doc);
    // The ink is at the new place, all of it, and none at the old place.
    assert_eq!(shot.inked(grown(item.rect, 2.0)), 0);
    let ink_after = shot.inked(grown(moved.rect, 2.0));
    assert!(
        (ink_after as i64 - ink_before as i64).abs() < 12,
        "{ink_before} {ink_after}"
    );
    assert_eq!(shot.inked([0.0, 0.0, 612.0, 792.0]), ink_after);
}

#[test]
fn the_picture_is_resized_with_its_proportions_and_stays_sharp() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open_simple(e);
    let item = added(e, doc, [100.0, 100.0, 340.0, 200.0], scribble());
    let (w, h) = (item.rect[2] - item.rect[0], item.rect[3] - item.rect[1]);
    // Twice as wide, whatever height the box says: the proportions are the picture's own.
    let bigger = annot(place(
        e,
        doc,
        item.id,
        [
            item.rect[0],
            item.rect[1],
            item.rect[0] + 2.0 * w,
            item.rect[1] + 5.0,
        ],
    ));
    let (bw, bh) = (
        bigger.rect[2] - bigger.rect[0],
        bigger.rect[3] - bigger.rect[1],
    );
    assert!(
        (bw - 2.0 * w).abs() < 0.3 && (bh - 2.0 * h).abs() < 0.3,
        "{:?}",
        bigger.rect
    );
    assert_eq!(bigger.id, item.id);
    assert_eq!(bigger.kind, AnnotKind::Signature);
    let shot = page_shot(e, doc);
    let n = shot.inked(grown(bigger.rect, 2.0));
    assert!(n > 100);
    assert_eq!(
        shot.inked([0.0, 0.0, 612.0, 792.0]),
        n,
        "ink outside the new box"
    );
    // Smaller and elsewhere: the corner and the width of the box are taken.
    let smaller = annot(place(
        e,
        doc,
        item.id,
        [300.0, 500.0, 300.0 + w / 2.0, 520.0],
    ));
    assert!((smaller.rect[0] - 300.0).abs() < 0.2 && (smaller.rect[1] - 500.0).abs() < 0.2);
    assert!(((smaller.rect[2] - smaller.rect[0]) - w / 2.0).abs() < 0.3);
    let shot = page_shot(e, doc);
    assert_eq!(
        shot.inked(grown(item.rect, 2.0)),
        0,
        "ink left at the old place"
    );
    assert!(shot.inked(grown(smaller.rect, 2.0)) > 40);
    // The list tells the same.
    assert_eq!(list(e, doc, 0)[0].rect, smaller.rect);
}

#[test]
fn a_resize_that_asks_for_too_much_or_too_little_is_held_in_range() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open_simple(e);
    let item = added(e, doc, [100.0, 100.0, 340.0, 200.0], scribble());
    let huge = annot(place(e, doc, item.id, [0.0, 0.0, 9_000.0, 9_000.0]));
    assert!(
        huge.rect[2] <= 612.01 && huge.rect[3] <= 792.01,
        "{:?}",
        huge.rect
    );
    let tiny = annot(place(e, doc, item.id, [200.0, 200.0, 201.0, 201.0]));
    assert!(tiny.rect[2] - tiny.rect[0] >= 8.0 - 0.1 && tiny.rect[3] - tiny.rect[1] >= 8.0 - 0.1);
    let off = annot(place(e, doc, item.id, [590.0, 780.0, 690.0, 880.0]));
    assert!(off.rect[2] <= 612.01 && off.rect[3] <= 792.01);
}

#[test]
fn only_a_picture_of_this_session_can_be_resized() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open_simple(e);
    let note = annot(ask(
        e,
        RequestBody::AddNote {
            doc,
            page: 0,
            x: 50.0,
            y: 50.0,
            color: YELLOW,
            text: "n".into(),
        },
    ));
    assert_eq!(
        error_code(place(e, doc, note.id, [10.0, 10.0, 100.0, 100.0])),
        ErrorCode::AnnotationRefused
    );
    assert_eq!(
        error_code(place(e, doc, 9999, [10.0, 10.0, 100.0, 100.0])),
        ErrorCode::NoSuchAnnotation
    );
    // After a save and a reopen the stamp is just a stamp: it can be taken off, nothing else.
    let item = added(e, doc, [100.0, 100.0, 340.0, 200.0], scribble());
    let bytes = save_all(e, doc);
    let again = open(e, &bytes, None).unwrap();
    let stamp = list(e, again, 0)
        .into_iter()
        .find(|a| a.kind == AnnotKind::Stamp)
        .unwrap();
    assert_eq!(stamp.abilities & (ability::RESIZE | ability::MOVE), 0);
    assert_eq!(
        error_code(place(e, again, stamp.id, [10.0, 10.0, 100.0, 100.0])),
        ErrorCode::AnnotationRefused
    );
    let _ = item;
}

#[test]
fn a_picture_taken_off_and_put_back_is_where_it_was_left() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open_simple(e);
    let item = added(e, doc, [100.0, 100.0, 340.0, 200.0], scribble());
    let moved = annot(place(e, doc, item.id, [300.0, 500.0, 420.0, 540.0]));
    let ink = page_shot(e, doc).inked(grown(moved.rect, 2.0));
    assert!(ink > 60);
    // Taken off: no ink, no annotation.
    assert!(matches!(
        ask(
            e,
            RequestBody::RemoveAnnot {
                doc,
                page: 0,
                id: item.id
            }
        ),
        ResponseBody::AnnotRemoved
    ));
    assert!(list(e, doc, 0).is_empty());
    assert_eq!(page_shot(e, doc).inked([0.0, 0.0, 612.0, 792.0]), 0);
    // Put back: the same id, the same box, the same ink, still a signature picture.
    let back = annot(ask(
        e,
        RequestBody::RestoreAnnot {
            doc,
            page: 0,
            id: item.id,
        },
    ));
    assert_eq!(back.id, item.id);
    assert_eq!(back.kind, AnnotKind::Signature);
    assert!(
        close(back.rect, moved.rect, 0.2),
        "{:?} {:?}",
        back.rect,
        moved.rect
    );
    let n = page_shot(e, doc).inked(grown(back.rect, 2.0));
    assert!((n as i64 - ink as i64).abs() < 12, "{ink} {n}");
    assert_eq!(flags(e, doc, 0).last(), Some(&4));
    // And it can still be resized.
    let bigger = annot(place(e, doc, item.id, [300.0, 500.0, 480.0, 500.0]));
    assert!(bigger.rect[2] - bigger.rect[0] > 170.0);
}

#[test]
fn a_picture_that_was_moved_is_put_back_where_it_was_moved_to() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open_simple(e);
    let item = added(e, doc, [100.0, 100.0, 340.0, 200.0], scribble());
    let moved = annot(ask(
        e,
        RequestBody::UpdateAnnot {
            doc,
            page: 0,
            id: item.id,
            text: None,
            color: None,
            dx: 100.0,
            dy: 250.0,
        },
    ));
    ask(
        e,
        RequestBody::RemoveAnnot {
            doc,
            page: 0,
            id: item.id,
        },
    );
    let back = annot(ask(
        e,
        RequestBody::RestoreAnnot {
            doc,
            page: 0,
            id: item.id,
        },
    ));
    assert!(
        close(back.rect, moved.rect, 0.3),
        "{:?} {:?}",
        back.rect,
        moved.rect
    );
}

#[test]
fn the_transparent_part_of_an_image_stays_see_through() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open_simple(e);
    let item = added(e, doc, [100.0, 100.0, 300.0, 200.0], half_red(100, 50));
    let shot = page_shot(e, doc);
    let (l, t, r, b) = (item.rect[0], item.rect[1], item.rect[2], item.rect[3]);
    let mid = (l + r) / 2.0;
    let left = [l + 2.0, t + 2.0, mid - 2.0, b - 2.0];
    let right = [mid + 2.0, t + 2.0, r - 2.0, b - 2.0];
    let area = ((left[2] - left[0]) * (left[3] - left[1])) as usize;
    assert!(
        shot.near(left, [255, 0, 0], 20) > area * 9 / 10,
        "the left half is not red"
    );
    assert_eq!(shot.inked(right), 0, "the transparent half drew something");
}

#[test]
fn turned_pages_show_the_picture_upright_in_its_box() {
    let mut guard = engine();
    let e = &mut guard.0;
    for (rotate, crop) in [
        (0, None),
        (90, None),
        (180, None),
        (270, None),
        (90, Some([50, 60, 500, 700])),
    ] {
        let (doc, sizes) = open_sized(e, &turned_blank(rotate, crop), None).unwrap();
        let (w, h) = sizes[0];
        let name = format!("turned {rotate}, crop {crop:?}");
        let want = [100.0, 150.0, 340.0, 250.0];
        let item = annot(add(e, doc, 0, want, scribble()));
        assert!(
            item.rect[0] >= want[0] - 0.5 && item.rect[2] <= want[2] + 0.5,
            "{name}: {:?}",
            item.rect
        );
        let shot = Shot::take(e, doc, 0, w, h);
        let n = shot.inked(grown(item.rect, 2.0));
        assert!(n > 80, "{name}: {n}");
        // All of the ink is inside the box: the picture is upright and nothing is turned out of it.
        assert_eq!(shot.inked([0.0, 0.0, w as f32, h as f32]), n, "{name}");
        // A move on a turned page goes the way the pointer went.
        let moved = annot(ask(
            e,
            RequestBody::UpdateAnnot {
                doc,
                page: 0,
                id: item.id,
                text: None,
                color: None,
                dx: 60.0,
                dy: 30.0,
            },
        ));
        assert!(
            close(
                moved.rect,
                [
                    item.rect[0] + 60.0,
                    item.rect[1] + 30.0,
                    item.rect[2] + 60.0,
                    item.rect[3] + 30.0
                ],
                0.3
            ),
            "{name}: {:?} {:?}",
            item.rect,
            moved.rect
        );
        let after = Shot::take(e, doc, 0, w, h);
        let n = after.inked(grown(moved.rect, 2.0));
        assert!(n > 80, "{name}: {n}");
        assert_eq!(after.inked([0.0, 0.0, w as f32, h as f32]), n, "{name}");
        // A resize on a turned page keeps the proportions of the drawn page.
        let sized = annot(place(
            e,
            doc,
            item.id,
            [
                moved.rect[0],
                moved.rect[1],
                moved.rect[0] + 100.0,
                moved.rect[1],
            ],
        ));
        let aspect = (item.rect[2] - item.rect[0]) / (item.rect[3] - item.rect[1]);
        let now = (sized.rect[2] - sized.rect[0]) / (sized.rect[3] - sized.rect[1]);
        assert!((aspect - now).abs() < 0.05, "{name}: {aspect} {now}");
        let last = Shot::take(e, doc, 0, w, h);
        let n = last.inked(grown(sized.rect, 2.0));
        assert!(n > 40, "{name}: {n}");
        assert_eq!(last.inked([0.0, 0.0, w as f32, h as f32]), n, "{name}");
    }
}

#[test]
fn a_document_that_does_not_allow_annotations_takes_no_picture() {
    let mut guard = engine();
    let e = &mut guard.0;
    let bytes = kit::encrypted_pdf_with_permissions("secret", 1, 0);
    let doc = open(e, &bytes, Some("secret")).unwrap();
    assert_eq!(
        error_code(add(e, doc, 0, [100.0, 100.0, 300.0, 200.0], scribble())),
        ErrorCode::AnnotationsNotAllowed
    );
}

#[test]
fn the_pictures_kept_for_an_undo_are_held_to_a_bound() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open_simple(e);
    let mut made = 0;
    let mut refused = None;
    for _ in 0..20 {
        match add(
            e,
            doc,
            0,
            [100.0, 100.0, 400.0, 300.0],
            half_red(1_024, 512),
        ) {
            ResponseBody::Annot { .. } => made += 1,
            other => {
                refused = Some(error_code(other));
                break;
            }
        }
    }
    assert!((8..=16).contains(&made), "{made} pictures fitted");
    assert_eq!(refused, Some(ErrorCode::AnnotationRefused));
    // Nothing was left behind by the refusal.
    assert_eq!(list(e, doc, 0).len(), made);
}

/// A move or a resize does not draw the page: nothing draws it between the move and the save here,
/// and the saved file must still hold the picture at the place it was moved to, drawn.
#[test]
fn a_picture_moved_and_saved_without_the_page_drawn_is_saved_where_it_was_moved_to() {
    let mut guard = engine();
    let e = &mut guard.0;
    for (name, mark) in [
        ("drawn", scribble()),
        ("typed", typed(SignatureFont::GreatVibes, "Ola Nordmann")),
        ("image", half_red(80, 30)),
    ] {
        let doc = open_simple(e);
        let item = added(e, doc, [100.0, 100.0, 340.0, 200.0], mark);
        let w = item.rect[2] - item.rect[0];
        // Two steps of a drag and then a resize, with no page drawn in between.
        let first = annot(place(e, doc, item.id, [150.0, 220.0, 150.0 + w, 240.0]));
        let last = annot(place(
            e,
            doc,
            item.id,
            [300.0, 500.0, 300.0 + w / 2.0, 520.0],
        ));
        assert_ne!(first.rect, last.rect, "{name}");
        let bytes = save_all(e, doc);
        let again = open(e, &bytes, None).unwrap();
        let shot = page_shot(e, again);
        assert!(
            shot.inked(grown(last.rect, 2.0)) > 40,
            "{name}: no picture at the new place"
        );
        assert_eq!(
            shot.inked(grown(item.rect, 2.0)),
            0,
            "{name}: ink left at the first place"
        );
        assert_eq!(
            shot.inked(grown(first.rect, 2.0)),
            0,
            "{name}: ink left at the middle place"
        );
        assert_eq!(
            shot.inked([0.0, 0.0, 612.0, 792.0]),
            shot.inked(grown(last.rect, 2.0)),
            "{name}: ink outside the new box"
        );
    }
}
