//! Filling in forms in the engine: what the fields look like to the reader, what a change does to
//! them, and what is in the bytes that a save writes.
//!
//! These tests run the engine in this process, without the sandbox, and read the saved bytes back
//! with PDFium itself (a second document in the same engine). `tests/forms_process.rs` runs the real
//! engine process with the sandbox, and `scripts/check-forms.sh` hands saved files to tools that are
//! not PDFium. PDFium wants one user at a time, so the tests take turns on one engine.

use std::fs::File;
use std::sync::{Mutex, MutexGuard, OnceLock};

use partridge_engine::Engine;
use partridge_protocol::{
    field_flag, ErrorCode, FormField, FormFieldKind, FormKind, FormValue, Request, RequestBody,
    Response, ResponseBody, MAX_FORM_ANSWER_BYTES, MAX_FORM_FIELDS_PER_PAGE, MAX_FORM_NAME,
    MAX_FORM_OPTIONS, MAX_FORM_OPTION_LABEL, MAX_FORM_TOOLTIP, MAX_FORM_VALUE,
};
use partridge_testkit as kit;
use pdfium_render::prelude::*;

struct Shared(Engine);
unsafe impl Send for Shared {}

fn engine() -> MutexGuard<'static, Shared> {
    static ENGINE: OnceLock<Mutex<Shared>> = OnceLock::new();
    ENGINE
        .get_or_init(|| Mutex::new(Shared(Engine::new(&kit::pdfium_dir()))))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn ask(engine: &mut Engine, body: RequestBody) -> ResponseBody {
    engine.handle(Request { id: 1, body }).body
}

/// Opens `bytes`; the file stays on disk until the returned guard is dropped.
fn open(engine: &mut Engine, bytes: &[u8]) -> (u32, kit::TempFile) {
    let file = kit::TempFile::new("forms.pdf", bytes);
    engine.set_document(File::open(file.path()).unwrap());
    match ask(engine, RequestBody::Open { password: None }) {
        ResponseBody::Opened { doc, .. } => (doc, file),
        other => panic!("{other:?}"),
    }
}

fn fields(engine: &mut Engine, doc: u32, page: u32) -> Vec<FormField> {
    match ask(engine, RequestBody::ListFormFields { doc, page }) {
        ResponseBody::FormFields { fields } => fields,
        other => panic!("{other:?}"),
    }
}

fn named<'a>(fields: &'a [FormField], name: &str) -> Vec<&'a FormField> {
    fields.iter().filter(|f| f.name == name).collect()
}

fn one(fields: &[FormField], name: &str) -> FormField {
    let found = named(fields, name);
    assert_eq!(found.len(), 1, "{name}");
    found[0].clone()
}

fn set_on(
    engine: &mut Engine,
    doc: u32,
    page: u32,
    field: &FormField,
    value: FormValue,
) -> ResponseBody {
    ask(
        engine,
        RequestBody::SetFormField {
            doc,
            page,
            index: field.index,
            name: field.name.clone(),
            value,
        },
    )
}

fn set(engine: &mut Engine, doc: u32, field: &FormField, value: FormValue) -> ResponseBody {
    set_on(engine, doc, 0, field, value)
}

/// Sets a field and returns the fields of the page after it; fails the test on an error.
fn fill(engine: &mut Engine, doc: u32, field: &FormField, value: FormValue) -> Vec<FormField> {
    match set(engine, doc, field, value) {
        ResponseBody::FormFields { fields } => fields,
        other => panic!("{other:?}"),
    }
}

fn refusal(body: ResponseBody) -> ErrorCode {
    match body {
        ResponseBody::Error { code, .. } => code,
        other => panic!("expected an error, got {other:?}"),
    }
}

fn is_on(field: &FormField) -> bool {
    field.has(field_flag::CHECKED)
}

fn selected(field: &FormField) -> Vec<&str> {
    field
        .options
        .iter()
        .filter(|o| o.selected)
        .map(|o| o.label.as_str())
        .collect()
}

fn save_all(engine: &mut Engine, doc: u32) -> Vec<u8> {
    let ResponseBody::SaveReady { size } = ask(engine, RequestBody::SaveBegin { doc }) else {
        panic!("no SaveReady");
    };
    let mut out = Vec::new();
    while (out.len() as u32) < size {
        let len = (1 << 20).min(size - out.len() as u32);
        match ask(
            engine,
            RequestBody::SaveChunk {
                doc,
                offset: out.len() as u32,
                len,
            },
        ) {
            ResponseBody::SaveData { data, .. } => out.extend_from_slice(&data),
            other => panic!("{other:?}"),
        }
    }
    assert!(matches!(
        ask(engine, RequestBody::SaveEnd { doc }),
        ResponseBody::SaveEnded
    ));
    out
}

