//! What the reader must not lose, leave behind or allow when an annotation comes from a file:
//! every entry of the dictionary of an annotation that is recoloured, the whole comment, the text
//! of what was deleted, and the limits the file and the document set.

mod common;

use common::*;
use partridge_protocol::{ability, AnnotInfo, AnnotKind, ErrorCode, RequestBody, ResponseBody};
use partridge_testkit as kit;
use pdfium_render::prelude::*;

const GREEN: [u8; 4] = [0, 200, 0, 255];

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

fn remove(e: &mut partridge_engine::Engine, doc: u32, id: u32) -> ResponseBody {
    ask(e, RequestBody::RemoveAnnot { doc, page: 0, id })
}

fn restore(e: &mut partridge_engine::Engine, doc: u32, id: u32) -> ResponseBody {
    ask(e, RequestBody::RestoreAnnot { doc, page: 0, id })
}

fn open_foreign(e: &mut partridge_engine::Engine) -> (u32, Vec<AnnotInfo>) {
    let doc = open(e, &kit::foreign_annots_pdf(), None).unwrap();
    let items = list(e, doc, 0);
    (doc, items)
}

fn by_author(items: &[AnnotInfo], author: &str) -> AnnotInfo {
    items
        .iter()
        .find(|i| i.author == author)
        .unwrap_or_else(|| panic!("no annotation by {author}"))
        .clone()
}

fn by_text(items: &[AnnotInfo], text: &str) -> AnnotInfo {
    items
        .iter()
        .find(|i| i.text == text)
        .unwrap_or_else(|| panic!("no annotation with the text {text}"))
        .clone()
}

/// The comment of the annotation by `author`, exactly as the document holds it.
fn contents_by(e: &mut partridge_engine::Engine, doc: u32, author: &str) -> String {
    e.edit_for_tests(doc, |document| {
        let page = document.pages().get(0).unwrap();
        for annotation in page.annotations().iter() {
            if annotation.creator().as_deref() == Some(author) {
                return annotation.contents().unwrap_or_default();
            }
        }
        panic!("no annotation by {author}");
    })
    .expect("the document is open")
}

fn annotation_count(e: &mut partridge_engine::Engine, doc: u32) -> usize {
    e.edit_for_tests(doc, |document| {
        document.pages().get(0).unwrap().annotations().len()
    })
    .expect("the document is open")
}

/// The text of the object of the saved file that holds `marker`.
fn object_with(saved: &[u8], marker: &str) -> String {
    let text = String::from_utf8_lossy(saved);
    let at = text
        .find(marker)
        .unwrap_or_else(|| panic!("{marker} is not in the file"));
    let start = text[..at].rfind(" obj").map(|p| p + 4).unwrap_or(0);
    let end = text[at..]
        .find("endobj")
        .map(|p| p + at)
        .unwrap_or(text.len());
    text[start..end].to_string()
}

fn number_after(text: &str, key: &str) -> Option<f32> {
    let rest = &text[text.find(key)? + key.len()..];
    let number: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    number.parse().ok()
}

// ---- a file annotation keeps everything it has --------------------------------------------------

