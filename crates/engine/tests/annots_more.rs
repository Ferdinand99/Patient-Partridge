//! More annotation tests: turned pages, right-to-left text, changing and removing, annotations that
//! a file already has, hostile annotations, protected and signed documents, and refused requests.

mod common;

use common::*;
use partridge_protocol::{
    ability, AnnotKind, ErrorCode, RequestBody, ResponseBody, StampKind, MAX_ANNOTS_PER_PAGE,
    MAX_ANNOT_AUTHOR, MAX_ANNOT_TEXT,
};
use partridge_testkit as kit;
use pdfium_render::prelude::*;

const GREEN: [u8; 4] = [0, 200, 0, 255];

fn highlight(
    e: &mut partridge_engine::Engine,
    doc: u32,
    page: u32,
    start: u32,
    end: u32,
) -> ResponseBody {
    ask(
        e,
        RequestBody::AddMarkup {
            doc,
            page,
            kind: AnnotKind::Highlight,
            color: YELLOW,
            start,
            end,
        },
    )
}

// ---- turned pages ------------------------------------------------------------------

#[test]
fn markup_lands_on_the_text_of_every_kind_of_page_before_and_after_a_save() {
    let mut guard = engine();
    let e = &mut guard.0;
    let bytes = kit::turned_pdf();
    let doc = open(e, &bytes, None).unwrap();
    let mut boxes = Vec::new();
    for page in 0..5u32 {
        let (start, len) = find(e, doc, page, "quick brown fox");
        let b = text_box(e, doc, page, start, len);
        let item = annot(highlight(e, doc, page, start, start + len));
        assert!(
            close(item.rect, b, 2.5),
            "page {page}: the highlight {:?} is not on the text {b:?}",
            item.rect
        );
        boxes.push((start, len, b));
    }
    let saved = save_all(e, doc);
    let (again, sizes) = open_sized(e, &saved, None).unwrap();
    for page in 0..5u32 {
        let (_, _, b) = boxes[page as usize];
        // Each page is drawn at its own size (a quarter turn swaps width and height).
        let (w, h) = sizes[page as usize];
        let shot = Shot::take(e, again, page, w, h);
        assert!(
            shot.near(b, [255, 235, 59], 45) > 40,
            "page {page}: the highlight is not drawn on the text after a save"
        );
        let items = list(e, again, page);
        assert_eq!(items.len(), 1, "page {page}");
        assert!(
            close(items[0].rect, b, 2.5),
            "page {page}: {:?} vs {b:?}",
            items[0].rect
        );
    }
}

#[test]
fn a_drawing_lands_where_it_was_drawn_on_every_kind_of_page() {
    let mut guard = engine();
    let e = &mut guard.0;
    let bytes = kit::turned_pdf();
    let doc = open(e, &bytes, None).unwrap();
    let red = [200, 0, 0, 255];
    for page in 0..5u32 {
        // A horizontal stroke and a vertical one, in points from the top left of the page as drawn.
        annot(ask(
            e,
            RequestBody::AddInk {
                doc,
                page,
                color: red,
                width: 3.0,
                strokes: vec![
                    vec![[60.0, 300.0], [200.0, 300.0]],
                    vec![[300.0, 250.0], [300.0, 330.0]],
                ],
            },
        ));
    }
    let saved = save_all(e, doc);
    let (again, sizes) = open_sized(e, &saved, None).unwrap();
    for page in 0..5u32 {
        let (w, h) = sizes[page as usize];
        let shot = Shot::take(e, again, page, w, h);
        let horizontal = shot.near([60.0, 298.0, 200.0, 302.0], [200, 0, 0], 50);
        let vertical = shot.near([298.0, 250.0, 302.0, 330.0], [200, 0, 0], 50);
        assert!(
            horizontal > 220,
            "page {page}: horizontal stroke is {horizontal} pixels"
        );
        assert!(
            vertical > 140,
            "page {page}: vertical stroke is {vertical} pixels"
        );
        // It is where it was drawn and nowhere else: the corners are empty.
        assert_eq!(
            shot.near([0.0, 0.0, 30.0, 30.0], [200, 0, 0], 50),
            0,
            "page {page}"
        );
        let items = list(e, again, page);
        assert_eq!(items.len(), 1);
        let r = items[0].rect;
        assert!(
            r[0] <= 60.0 && r[2] >= 300.0 && r[1] <= 250.0 && r[3] >= 330.0,
            "page {page}: {r:?}"
        );
    }
}

