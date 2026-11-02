//! Page changes, annotations and form fields on one document: they go through the same engine and
//! the same document, one after the other in any order, and all of them reach the saved file.
//!
//! The window keeps one history for all three kinds of change and takes them back in order. These
//! tests do what that history does to the engine, step by step, and check that every step finds
//! what it needs: the ids of the annotations, what can be put back after a removal, and the fields
//! of a form, on the pages as they were when the change was made.
//!
//! These tests run the engine in this process, without the sandbox.

mod common;

use common::*;
use partridge_protocol::{
    AnnotInfo, ErrorCode, FormField, FormValue, PagesRequest, PagesResponse, RequestBody,
    ResponseBody,
};
use partridge_testkit as kit;

type Eng = partridge_engine::Engine;

fn fields(e: &mut Eng, doc: u32, page: u32) -> Vec<FormField> {
    match ask(e, RequestBody::ListFormFields { doc, page }) {
        ResponseBody::FormFields { fields } => fields,
        other => panic!("{other:?}"),
    }
}

fn set_text(e: &mut Eng, doc: u32, page: u32, name: &str, text: &str) -> Vec<FormField> {
    let field = fields(e, doc, page)
        .into_iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no field {name} on page {page}"));
    match ask(
        e,
        RequestBody::SetFormField {
            doc,
            page,
            index: field.index,
            name: field.name,
            value: FormValue::Text(text.into()),
        },
    ) {
        ResponseBody::FormFields { fields } => fields,
        other => panic!("{other:?}"),
    }
}

fn value_of(e: &mut Eng, doc: u32, page: u32, name: &str) -> String {
    fields(e, doc, page)
        .into_iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no field {name} on page {page}"))
        .value
}

fn pages_op(e: &mut Eng, op: PagesRequest) -> PagesResponse {
    match ask(e, RequestBody::Pages(op)) {
        ResponseBody::Pages(r) => r,
        other => panic!("{other:?}"),
    }
}

/// The number of the copy a change left (to go back to), and the sizes of the pages.
fn snapshot_of(r: PagesResponse) -> u32 {
    match r {
        PagesResponse::Changed { snapshot, .. } => snapshot,
        other => panic!("{other:?}"),
    }
}

fn note(e: &mut Eng, doc: u32, page: u32, text: &str) -> AnnotInfo {
    annot(ask(
        e,
        RequestBody::AddNote {
            doc,
            page,
            x: 300.0,
            y: 300.0,
            color: YELLOW,
            text: text.into(),
        },
    ))
}

fn texts(e: &mut Eng, doc: u32, count: u32) -> Vec<String> {
    (0..count)
        .map(|p| page_text(e, doc, p).0.trim().to_string())
        .collect()
}

fn remove(e: &mut Eng, doc: u32, page: u32, id: u32) {
    assert!(
        matches!(
            ask(e, RequestBody::RemoveAnnot { doc, page, id }),
            ResponseBody::AnnotRemoved
        ),
        "removing annotation {id} from page {page}"
    );
}

fn restore(e: &mut Eng, doc: u32, page: u32, id: u32) -> AnnotInfo {
    annot(ask(e, RequestBody::RestoreAnnot { doc, page, id }))
}

#[test]
fn an_annotation_a_field_and_a_move_of_pages_all_reach_the_saved_file() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::forms_multipage_pdf(false), None).unwrap();
    assert!(matches!(
        ask(e, RequestBody::FormInfo { doc }),
        ResponseBody::FormInfo { .. }
    ));
    assert_eq!(texts(e, doc, 3), ["Page 1", "Page 2", "Page 3"]);

    // An annotation on the third page, a field with widgets on the first and the third page.
    let made = note(e, doc, 2, "NOTE-ON-PAGE-THREE");
    set_text(e, doc, 0, "applicant", "Ada Lovelace");
    assert_eq!(value_of(e, doc, 2, "applicant"), "Ada Lovelace");

    // The third page goes first.
    let snapshot = snapshot_of(pages_op(
        e,
        PagesRequest::Reorder {
            doc,
            order: vec![2, 0, 1],
        },
    ));
    assert_ne!(snapshot, 0);
    assert_eq!(texts(e, doc, 3), ["Page 3", "Page 1", "Page 2"]);
    // The note went with its page, and the field with both of its widgets.
    let on_first = list(e, doc, 0);
    assert_eq!(on_first.len(), 1, "{on_first:?}");
    assert_eq!(on_first[0].text, "NOTE-ON-PAGE-THREE");
    assert!(list(e, doc, 1).is_empty() && list(e, doc, 2).is_empty());
    assert_eq!(value_of(e, doc, 0, "applicant"), "Ada Lovelace");
    assert_eq!(value_of(e, doc, 1, "applicant"), "Ada Lovelace");

    // The form still takes a change, on the pages as they are now, and so do the annotations.
    set_text(e, doc, 1, "applicant", "Ada King");
    assert_eq!(value_of(e, doc, 0, "applicant"), "Ada King");
    let second = note(e, doc, 1, "NOTE-ON-PAGE-ONE");

    // Saved, and read again by a second engine session.
    let bytes = save_all(e, doc);
    let again = open(e, &bytes, None).unwrap();
    assert_eq!(texts(e, again, 3), ["Page 3", "Page 1", "Page 2"]);
    let first = list(e, again, 0);
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first[0].text, "NOTE-ON-PAGE-THREE");
    let middle = list(e, again, 1);
    assert_eq!(middle.len(), 1, "{middle:?}");
    assert_eq!(middle[0].text, "NOTE-ON-PAGE-ONE");
    assert!(list(e, again, 2).is_empty());
    assert!(matches!(
        ask(e, RequestBody::FormInfo { doc: again }),
        ResponseBody::FormInfo { .. }
    ));
    assert_eq!(value_of(e, again, 0, "applicant"), "Ada King");
    assert_eq!(value_of(e, again, 1, "applicant"), "Ada King");
    assert!(
        contains(&bytes, b"Ada King"),
        "the field value is in the file"
    );
    let _ = (made, second);
}