#[test]
fn recolouring_a_highlight_from_a_file_changes_only_its_colour() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, items) = open_foreign(e);
    let h = by_author(&items, "Alice");
    assert!(h.abilities & ability::RECOLOUR != 0);
    let green = annot(update(e, doc, 0, h.id, None, Some(GREEN), 0.0, 0.0));
    assert_eq!(green.id, h.id);
    assert!(
        green.color[1] > 150 && green.color[0] < 80,
        "{:?}",
        green.color
    );
    let saved = save_all(e, doc);
    // It is the same annotation, not a copy: its marker is in one object.
    assert_eq!(
        saved.windows(8).filter(|w| *w == b"NMMARKER").count(),
        1,
        "the annotation was made again"
    );
    let object = object_with(&saved, "NMMARKER");
    for key in [
        "/RC",
        "/NM",
        "/CreationDate",
        "/OC",
        "/Popup",
        "/Subj",
        "D:20240102030405Z",
        "D:20230101000000Z",
    ] {
        assert!(object.contains(key), "{key} was lost");
    }
    let opacity = number_after(&object, "/CA").expect("the opacity was lost");
    assert!((opacity - 0.5).abs() < 0.01, "opacity {opacity}");
    // The rich text and the subject still say what they said.
    assert!(kit::text_in_file(&saved, "RCMARKERONE"));
    assert!(kit::text_in_file(&saved, "SUBJMARKERONE"));
    // The reply and the popup are still on the page.
    assert!(kit::text_in_file(&saved, "REPLYMARKER"));
    assert!(object.contains("/Popup"));
    // The whole comment, line ends and all, and the colour.
    let again = open(e, &saved, None).unwrap();
    assert_eq!(contents_by(e, again, "Alice"), kit::foreign_long_comment());
    let back = by_author(&list(e, again, 0), "Alice");
    assert!(
        back.color[1] > 150 && back.color[0] < 80,
        "{:?}",
        back.color
    );
}

#[test]
fn a_comment_that_was_cut_is_not_edited_and_one_with_breaks_comes_back_whole() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, items) = open_foreign(e);
    let long = by_author(&items, "Alice");
    assert!(long.text.len() <= partridge_protocol::MAX_ANNOT_TEXT);
    assert_eq!(
        long.abilities & ability::EDIT_TEXT,
        0,
        "a cut comment can be edited"
    );
    assert_eq!(
        error_code(update(e, doc, 0, long.id, Some("short"), None, 0.0, 0.0)),
        ErrorCode::AnnotationRefused
    );
    assert_eq!(contents_by(e, doc, "Alice"), kit::foreign_long_comment());

    // A comment whose lines end with CR is shown with line feeds and can be edited; taking the
    // edit back writes the CR again.
    let dave = by_author(&items, "Dave");
    assert!(dave.abilities & ability::EDIT_TEXT != 0);
    assert_eq!(dave.text, "line one\nline two");
    annot(update(e, doc, 0, dave.id, Some("changed"), None, 0.0, 0.0));
    assert_eq!(contents_by(e, doc, "Dave"), "changed");
    annot(update(
        e,
        doc,
        0,
        dave.id,
        Some("line one\nline two"),
        None,
        0.0,
        0.0,
    ));
    let saved = save_all(e, doc);
    let again = open(e, &saved, None).unwrap();
    assert_eq!(contents_by(e, again, "Dave"), "line one\rline two");

    // A note whose contents are locked cannot be edited, but can be moved.
    let carol = by_author(&items, "Carol");
    assert_eq!(carol.abilities & ability::EDIT_TEXT, 0);
    assert!(carol.abilities & ability::MOVE != 0);
}

#[test]
fn only_what_the_engine_made_is_put_back() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, items) = open_foreign(e);
    let dave = by_author(&items, "Dave");
    assert_eq!(
        dave.abilities & ability::RESTORE,
        0,
        "a highlight from a file is offered a restore"
    );
    assert!(matches!(
        remove(e, doc, dave.id),
        ResponseBody::AnnotRemoved
    ));
    assert_eq!(
        error_code(restore(e, doc, dave.id)),
        ErrorCode::AnnotationRefused
    );

    // One the engine made can be removed and put back, and has the restore ability.
    let fresh = open(e, &kit::features_pdf(), None).unwrap();
    let (start, len) = find(e, fresh, 0, "needle");
    let h = annot(ask(
        e,
        RequestBody::AddMarkup {
            doc: fresh,
            page: 0,
            kind: AnnotKind::Highlight,
            color: YELLOW,
            start,
            end: start + len,
        },
    ));
    assert!(h.abilities & ability::RESTORE != 0);
    assert!(matches!(
        ask(
            e,
            RequestBody::RemoveAnnot {
                doc: fresh,
                page: 0,
                id: h.id
            }
        ),
        ResponseBody::AnnotRemoved
    ));
    let back = annot(ask(
        e,
        RequestBody::RestoreAnnot {
            doc: fresh,
            page: 0,
            id: h.id,
        },
    ));
    assert_eq!(back.id, h.id);
}

