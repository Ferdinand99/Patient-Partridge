//! The form designer through the engine and the real PDFium: fields of every kind are made, listed,
//! filled in by the reader's own fill-in code, saved and read again; names and properties follow
//! their rules; the reload keeps what the engine knows of the annotations and of the form; pages
//! that are turned get fields that read upright; a step back and forward go through the copies the
//! page tools keep; documents the designer cannot work on are refused with a reason.
//!
//! `tests/author_readers.rs` hands the saved files to programs that share no code with PDFium.

mod common;

use std::fs::File;

use common::{ask, engine, save_all, Shot};
use partridge_engine::Engine;
use partridge_protocol::{
    field_flag, AuthorOp, AuthorRequest, AuthorResponse, DesignField, DesignKind, DesignProps,
    DesignWidget, DesignedWidget, ErrorCode, FormField, FormFieldKind, FormValue, PagesRequest,
    PagesResponse, RequestBody, ResponseBody, WidgetRef,
};
use partridge_testkit as kit;

fn open(e: &mut Engine, bytes: &[u8]) -> (u32, kit::TempFile) {
    let file = kit::TempFile::new("author.pdf", bytes);
    e.set_document(File::open(file.path()).unwrap());
    match ask(e, RequestBody::Open { password: None }) {
        ResponseBody::Opened { doc, .. } => (doc, file),
        other => panic!("{other:?}"),
    }
}

fn open_with_pages(e: &mut Engine, bytes: &[u8]) -> (u32, Vec<(f32, f32)>, kit::TempFile) {
    let file = kit::TempFile::new("author.pdf", bytes);
    e.set_document(File::open(file.path()).unwrap());
    match ask(e, RequestBody::Open { password: None }) {
        ResponseBody::Opened { doc, pages } => (
            doc,
            pages.iter().map(|p| (p.width, p.height)).collect(),
            file,
        ),
        other => panic!("{other:?}"),
    }
}

type Failed = (ErrorCode, String);

fn failed(body: ResponseBody) -> Failed {
    match body {
        ResponseBody::Error { code, message } => (code, message),
        other => panic!("expected an error, got {other:?}"),
    }
}

/// Makes the changes; returns the number of the copy to go back to.
fn apply(e: &mut Engine, doc: u32, ops: Vec<AuthorOp>) -> Result<u32, Failed> {
    match ask(e, RequestBody::Author(AuthorRequest::Apply { doc, ops })) {
        ResponseBody::Pages(PagesResponse::Changed { snapshot, .. }) => Ok(snapshot),
        other => Err(failed(other)),
    }
}

fn apply_ok(e: &mut Engine, doc: u32, ops: Vec<AuthorOp>) -> u32 {
    apply(e, doc, ops).unwrap_or_else(|f| panic!("{f:?}"))
}

fn restore(e: &mut Engine, doc: u32, snapshot: u32) -> u32 {
    match ask(
        e,
        RequestBody::Pages(PagesRequest::Restore { doc, snapshot }),
    ) {
        ResponseBody::Pages(PagesResponse::Changed { snapshot, .. }) => snapshot,
        other => panic!("{other:?}"),
    }
}

fn designed(e: &mut Engine, doc: u32, page: u32) -> Vec<DesignedWidget> {
    match ask(e, RequestBody::Author(AuthorRequest::List { doc, page })) {
        ResponseBody::Author(AuthorResponse::Widgets { items }) => items,
        other => panic!("{other:?}"),
    }
}

fn names(e: &mut Engine, doc: u32) -> Vec<String> {
    match ask(e, RequestBody::Author(AuthorRequest::Check { doc })) {
        ResponseBody::Author(AuthorResponse::Ready { names }) => names,
        other => panic!("{other:?}"),
    }
}

fn fields(e: &mut Engine, doc: u32, page: u32) -> Vec<FormField> {
    match ask(e, RequestBody::ListFormFields { doc, page }) {
        ResponseBody::FormFields { fields } => fields,
        other => panic!("{other:?}"),
    }
}

fn named<'a>(list: &'a [FormField], name: &str) -> Vec<&'a FormField> {
    list.iter().filter(|f| f.name == name).collect()
}