fn fingerprint(engine: &mut Engine, doc: u32) -> (u32, u32, u64) {
    match ask(engine, RequestBody::Fingerprint { doc }) {
        ResponseBody::Fingerprint {
            pages,
            failed,
            digest,
        } => (pages, failed, digest),
        other => panic!("{other:?}"),
    }
}

fn info(engine: &mut Engine, doc: u32) -> (FormKind, bool) {
    match ask(engine, RequestBody::FormInfo { doc }) {
        ResponseBody::FormInfo { kind, has_actions } => (kind, has_actions),
        other => panic!("{other:?}"),
    }
}

/// The bytes of a file without the spaces and line breaks, so that a test looks for `/AS/Yes`
/// whichever way PDFium spaces its dictionaries.
fn squeezed(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// The content of the normal appearance stream of the field `name` on the first page, as the
/// document in the engine has it.
fn appearance(engine: &mut Engine, doc: u32, name: &str, index: usize) -> Option<String> {
    engine
        .edit_for_tests(doc, |document| {
            let page = document.pages().get(0).ok()?;
            let mut seen = 0;
            for annotation in page.annotations().iter() {
                let Some(field) = annotation.as_form_field() else {
                    continue;
                };
                if field.name().as_deref() != Some(name) {
                    continue;
                }
                if seen == index {
                    return field.appearance_mode_value(PdfAppearanceMode::Normal);
                }
                seen += 1;
            }
            None
        })
        .flatten()
}

// ---- what the reader sees of a form -------------------------------------------------------------

#[test]
fn a_form_is_listed_with_its_names_tooltips_flags_and_places() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_pdf());
    assert_eq!(info(engine, doc), (FormKind::Acro, false));
    let list = fields(engine, doc, 0);
    let names: Vec<&str> = list.iter().map(|f| f.name.as_str()).collect();
    // In the order of the widgets on the page; the push button is not a field the reader fills in.
    assert_eq!(
        names,
        [
            "fullname", "notes", "locked", "agree", "size", "size", "size", "mode", "mode",
            "country", "city", "colours", "toppings"
        ]
    );
    let indexes: Vec<u32> = list.iter().map(|f| f.index).collect();
    assert_eq!(indexes, (0..13).collect::<Vec<u32>>());

    let full = one(&list, "fullname");
    assert_eq!(full.kind, FormFieldKind::Text);
    assert_eq!(full.tooltip, "Full name");
    assert!(full.has(field_flag::REQUIRED) && !full.has(field_flag::READ_ONLY));
    assert!(!full.has(field_flag::MULTILINE));
    // 150,700 to 400,724 on a 612 by 792 page: from the top left of the page it is 68 to 92.
    assert_eq!(full.rect, [150.0, 68.0, 400.0, 92.0]);
    assert_eq!(full.font_size, 12.0);
    assert_eq!(full.turns, 0);
    assert_eq!(full.value, "");

    let notes = one(&list, "notes");
    assert!(notes.has(field_flag::MULTILINE));
    assert_eq!(notes.font_size, 10.0);

    let locked = one(&list, "locked");
    assert!(locked.has(field_flag::READ_ONLY));
    assert_eq!(locked.value, "Fixed");

    let agree = one(&list, "agree");
    assert_eq!(agree.kind, FormFieldKind::Checkbox);
    assert!(!is_on(&agree));
    assert_eq!(agree.tooltip, "I agree to the terms");

    let size = named(&list, "size");
    assert!(size.iter().all(|f| f.kind == FormFieldKind::Radio));
    assert!(size.iter().all(|f| f.has(field_flag::NO_TOGGLE_OFF)));
    assert!(named(&list, "mode")
        .iter()
        .all(|f| !f.has(field_flag::NO_TOGGLE_OFF)));

    let country = one(&list, "country");
    assert_eq!(country.kind, FormFieldKind::Combo);
    assert!(!country.has(field_flag::EDITABLE));
    let labels: Vec<&str> = country.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(labels, ["Norway", "Sweden", "Denmark"]);
    assert!(one(&list, "city").has(field_flag::EDITABLE));
    assert_eq!(one(&list, "colours").kind, FormFieldKind::List);
    assert!(!one(&list, "colours").has(field_flag::MULTISELECT));
    assert!(one(&list, "toppings").has(field_flag::MULTISELECT));
}