#[test]
fn notes_and_stamps_keep_their_place_on_every_kind_of_page() {
    let mut guard = engine();
    let e = &mut guard.0;
    let bytes = kit::turned_pdf();
    let (doc, sizes) = open_sized(e, &bytes, None).unwrap();
    let blank: Vec<Shot> = (0..5u32)
        .map(|page| Shot::take(e, doc, page, sizes[page as usize].0, sizes[page as usize].1))
        .collect();
    for page in 0..5u32 {
        let note = annot(ask(
            e,
            RequestBody::AddNote {
                doc,
                page,
                x: 120.0,
                y: 200.0,
                color: YELLOW,
                text: format!("note {page}"),
            },
        ));
        assert!(
            close(note.rect, [120.0, 200.0, 140.0, 220.0], 1.5),
            "page {page}: {:?}",
            note.rect
        );
        let stamp = annot(ask(
            e,
            RequestBody::AddStamp {
                doc,
                page,
                x: 150.0,
                y: 400.0,
                stamp: StampKind::Approved,
            },
        ));
        assert!(
            (stamp.rect[0] - 150.0).abs() < 1.5 && (stamp.rect[1] - 400.0).abs() < 1.5,
            "page {page}: {:?}",
            stamp.rect
        );
        // The stamp is wider than tall on the page as it is seen, whatever the turn of the page.
        assert!(
            stamp.rect[2] - stamp.rect[0] > 2.0 * (stamp.rect[3] - stamp.rect[1]),
            "page {page}: {:?}",
            stamp.rect
        );
    }
    let saved = save_all(e, doc);
    let (again, sizes) = open_sized(e, &saved, None).unwrap();
    for page in 0..5u32 {
        let (w, h) = sizes[page as usize];
        let shot = Shot::take(e, again, page, w, h);
        let items = list(e, again, page);
        let stamp = items.iter().find(|i| i.kind == AnnotKind::Stamp).unwrap();
        let note = items.iter().find(|i| i.kind == AnnotKind::Note).unwrap();
        let was = &blank[page as usize];
        // Where the page changed: inside the stamp's box and the note's box, and nowhere else.
        let mut changed = 0;
        let mut outside = 0;
        for y in 0..h as usize {
            for x in 0..w as usize {
                if shot.pixel(x, y) == was.pixel(x, y) {
                    continue;
                }
                let (fx, fy) = (x as f32, y as f32);
                let inside = |r: [f32; 4]| {
                    fx >= r[0] - 2.0 && fx <= r[2] + 2.0 && fy >= r[1] - 2.0 && fy <= r[3] + 2.0
                };
                if inside(stamp.rect) || inside(note.rect) {
                    changed += 1;
                } else {
                    outside += 1;
                }
            }
        }
        assert!(
            changed > 300,
            "page {page}: the stamp and the note are not drawn ({changed})"
        );
        assert_eq!(
            outside, 0,
            "page {page}: something changed outside the two boxes"
        );
    }
}

// ---- right-to-left text -------------------------------------------------------------