fn one(list: &[FormField], name: &str) -> FormField {
    let found = named(list, name);
    assert_eq!(
        found.len(),
        1,
        "{name} in {:?}",
        list.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
    found[0].clone()
}

fn fill(
    e: &mut Engine,
    doc: u32,
    page: u32,
    field: &FormField,
    value: FormValue,
) -> Vec<FormField> {
    match ask(
        e,
        RequestBody::SetFormField {
            doc,
            page,
            index: field.index,
            name: field.name.clone(),
            value,
        },
    ) {
        ResponseBody::FormFields { fields } => fields,
        other => panic!("{other:?}"),
    }
}

fn signatures(e: &mut Engine, doc: u32) -> (u32, bool) {
    match ask(e, RequestBody::DocInfo { doc }) {
        ResponseBody::DocInfo {
            signatures,
            maybe_signed,
            ..
        } => (signatures, maybe_signed),
        other => panic!("{other:?}"),
    }
}

fn widget(page: u32, rect: [f32; 4]) -> DesignWidget {
    DesignWidget {
        page,
        rect,
        export: String::new(),
    }
}

fn text(name: &str, rect: [f32; 4]) -> DesignField {
    DesignField {
        name: name.into(),
        kind: DesignKind::Text,
        props: DesignProps::default(),
        widgets: vec![widget(0, rect)],
    }
}

fn add(field: DesignField) -> AuthorOp {
    AuthorOp::Add(field)
}

fn radio(name: &str, on: &str, y: f32) -> DesignField {
    DesignField {
        name: name.into(),
        kind: DesignKind::Radio,
        props: DesignProps {
            default_value: on.into(),
            ..DesignProps::default()
        },
        widgets: ["S", "M", "L"]
            .iter()
            .enumerate()
            .map(|(i, e)| DesignWidget {
                page: 0,
                rect: [72.0 + 40.0 * i as f32, y, 86.0 + 40.0 * i as f32, y + 14.0],
                export: (*e).into(),
            })
            .collect(),
    }
}

/// One field of each kind, in rows down the first page.
fn everything() -> Vec<AuthorOp> {
    let mut name = text("name", [72.0, 72.0, 272.0, 94.0]);
    name.props = DesignProps {
        tooltip: "Your full name".into(),
        required: true,
        font_size: 12.0,
        max_len: 30,
        default_value: "Ola Nordmann".into(),
        ..DesignProps::default()
    };
    let mut notes = text("notes", [72.0, 104.0, 272.0, 164.0]);
    notes.props.multiline = true;
    let mut secret = text("secret", [72.0, 174.0, 272.0, 196.0]);
    secret.props.password = true;
    let mut code = text("code", [72.0, 206.0, 272.0, 228.0]);
    code.props.comb = true;
    code.props.max_len = 6;
    let agree = DesignField {
        name: "agree".into(),
        kind: DesignKind::Checkbox,
        props: DesignProps {
            export_value: "Ja".into(),
            tooltip: "I agree".into(),
            ..DesignProps::default()
        },
        widgets: vec![widget(0, [72.0, 238.0, 86.0, 252.0])],
    };
    let country = DesignField {
        name: "country".into(),
        kind: DesignKind::Combo,
        props: DesignProps {
            choices: vec![
                "Norge".into(),
                "Sverige".into(),
                "Danmark".into(),
                "\u{c5}land".into(),
            ],
            default_value: "Sverige".into(),
            ..DesignProps::default()
        },
        widgets: vec![widget(0, [72.0, 290.0, 272.0, 312.0])],
    };
    let city = DesignField {
        name: "city".into(),
        kind: DesignKind::Combo,
        props: DesignProps {
            choices: vec!["Oslo".into(), "Troms\u{f8}".into()],
            editable: true,
            ..DesignProps::default()
        },
        widgets: vec![widget(0, [72.0, 322.0, 272.0, 344.0])],
    };
    let colours = DesignField {
        name: "colours".into(),
        kind: DesignKind::List,
        props: DesignProps {
            choices: vec!["R\u{f8}d".into(), "Gr\u{f8}nn".into(), "Bl\u{e5}".into()],
            multiselect: true,
            default_value: "Gr\u{f8}nn".into(),
            ..DesignProps::default()
        },
        widgets: vec![widget(0, [72.0, 354.0, 272.0, 414.0])],
    };
    let sign = DesignField {
        name: "sign".into(),
        kind: DesignKind::Signature,
        props: DesignProps::default(),
        widgets: vec![widget(0, [72.0, 424.0, 272.0, 474.0])],
    };
    vec![
        add(name),
        add(notes),
        add(secret),
        add(code),
        add(agree),
        add(radio("size", "M", 262.0)),
        add(country),
        add(city),
        add(colours),
        add(sign),
    ]
}

fn labels(f: &FormField) -> Vec<&str> {
    f.options.iter().map(|o| o.label.as_str()).collect()
}

fn picked(f: &FormField) -> Vec<&str> {
    f.options
        .iter()
        .filter(|o| o.selected)
        .map(|o| o.label.as_str())
        .collect()
}

/// The fields a reader lists for the first page of `everything()`, checked against what was made.
fn check_everything(list: &[FormField]) {
    let order: Vec<&str> = list.iter().map(|f| f.name.as_str()).collect();
    // A place to sign is not a field the reader fills in.
    assert_eq!(
        order,
        [
            "name", "notes", "secret", "code", "agree", "size", "size", "size", "country", "city",
            "colours"
        ]
    );
    let name = one(list, "name");
    assert_eq!(name.kind, FormFieldKind::Text);
    assert_eq!(name.tooltip, "Your full name");
    assert!(name.has(field_flag::REQUIRED));
    assert_eq!(name.font_size, 12.0);
    assert_eq!(name.max_len, 30);
    assert_eq!(name.value, "Ola Nordmann");
    assert_eq!(name.rect, [72.0, 72.0, 272.0, 94.0]);
    assert!(one(list, "notes").has(field_flag::MULTILINE));
    assert!(one(list, "secret").has(field_flag::PASSWORD));
    let code = one(list, "code");
    assert!(code.has(field_flag::COMB));
    assert_eq!(code.max_len, 6);
    let agree = one(list, "agree");
    assert_eq!(agree.kind, FormFieldKind::Checkbox);
    assert!(!agree.has(field_flag::CHECKED));
    assert_eq!(agree.tooltip, "I agree");
    let size = named(list, "size");
    assert!(size.iter().all(|f| f.kind == FormFieldKind::Radio));
    assert!(size.iter().all(|f| f.has(field_flag::NO_TOGGLE_OFF)));
    let on: Vec<bool> = size.iter().map(|f| f.has(field_flag::CHECKED)).collect();
    assert_eq!(on, [false, true, false], "the button M starts on");
    let country = one(list, "country");
    assert_eq!(country.kind, FormFieldKind::Combo);
    assert!(!country.has(field_flag::EDITABLE));
    assert_eq!(
        labels(&country),
        ["Norge", "Sverige", "Danmark", "\u{c5}land"]
    );
    assert_eq!(picked(&country), ["Sverige"]);
    let city = one(list, "city");
    assert!(city.has(field_flag::EDITABLE));
    assert_eq!(labels(&city), ["Oslo", "Troms\u{f8}"]);
    let colours = one(list, "colours");
    assert_eq!(colours.kind, FormFieldKind::List);
    assert!(colours.has(field_flag::MULTISELECT));
    assert_eq!(labels(&colours), ["R\u{f8}d", "Gr\u{f8}nn", "Bl\u{e5}"]);
    assert_eq!(picked(&colours), ["Gr\u{f8}nn"]);
}

// ---- every kind ---------------------------------------------------------------------------------

#[test]
fn every_kind_is_made_listed_filled_in_and_saved() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open(e, &kit::simple_pdf(1));
    assert!(names(e, doc).is_empty());
    apply_ok(e, doc, everything());
    let list = fields(e, doc, 0);
    check_everything(&list);

    // The reader's own fill-in code takes a value in each, whatever wrote the field.
    let list = fill(
        e,
        doc,
        0,
        &one(&list, "name"),
        FormValue::Text("Bj\u{f8}rn \u{c5}se".into()),
    );
    let list = fill(
        e,
        doc,
        0,
        &one(&list, "notes"),
        FormValue::Text("one\ntwo".into()),
    );
    let list = fill(
        e,
        doc,
        0,
        &one(&list, "secret"),
        FormValue::Text("hunter2".into()),
    );
    let list = fill(
        e,
        doc,
        0,
        &one(&list, "code"),
        FormValue::Text("ABCDEFGH".into()),
    );
    let list = fill(e, doc, 0, &one(&list, "agree"), FormValue::Checked(true));
    let small = named(&list, "size")[0].clone();
    let list = fill(e, doc, 0, &small, FormValue::Checked(true));
    let list = fill(
        e,
        doc,
        0,
        &one(&list, "country"),
        FormValue::Choice(vec![0]),
    );
    let list = fill(
        e,
        doc,
        0,
        &one(&list, "city"),
        FormValue::Text("Bergen".into()),
    );
    let list = fill(
        e,
        doc,
        0,
        &one(&list, "colours"),
        FormValue::Choice(vec![0, 2]),
    );
    assert_eq!(one(&list, "name").value, "Bj\u{f8}rn \u{c5}se");
    assert_eq!(one(&list, "notes").value, "one\ntwo");
    assert_eq!(one(&list, "secret").value, "hunter2");
    assert_eq!(
        one(&list, "code").value,
        "ABCDEF",
        "a comb takes as many characters as it has boxes"
    );
    assert!(one(&list, "agree").has(field_flag::CHECKED));
    let on: Vec<bool> = named(&list, "size")
        .iter()
        .map(|f| f.has(field_flag::CHECKED))
        .collect();
    assert_eq!(on, [true, false, false], "picking S turns M off");
    assert_eq!(picked(&one(&list, "country")), ["Norge"]);
    assert_eq!(one(&list, "city").value, "Bergen");
    assert_eq!(picked(&one(&list, "colours")), ["R\u{f8}d", "Bl\u{e5}"]);

    // Saved and opened again, in the same engine, it is all still there.
    let saved = save_all(e, doc);
    let (again, _file2) = open(e, &saved);
    let list = fields(e, again, 0);
    assert_eq!(one(&list, "name").value, "Bj\u{f8}rn \u{c5}se");
    assert_eq!(one(&list, "notes").value, "one\ntwo");
    assert_eq!(one(&list, "secret").value, "hunter2");
    assert_eq!(one(&list, "code").value, "ABCDEF");
    assert!(one(&list, "agree").has(field_flag::CHECKED));
    let on: Vec<bool> = named(&list, "size")
        .iter()
        .map(|f| f.has(field_flag::CHECKED))
        .collect();
    assert_eq!(on, [true, false, false]);
    assert_eq!(picked(&one(&list, "country")), ["Norge"]);
    assert_eq!(one(&list, "city").value, "Bergen");
    assert_eq!(picked(&one(&list, "colours")), ["R\u{f8}d", "Bl\u{e5}"]);
    // And the designer sees them all, including the place to sign.
    let items = designed(e, again, 0);
    assert_eq!(items.len(), 12);
    assert!(items.iter().all(|w| w.editable));
    assert_eq!(
        items
            .iter()
            .filter(|w| w.kind == DesignKind::Signature)
            .count(),
        1
    );
}