#[test]
fn a_nested_field_is_listed_with_its_full_name() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_nested_pdf());
    let list = fields(engine, doc, 0);
    assert_eq!(list[0].name, "person.first");
    assert!(named(&list, "person.last").len() == 1);
    // A change goes to the field the name says.
    let after = fill(
        engine,
        doc,
        &one(&list, "person.last"),
        FormValue::Text("Lovelace".into()),
    );
    assert_eq!(one(&after, "person.last").value, "Lovelace");
    assert_eq!(one(&after, "person.first").value, "");
}

// ---- filling in -------------------------------------------------------------------------------

#[test]
fn each_kind_of_field_takes_a_value_and_gives_it_back() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_pdf());
    let list = fields(engine, doc, 0);

    let list = fill(
        engine,
        doc,
        &one(&list, "fullname"),
        FormValue::Text("Ada Lovelace".into()),
    );
    assert_eq!(one(&list, "fullname").value, "Ada Lovelace");
    // Letters beyond ASCII, a bracket and a backslash survive.
    let list = fill(
        engine,
        doc,
        &one(&list, "fullname"),
        FormValue::Text("Bj\u{f8}rn \u{c5}se (b) a\\b".into()),
    );
    assert_eq!(one(&list, "fullname").value, "Bj\u{f8}rn \u{c5}se (b) a\\b");
    // A value can be taken away again.
    let list = fill(
        engine,
        doc,
        &one(&list, "fullname"),
        FormValue::Text(String::new()),
    );
    assert_eq!(one(&list, "fullname").value, "");

    let list = fill(
        engine,
        doc,
        &one(&list, "notes"),
        FormValue::Text("line one\nline two\n\nline four".into()),
    );
    assert_eq!(one(&list, "notes").value, "line one\nline two\n\nline four");

    let list = fill(engine, doc, &one(&list, "agree"), FormValue::Checked(true));
    assert!(is_on(&one(&list, "agree")));
    let list = fill(engine, doc, &one(&list, "agree"), FormValue::Checked(false));
    assert!(!is_on(&one(&list, "agree")));

    let list = fill(
        engine,
        doc,
        &one(&list, "country"),
        FormValue::Choice(vec![1]),
    );
    assert_eq!(selected(&one(&list, "country")), ["Sweden"]);
    let list = fill(
        engine,
        doc,
        &one(&list, "country"),
        FormValue::Choice(vec![2]),
    );
    assert_eq!(selected(&one(&list, "country")), ["Denmark"]);
    // A combo box cannot go back to showing nothing (see the next test).

    let list = fill(
        engine,
        doc,
        &one(&list, "city"),
        FormValue::Text("Troms\u{f8}".into()),
    );
    assert_eq!(one(&list, "city").value, "Troms\u{f8}");

    let list = fill(
        engine,
        doc,
        &one(&list, "colours"),
        FormValue::Choice(vec![2]),
    );
    assert_eq!(selected(&one(&list, "colours")), ["Blue"]);

    let list = fill(
        engine,
        doc,
        &one(&list, "toppings"),
        FormValue::Choice(vec![0, 2]),
    );
    assert_eq!(selected(&one(&list, "toppings")), ["Ham", "Olives"]);
    let list = fill(
        engine,
        doc,
        &one(&list, "toppings"),
        FormValue::Choice(vec![3]),
    );
    assert_eq!(selected(&one(&list, "toppings")), ["Onions"]);

    // The fields that were not touched are as they were.
    assert_eq!(one(&list, "locked").value, "Fixed");
    assert!(named(&list, "size").iter().all(|f| !is_on(f)));
}