// ---- what was deleted is gone from the file ------------------------------------------------------------

const DELETED: [&str; 12] = [
    "salary 90k",
    "RCSALARY",
    "SUBJSALARY",
    "POPUPSECRET",
    "FREECONTENTSONE",
    "FREESUBJONE",
    "FREERCONE",
    "FREECONTENTSTWO",
    "APSECRETONE",
    "APSECRETTWO",
    // Made here: the first text is replaced in place, the second is deleted with the note.
    "MADESECRETONE",
    "MADESECRETTWO",
];

#[test]
fn deleted_comments_leave_nothing_of_their_text_in_the_saved_file() {
    let mut guard = engine();
    let e = &mut guard.0;
    // The search can see the text when it is there, whatever its form: in a string, in hex, in
    // an appearance stream, and in one that is compressed.
    let whole = open(e, &kit::foreign_annots_pdf(), None).unwrap();
    let untouched = save_all(e, whole);
    for marker in &DELETED[..10] {
        assert!(
            kit::text_in_file(&untouched, marker),
            "the search does not find {marker} in a file that has it"
        );
    }

    let (doc, items) = open_foreign(e);
    let before = annotation_count(e, doc);
    let erin = by_author(&items, "Erin");
    // Recolour, then delete.
    annot(update(e, doc, 0, erin.id, None, Some(GREEN), 0.0, 0.0));
    assert!(matches!(
        remove(e, doc, erin.id),
        ResponseBody::AnnotRemoved
    ));
    for text in ["FREECONTENTSONE", "FREECONTENTSTWO"] {
        let free = by_text(&list(e, doc, 0), text);
        assert!(matches!(
            remove(e, doc, free.id),
            ResponseBody::AnnotRemoved
        ));
    }
    // A note made here, with its text replaced and then deleted: its dictionary is a new object
    // that PDFium writes even when nothing refers to it, so its text must be overwritten.
    let note = annot(ask(
        e,
        RequestBody::AddNote {
            doc,
            page: 0,
            x: 400.0,
            y: 300.0,
            color: YELLOW,
            text: "MADESECRETONE".into(),
        },
    ));
    annot(update(
        e,
        doc,
        0,
        note.id,
        Some("MADESECRETTWO"),
        None,
        0.0,
        0.0,
    ));
    assert!(matches!(
        remove(e, doc, note.id),
        ResponseBody::AnnotRemoved
    ));
    // The popup of the highlight went with it.
    assert_eq!(annotation_count(e, doc), before - 4);
    let saved = save_all(e, doc);
    for marker in DELETED {
        assert!(
            !kit::text_in_file(&saved, marker),
            "{marker} is still in the saved file"
        );
    }
    // What was not deleted is still there.
    assert!(kit::text_in_file(&saved, "RCMARKERONE"));
    assert!(kit::text_in_file(&saved, "REPLYMARKER"));
}

// ---- what the file and the document forbid --------------------------------------------------------------

#[test]
fn locked_and_read_only_annotations_cannot_be_changed() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, items) = open_foreign(e);
    for text in ["LOCKEDSTAMP", "READONLYSTAMP"] {
        let stamp = by_text(&items, text);
        assert_eq!(stamp.abilities, ability::LOCKED, "{text}");
        assert_eq!(
            error_code(remove(e, doc, stamp.id)),
            ErrorCode::AnnotationRefused,
            "{text} was removed"
        );
        assert_eq!(
            error_code(update(e, doc, 0, stamp.id, None, None, 5.0, 5.0)),
            ErrorCode::AnnotationRefused
        );
    }
    // The others are not locked.
    assert!(by_author(&items, "Erin").abilities & ability::REMOVE != 0);
}