#[test]
fn the_pictures_are_in_the_saved_file() {
    // Fields without a picture are drawn by some readers and not by others. Every widget the
    // designer made has a normal appearance in the saved file.
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open(e, &kit::simple_pdf(1));
    apply_ok(e, doc, everything());
    let saved = save_all(e, doc);
    let widgets = widget_dictionaries(&saved);
    assert_eq!(widgets.len(), 12, "{} widgets", widgets.len());
    for (n, dict) in &widgets {
        assert!(
            dict.contains("/AP"),
            "the widget object {n} has no appearance: {dict}"
        );
    }
    // A radio button and a check box have a picture for each state, named with the words that
    // were chosen, and a place to sign has one picture.
    let squeezed: String = String::from_utf8_lossy(&saved)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(squeezed.contains("/Ja"), "the state of the check box");
    for state in ["/S", "/M", "/L"] {
        assert!(squeezed.contains(state), "{state}");
    }
    assert!(!squeezed.contains("/NeedAppearances"));
    for forbidden in ["/JavaScript", "/AA", "/SigFlags", "/ByteRange"] {
        assert!(!squeezed.contains(forbidden), "{forbidden}");
    }
}

/// The widget dictionaries of a saved file, as text, with their object numbers. PDFium writes the
/// objects in the open, so a scan of the text finds them.
fn widget_dictionaries(saved: &[u8]) -> Vec<(u32, String)> {
    let text = String::from_utf8_lossy(saved).into_owned();
    let mut out = Vec::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find(" 0 obj") {
        let start = rest[..at].rfind('\n').map_or(0, |p| p + 1);
        let number: u32 = rest[start..at].trim().parse().unwrap_or(0);
        let after = &rest[at..];
        let end = after.find("endobj").unwrap_or(after.len());
        let body = &after[..end];
        if body.contains("/Subtype/Widget") || body.contains("/Subtype /Widget") {
            out.push((number, body.to_string()));
        }
        rest = &after[end..];
    }
    out
}

// ---- names and properties -------------------------------------------------------------------------