#[test]
fn a_radio_group_has_one_choice_and_the_reader_cannot_clear_it() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_pdf());
    let list = fields(engine, doc, 0);
    let on_of = |list: &[FormField], name: &str| -> Vec<u32> {
        named(list, name)
            .iter()
            .filter(|f| is_on(f))
            .map(|f| f.index)
            .collect()
    };
    let first = named(&list, "size")[0].clone();
    let list = fill(engine, doc, &first, FormValue::Checked(true));
    assert_eq!(on_of(&list, "size"), [first.index]);
    let third = named(&list, "size")[2].clone();
    let list = fill(engine, doc, &third, FormValue::Checked(true));
    // Choosing another turns the first off.
    assert_eq!(on_of(&list, "size"), [third.index]);
    // The other group is not touched.
    assert!(on_of(&list, "mode").is_empty());
    // Back to the first: the way an undo goes when the choice before was the first.
    let first = named(&list, "size")[0].clone();
    let list = fill(engine, doc, &first, FormValue::Checked(true));
    assert_eq!(on_of(&list, "size"), [first.index]);

    // Neither group can be cleared by pressing the button that is on. PDFium offers no way to
    // clear a group, so an undo of the very first choice cannot be carried out; the window says so.
    let first = named(&list, "size")[0].clone();
    assert_eq!(
        refusal(set(engine, doc, &first, FormValue::Checked(false))),
        ErrorCode::FormFieldRefused
    );
    let a = named(&list, "mode")[0].clone();
    let list = fill(engine, doc, &a, FormValue::Checked(true));
    let a = named(&list, "mode")[0].clone();
    assert!(is_on(&a));
    assert_eq!(
        refusal(set(engine, doc, &a, FormValue::Checked(false))),
        ErrorCode::FormFieldRefused
    );
    assert!(is_on(&named(&fields(engine, doc, 0), "mode")[0].clone()));
}

#[test]
fn a_change_that_is_taken_back_leaves_the_field_as_it_was() {
    // What undo does in the window: it sets the field to what it was.
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_pdf());
    let before = fields(engine, doc, 0);
    let mut list = before.clone();
    for (name, value, back) in [
        (
            "fullname",
            FormValue::Text("x".into()),
            FormValue::Text(String::new()),
        ),
        (
            "notes",
            FormValue::Text("a\nb".into()),
            FormValue::Text(String::new()),
        ),
        ("agree", FormValue::Checked(true), FormValue::Checked(false)),
        (
            "colours",
            FormValue::Choice(vec![3]),
            FormValue::Choice(vec![]),
        ),
        (
            "toppings",
            FormValue::Choice(vec![1, 2]),
            FormValue::Choice(vec![]),
        ),
    ] {
        list = fill(engine, doc, &one(&list, name), value);
        assert_ne!(one(&list, name), one(&before, name), "{name} changed");
        list = fill(engine, doc, &one(&list, name), back);
        assert_eq!(one(&list, name), one(&before, name), "{name} is back");
    }
    assert_eq!(list, before);
}

// ---- what is refused ------------------------------------------------------------------------

#[test]
fn a_request_that_does_not_fit_the_field_is_refused_and_changes_nothing() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_pdf());
    let before = fields(engine, doc, 0);
    let refused = ErrorCode::FormFieldRefused;
    let text = |s: &str| FormValue::Text(s.into());
    let cases: Vec<(FormField, FormValue)> = vec![
        // A read-only field.
        (one(&before, "locked"), text("changed")),
        // The wrong kind of value for the kind of field.
        (one(&before, "fullname"), FormValue::Checked(true)),
        (one(&before, "fullname"), FormValue::Choice(vec![0])),
        (one(&before, "agree"), text("yes")),
        (one(&before, "agree"), FormValue::Choice(vec![0])),
        (one(&before, "country"), text("Finland")),
        (one(&before, "country"), FormValue::Checked(true)),
        (one(&before, "colours"), text("Red")),
        // A choice that is not in the list, and two choices for a list that takes one.
        (one(&before, "country"), FormValue::Choice(vec![3])),
        (one(&before, "toppings"), FormValue::Choice(vec![4])),
        (one(&before, "toppings"), FormValue::Choice(vec![u32::MAX])),
        (one(&before, "colours"), FormValue::Choice(vec![0, 1])),
        (one(&before, "country"), FormValue::Choice(vec![0, 1])),
    ];
    for (field, value) in cases {
        assert_eq!(
            refusal(set(engine, doc, &field, value.clone())),
            refused,
            "{} {value:?}",
            field.name
        );
    }
    // A name that is not the one at that place, and places that do not exist.
    let mut wrong = one(&before, "fullname");
    wrong.name = "notes".into();
    assert_eq!(refusal(set(engine, doc, &wrong, text("x"))), refused);
    for index in [14u32, 99, u32::MAX] {
        let mut gone = one(&before, "fullname");
        gone.index = index;
        assert_eq!(
            refusal(set(engine, doc, &gone, text("x"))),
            refused,
            "{index}"
        );
    }
    // The push button is a widget (the 14th), but not a field the reader fills in.
    let mut button = one(&before, "fullname");
    button.index = 13;
    button.name = "reset".into();
    assert_eq!(refusal(set(engine, doc, &button, text("x"))), refused);
    // A page and a document that are not there.
    assert_eq!(
        refusal(set_on(engine, doc, 5, &one(&before, "fullname"), text("x"))),
        ErrorCode::PageOutOfRange
    );
    assert_eq!(
        refusal(ask(
            engine,
            RequestBody::ListFormFields {
                doc: doc + 1000,
                page: 0
            }
        )),
        ErrorCode::UnknownDocument
    );
    assert_eq!(fields(engine, doc, 0), before);
}