#[test]
fn the_history_of_a_mix_of_all_three_goes_back_and_forward_in_order() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::forms_multipage_pdf(false), None).unwrap();
    assert!(matches!(
        ask(e, RequestBody::FormInfo { doc }),
        ResponseBody::FormInfo { .. }
    ));

    // 1. a note on the third page, 2. a field, 3. the pages move, 4. a note on the (new) first
    // page, 5. the field changes again.
    let first = note(e, doc, 2, "FIRST");
    set_text(e, doc, 0, "applicant", "One");
    let moved = snapshot_of(pages_op(
        e,
        PagesRequest::Reorder {
            doc,
            order: vec![2, 0, 1],
        },
    ));
    let second = note(e, doc, 0, "SECOND");
    set_text(e, doc, 1, "applicant", "Two");
    assert_eq!(texts(e, doc, 3), ["Page 3", "Page 1", "Page 2"]);

    // Back, newest first: the field, the second note, the move, the field, the first note.
    set_text(e, doc, 1, "applicant", "One");
    remove(e, doc, 0, second.id);
    let redo_move = snapshot_of(pages_op(
        e,
        PagesRequest::Restore {
            doc,
            snapshot: moved,
        },
    ));
    assert_eq!(texts(e, doc, 3), ["Page 1", "Page 2", "Page 3"]);
    assert_eq!(value_of(e, doc, 0, "applicant"), "One");
    // The first note is where it was made, with the id it had, and can be taken off and put back.
    let listed = list(e, doc, 2);
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].id, first.id);
    set_text(e, doc, 0, "applicant", "");
    remove(e, doc, 2, first.id);
    assert!(list(e, doc, 2).is_empty());

    // Forward again, oldest first.
    assert_eq!(restore(e, doc, 2, first.id).id, first.id);
    set_text(e, doc, 0, "applicant", "One");
    let back_again = snapshot_of(pages_op(
        e,
        PagesRequest::Restore {
            doc,
            snapshot: redo_move,
        },
    ));
    assert_ne!(back_again, 0);
    assert_eq!(texts(e, doc, 3), ["Page 3", "Page 1", "Page 2"]);
    // The second note can be put back where it was, on the first page as it is now.
    assert_eq!(restore(e, doc, 0, second.id).text, "SECOND");
    set_text(e, doc, 1, "applicant", "Two");
    assert_eq!(value_of(e, doc, 0, "applicant"), "Two");

    // Everything is in the saved file.
    let bytes = save_all(e, doc);
    let reopened = open(e, &bytes, None).unwrap();
    assert_eq!(texts(e, reopened, 3), ["Page 3", "Page 1", "Page 2"]);
    let mut on_first: Vec<String> = list(e, reopened, 0).into_iter().map(|a| a.text).collect();
    on_first.sort();
    assert_eq!(on_first, ["FIRST", "SECOND"]);
}

#[test]
fn a_change_of_the_pages_does_not_leave_the_form_environment_or_the_annotation_record_behind() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::forms_multipage_pdf(false), None).unwrap();
    assert!(matches!(
        ask(e, RequestBody::FormInfo { doc }),
        ResponseBody::FormInfo { .. }
    ));
    set_text(e, doc, 0, "applicant", "Before");
    assert!(e.form_scans_for_tests(doc).is_some());
    let made = note(e, doc, 0, "KEEP");

    // A move of the pages: the environment of the form is dropped (it holds the document and its
    // pages as they were), and made again from the document when it is next asked for.
    let moved = snapshot_of(pages_op(
        e,
        PagesRequest::Reorder {
            doc,
            order: vec![1, 2, 0],
        },
    ));
    assert_eq!(e.form_scans_for_tests(doc), None);
    assert_eq!(value_of(e, doc, 2, "applicant"), "Before");
    assert!(e.form_scans_for_tests(doc).is_some());
    // The annotation moved with its page. Its id is the document's own again, and the page can be
    // annotated and the new annotation listed with it.
    let listed = list(e, doc, 2);
    assert_eq!(listed.len(), 1, "{listed:?}");
    let other = note(e, doc, 2, "ALSO");
    assert_ne!(other.id, listed[0].id);
    assert_eq!(list(e, doc, 2).len(), 2);
    remove(e, doc, 2, other.id);

    // Back: the record that belongs to the earlier state is back, so the note made then can be
    // taken off and put back by the id it was given then.
    pages_op(
        e,
        PagesRequest::Restore {
            doc,
            snapshot: moved,
        },
    );
    assert_eq!(e.form_scans_for_tests(doc), None);
    remove(e, doc, 0, made.id);
    assert_eq!(restore(e, doc, 0, made.id).text, "KEEP");
    // An id from the other layout names nothing on this page.
    assert_eq!(
        error_code(ask(
            e,
            RequestBody::RemoveAnnot {
                doc,
                page: 1,
                id: made.id
            }
        )),
        ErrorCode::NoSuchAnnotation
    );
}