#[test]
fn a_selection_of_right_to_left_text_is_one_box_over_exactly_the_letters() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::rtl_pdf(), None).unwrap();
    let (text, rects) = page_text(e, doc, 0);
    assert!(text.starts_with(kit::RTL_LINE), "{text:?}");
    // Letters 3 to 7 in reading order: the box is theirs, and it is a single box.
    let b = text_box(e, doc, 0, 3, 4);
    let item = annot(highlight(e, doc, 0, 3, 7));
    assert!(close(item.rect, b, 2.0), "{:?} vs {b:?}", item.rect);
    // The whole first line, and a range that runs on into the second.
    let first_line = text_box(e, doc, 0, 0, 10);
    let all = annot(highlight(e, doc, 0, 0, 10));
    assert!(close(all.rect, first_line, 2.0));
    assert!(
        first_line[2] - first_line[0] > 80.0,
        "ten letters are wide: {first_line:?}"
    );
    let second_line_start = text
        .chars()
        .position(|c| c == '\n')
        .map(|n| n + 1)
        .unwrap_or(11) as u32;
    let two = annot(highlight(e, doc, 0, 0, second_line_start + 5));
    assert!(
        two.rect[3] > first_line[3] + 10.0,
        "the second line is covered too"
    );
    let _ = rects;

    // In the saved file the first two are one quadrilateral each, the third two.
    let saved = save_all(e, doc);
    let again = open(e, &saved, None).unwrap();
    let counts = e
        .edit_for_tests(again, |document| {
            let page = document.pages().get(0).unwrap();
            page.annotations()
                .iter()
                .filter(|a| a.annotation_type() == PdfPageAnnotationType::Highlight)
                .map(|a| a.attachment_points().len())
                .collect::<Vec<_>>()
        })
        .unwrap();
    assert_eq!(counts, vec![1, 1, 2]);
    // The shading is over exactly the letters: one highlight alone, in a copy of the document.
    let doc = open(e, &kit::rtl_pdf(), None).unwrap();
    highlight(e, doc, 0, 3, 7);
    let shot = Shot::take(e, doc, 0, 612, 792);
    assert!(shot.near(b, [255, 235, 59], 45) > 300);
    let saved = save_all(e, doc);
    let again = open(e, &saved, None).unwrap();
    let shot = Shot::take(e, again, 0, 612, 792);
    assert!(shot.near(b, [255, 235, 59], 45) > 300);
    // Nothing outside the box is shaded.
    let wide = [b[0] - 30.0, b[1] - 10.0, b[2] + 30.0, b[3] + 10.0];
    assert_eq!(
        shot.near(wide, [255, 235, 59], 45),
        shot.near(b, [255, 235, 59], 45)
    );
}

// ---- changing -----------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn update(
    e: &mut partridge_engine::Engine,
    doc: u32,
    page: u32,
    id: u32,
    text: Option<&str>,
    color: Option<[u8; 4]>,
    dx: f32,
    dy: f32,
) -> ResponseBody {
    ask(
        e,
        RequestBody::UpdateAnnot {
            doc,
            page,
            id,
            text: text.map(str::to_string),
            color,
            dx,
            dy,
        },
    )
}

#[test]
fn a_note_can_get_new_text_and_move_and_the_changes_are_saved() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::turned_pdf(), None).unwrap();
    // On a turned page, the move is in points of the page as it is drawn.
    let note = annot(ask(
        e,
        RequestBody::AddNote {
            doc,
            page: 1,
            x: 100.0,
            y: 100.0,
            color: YELLOW,
            text: "first".into(),
        },
    ));
    let changed = annot(update(
        e,
        doc,
        1,
        note.id,
        Some("second \u{2713}"),
        None,
        30.0,
        40.0,
    ));
    assert_eq!(changed.id, note.id);
    assert_eq!(changed.text, "second \u{2713}");
    assert!(
        close(changed.rect, [130.0, 140.0, 150.0, 160.0], 1.5),
        "{:?}",
        changed.rect
    );
    let saved = save_all(e, doc);
    let again = open(e, &saved, None).unwrap();
    let items = list(e, again, 1);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].text, "second \u{2713}");
    assert!(close(items[0].rect, changed.rect, 1.5));
    // The old text is not in the file.
    assert!(!contains(&saved, b"first"));
}

#[test]
fn a_changed_colour_is_drawn_and_the_annotation_keeps_its_id() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::features_pdf(), None).unwrap();
    let (start, len) = find(e, doc, 0, "needle");
    let b = text_box(e, doc, 0, start, len);
    let item = annot(highlight(e, doc, 0, start, start + len));
    let other = annot(ask(
        e,
        RequestBody::AddNote {
            doc,
            page: 0,
            x: 400.0,
            y: 300.0,
            color: YELLOW,
            text: "bystander".into(),
        },
    ));
    let green = annot(update(e, doc, 0, item.id, None, Some(GREEN), 0.0, 0.0));
    assert_eq!(green.id, item.id);
    assert_eq!(&green.color[..3], &GREEN[..3]);
    let shot = Shot::take(e, doc, 0, 612, 792);
    assert!(
        shot.near(b, [0, 200, 0], 60) > 30,
        "the new colour is not drawn"
    );
    assert_eq!(
        shot.near(b, [255, 235, 59], 20),
        0,
        "the old colour is still drawn"
    );
    // Still the only highlight, and the other annotation was not touched.
    let items = list(e, doc, 0);
    assert_eq!(
        items
            .iter()
            .filter(|i| i.kind == AnnotKind::Highlight)
            .count(),
        1
    );
    assert!(items
        .iter()
        .any(|i| i.id == other.id && i.text == "bystander"));
    let saved = save_all(e, doc);
    let again = open(e, &saved, None).unwrap();
    let shot = Shot::take(e, again, 0, 612, 792);
    assert!(shot.near(b, [0, 200, 0], 60) > 30);
}