#[test]
fn a_form_that_xfa_shows_is_not_filled_in_and_a_document_without_a_form_has_none() {
    let mut guard = engine();
    let engine = &mut guard.0;
    for bytes in [kit::xfa_pdf(), kit::xfa_full_pdf()] {
        let (doc, _file) = open(engine, &bytes);
        assert_eq!(info(engine, doc).0, FormKind::Xfa);
        assert_eq!(
            refusal(ask(engine, RequestBody::ListFormFields { doc, page: 0 })),
            ErrorCode::FormUnsupported
        );
        let some = FormField {
            index: 0,
            kind: FormFieldKind::Text,
            flags: 0,
            turns: 0,
            rect: [0.0; 4],
            font_size: 0.0,
            name: "fullname".into(),
            tooltip: String::new(),
            value: String::new(),
            max_len: 0,
            options: vec![],
        };
        assert_eq!(
            refusal(set(engine, doc, &some, FormValue::Text("x".into()))),
            ErrorCode::FormUnsupported
        );
        // The pages still draw: the form is not broken, it is only not filled in here.
        assert!(matches!(
            ask(
                engine,
                RequestBody::Render {
                    doc,
                    page: 0,
                    width: 100,
                    height: 130
                }
            ),
            ResponseBody::Rendered { .. }
        ));
    }
    let (doc, _file) = open(engine, &kit::simple_pdf(1));
    assert_eq!(info(engine, doc), (FormKind::None, false));
    assert_eq!(
        refusal(ask(engine, RequestBody::ListFormFields { doc, page: 0 })),
        ErrorCode::FormUnsupported
    );
}

#[test]
fn scripts_and_actions_are_noticed_and_never_run() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_script_pdf());
    assert_eq!(info(engine, doc), (FormKind::Acro, true));
    let list = fields(engine, doc, 0);
    assert!(one(&list, "fullname").has(field_flag::ACTION));
    assert!(!one(&list, "notes").has(field_flag::ACTION));
    // The calculation on the first field would give it the value 2 as soon as the page loaded, or
    // once any field changed. Neither happens: no script runs.
    assert_eq!(one(&list, "fullname").value, "");
    let list = fill(
        engine,
        doc,
        &one(&list, "notes"),
        FormValue::Text("hello".into()),
    );
    assert_eq!(one(&list, "fullname").value, "");
    // The field with the format and the keystroke scripts takes what is typed, as typed.
    let list = fill(
        engine,
        doc,
        &one(&list, "fullname"),
        FormValue::Text("abc".into()),
    );
    assert_eq!(one(&list, "fullname").value, "abc");
    // The push button with its action is not listed, and nothing presses it.
    assert!(named(&list, "reset").is_empty());
    // The plain form has no action.
    let (plain, _file2) = open(engine, &kit::forms_pdf());
    assert_eq!(info(engine, plain), (FormKind::Acro, false));
    assert!(fields(engine, plain, 0)
        .iter()
        .all(|f| !f.has(field_flag::ACTION)));
}

#[test]
fn the_fields_of_a_turned_page_are_where_they_are_drawn() {
    let mut guard = engine();
    let engine = &mut guard.0;
    // The field `fullname` is [150, 700, 400, 724] on the page of 612 by 792 points. Worked out by
    // hand: a quarter turn clockwise puts the bottom left corner at the top left, so a point (x, y)
    // is drawn at (y, x); a half turn at (612 - x, y); three quarters at (792 - y, 612 - x).
    for (degrees, turns, expected) in [
        (90, 1, [700.0, 150.0, 724.0, 400.0]),
        (180, 2, [212.0, 700.0, 462.0, 724.0]),
        (270, 3, [68.0, 212.0, 92.0, 462.0]),
    ] {
        let (doc, _file) = open(engine, &kit::forms_rotated_pdf(degrees));
        let list = fields(engine, doc, 0);
        let full = one(&list, "fullname");
        assert_eq!(full.rect, expected, "{degrees} degrees");
        assert_eq!(full.turns, turns);
        assert!(list.iter().all(|f| f.turns == turns));
        // A field on a turned page is filled in like any other.
        let list = fill(engine, doc, &full, FormValue::Text("turned".into()));
        assert_eq!(one(&list, "fullname").value, "turned");
        let list = fill(engine, doc, &one(&list, "agree"), FormValue::Checked(true));
        assert!(is_on(&one(&list, "agree")));
        let size = named(&list, "size")[1].clone();
        let list = fill(engine, doc, &size, FormValue::Checked(true));
        assert!(is_on(named(&list, "size")[1]));
    }
}