#[test]
fn a_document_that_forbids_annotating_refuses_every_change() {
    let mut guard = engine();
    let e = &mut guard.0;
    // Bit 6 of the permissions (value 32) is the one for annotating.
    let closed = open(
        e,
        &kit::encrypted_pdf_with_permissions("pw", 1, -4 & !32),
        Some("pw"),
    )
    .unwrap();
    let note = |doc| RequestBody::AddNote {
        doc,
        page: 0,
        x: 50.0,
        y: 50.0,
        color: YELLOW,
        text: "hello".into(),
    };
    assert_eq!(
        error_code(ask(e, note(closed))),
        ErrorCode::AnnotationsNotAllowed
    );
    assert_eq!(
        error_code(ask(
            e,
            RequestBody::AddMarkup {
                doc: closed,
                page: 0,
                kind: AnnotKind::Highlight,
                color: YELLOW,
                start: 0,
                end: 3,
            }
        )),
        ErrorCode::AnnotationsNotAllowed
    );
    // The same file with the bit set takes a note.
    let open_doc = open(e, &kit::encrypted_pdf("pw", 1), Some("pw")).unwrap();
    annot(ask(e, note(open_doc)));
}

// ---- boxes and moves stay on the page ----------------------------------------------------------------------

#[test]
fn a_box_far_outside_its_page_is_cut_to_the_page() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::hostile_annots_pdf(), None).unwrap();
    let items = list(e, doc, 0);
    let square = items.iter().find(|i| i.kind == AnnotKind::Other).unwrap();
    assert!(
        square.rect[0] >= 0.0
            && square.rect[1] >= 0.0
            && square.rect[2] <= 612.0
            && square.rect[3] <= 792.0,
        "{:?}",
        square.rect
    );
}

#[test]
fn a_note_cannot_be_pushed_off_its_page() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::simple_pdf(1), None).unwrap();
    let note = annot(ask(
        e,
        RequestBody::AddNote {
            doc,
            page: 0,
            x: 100.0,
            y: 100.0,
            color: YELLOW,
            text: "n".into(),
        },
    ));
    let far = annot(update(e, doc, 0, note.id, None, None, 100_000.0, 100_000.0));
    assert!(
        far.rect[2] <= 612.5 && far.rect[3] <= 792.5,
        "{:?}",
        far.rect
    );
    let near = annot(update(
        e, doc, 0, note.id, None, None, -100_000.0, -100_000.0,
    ));
    assert!(
        near.rect[0] >= -0.5 && near.rect[1] >= -0.5,
        "{:?}",
        near.rect
    );
}

// ---- taking a change back writes the file's own data again ---------------------------------------------

#[test]
fn taking_back_a_text_change_writes_the_files_own_rich_text_and_date_again() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, items) = open_foreign(e);
    let erin = by_author(&items, "Erin");
    assert!(erin.abilities & ability::EDIT_TEXT != 0);
    let rich = "<body xmlns=\"http://www.w3.org/1999/xhtml\"><p>RCSALARY</p></body>";

    // While the text is changed the rich text says the new words, so the search can tell the two apart.
    annot(update(e, doc, 0, erin.id, Some("changed"), None, 0.0, 0.0));
    let middle = save_all(e, doc);
    assert!(!kit::text_in_file(&middle, "RCSALARY"));
    assert!(kit::text_in_file(&middle, "changed"));

    // Taking the change back (what Undo asks for) puts the file's rich text and date back as well,
    // and the document then reads as it did.
    annot(update(
        e,
        doc,
        0,
        erin.id,
        Some("salary 90k"),
        None,
        0.0,
        0.0,
    ));
    let saved = save_all(e, doc);
    assert!(
        kit::text_in_file(&saved, rich),
        "the rich text is not as it was"
    );
    assert!(kit::text_in_file(&saved, "D:20240303040506Z"));
    assert!(!kit::text_in_file(&saved, "changed"));
    let again = open(e, &saved, None).unwrap();
    assert_eq!(contents_by(e, again, "Erin"), "salary 90k");
}