#[test]
fn changes_an_annotation_does_not_allow_are_refused() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::simple_pdf(1), None).unwrap();
    let stamp = annot(ask(
        e,
        RequestBody::AddStamp {
            doc,
            page: 0,
            x: 50.0,
            y: 50.0,
            stamp: StampKind::Draft,
        },
    ));
    let note = annot(ask(
        e,
        RequestBody::AddNote {
            doc,
            page: 0,
            x: 300.0,
            y: 50.0,
            color: YELLOW,
            text: "n".into(),
        },
    ));
    let ink = annot(ask(
        e,
        RequestBody::AddInk {
            doc,
            page: 0,
            color: YELLOW,
            width: 2.0,
            strokes: vec![vec![[10.0, 10.0], [20.0, 20.0]]],
        },
    ));
    for (id, text, color, dx) in [
        (stamp.id, Some("x"), None, 0.0),
        (stamp.id, None, Some(GREEN), 0.0),
        (stamp.id, None, None, 5.0),
        (note.id, None, Some(GREEN), 0.0),
        (ink.id, None, Some(GREEN), 0.0),
        (ink.id, None, None, 5.0),
    ] {
        assert_eq!(
            error_code(update(e, doc, 0, id, text, color, dx, 0.0)),
            ErrorCode::AnnotationRefused,
            "{id} {text:?} {color:?} {dx}"
        );
    }
    // Nothing changed.
    let items = list(e, doc, 0);
    assert_eq!(items.len(), 3);
    assert_eq!(
        items.iter().find(|i| i.id == note.id).unwrap().rect,
        note.rect
    );
}

// ---- removing and putting back ---------------------------------------------------------

#[test]
fn removing_and_putting_back_keep_the_id_and_the_drawing() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::features_pdf(), None).unwrap();
    let (start, len) = find(e, doc, 0, "needle");
    let b = text_box(e, doc, 0, start, len);
    let item = annot(highlight(e, doc, 0, start, start + len));
    let other = annot(ask(
        e,
        RequestBody::AddNote {
            doc,
            page: 0,
            x: 400.0,
            y: 300.0,
            color: YELLOW,
            text: "stays".into(),
        },
    ));
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
    let items = list(e, doc, 0);
    assert!(items.iter().all(|i| i.id != item.id));
    // The other annotation kept its id although it moved up in the page's list.
    assert!(items.iter().any(|i| i.id == other.id && i.text == "stays"));
    let shot = Shot::take(e, doc, 0, 612, 792);
    assert_eq!(
        shot.near(b, [255, 235, 59], 40),
        0,
        "the highlight is still drawn"
    );
    // Gone from the file too: the second remove says so.
    assert_eq!(
        error_code(ask(
            e,
            RequestBody::RemoveAnnot {
                doc,
                page: 0,
                id: item.id
            }
        )),
        ErrorCode::NoSuchAnnotation
    );

    let back = annot(ask(
        e,
        RequestBody::RestoreAnnot {
            doc,
            page: 0,
            id: item.id,
        },
    ));
    assert_eq!(back.id, item.id);
    assert!(close(back.rect, item.rect, 0.5));
    assert_eq!(&back.color[..3], &YELLOW[..3]);
    let shot = Shot::take(e, doc, 0, 612, 792);
    assert!(
        shot.near(b, [255, 235, 59], 40) > 50,
        "the highlight is not back"
    );
    // A snapshot is used once.
    assert_eq!(
        error_code(ask(
            e,
            RequestBody::RestoreAnnot {
                doc,
                page: 0,
                id: item.id
            }
        )),
        ErrorCode::AnnotationRefused
    );
    // And the whole of it survives a save.
    let saved = save_all(e, doc);
    let again = open(e, &saved, None).unwrap();
    assert_eq!(
        list(e, again, 0)
            .iter()
            .filter(|i| i.kind == AnnotKind::Highlight)
            .count(),
        1
    );
}