#[test]
fn names_are_checked_against_the_whole_document() {
    let mut guard = engine();
    let e = &mut guard.0;
    // A field on the fourth page; the name is taken although page 1 does not show it.
    let (doc, _file) = open(e, &kit::simple_pdf(4));
    let mut far = text("far", [72.0, 72.0, 272.0, 94.0]);
    far.widgets[0].page = 3;
    apply_ok(e, doc, vec![add(far)]);
    let (code, message) =
        apply(e, doc, vec![add(text("far", [72.0, 100.0, 272.0, 122.0]))]).unwrap_err();
    assert_eq!(code, ErrorCode::AuthorRefused);
    assert!(message.contains("already a field called"), "{message}");
    assert!(names(e, doc).contains(&"far".to_string()));
    // A dot means a field inside another field, so it is refused.
    let (code, message) =
        apply(e, doc, vec![add(text("a.b", [72.0, 100.0, 272.0, 122.0]))]).unwrap_err();
    assert_eq!(code, ErrorCode::AuthorRefused);
    assert!(message.contains("dot"), "{message}");
    // An empty name, a name with only spaces.
    for bad in ["", "  "] {
        let (code, _) =
            apply(e, doc, vec![add(text(bad, [72.0, 100.0, 272.0, 122.0]))]).unwrap_err();
        assert_eq!(code, ErrorCode::AuthorRefused);
    }
    // The part of a name before a dot is taken by the field that has the full name.
    let (nested, _file2) = open(e, &kit::forms_nested_pdf());
    let (code, message) = apply(
        e,
        nested,
        vec![add(text("person", [20.0, 20.0, 120.0, 40.0]))],
    )
    .unwrap_err();
    assert_eq!(code, ErrorCode::AuthorRefused, "{message}");
    // Two new fields with the same name in one batch: the second is refused, and the first is not kept.
    let (code, _) = apply(
        e,
        doc,
        vec![
            add(text("twin", [72.0, 100.0, 272.0, 122.0])),
            add(text("twin", [72.0, 130.0, 272.0, 152.0])),
        ],
    )
    .unwrap_err();
    assert_eq!(code, ErrorCode::AuthorRefused);
    assert!(
        !names(e, doc).contains(&"twin".to_string()),
        "a batch is all or nothing"
    );
}

#[test]
fn what_the_font_cannot_draw_is_refused_and_the_nordic_letters_are_not() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open(e, &kit::simple_pdf(1));
    let mut f = text("t", [72.0, 72.0, 272.0, 94.0]);
    f.props.default_value = "Bl\u{e5}b\u{e6}rsyltet\u{f8}y \u{c6}\u{d8}\u{c5}".into();
    apply_ok(e, doc, vec![add(f)]);
    let list = fields(e, doc, 0);
    assert_eq!(
        one(&list, "t").value,
        "Bl\u{e5}b\u{e6}rsyltet\u{f8}y \u{c6}\u{d8}\u{c5}"
    );
    for bad in [
        "\u{20ac}5",
        "\u{141}\u{f3}d\u{17a}",
        "\u{41f}\u{440}\u{438}\u{432}\u{435}\u{442}",
        "\u{65e5}\u{672c}",
    ] {
        let mut f = text("u", [72.0, 100.0, 272.0, 122.0]);
        f.props.default_value = bad.into();
        let (code, message) = apply(e, doc, vec![add(f)]).unwrap_err();
        assert_eq!(code, ErrorCode::AuthorRefused);
        assert!(message.contains("Latin"), "{message}");
    }
    // The name and the tooltip are not drawn: anything goes.
    let mut f = text("\u{65e5}\u{672c}_\u{e6}", [72.0, 100.0, 272.0, 122.0]);
    f.props.tooltip = "\u{20ac} \u{65e5}\u{672c}".into();
    apply_ok(e, doc, vec![add(f)]);
    let list = fields(e, doc, 0);
    assert_eq!(
        one(&list, "\u{65e5}\u{672c}_\u{e6}").tooltip,
        "\u{20ac} \u{65e5}\u{672c}"
    );
}