// ---- hostile documents ------------------------------------------------------------------------

#[test]
fn what_a_document_says_about_its_fields_is_cut_to_the_caps() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_hostile_pdf());
    let response = engine.handle(Request {
        id: 7,
        body: RequestBody::ListFormFields { doc, page: 0 },
    });
    // The answer fits the limit the app holds the engine to, and decodes.
    let bytes = response.encode();
    assert!(
        bytes.len() <= MAX_FORM_ANSWER_BYTES,
        "{} bytes",
        bytes.len()
    );
    let decoded = Response::decode(bytes).expect("the answer decodes");
    let ResponseBody::FormFields { fields: list } = decoded.body else {
        panic!("{decoded:?}");
    };
    assert_eq!(list.len(), 3);

    let text = &list[0];
    assert_eq!(text.name.len(), MAX_FORM_NAME);
    assert!(text.tooltip.len() <= MAX_FORM_TOOLTIP);
    assert!(text.value.len() <= MAX_FORM_VALUE);
    // The value in the file is longer than the cap, so the field is marked and left alone.
    assert!(text.has(field_flag::VALUE_CUT));
    assert_eq!(
        refusal(set(engine, doc, text, FormValue::Text("replace".into()))),
        ErrorCode::FormFieldRefused
    );

    let many = &list[1];
    assert_eq!(many.options.len(), MAX_FORM_OPTIONS);
    assert!(many
        .options
        .iter()
        .all(|o| o.label.len() <= MAX_FORM_OPTION_LABEL));
    // Choices the window cannot all show: nothing is changed in such a list.
    assert!(many.has(field_flag::READ_ONLY));
    assert_eq!(
        refusal(set(engine, doc, many, FormValue::Choice(vec![0]))),
        ErrorCode::FormFieldRefused
    );

    // Names and values in other scripts and with characters outside the basic plane come through.
    let wide = &list[2];
    assert_eq!(wide.name, "\u{e5}\u{f8}\u{e6}");
    assert_eq!(wide.tooltip, "\u{263a}");
    assert_eq!(wide.value, "\u{5d0}\u{5d1}\u{5d2}\u{1f600}");
}

#[test]
fn a_page_lists_at_most_the_most_fields() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let count = MAX_FORM_FIELDS_PER_PAGE + 100;
    let (doc, _file) = open(engine, &kit::forms_many_pdf(count));
    let list = fields(engine, doc, 0);
    assert_eq!(list.len(), MAX_FORM_FIELDS_PER_PAGE);
    assert_eq!(list[0].name, "f0");
    assert_eq!(
        list[MAX_FORM_FIELDS_PER_PAGE - 1].index as usize,
        MAX_FORM_FIELDS_PER_PAGE - 1
    );
}

#[test]
fn a_closed_document_has_no_form_left() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_pdf());
    assert!(!fields(engine, doc, 0).is_empty());
    assert!(matches!(
        ask(engine, RequestBody::Close { doc }),
        ResponseBody::Closed
    ));
    assert_eq!(
        refusal(ask(engine, RequestBody::ListFormFields { doc, page: 0 })),
        ErrorCode::UnknownDocument
    );
}

// ---- saving -----------------------------------------------------------------------------------

/// Fills in every field of `forms_pdf` that the checks below look at.
fn fill_everything(engine: &mut Engine, doc: u32) {
    let list = fields(engine, doc, 0);
    let list = fill(
        engine,
        doc,
        &one(&list, "fullname"),
        FormValue::Text("Ada Lovelace".into()),
    );
    let list = fill(
        engine,
        doc,
        &one(&list, "notes"),
        FormValue::Text("First line\nSecond line".into()),
    );
    let list = fill(engine, doc, &one(&list, "agree"), FormValue::Checked(true));
    let medium = named(&list, "size")[1].clone();
    let list = fill(engine, doc, &medium, FormValue::Checked(true));
    let list = fill(
        engine,
        doc,
        &one(&list, "country"),
        FormValue::Choice(vec![1]),
    );
    let list = fill(
        engine,
        doc,
        &one(&list, "city"),
        FormValue::Text("Troms\u{f8}".into()),
    );
    let list = fill(
        engine,
        doc,
        &one(&list, "colours"),
        FormValue::Choice(vec![2]),
    );
    fill(
        engine,
        doc,
        &one(&list, "toppings"),
        FormValue::Choice(vec![0, 2]),
    );
}