#[test]
fn drawings_and_stamps_come_back_as_they_were() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::simple_pdf(1), None).unwrap();
    let blank = Shot::take(e, doc, 0, 612, 792);
    let ink = annot(ask(
        e,
        RequestBody::AddInk {
            doc,
            page: 0,
            color: [0, 0, 255, 255],
            width: 5.0,
            strokes: vec![vec![[100.0, 100.0], [300.0, 140.0], [200.0, 220.0]]],
        },
    ));
    let stamp = annot(ask(
        e,
        RequestBody::AddStamp {
            doc,
            page: 0,
            x: 200.0,
            y: 400.0,
            stamp: StampKind::Final,
        },
    ));
    let before = Shot::take(e, doc, 0, 612, 792);
    for item in [&ink, &stamp] {
        assert!(item.abilities & ability::RESTORE != 0);
        ask(
            e,
            RequestBody::RemoveAnnot {
                doc,
                page: 0,
                id: item.id,
            },
        );
    }
    let gone = Shot::take(e, doc, 0, 612, 792);
    assert_eq!(gone.near([90.0, 90.0, 310.0, 230.0], [0, 0, 255], 40), 0);
    assert_ne!(gone.rgba, before.rgba);
    assert!(
        gone.rgba == blank.rgba,
        "the page does not look as it did before the drawing and the stamp"
    );
    for item in [&ink, &stamp] {
        let back = annot(ask(
            e,
            RequestBody::RestoreAnnot {
                doc,
                page: 0,
                id: item.id,
            },
        ));
        assert_eq!(back.id, item.id);
        assert!(
            close(back.rect, item.rect, 0.5),
            "{:?} vs {:?}",
            back.rect,
            item.rect
        );
    }
    let after = Shot::take(e, doc, 0, 612, 792);
    assert_eq!(
        before.rgba, after.rgba,
        "the page does not look the same after remove and restore"
    );
}

#[test]
fn removed_text_is_not_left_in_the_saved_file() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::simple_pdf(1), None).unwrap();
    let note = annot(ask(
        e,
        RequestBody::AddNote {
            doc,
            page: 0,
            x: 50.0,
            y: 50.0,
            color: YELLOW,
            text: "ZZUNIQUEMARKERZZ".into(),
        },
    ));
    ask(
        e,
        RequestBody::RemoveAnnot {
            doc,
            page: 0,
            id: note.id,
        },
    );
    let saved = save_all(e, doc);
    assert!(
        !contains(&saved, b"ZZUNIQUEMARKERZZ"),
        "a removed note's text is still in the saved file"
    );
}

// ---- annotations a file already has ---------------------------------------------------------