#[test]
fn properties_can_be_changed_and_read_back() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open(e, &kit::simple_pdf(1));
    apply_ok(e, doc, everything());
    let items = designed(e, doc, 0);
    let mut props = items
        .iter()
        .find(|w| w.name == "name")
        .unwrap()
        .props
        .clone();
    assert_eq!(props.tooltip, "Your full name");
    props.tooltip = "Name".into();
    props.required = false;
    props.read_only = true;
    props.max_len = 10;
    props.default_value = "Kari".into();
    props.font_size = 0.0;
    apply_ok(
        e,
        doc,
        vec![AuthorOp::Update {
            name: "name".into(),
            new_name: "fullname".into(),
            props,
            exports: vec![],
        }],
    );
    let list = fields(e, doc, 0);
    assert!(named(&list, "name").is_empty());
    let f = one(&list, "fullname");
    assert_eq!(f.tooltip, "Name");
    assert!(!f.has(field_flag::REQUIRED));
    assert!(f.has(field_flag::READ_ONLY));
    assert_eq!(f.max_len, 10);
    assert_eq!(f.value, "Kari");
    assert_eq!(f.font_size, 0.0);
    // The choices of a drop-down, its default, and whether the reader may type.
    let items = designed(e, doc, 0);
    let mut props = items
        .iter()
        .find(|w| w.name == "country")
        .unwrap()
        .props
        .clone();
    props.choices = vec!["Finland".into(), "Island".into()];
    props.default_value = "Island".into();
    props.editable = true;
    apply_ok(
        e,
        doc,
        vec![AuthorOp::Update {
            name: "country".into(),
            new_name: String::new(),
            props,
            exports: vec![],
        }],
    );
    let list = fields(e, doc, 0);
    let c = one(&list, "country");
    assert_eq!(labels(&c), ["Finland", "Island"]);
    assert_eq!(picked(&c), ["Island"]);
    assert!(c.has(field_flag::EDITABLE));
    // A check box: a new word, and starting on.
    let items = designed(e, doc, 0);
    let mut props = items
        .iter()
        .find(|w| w.name == "agree")
        .unwrap()
        .props
        .clone();
    props.export_value = "Oui".into();
    props.default_value = "on".into();
    apply_ok(
        e,
        doc,
        vec![AuthorOp::Update {
            name: "agree".into(),
            new_name: String::new(),
            props,
            exports: vec![],
        }],
    );
    let list = fields(e, doc, 0);
    assert!(one(&list, "agree").has(field_flag::CHECKED));
    let items = designed(e, doc, 0);
    assert_eq!(
        items.iter().find(|w| w.name == "agree").unwrap().export,
        "Oui"
    );
    // A radio group: other words, and the same button stays on.
    let items = designed(e, doc, 0);
    let mut props = items
        .iter()
        .find(|w| w.name == "size")
        .unwrap()
        .props
        .clone();
    props.default_value = "Medium".into();
    apply_ok(
        e,
        doc,
        vec![AuthorOp::Update {
            name: "size".into(),
            new_name: String::new(),
            props,
            exports: vec!["Small".into(), "Medium".into(), "Large".into()],
        }],
    );
    let list = fields(e, doc, 0);
    let on: Vec<bool> = named(&list, "size")
        .iter()
        .map(|f| f.has(field_flag::CHECKED))
        .collect();
    assert_eq!(on, [false, true, false]);
    // A property that does not fit the kind is refused with a reason.
    let items = designed(e, doc, 0);
    let mut props = items
        .iter()
        .find(|w| w.name == "code")
        .unwrap()
        .props
        .clone();
    props.max_len = 0;
    let (code, message) = apply(
        e,
        doc,
        vec![AuthorOp::Update {
            name: "code".into(),
            new_name: String::new(),
            props,
            exports: vec![],
        }],
    )
    .unwrap_err();
    assert_eq!(code, ErrorCode::AuthorRefused);
    assert!(message.contains("box for each character"), "{message}");
    // A field the designer did not make is left alone.
    let (code, _) = apply(
        e,
        doc,
        vec![AuthorOp::Remove {
            name: "nothing".into(),
        }],
    )
    .unwrap_err();
    assert_eq!(code, ErrorCode::AuthorRefused);
}

#[test]
fn a_changed_property_keeps_the_text_the_reader_typed() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open(e, &kit::simple_pdf(1));
    apply_ok(e, doc, vec![add(text("t", [72.0, 72.0, 272.0, 94.0]))]);
    let list = fields(e, doc, 0);
    fill(e, doc, 0, &one(&list, "t"), FormValue::Text("typed".into()));
    let mut props = designed(e, doc, 0)[0].props.clone();
    props.tooltip = "only the tooltip".into();
    apply_ok(
        e,
        doc,
        vec![AuthorOp::Update {
            name: "t".into(),
            new_name: String::new(),
            props,
            exports: vec![],
        }],
    );
    let list = fields(e, doc, 0);
    assert_eq!(one(&list, "t").value, "typed");
    assert_eq!(one(&list, "t").tooltip, "only the tooltip");
}

// ---- the reload ---------------------------------------------------------------------------------

fn annotation_ids(e: &mut Engine, doc: u32, page: u32) -> Vec<(u32, String)> {
    common::list(e, doc, page)
        .into_iter()
        .map(|a| (a.id, a.text))
        .collect()
}

#[test]
fn the_reload_keeps_the_ids_of_the_annotations_and_the_form_scan() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open(e, &kit::annotated_pdf());
    let before = annotation_ids(e, doc, 0);
    assert!(!before.is_empty());
    // The form environment is made (it searches the written document for actions).
    assert!(matches!(
        ask(e, RequestBody::FormInfo { doc }),
        ResponseBody::FormInfo { .. }
    ));
    let scans = partridge_engine::forms::ENV_SCANS.load(std::sync::atomic::Ordering::SeqCst);
    apply_ok(
        e,
        doc,
        vec![
            add(text("a", [72.0, 72.0, 272.0, 94.0])),
            add(text("b", [72.0, 104.0, 272.0, 126.0])),
        ],
    );
    assert_eq!(
        annotation_ids(e, doc, 0),
        before,
        "the ids of the annotations stay"
    );
    // A note added after the fields, then the first field taken away: the note keeps its id.
    let note = common::annot(ask(
        e,
        RequestBody::AddNote {
            doc,
            page: 0,
            x: 300.0,
            y: 300.0,
            color: common::YELLOW,
            text: "after the fields".into(),
        },
    ));
    apply_ok(e, doc, vec![AuthorOp::Remove { name: "a".into() }]);
    let after = annotation_ids(e, doc, 0);
    assert!(
        after
            .iter()
            .any(|(id, text)| *id == note.id && text == "after the fields"),
        "{after:?}"
    );
    for kept in &before {
        assert!(after.contains(kept), "{kept:?} is gone from {after:?}");
    }
    // An annotation made before the reload can still be changed with the id it had.
    let first = before[0].0;
    let changed = ask(
        e,
        RequestBody::UpdateAnnot {
            doc,
            page: 0,
            id: first,
            text: None,
            color: Some([10, 200, 10, 255]),
            dx: 0.0,
            dy: 0.0,
        },
    );
    assert!(
        matches!(
            changed,
            ResponseBody::Annot { .. } | ResponseBody::Error { .. }
        ),
        "{changed:?}"
    );
    if let ResponseBody::Error { code, .. } = changed {
        assert_ne!(
            code,
            ErrorCode::NoSuchAnnotation,
            "the id went with the reload"
        );
    }
    // The form environment was made again without writing the document out to search it.
    assert_eq!(
        partridge_engine::forms::ENV_SCANS.load(std::sync::atomic::Ordering::SeqCst),
        scans,
        "the search for actions was made again"
    );
    assert!(e.form_scans_for_tests(doc).is_some());
    // The reader can fill in the field that is left.
    let list = fields(e, doc, 0);
    let filled = fill(e, doc, 0, &one(&list, "b"), FormValue::Text("hello".into()));
    assert_eq!(one(&filled, "b").value, "hello");
}

