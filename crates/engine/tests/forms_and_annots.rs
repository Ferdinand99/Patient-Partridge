//! A document that is filled in and annotated: the two kinds of change go through the same engine
//! and the same document, one after the other in any order, and both reach the saved file.
//!
//! These tests run the engine in this process, without the sandbox.

mod common;

use common::*;
use partridge_protocol::{
    AnnotKind, ErrorCode, FormField, FormValue, RequestBody, ResponseBody, StampKind,
};
use partridge_testkit as kit;

fn fields(e: &mut partridge_engine::Engine, doc: u32) -> Vec<FormField> {
    match ask(e, RequestBody::ListFormFields { doc, page: 0 }) {
        ResponseBody::FormFields { fields } => fields,
        other => panic!("{other:?}"),
    }
}

fn named(fields: &[FormField], name: &str) -> FormField {
    fields
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no field {name}"))
        .clone()
}

fn set(
    e: &mut partridge_engine::Engine,
    doc: u32,
    field: &FormField,
    value: FormValue,
) -> Vec<FormField> {
    match ask(
        e,
        RequestBody::SetFormField {
            doc,
            page: 0,
            index: field.index,
            name: field.name.clone(),
            value,
        },
    ) {
        ResponseBody::FormFields { fields } => fields,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_form_that_is_filled_in_and_annotated_keeps_both_through_edits_removals_and_a_save() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::forms_pdf(), None).unwrap();
    // The window asks about the form first, as it does when a document is opened.
    assert!(matches!(
        ask(e, RequestBody::FormInfo { doc }),
        ResponseBody::FormInfo { .. }
    ));

    // The fields of the form are not annotations of the reader's: the list holds none of them.
    assert!(list(e, doc, 0).is_empty(), "{:?}", list(e, doc, 0));

    // An annotation, then a field, then a second annotation, then another field.
    let note = annot(ask(
        e,
        RequestBody::AddNote {
            doc,
            page: 0,
            x: 500.0,
            y: 700.0,
            color: YELLOW,
            text: "A note on a form".into(),
        },
    ));
    let before = fields(e, doc);
    let name = named(&before, "fullname");
    let after = set(e, doc, &name, FormValue::Text("Ada Lovelace".into()));
    assert_eq!(named(&after, "fullname").value, "Ada Lovelace");
    let stamp = annot(ask(
        e,
        RequestBody::AddStamp {
            doc,
            page: 0,
            x: 400.0,
            y: 100.0,
            stamp: StampKind::Draft,
        },
    ));
    assert_ne!(note.id, stamp.id);
    let agree = named(&after, "agree");
    let after = set(e, doc, &agree, FormValue::Checked(true));
    assert!(named(&after, "agree").has(partridge_protocol::field_flag::CHECKED));

    // Neither kind of change disturbed the other.
    let annots = list(e, doc, 0);
    assert_eq!(annots.len(), 2, "{annots:?}");
    assert!(annots
        .iter()
        .any(|a| a.id == note.id && a.kind == AnnotKind::Note));
    assert!(annots
        .iter()
        .any(|a| a.id == stamp.id && a.kind == AnnotKind::Stamp));
    let now = fields(e, doc);
    assert_eq!(named(&now, "fullname").value, "Ada Lovelace");
    assert_eq!(now.len(), before.len());

    // Taking the two kinds back, in an order that mixes them (the window's Undo): the field first,
    // then the note, then the check box; and putting them back (Redo).
    let cleared = set(e, doc, &name, FormValue::Text(String::new()));
    assert_eq!(named(&cleared, "fullname").value, "");
    assert!(matches!(
        ask(
            e,
            RequestBody::RemoveAnnot {
                doc,
                page: 0,
                id: note.id
            }
        ),
        ResponseBody::AnnotRemoved
    ));
    let unchecked = set(e, doc, &agree, FormValue::Checked(false));
    assert!(!named(&unchecked, "agree").has(partridge_protocol::field_flag::CHECKED));
    assert_eq!(list(e, doc, 0).len(), 1);
    let back = annot(ask(
        e,
        RequestBody::RestoreAnnot {
            doc,
            page: 0,
            id: note.id,
        },
    ));
    assert_eq!(back.id, note.id);
    let again = set(e, doc, &name, FormValue::Text("Ada Lovelace".into()));
    assert_eq!(named(&again, "fullname").value, "Ada Lovelace");
    let again = set(e, doc, &agree, FormValue::Checked(true));
    assert!(named(&again, "agree").has(partridge_protocol::field_flag::CHECKED));
    assert_eq!(list(e, doc, 0).len(), 2);

    // The save holds both, and so does the file read again.
    let saved = save_all(e, doc);
    assert!(contains(&saved, b"/Text"));
    assert!(contains(&saved, b"/Stamp"));
    assert!(contains(&saved, b"Ada Lovelace"));
    let second = open(e, &saved, None).unwrap();
    let kinds: Vec<AnnotKind> = list(e, second, 0).iter().map(|a| a.kind).collect();
    assert!(
        kinds.contains(&AnnotKind::Note) && kinds.contains(&AnnotKind::Stamp),
        "{kinds:?}"
    );
    assert_eq!(list(e, second, 0).len(), 2);
    let read_back = fields(e, second);
    assert_eq!(named(&read_back, "fullname").value, "Ada Lovelace");
    assert!(named(&read_back, "agree").has(partridge_protocol::field_flag::CHECKED));
    // Another change to the form of the file read again leaves its annotations alone.
    let country = named(&read_back, "country");
    let changed = set(e, second, &country, FormValue::Choice(vec![1]));
    assert!(changed.iter().any(|f| f.name == "country"));
    assert_eq!(list(e, second, 0).len(), 2);
}

#[test]
fn an_annotation_made_after_the_form_was_listed_does_not_change_what_the_form_lists() {
    // PDFium makes the form environment on the document that the annotation code works on too.
    // Adding, changing and removing annotations on the page a form sits on must leave the list of
    // fields, and the set of one of them, as they were.
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::forms_pdf(), None).unwrap();
    let _ = ask(e, RequestBody::FormInfo { doc });
    let listed = fields(e, doc);
    let (start, len) = find(e, doc, 0, "Name");
    for _ in 0..3 {
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
    }
    let after = fields(e, doc);
    assert_eq!(
        after.iter().map(|f| (&f.name, f.index)).collect::<Vec<_>>(),
        listed
            .iter()
            .map(|f| (&f.name, f.index))
            .collect::<Vec<_>>()
    );
    let done = set(
        e,
        doc,
        &named(&after, "fullname"),
        FormValue::Text("Grace".into()),
    );
    assert_eq!(named(&done, "fullname").value, "Grace");
    // A field that is not there any more, or is not the one named, is still refused.
    match ask(
        e,
        RequestBody::SetFormField {
            doc,
            page: 0,
            index: named(&after, "fullname").index,
            name: "agree".into(),
            value: FormValue::Text("x".into()),
        },
    ) {
        ResponseBody::Error {
            code: ErrorCode::FormFieldRefused,
            ..
        } => {}
        other => panic!("{other:?}"),
    }
}