#[test]
fn annotations_a_file_already_has_are_listed_changed_and_removed() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::annotated_pdf(), None).unwrap();
    let items = list(e, doc, 0);
    let kinds: Vec<AnnotKind> = items.iter().map(|i| i.kind).collect();
    assert_eq!(
        kinds,
        vec![
            AnnotKind::Highlight,
            AnnotKind::Note,
            AnnotKind::Ink,
            AnnotKind::Stamp,
            AnnotKind::Other
        ],
        "the link is not listed"
    );
    let [h, n, ink, stamp, free] = [&items[0], &items[1], &items[2], &items[3], &items[4]];
    assert_eq!(h.text, "Check this title");
    assert_eq!(h.author, "Alice");
    assert!(
        h.color[0] > 200 && h.color[1] < 60 && h.color[2] < 60,
        "{:?}",
        h.color
    );
    assert_eq!(n.text, "A note from the file \u{2713}");
    assert_eq!(n.author, "Bob");
    assert_eq!(free.text, "A text box");
    assert!(
        close(n.rect, [300.0, 72.0, 320.0, 92.0], 1.5),
        "{:?}",
        n.rect
    );
    // What can be done with each.
    assert_eq!(
        h.abilities & ability::RESTORE,
        0,
        "a highlight from a file cannot be put back"
    );
    assert!(
        ink.abilities & ability::RESTORE == 0,
        "a drawing from a file cannot be put back"
    );
    assert!(stamp.abilities & ability::RESTORE == 0);
    assert_eq!(free.abilities, ability::REMOVE);
    // Ids are the same when asked again.
    assert_eq!(
        list(e, doc, 0).iter().map(|i| i.id).collect::<Vec<_>>(),
        items.iter().map(|i| i.id).collect::<Vec<_>>()
    );

    // Change the note's text, remove the drawing and the text box, and save.
    update(e, doc, 0, n.id, Some("changed"), None, 0.0, 0.0);
    assert!(matches!(
        ask(
            e,
            RequestBody::RemoveAnnot {
                doc,
                page: 0,
                id: ink.id
            }
        ),
        ResponseBody::AnnotRemoved
    ));
    assert!(matches!(
        ask(
            e,
            RequestBody::RemoveAnnot {
                doc,
                page: 0,
                id: free.id
            }
        ),
        ResponseBody::AnnotRemoved
    ));
    assert_eq!(
        error_code(ask(
            e,
            RequestBody::RestoreAnnot {
                doc,
                page: 0,
                id: ink.id
            }
        )),
        ErrorCode::AnnotationRefused
    );
    let saved = save_all(e, doc);
    let again = open(e, &saved, None).unwrap();
    let items = list(e, again, 0);
    let kinds: Vec<AnnotKind> = items.iter().map(|i| i.kind).collect();
    assert_eq!(
        kinds,
        vec![AnnotKind::Highlight, AnnotKind::Note, AnnotKind::Stamp]
    );
    assert_eq!(items[1].text, "changed");
    assert_eq!(items[0].author, "Alice");
    assert_eq!(items[0].text, "Check this title");
    // The link is still there, untouched.
    assert!(contains(&saved, b"https://example.com/page"));
}

#[test]
fn a_foreign_highlight_that_is_removed_leaves_the_page_and_is_not_copied_back() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::annotated_pdf(), None).unwrap();
    let h = list(e, doc, 0)
        .into_iter()
        .find(|i| i.kind == AnnotKind::Highlight)
        .unwrap();
    let b = h.rect;
    let before = Shot::take(e, doc, 0, 612, 792);
    assert!(before.near(b, [255, 0, 0], 70) > 100);
    ask(
        e,
        RequestBody::RemoveAnnot {
            doc,
            page: 0,
            id: h.id,
        },
    );
    let gone = Shot::take(e, doc, 0, 612, 792);
    assert_eq!(gone.near(b, [255, 0, 0], 70), 0);
    // A copy would lose what the engine does not read, so a file's annotation is not copied back.
    assert_eq!(
        error_code(ask(
            e,
            RequestBody::RestoreAnnot {
                doc,
                page: 0,
                id: h.id,
            },
        )),
        ErrorCode::AnnotationRefused
    );
    let after = Shot::take(e, doc, 0, 612, 792);
    assert_eq!(after.near(b, [255, 0, 0], 70), 0);
}

// ---- hostile annotations ----------------------------------------------------------------------

#[test]
fn text_and_boxes_from_a_hostile_file_are_cut_cleaned_and_only_text() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::hostile_annots_pdf(), None).unwrap();
    let items = list(e, doc, 0);
    assert_eq!(items.len(), 3);
    for item in &items {
        assert!(item.text.len() <= MAX_ANNOT_TEXT, "{}", item.text.len());
        assert!(
            item.author.len() <= MAX_ANNOT_AUTHOR,
            "{}",
            item.author.len()
        );
        assert!(
            item.rect.iter().all(|v| v.is_finite() && v.abs() <= 1.0e6),
            "{:?}",
            item.rect
        );
    }
    assert_eq!(items[0].text.len(), MAX_ANNOT_TEXT);
    let t = &items[1].text;
    assert!(
        t.contains("<img src=x onerror=alert(1)>"),
        "markup stays as it is: text, {t:?}"
    );
    assert!(
        !t.chars()
            .any(|c| matches!(c, '\u{7}' | '\u{202E}' | '\u{200B}' | '\r')),
        "{t:?}"
    );
    assert!(t.contains("second line"));
    assert_eq!(items[1].author, "<b>author</b>");
    // The huge square can be removed, and the rest is not hurt by it.
    assert!(matches!(
        ask(
            e,
            RequestBody::RemoveAnnot {
                doc,
                page: 0,
                id: items[2].id
            }
        ),
        ResponseBody::AnnotRemoved
    ));
    assert_eq!(list(e, doc, 0).len(), 2);
}