#[test]
fn a_highlight_drawn_through_a_form_keeps_its_colour_when_a_recolour_is_taken_back() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::nested_appearance_pdf(), None).unwrap();
    let items = list(e, doc, 0);
    let one = by_text(&items, "NESTEDONE");
    // The red is read from the form inside the appearance, not guessed.
    assert!(
        one.color[0] > 200 && one.color[1] < 60 && one.color[2] < 60,
        "{:?}",
        one.color
    );
    assert!(one.abilities & ability::RECOLOUR != 0);
    let green = annot(update(e, doc, 0, one.id, None, Some(GREEN), 0.0, 0.0));
    assert!(
        green.color[1] > 150 && green.color[0] < 80,
        "{:?}",
        green.color
    );
    // What Undo asks for: the colour the list gave.
    let back = annot(update(e, doc, 0, one.id, None, Some(one.color), 0.0, 0.0));
    assert!(
        back.color[0] > 200 && back.color[1] < 60 && back.color[2] < 60,
        "{:?}",
        back.color
    );
}

#[test]
fn a_highlight_whose_colour_cannot_be_read_is_not_recoloured() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::nested_appearance_pdf(), None).unwrap();
    let two = by_text(&list(e, doc, 0), "NESTEDTWO");
    assert_eq!(two.abilities & ability::RECOLOUR, 0);
    assert!(two.abilities & ability::REMOVE != 0);
    assert_eq!(
        error_code(update(e, doc, 0, two.id, None, Some(GREEN), 0.0, 0.0)),
        ErrorCode::AnnotationRefused
    );
    // Its own appearance is still the file's.
    let saved = save_all(e, doc);
    assert!(kit::text_in_file(&saved, "NOCOLOURAP"));
}

#[test]
fn a_stroked_highlight_and_a_filled_underline_list_in_the_colour_they_are_drawn_in() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::nested_appearance_pdf(), None).unwrap();
    let items = list(e, doc, 0);
    for words in ["STROKEDHL", "FILLEDUL"] {
        let item = by_text(&items, words);
        // Drawn in red; PDFium answers black for the side that is not painted.
        assert!(
            item.color[0] > 200 && item.color[1] < 60 && item.color[2] < 60,
            "{words}: {:?}",
            item.color
        );
        assert!(item.abilities & ability::RECOLOUR != 0, "{words}");
        // Recolour, then take it back with the colour the list gave.
        let green = annot(update(e, doc, 0, item.id, None, Some(GREEN), 0.0, 0.0));
        assert!(green.color[1] > 150 && green.color[0] < 80, "{words}");
        let back = annot(update(e, doc, 0, item.id, None, Some(item.color), 0.0, 0.0));
        assert!(
            back.color[0] > 200 && back.color[1] < 60 && back.color[2] < 60,
            "{words}: {:?}",
            back.color
        );
    }
}

// ---- a popup that is not a popup --------------------------------------------------------------------------

#[test]
fn a_highlight_whose_popup_names_another_annotation_does_not_take_it_along() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::odd_links_pdf(), None).unwrap();
    let items = list(e, doc, 0);
    let highlight = by_text(&items, "OWNERWORDS");
    let stamp = by_text(&items, "STAMPSTAYS");
    assert!(stamp.abilities & ability::LOCKED != 0);
    assert_eq!(annotation_count(e, doc), 3);
    assert!(matches!(
        remove(e, doc, highlight.id),
        ResponseBody::AnnotRemoved
    ));
    // The locked stamp is still on the page, with its words.
    assert_eq!(annotation_count(e, doc), 2);
    let left = list(e, doc, 0);
    assert!(left.iter().any(|i| i.text == "STAMPSTAYS"));
    assert!(kit::text_in_file(&save_all(e, doc), "STAMPSTAYS"));
}

