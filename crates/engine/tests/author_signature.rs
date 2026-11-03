//! The two writers of the engine on one page: a picture of a signature (a stamp made through PDFium's
//! calls) and fields made by the form designer (objects written after the bytes PDFium saved). Each
//! reloads or rewrites the document, so the page is taken through both, saved twice, and opened again.

mod common;

use common::designer::{add, apply_ok, designed, names, restore, text};
use common::{ask, engine, list, save_all, Shot};
use partridge_engine::Engine;
use partridge_protocol::{AuthorOp, RequestBody, ResponseBody, SignatureMark};
use partridge_testkit as kit;

const INK: [u8; 4] = [10, 40, 120, 255];

fn scribble() -> SignatureMark {
    let line: Vec<[f32; 2]> = (0..=40)
        .map(|i| {
            let t = i as f32 / 40.0;
            [10.0 + t * 200.0, 50.0 + 30.0 * (t * 17.0).sin()]
        })
        .collect();
    SignatureMark::Strokes {
        width: 240.0,
        height: 100.0,
        line: 5.0,
        strokes: vec![line],
    }
}

fn sign(e: &mut Engine, doc: u32, rect: [f32; 4]) -> u32 {
    match ask(
        e,
        RequestBody::AddSignature {
            doc,
            page: 0,
            rect,
            color: INK,
            mark: scribble(),
        },
    ) {
        ResponseBody::Annot { item } => item.id,
        other => panic!("{other:?}"),
    }
}

fn inked(shot: &Shot, rect: [f32; 4]) -> bool {
    shot.inked(rect) > 20
}

/// How many annotations the reader lists on the page: the picture, and no field.
fn pictures(e: &mut Engine, doc: u32) -> usize {
    list(e, doc, 0).len()
}

#[test]
fn a_picture_of_a_signature_and_new_fields_share_a_page_through_two_saves_and_a_reopen() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = common::designer::open(e, &kit::blank_pages_pdf(1));
    let picture = [100.0, 100.0, 340.0, 200.0];
    let id = sign(e, doc, picture);
    // Fields on the same page, in one batch and then another.
    let step = apply_ok(
        e,
        doc,
        vec![
            add(text("name", [100.0, 300.0, 300.0, 322.0])),
            add(text("city", [100.0, 340.0, 300.0, 362.0])),
        ],
    );
    assert!(step > 0);
    assert_eq!(pictures(e, doc), 1, "the picture is still there");
    // The picture is moved after the fields were made, and a field taken away after that.
    let moved = [100.0, 450.0, 340.0, 550.0];
    match ask(
        e,
        RequestBody::PlaceSignature {
            doc,
            page: 0,
            id,
            rect: moved,
        },
    ) {
        ResponseBody::Annot { .. } => {}
        other => panic!("{other:?}"),
    }
    apply_ok(
        e,
        doc,
        vec![AuthorOp::Remove {
            name: "city".into(),
        }],
    );
    assert_eq!(names(e, doc), ["name"]);
    // One page, one stamp, one field, where they were put.
    let shot = Shot::take(e, doc, 0, 612, 792);
    assert!(inked(&shot, moved), "the picture is on the page");
    assert!(!inked(&shot, picture), "and not where it was");
    assert!(
        inked(&shot, [100.0, 300.0, 300.0, 322.0]),
        "the field is drawn"
    );
    assert_eq!(designed(e, doc, 0).len(), 1);
    // Two saves in a row give a file that reads the same each time.
    let first = save_all(e, doc);
    let second = save_all(e, doc);
    assert!(first.starts_with(b"%PDF-") && second.starts_with(b"%PDF-"));
    for saved in [&first, &second] {
        let (again, _keep) = common::designer::open(e, saved);
        assert_eq!(names(e, again), ["name"]);
        assert_eq!(pictures(e, again), 1);
        let shot = Shot::take(e, again, 0, 612, 792);
        assert!(inked(&shot, moved));
        assert!(inked(&shot, [100.0, 300.0, 300.0, 322.0]));
        // The text field of the file can be filled in and the picture is not touched.
        assert_eq!(designed(e, again, 0).len(), 1);
    }
    // A step back takes the fields away and leaves the picture.
    restore(e, doc, step);
    let shot = Shot::take(e, doc, 0, 612, 792);
    assert!(
        inked(&shot, picture),
        "the picture is where it was before the fields"
    );
    assert!(names(e, doc).is_empty());
    assert_eq!(pictures(e, doc), 1);
}