#[test]
fn a_page_with_too_many_annotations_lists_the_first_ones() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::many_annots_pdf(MAX_ANNOTS_PER_PAGE + 100), None).unwrap();
    let items = list(e, doc, 0);
    assert_eq!(items.len(), MAX_ANNOTS_PER_PAGE);
    let mut ids: Vec<u32> = items.iter().map(|i| i.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), MAX_ANNOTS_PER_PAGE, "ids are unique");
    assert_eq!(items[0].text, "Note 0");
    // One that is listed can be removed; the ids of the others stay.
    ask(
        e,
        RequestBody::RemoveAnnot {
            doc,
            page: 0,
            id: items[3].id,
        },
    );
    let after = list(e, doc, 0);
    assert_eq!(after.len(), MAX_ANNOTS_PER_PAGE);
    assert_eq!(after[3].id, items[4].id);
}

#[test]
fn nothing_is_added_to_a_page_that_has_more_annotations_than_can_be_listed() {
    let note = |e: &mut partridge_engine::Engine, doc: u32| {
        ask(
            e,
            RequestBody::AddNote {
                doc,
                page: 0,
                x: 300.0,
                y: 700.0,
                color: YELLOW,
                text: "one more".into(),
            },
        )
    };
    let mut guard = engine();
    let e = &mut guard.0;
    // One short of the most that is looked at: the next one is added, and then no more.
    let doc = open(e, &kit::many_annots_pdf(4_999), None).unwrap();
    let added = annot(note(e, doc));
    assert_eq!(added.text, "one more");
    assert_eq!(
        error_code(note(e, doc)),
        ErrorCode::AnnotationRefused,
        "the page holds 5,000 now"
    );
    // The page is as it was.
    assert_eq!(list(e, doc, 0).len(), MAX_ANNOTS_PER_PAGE);
    // A page that came with more than that takes nothing either.
    let big = open(e, &kit::many_annots_pdf(5_100), None).unwrap();
    assert_eq!(error_code(note(e, big)), ErrorCode::AnnotationRefused);
}

// ---- protected and signed documents ---------------------------------------------------------------

#[test]
fn a_protected_document_stays_protected_with_its_annotation() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::encrypted_pdf("secret", 1), Some("secret")).unwrap();
    annot(ask(
        e,
        RequestBody::AddNote {
            doc,
            page: 0,
            x: 60.0,
            y: 60.0,
            color: YELLOW,
            text: "ZZSECRETNOTEZZ".into(),
        },
    ));
    let saved = save_all(e, doc);
    assert!(
        !contains(&saved, b"ZZSECRETNOTEZZ"),
        "the note is in the file in the clear"
    );
    assert_eq!(
        open(e, &saved, None).unwrap_err(),
        ErrorCode::PasswordRequired
    );
    assert_eq!(
        open(e, &saved, Some("wrong")).unwrap_err(),
        ErrorCode::PasswordIncorrect
    );
    let again = open(e, &saved, Some("secret")).unwrap();
    let items = list(e, again, 0);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].text, "ZZSECRETNOTEZZ");
}

#[test]
fn a_signed_document_is_still_reported_as_signed_after_an_annotation() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::signed_pdf(), None).unwrap();
    annot(ask(
        e,
        RequestBody::AddNote {
            doc,
            page: 0,
            x: 60.0,
            y: 60.0,
            color: YELLOW,
            text: "n".into(),
        },
    ));
    // The signature field is a widget: it is not in the list, and cannot be changed.
    let items = list(e, doc, 0);
    assert_eq!(items.len(), 1);
    match ask(e, RequestBody::DocInfo { doc }) {
        ResponseBody::DocInfo {
            signatures,
            maybe_signed,
            ..
        } => {
            assert!(signatures >= 1 && maybe_signed);
        }
        other => panic!("{other:?}"),
    }
    let saved = save_all(e, doc);
    assert!(contains(&saved, b"/Sig"));
}