// ---- a note that a file puts partly outside its page -----------------------------------------------------

/// The left edge of the box of the note, as the document holds it (not cut to the page).
fn left_edge(e: &mut partridge_engine::Engine, doc: u32) -> f32 {
    e.edit_for_tests(doc, |document| {
        let page = document.pages().get(0).unwrap();
        for annotation in page.annotations().iter() {
            if annotation.contents().as_deref() == Some("EDGENOTE") {
                return annotation.bounds().unwrap().left().value;
            }
        }
        panic!("no EDGENOTE");
    })
    .expect("the document is open")
}

#[test]
fn a_move_of_a_note_cut_at_the_page_edge_is_taken_back_to_where_it_was() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::odd_links_pdf(), None).unwrap();
    let note = by_text(&list(e, doc, 0), "EDGENOTE");
    // The list shows the box cut to the page; the file has it 10 points further out.
    assert_eq!(note.rect[0], 0.0);
    assert!((left_edge(e, doc) + 10.0).abs() < 0.01);
    // It cannot be pushed further out.
    update(e, doc, 0, note.id, None, None, -4.0, 0.0);
    assert!((left_edge(e, doc) + 10.0).abs() < 0.01);
    // A move in, twice (as two quick key presses), and the same distances back.
    let moved = annot(update(e, doc, 0, note.id, None, None, 4.0, 0.0));
    assert!((moved.rect[2] - note.rect[2] - 4.0).abs() < 0.01);
    update(e, doc, 0, note.id, None, None, 4.0, 0.0);
    assert!((left_edge(e, doc) + 2.0).abs() < 0.01);
    update(e, doc, 0, note.id, None, None, -4.0, 0.0);
    let back = annot(update(e, doc, 0, note.id, None, None, -4.0, 0.0));
    assert!(
        (left_edge(e, doc) + 10.0).abs() < 0.01,
        "{}",
        left_edge(e, doc)
    );
    assert!((back.rect[2] - note.rect[2]).abs() < 0.01);
}

// ---- the owner of a document -----------------------------------------------------------------------------

#[test]
fn the_owner_password_allows_what_the_user_password_does_not() {
    let mut guard = engine();
    let e = &mut guard.0;
    // Bit 6 of the permissions (value 32) is the one for annotating; it is clear for the user.
    let file = kit::encrypted_pdf_with_owner("user", "owner", 1, -4 & !32);
    let note = |doc| RequestBody::AddNote {
        doc,
        page: 0,
        x: 50.0,
        y: 50.0,
        color: YELLOW,
        text: "hello".into(),
    };
    let as_user = open(e, &file, Some("user")).unwrap();
    assert_eq!(
        error_code(ask(e, note(as_user))),
        ErrorCode::AnnotationsNotAllowed
    );
    let as_owner = open(e, &file, Some("owner")).unwrap();
    annot(ask(e, note(as_owner)));
}

// ---- a limit that is known -------------------------------------------------------------------------------

#[test]
fn a_free_text_box_without_an_appearance_keeps_its_words_in_the_saved_file_after_a_delete() {
    // PDFium makes the appearance of such a box, with its words, when the page is drawn, and a
    // save writes what it made even when nothing refers to it any more. No call overwrites it.
    // The README says so. If this test fails because the words are gone, PDFium changed: take the
    // exception out of the README and out of the question that is asked before a delete.
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::free_text_without_appearance_pdf(), None).unwrap();
    let free = by_text(&list(e, doc, 0), "BAREFREEWORDS");
    assert!(matches!(
        remove(e, doc, free.id),
        ResponseBody::AnnotRemoved
    ));
    assert_eq!(annotation_count(e, doc), 0);
    let saved = save_all(e, doc);
    assert!(
        kit::text_in_file(&saved, "BAREFREEWORDS"),
        "the words are gone from the saved file: the README no longer needs its exception"
    );
}