fn check_everything(list: &[FormField]) {
    assert_eq!(one(list, "fullname").value, "Ada Lovelace");
    assert_eq!(one(list, "notes").value, "First line\nSecond line");
    assert!(is_on(&one(list, "agree")));
    let size = named(list, "size");
    assert_eq!(
        size.iter().map(|f| is_on(f)).collect::<Vec<_>>(),
        [false, true, false]
    );
    assert_eq!(selected(&one(list, "country")), ["Sweden"]);
    assert_eq!(one(list, "city").value, "Troms\u{f8}");
    assert_eq!(selected(&one(list, "colours")), ["Blue"]);
    assert_eq!(selected(&one(list, "toppings")), ["Ham", "Olives"]);
    assert_eq!(one(list, "locked").value, "Fixed");
    assert!(!named(list, "mode").iter().any(|f| is_on(f)));
}

#[test]
fn a_filled_form_is_saved_and_opens_again_with_every_value() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_pdf());
    fill_everything(engine, doc);
    let live = fingerprint(engine, doc);
    let bytes = save_all(engine, doc);

    // The values are in the file in the forms that PDF readers look for.
    let flat = squeezed(&bytes);
    for expected in [
        "/V(AdaLovelace)",
        "/AS/Yes",
        "/V/Yes",
        // The radio group: the value on the group, the state on the button.
        "/V/M",
        "/AS/M",
        "/V(Sweden)",
        "/I[1]",
        "/V[(Ham)(Olives)]",
        "/I[02]",
    ] {
        assert!(flat.contains(expected), "{expected} is not in the file");
    }

    let (second, _file2) = open(engine, &bytes);
    check_everything(&fields(engine, second, 0));
    // The page looks the same to the checks of the save path.
    assert_eq!(fingerprint(engine, second), live);
}

#[test]
fn a_saved_form_carries_an_appearance_that_shows_the_value() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_pdf());
    fill_everything(engine, doc);
    let bytes = save_all(engine, doc);
    let (second, _file2) = open(engine, &bytes);

    // What another reader draws is the appearance stream of the widget, not the value. Read the
    // streams of the saved file: the text must be in them.
    let mut drawn = |name: &str| appearance(engine, second, name, 0).unwrap_or_default();
    let full = drawn("fullname");
    assert!(full.contains("Ada Lovelace"), "{full}");
    let notes = drawn("notes");
    assert!(
        notes.contains("First line") && notes.contains("Second line"),
        "{notes}"
    );
    assert!(drawn("country").contains("Sweden"));
    assert!(drawn("city").contains("Troms"));
    let colours = drawn("colours");
    assert!(colours.contains("Red") && colours.contains("Blue"));
    // Check boxes and radio buttons draw by the state they are in: the file names the state of
    // each button, and the stream of that state is the one that has the mark.
    let flat = squeezed(&bytes);
    assert!(flat.contains("/AS/Yes") && flat.contains("/AS/M"));
    assert_eq!(
        flat.matches("/AS/Off").count(),
        4,
        "the other buttons stay off"
    );
}

#[test]
fn a_form_can_be_saved_filled_in_again_and_saved_again() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_pdf());
    let list = fields(engine, doc, 0);
    fill(
        engine,
        doc,
        &one(&list, "fullname"),
        FormValue::Text("First".into()),
    );
    let first = save_all(engine, doc);
    let list = fields(engine, doc, 0);
    fill(
        engine,
        doc,
        &one(&list, "fullname"),
        FormValue::Text("Second".into()),
    );
    let second = save_all(engine, doc);
    let (a, _fa) = open(engine, &first);
    let (b, _fb) = open(engine, &second);
    assert_eq!(one(&fields(engine, a, 0), "fullname").value, "First");
    assert_eq!(one(&fields(engine, b, 0), "fullname").value, "Second");
}