#[test]
fn a_batch_is_one_reload_and_one_step_back() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open(e, &kit::simple_pdf(1));
    let ops: Vec<AuthorOp> = (0..40)
        .map(|i| {
            let y = 10.0 + 18.0 * i as f32;
            add(text(&format!("f{i}"), [20.0, y, 120.0, y + 16.0]))
        })
        .collect();
    let step = apply_ok(e, doc, ops);
    assert!(step > 0);
    assert_eq!(fields(e, doc, 0).len(), 40);
    // One step back takes them all, and the document has no form again.
    let forward = restore(e, doc, step);
    let (code, _) = failed(ask(e, RequestBody::ListFormFields { doc, page: 0 }));
    assert_eq!(code, ErrorCode::FormUnsupported);
    assert!(names(e, doc).is_empty());
    // And one step forward brings them all back, in order.
    restore(e, doc, forward);
    let list = fields(e, doc, 0);
    assert_eq!(list.len(), 40);
    assert_eq!(list[0].name, "f0");
    assert_eq!(list[39].name, "f39");
    // More changes than a batch may hold are refused.
    let many: Vec<AuthorOp> = (0..101)
        .map(|i| add(text(&format!("g{i}"), [20.0, 20.0, 120.0, 36.0])))
        .collect();
    let (code, _) = apply(e, doc, many).unwrap_err();
    assert_eq!(code, ErrorCode::InvalidRequest);
}

#[test]
fn undo_and_redo_bring_back_fields_and_the_ids_of_the_annotations() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open(e, &kit::annotated_pdf());
    let before = annotation_ids(e, doc, 0);
    let step1 = apply_ok(e, doc, vec![add(text("one", [72.0, 72.0, 272.0, 94.0]))]);
    let step2 = apply_ok(e, doc, vec![add(text("two", [72.0, 104.0, 272.0, 126.0]))]);
    assert_ne!(step1, step2);
    // Back over the second, then the first.
    let redo2 = restore(e, doc, step2);
    assert_eq!(names(e, doc), ["one"]);
    let redo1 = restore(e, doc, step1);
    assert!(names(e, doc).is_empty());
    assert_eq!(annotation_ids(e, doc, 0), before);
    // Forward again, in the same order.
    restore(e, doc, redo1);
    assert_eq!(names(e, doc), ["one"]);
    restore(e, doc, redo2);
    assert_eq!(names(e, doc), ["one", "two"]);
    // A removal is taken back as well, with what the reader typed in the field.
    let list = fields(e, doc, 0);
    fill(
        e,
        doc,
        0,
        &one(&list, "one"),
        FormValue::Text("kept".into()),
    );
    let removal = apply_ok(e, doc, vec![AuthorOp::Remove { name: "one".into() }]);
    assert_eq!(names(e, doc), ["two"]);
    restore(e, doc, removal);
    assert_eq!(one(&fields(e, doc, 0), "one").value, "kept");
}

// ---- removing, moving, the order ----------------------------------------------------------------

#[test]
fn a_field_can_be_removed_moved_and_put_in_a_new_tab_order() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open(e, &kit::simple_pdf(2));
    let ops = ["a", "b", "c"]
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let y = 20.0 + 30.0 * i as f32;
            add(text(n, [20.0, y, 120.0, y + 20.0]))
        })
        .collect();
    apply_ok(e, doc, ops);
    let order =
        |e: &mut Engine| -> Vec<String> { fields(e, doc, 0).into_iter().map(|f| f.name).collect() };
    assert_eq!(order(e), ["a", "b", "c"]);
    // The tab order follows the list of annotations of the page.
    apply_ok(
        e,
        doc,
        vec![AuthorOp::Reorder {
            page: 0,
            order: ["c", "a", "b"]
                .iter()
                .map(|n| WidgetRef {
                    name: (*n).into(),
                    widget: 0,
                })
                .collect(),
        }],
    );
    assert_eq!(order(e), ["c", "a", "b"]);
    // Move one: to another page, and a resize on its page.
    apply_ok(
        e,
        doc,
        vec![
            AuthorOp::Move {
                name: "a".into(),
                widget: 0,
                page: 1,
                rect: [40.0, 40.0, 240.0, 70.0],
            },
            AuthorOp::Move {
                name: "b".into(),
                widget: 0,
                page: 0,
                rect: [50.0, 300.0, 150.0, 330.0],
            },
        ],
    );
    assert_eq!(order(e), ["c", "b"]);
    let second = fields(e, doc, 1);
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].name, "a");
    assert_eq!(second[0].rect, [40.0, 40.0, 240.0, 70.0]);
    assert_eq!(
        one(&fields(e, doc, 0), "b").rect,
        [50.0, 300.0, 150.0, 330.0]
    );
    // Remove: gone from the page, from the form and from the names.
    apply_ok(e, doc, vec![AuthorOp::Remove { name: "c".into() }]);
    assert_eq!(order(e), ["b"]);
    assert_eq!(names(e, doc), ["a", "b"]);
    let saved = save_all(e, doc);
    let (again, _file2) = open(e, &saved);
    assert_eq!(names(e, again), ["a", "b"]);
}

#[test]
fn a_radio_group_can_be_made_bigger_and_smaller() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open(e, &kit::simple_pdf(1));
    apply_ok(e, doc, vec![add(radio("size", "M", 100.0))]);
    apply_ok(
        e,
        doc,
        vec![AuthorOp::AddWidget {
            name: "size".into(),
            widget: DesignWidget {
                page: 0,
                rect: [200.0, 100.0, 214.0, 114.0],
                export: "XL".into(),
            },
        }],
    );
    assert_eq!(named(&fields(e, doc, 0), "size").len(), 4);
    apply_ok(
        e,
        doc,
        vec![AuthorOp::RemoveWidget {
            name: "size".into(),
            widget: 0,
        }],
    );
    let list = fields(e, doc, 0);
    let group = named(&list, "size");
    assert_eq!(group.len(), 3);
    assert!(
        group[0].has(field_flag::CHECKED),
        "M is the first one now and is still on"
    );
    // Another button of the group with a word the group has is refused.
    let (code, _) = apply(
        e,
        doc,
        vec![AuthorOp::AddWidget {
            name: "size".into(),
            widget: DesignWidget {
                page: 0,
                rect: [240.0, 100.0, 254.0, 114.0],
                export: "M".into(),
            },
        }],
    )
    .unwrap_err();
    assert_eq!(code, ErrorCode::AuthorRefused);
}