// ---- requests that cannot be done ----------------------------------------------------------------

#[test]
fn requests_that_cannot_be_done_say_why() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::simple_pdf(1), None).unwrap();
    assert_eq!(
        error_code(ask(e, RequestBody::ListAnnots { doc: 999, page: 0 })),
        ErrorCode::UnknownDocument
    );
    assert_eq!(
        error_code(ask(e, RequestBody::ListAnnots { doc, page: 5 })),
        ErrorCode::PageOutOfRange
    );
    assert_eq!(
        error_code(ask(
            e,
            RequestBody::AddNote {
                doc,
                page: 7,
                x: 1.0,
                y: 1.0,
                color: YELLOW,
                text: String::new()
            }
        )),
        ErrorCode::PageOutOfRange
    );
    assert_eq!(
        error_code(highlight(e, doc, 0, 100_000, 100_005)),
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        error_code(ask(
            e,
            RequestBody::RemoveAnnot {
                doc,
                page: 0,
                id: 12345
            }
        )),
        ErrorCode::NoSuchAnnotation
    );
    assert_eq!(
        error_code(ask(
            e,
            RequestBody::RestoreAnnot {
                doc,
                page: 0,
                id: 12345
            }
        )),
        ErrorCode::AnnotationRefused
    );
    assert_eq!(
        error_code(update(e, doc, 0, 12345, Some("x"), None, 0.0, 0.0)),
        ErrorCode::NoSuchAnnotation
    );
    // A range that runs past the end of the text is cut to the text.
    let (text, _) = page_text(e, doc, 0);
    let n = text.chars().count() as u32;
    annot(highlight(e, doc, 0, n - 6, n + 500));
    // Points outside the page are pulled onto it, and a note outside the page is put inside.
    let ink = annot(ask(
        e,
        RequestBody::AddInk {
            doc,
            page: 0,
            color: YELLOW,
            width: 2.0,
            strokes: vec![vec![[-500.0, -500.0], [9000.0, 9000.0]]],
        },
    ));
    assert!(
        ink.rect[0] >= -4.0 && ink.rect[2] <= 616.0 && ink.rect[3] <= 796.0,
        "{:?}",
        ink.rect
    );
    let note = annot(ask(
        e,
        RequestBody::AddNote {
            doc,
            page: 0,
            x: 5000.0,
            y: -50.0,
            color: YELLOW,
            text: String::new(),
        },
    ));
    assert!(
        note.rect[2] <= 612.5 && note.rect[1] >= -0.5,
        "{:?}",
        note.rect
    );
}

#[test]
fn the_content_of_the_page_is_not_rewritten_by_an_annotation() {
    let mut guard = engine();
    let e = &mut guard.0;
    let bytes = kit::features_pdf();
    let doc = open(e, &bytes, None).unwrap();
    let count = |e: &mut partridge_engine::Engine, doc| {
        e.edit_for_tests(doc, |d| {
            let page = d.pages().get(0).unwrap();
            let n = page.objects().len();
            n
        })
        .unwrap()
    };
    let before = count(e, doc);
    let (start, len) = find(e, doc, 0, "needle");
    annot(highlight(e, doc, 0, start, start + len));
    annot(ask(
        e,
        RequestBody::AddInk {
            doc,
            page: 0,
            color: YELLOW,
            width: 2.0,
            strokes: vec![vec![[10.0, 10.0], [50.0, 50.0]]],
        },
    ));
    annot(ask(
        e,
        RequestBody::AddStamp {
            doc,
            page: 0,
            x: 300.0,
            y: 300.0,
            stamp: StampKind::Draft,
        },
    ));
    assert_eq!(count(e, doc), before, "the objects of the page changed");
    let saved = save_all(e, doc);
    let again = open(e, &saved, None).unwrap();
    assert_eq!(count(e, again), before);
    assert_eq!(page_text(e, again, 0).0, page_text(e, doc, 0).0);
}