#[test]
fn a_turn_or_a_crop_does_not_touch_the_record_of_the_annotations() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::forms_multipage_pdf(false), None).unwrap();
    let made = note(e, doc, 1, "STAYS");
    remove(e, doc, 1, made.id);
    // A turn changes no page number: what can be put back after a removal is still there.
    pages_op(
        e,
        PagesRequest::Rotate {
            doc,
            pages: vec![1],
            turns: 1,
        },
    );
    assert_eq!(restore(e, doc, 1, made.id).text, "STAYS");
}

#[test]
fn deleting_a_page_takes_the_notes_made_on_it_out_of_the_file() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::forms_multipage_pdf(false), None).unwrap();
    let on_two = note(e, doc, 1, "SECRET-NOTE-OF-PAGE-TWO");
    let on_one = note(e, doc, 0, "KEPT-NOTE-OF-PAGE-ONE");
    let highlight = annot(ask(
        e,
        RequestBody::AddMarkup {
            doc,
            page: 1,
            kind: partridge_protocol::AnnotKind::Highlight,
            color: YELLOW,
            start: 0,
            end: 4,
        },
    ));
    let _ = (on_two, highlight);
    let snapshot = snapshot_of(pages_op(
        e,
        PagesRequest::Delete {
            doc,
            pages: vec![1],
        },
    ));
    assert_ne!(snapshot, 0);
    assert_eq!(texts(e, doc, 2), ["Page 1", "Page 3"]);
    // The first page keeps its note, and it is listed on the page it is on.
    let listed = list(e, doc, 0);
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].text, "KEPT-NOTE-OF-PAGE-ONE");
    assert!(list(e, doc, 1).is_empty());
    // What the page held is not in the saved file; what the others hold is.
    let bytes = save_all(e, doc);
    assert!(
        !contains(&bytes, b"SECRET-NOTE-OF-PAGE-TWO"),
        "the text of a note of a deleted page is in the saved file"
    );
    assert!(contains(&bytes, b"KEPT-NOTE-OF-PAGE-ONE"));
    let _ = on_one;
    // Taking the delete back brings the page and its note back.
    pages_op(e, PagesRequest::Restore { doc, snapshot });
    assert_eq!(texts(e, doc, 3), ["Page 1", "Page 2", "Page 3"]);
    let back = list(e, doc, 1);
    assert!(
        back.iter().any(|a| a.text == "SECRET-NOTE-OF-PAGE-TWO"),
        "{back:?}"
    );
}

#[test]
fn deleting_a_page_with_form_fields_says_how_many_and_keeps_the_other_widgets_of_the_field() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::forms_multipage_pdf(false), None).unwrap();
    assert!(matches!(
        ask(e, RequestBody::FormInfo { doc }),
        ResponseBody::FormInfo { .. }
    ));
    set_text(e, doc, 2, "applicant", "TYPED-ON-PAGE-THREE");
    let reply = pages_op(
        e,
        PagesRequest::Delete {
            doc,
            pages: vec![2],
        },
    );
    let PagesResponse::Changed { form_fields, .. } = reply else {
        panic!("{reply:?}");
    };
    // Two widgets were on the page: the text field and the list.
    assert_eq!(form_fields, 2);
    // The field has a widget on the first page still, so it is still in the document and a new
    // request finds it there, with what was typed (the document is as it was for every page that
    // stays).
    assert_eq!(texts(e, doc, 2), ["Page 1", "Page 2"]);
    assert_eq!(
        value_of(e, doc, 0, "applicant"),
        "TYPED-ON-PAGE-THREE",
        "a field that has a widget on a page that stays keeps its value"
    );
    let bytes = save_all(e, doc);
    let again = open(e, &bytes, None).unwrap();
    assert!(matches!(
        ask(e, RequestBody::FormInfo { doc: again }),
        ResponseBody::FormInfo { .. }
    ));
    assert_eq!(value_of(e, again, 0, "applicant"), "TYPED-ON-PAGE-THREE");
}