// ---- pages that are turned ------------------------------------------------------------------------

/// The box around the ink inside `rect`, a few points in from its border, on a page drawn at one
/// pixel to the point.
fn ink_box(shot: &Shot, rect: [f32; 4]) -> Option<[usize; 4]> {
    let (x0, y0) = (rect[0] as usize + 3, rect[1] as usize + 3);
    let (x1, y1) = (rect[2] as usize - 3, rect[3] as usize - 3);
    let mut found: Option<[usize; 4]> = None;
    for y in y0..y1 {
        for x in x0..x1 {
            if shot.pixel(x, y).iter().any(|c| *c < 160) {
                found = Some(match found {
                    None => [x, y, x, y],
                    Some(b) => [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)],
                });
            }
        }
    }
    found
}

#[test]
fn a_field_on_a_turned_page_is_where_it_was_drawn_and_reads_upright() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, sizes, _file) = open_with_pages(e, &kit::turned_pdf());
    assert_eq!(sizes.len(), 5);
    let drawn = [60.0f32, 80.0, 300.0, 110.0];
    for page in 0..5u32 {
        let mut f = text(&format!("t{page}"), drawn);
        f.widgets[0].page = page;
        apply_ok(e, doc, vec![add(f)]);
        let list = fields(e, doc, page);
        let field = one(&list, &format!("t{page}"));
        // The reader lists the box where it was drawn, and the page's turn.
        for (got, want) in field.rect.iter().zip(drawn) {
            assert!(
                (got - want).abs() < 0.6,
                "page {page}: {:?} != {drawn:?}",
                field.rect
            );
        }
        let want_turns = [0u8, 1, 2, 3, 1][page as usize];
        assert_eq!(field.turns, want_turns, "page {page}");
        // The designer lists it in the same place.
        let items = designed(e, doc, page);
        let mine = items.iter().find(|w| w.name == format!("t{page}")).unwrap();
        for (got, want) in mine.rect.iter().zip(drawn) {
            assert!((got - want).abs() < 0.6, "page {page}: {:?}", mine.rect);
        }
        // A text that fits, filled in: it runs along the long side of the box on the screen.
        fill(e, doc, page, &field, FormValue::Text("Hello World".into()));
        let (w, h) = sizes[page as usize];
        let shot = Shot::take(e, doc, page, w as u32, h as u32);
        let ink = ink_box(&shot, drawn).unwrap_or_else(|| panic!("page {page}: no text drawn"));
        let (iw, ih) = (ink[2] - ink[0] + 1, ink[3] - ink[1] + 1);
        assert!(
            iw as f32 > 2.0 * ih as f32,
            "page {page}: the text is {iw} wide and {ih} tall, so it does not read along the box"
        );
    }
    // The turn is in the file as the field's own rotation.
    let saved = save_all(e, doc);
    let squeezed: String = String::from_utf8_lossy(&saved)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    for turn in [90, 180, 270] {
        assert!(
            squeezed.contains(&format!("/R{turn}")),
            "no /R {turn} in the file"
        );
    }
}

// ---- what is refused -----------------------------------------------------------------------------

#[test]
fn encrypted_and_xfa_documents_are_refused_with_a_reason() {
    let mut guard = engine();
    let e = &mut guard.0;
    let file = kit::TempFile::new("enc.pdf", &kit::encrypted_pdf("secret", 1));
    e.set_document(File::open(file.path()).unwrap());
    let ResponseBody::Opened { doc, .. } = ask(
        e,
        RequestBody::Open {
            password: Some("secret".into()),
        },
    ) else {
        panic!("not opened");
    };
    let (code, message) = failed(ask(e, RequestBody::Author(AuthorRequest::Check { doc })));
    assert_eq!(code, ErrorCode::AuthorUnsupported);
    assert!(message.contains("encrypted"), "{message}");
    let (code, message) =
        apply(e, doc, vec![add(text("t", [72.0, 72.0, 272.0, 94.0]))]).unwrap_err();
    assert_eq!(code, ErrorCode::AuthorUnsupported, "{message}");
    assert!(message.contains("encrypted"), "{message}");
    let (xfa, _file2) = open(e, &kit::xfa_pdf());
    let (code, message) = failed(ask(
        e,
        RequestBody::Author(AuthorRequest::Check { doc: xfa }),
    ));
    assert_eq!(code, ErrorCode::AuthorUnsupported);
    assert!(message.contains("XFA"), "{message}");
    let (code, _) = apply(e, xfa, vec![add(text("t", [72.0, 72.0, 272.0, 94.0]))]).unwrap_err();
    assert_eq!(code, ErrorCode::AuthorUnsupported);
    // Nothing was changed in either document.
    assert!(matches!(
        ask(e, RequestBody::FormInfo { doc: xfa }),
        ResponseBody::FormInfo { .. }
    ));
}