#[test]
fn filling_in_a_signed_form_leaves_the_words_of_the_signature_for_the_warning() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_signed_pdf());
    let ResponseBody::DocInfo {
        signatures,
        maybe_signed,
        ..
    } = ask(engine, RequestBody::DocInfo { doc })
    else {
        panic!("no DocInfo");
    };
    assert!(signatures >= 1 && maybe_signed);
    let list = fields(engine, doc, 0);
    // The signature field is not one the reader fills in.
    assert!(named(&list, "signature").is_empty());
    fill(
        engine,
        doc,
        &one(&list, "fullname"),
        FormValue::Text("Signed by".into()),
    );
    let bytes = save_all(engine, doc);
    // The save is a rewrite: the signature is still named in the file, so the warning before a save
    // has what it needs; the bytes it covered are not the same any more.
    assert!(squeezed(&bytes).contains("/ByteRange"));
    let (second, _file2) = open(engine, &bytes);
    let ResponseBody::DocInfo { maybe_signed, .. } =
        ask(engine, RequestBody::DocInfo { doc: second })
    else {
        panic!("no DocInfo");
    };
    assert!(maybe_signed);
}

/// When `PARTRIDGE_FORMS_OUT` names a folder, the files that `scripts/check-forms.sh` hands to
/// other tools are written there: a form as it was made and as it is after it was filled in, and a
/// list of what was set. Without the variable the test only fills and saves.
#[test]
fn the_files_for_other_tools_are_written() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let original = kit::forms_pdf();
    let (doc, _file) = open(engine, &original);
    fill_everything(engine, doc);
    let filled = save_all(engine, doc);
    let nested_original = kit::forms_nested_pdf();
    let (nested, _file2) = open(engine, &nested_original);
    let list = fields(engine, nested, 0);
    fill(
        engine,
        nested,
        &one(&list, "person.last"),
        FormValue::Text("Lovelace".into()),
    );
    let nested_filled = save_all(engine, nested);
    assert!(filled.starts_with(b"%PDF-") && nested_filled.starts_with(b"%PDF-"));

    let Some(dir) = std::env::var_os("PARTRIDGE_FORMS_OUT") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("empty.pdf"), &original).unwrap();
    std::fs::write(dir.join("filled.pdf"), &filled).unwrap();
    std::fs::write(dir.join("nested-empty.pdf"), &nested_original).unwrap();
    std::fs::write(dir.join("nested-filled.pdf"), &nested_filled).unwrap();
    // One `name<TAB>value` per line. A list box with more than one choice has one line for each.
    std::fs::write(
        dir.join("expected.txt"),
        "fullname\tAda Lovelace\n\
         notes\tFirst line\n\
         locked\tFixed\n\
         agree\tYes\n\
         size\tM\n\
         mode\tOff\n\
         country\tSweden\n\
         city\tTroms\u{f8}\n\
         colours\tBlue\n\
         toppings\tHam\n\
         toppings\tOlives\n",
    )
    .unwrap();
    std::fs::write(dir.join("nested-expected.txt"), "person.last\tLovelace\n").unwrap();
}

#[test]
fn a_combo_box_cannot_go_back_to_showing_nothing() {
    // PDFium has no way to clear the choice of a combo box that has no box to type in, so an undo
    // of the first choice cannot be carried out. The window says so. A list box can be cleared.
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_pdf());
    let list = fields(engine, doc, 0);
    let list = fill(
        engine,
        doc,
        &one(&list, "country"),
        FormValue::Choice(vec![0]),
    );
    let country = one(&list, "country");
    assert_eq!(
        refusal(set(engine, doc, &country, FormValue::Choice(vec![]))),
        ErrorCode::FormFieldRefused
    );
    let after = fields(engine, doc, 0);
    assert_eq!(selected(&one(&after, "country")), ["Norway"]);
    // The same for a box that takes typing, once it holds something: emptying it would put the
    // first choice back.
    let city = one(&after, "city");
    let after = fill(engine, doc, &city, FormValue::Text("Bergen".into()));
    assert_eq!(
        refusal(set(
            engine,
            doc,
            &one(&after, "city"),
            FormValue::Text(String::new())
        )),
        ErrorCode::FormFieldRefused
    );
    assert_eq!(one(&fields(engine, doc, 0), "city").value, "Bergen");
    // Another choice is fine, and so is the one before it.
    let after = fill(
        engine,
        doc,
        &one(&after, "country"),
        FormValue::Choice(vec![2]),
    );
    let after = fill(
        engine,
        doc,
        &one(&after, "country"),
        FormValue::Choice(vec![0]),
    );
    assert_eq!(selected(&one(&after, "country")), ["Norway"]);
}