#[test]
fn a_document_with_an_action_keeps_it_and_the_new_fields_have_none() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open(e, &kit::forms_script_pdf());
    let before = fields(e, doc, 0);
    apply_ok(e, doc, vec![add(text("added", [20.0, 20.0, 120.0, 40.0]))]);
    let list = fields(e, doc, 0);
    assert_eq!(list.len(), before.len() + 1);
    let added = one(&list, "added");
    assert!(!added.has(field_flag::ACTION), "a new field has no action");
    // The old fields keep what they had.
    for (old, now) in before.iter().zip(&list) {
        assert_eq!(now.name, old.name);
        assert_eq!(
            now.flags & field_flag::ACTION,
            old.flags & field_flag::ACTION,
            "{}",
            old.name
        );
    }
    // The designer lists the ones with actions and leaves them alone.
    let items = designed(e, doc, 0);
    assert!(items.iter().any(|w| !w.editable));
    assert!(items.iter().find(|w| w.name == "added").unwrap().editable);
    let blocked = items.iter().find(|w| !w.editable).unwrap().name.clone();
    let (code, _) = apply(e, doc, vec![AuthorOp::Remove { name: blocked }]).unwrap_err();
    assert_eq!(code, ErrorCode::AuthorRefused);
}

#[test]
fn a_place_to_sign_is_not_a_signature() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open(e, &kit::simple_pdf(1));
    assert_eq!(signatures(e, doc), (0, false));
    let sign = DesignField {
        name: "sign".into(),
        kind: DesignKind::Signature,
        props: DesignProps::default(),
        widgets: vec![widget(0, [72.0, 72.0, 272.0, 122.0])],
    };
    apply_ok(e, doc, vec![add(sign.clone())]);
    assert_eq!(
        signatures(e, doc),
        (0, false),
        "an empty place to sign does not make the document signed"
    );
    // A document that is signed stays signed with one more empty place.
    let (signed, _file2) = open(e, &kit::forms_signed_pdf());
    assert_eq!(signatures(e, signed).0, 1);
    let mut second = sign;
    second.name = "sign two".into();
    second.widgets[0].rect = [300.0, 72.0, 500.0, 122.0];
    apply_ok(e, signed, vec![add(second)]);
    assert_eq!(signatures(e, signed).0, 1);
}

#[test]
fn a_page_that_is_not_there_and_a_box_outside_the_page_are_refused() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open(e, &kit::simple_pdf(1));
    let mut f = text("t", [72.0, 72.0, 272.0, 94.0]);
    f.widgets[0].page = 5;
    let (code, _) = apply(e, doc, vec![add(f)]).unwrap_err();
    assert_eq!(code, ErrorCode::PageOutOfRange);
    let (code, message) =
        apply(e, doc, vec![add(text("t", [600.0, 72.0, 700.0, 94.0]))]).unwrap_err();
    assert_eq!(code, ErrorCode::AuthorRefused);
    assert!(message.contains("inside the page"), "{message}");
    let (code, _) = apply(e, doc, vec![add(text("t", [72.0, 72.0, 74.0, 94.0]))]).unwrap_err();
    assert_eq!(code, ErrorCode::AuthorRefused);
    assert!(names(e, doc).is_empty());
    let (code, _) = failed_list(e, doc, 4);
    assert_eq!(code, ErrorCode::PageOutOfRange);
}

fn failed_list(e: &mut Engine, doc: u32, page: u32) -> Failed {
    failed(ask(
        e,
        RequestBody::Author(AuthorRequest::List { doc, page }),
    ))
}

// ---- the files for other programs ----------------------------------------------------------------

/// With `PARTRIDGE_FORMS_OUT` set, the forms made here are written there for `scripts/check-created-forms.sh`
/// (qpdf, pdftk and poppler): one file as it was made, one filled in by the reader's code, and a
/// list of what is expected.
#[test]
fn the_files_for_other_programs_are_written() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open(e, &kit::simple_pdf(1));
    apply_ok(e, doc, everything());
    let empty = save_all(e, doc);
    let list = fields(e, doc, 0);
    let list = fill(
        e,
        doc,
        0,
        &one(&list, "name"),
        FormValue::Text("Bj\u{f8}rn \u{c5}se".into()),
    );
    let list = fill(
        e,
        doc,
        0,
        &one(&list, "notes"),
        FormValue::Text("one\ntwo".into()),
    );
    let list = fill(e, doc, 0, &one(&list, "agree"), FormValue::Checked(true));
    let small = named(&list, "size")[0].clone();
    let list = fill(e, doc, 0, &small, FormValue::Checked(true));
    let list = fill(
        e,
        doc,
        0,
        &one(&list, "country"),
        FormValue::Choice(vec![0]),
    );
    let list = fill(
        e,
        doc,
        0,
        &one(&list, "colours"),
        FormValue::Choice(vec![0, 2]),
    );
    drop(list);
    let filled = save_all(e, doc);
    assert!(empty.starts_with(b"%PDF-") && filled.starts_with(b"%PDF-"));
    let Some(dir) = std::env::var_os("PARTRIDGE_FORMS_OUT") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("created-empty.pdf"), &empty).unwrap();
    std::fs::write(dir.join("created-filled.pdf"), &filled).unwrap();
    std::fs::write(dir.join("created-blank-page.pdf"), kit::simple_pdf(1)).unwrap();
    // `name<TAB>value` as pdftk prints a value; a list box with two choices has a line for each.
    std::fs::write(
        dir.join("created-expected.txt"),
        "name\tBj\u{f8}rn \u{c5}se\n\
         notes\tone\n\
         agree\tJa\n\
         size\tS\n\
         country\tNorge\n\
         colours\tR\u{f8}d\n\
         colours\tBl\u{e5}\n",
    )
    .unwrap();
    // What pdftk must see of the fields as made: `name<TAB>type<TAB>flags<TAB>extra`.
    std::fs::write(
        dir.join("created-fields.txt"),
        "name\tText\tmaxlen=30\ttooltip=Your full name\n\
         notes\tText\t\ttooltip=\n\
         secret\tText\t\ttooltip=\n\
         code\tText\tmaxlen=6\ttooltip=\n\
         agree\tButton\t\ttooltip=I agree\n\
         size\tButton\t\ttooltip=\n\
         country\tChoice\t\ttooltip=\n\
         city\tChoice\t\ttooltip=\n\
         colours\tChoice\t\ttooltip=\n\
         sign\tSignature\t\ttooltip=\n",
    )
    .unwrap();
}
