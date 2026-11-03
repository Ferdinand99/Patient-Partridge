#![allow(dead_code)]
//! What the tests of the form designer share: opening a document, asking the designer, filling in a
//! field, and a batch that makes one field of every kind.

use std::fs::File;

use partridge_engine::Engine;
use partridge_protocol::{
    AuthorOp, AuthorRequest, AuthorResponse, DesignField, DesignKind, DesignProps, DesignWidget,
    DesignedWidget, ErrorCode, FormField, FormValue, PagesRequest, PagesResponse, RequestBody,
    ResponseBody,
};
use partridge_testkit as kit;

use super::ask;

pub fn open(e: &mut Engine, bytes: &[u8]) -> (u32, kit::TempFile) {
    let file = kit::TempFile::new("author.pdf", bytes);
    e.set_document(File::open(file.path()).unwrap());
    match ask(e, RequestBody::Open { password: None }) {
        ResponseBody::Opened { doc, .. } => (doc, file),
        other => panic!("{other:?}"),
    }
}

pub fn open_with_pages(e: &mut Engine, bytes: &[u8]) -> (u32, Vec<(f32, f32)>, kit::TempFile) {
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

pub type Failed = (ErrorCode, String);

pub fn failed(body: ResponseBody) -> Failed {
    match body {
        ResponseBody::Error { code, message } => (code, message),
        other => panic!("expected an error, got {other:?}"),
    }
}

/// Makes the changes; returns the number of the copy to go back to.
pub fn apply(e: &mut Engine, doc: u32, ops: Vec<AuthorOp>) -> Result<u32, Failed> {
    match ask(e, RequestBody::Author(AuthorRequest::Apply { doc, ops })) {
        ResponseBody::Pages(PagesResponse::Changed { snapshot, .. }) => Ok(snapshot),
        other => Err(failed(other)),
    }
}

pub fn apply_ok(e: &mut Engine, doc: u32, ops: Vec<AuthorOp>) -> u32 {
    apply(e, doc, ops).unwrap_or_else(|f| panic!("{f:?}"))
}

pub fn restore(e: &mut Engine, doc: u32, snapshot: u32) -> u32 {
    match ask(
        e,
        RequestBody::Pages(PagesRequest::Restore { doc, snapshot }),
    ) {
        ResponseBody::Pages(PagesResponse::Changed { snapshot, .. }) => snapshot,
        other => panic!("{other:?}"),
    }
}

pub fn designed(e: &mut Engine, doc: u32, page: u32) -> Vec<DesignedWidget> {
    match ask(e, RequestBody::Author(AuthorRequest::List { doc, page })) {
        ResponseBody::Author(AuthorResponse::Widgets { items }) => items,
        other => panic!("{other:?}"),
    }
}

pub fn names(e: &mut Engine, doc: u32) -> Vec<String> {
    match ask(e, RequestBody::Author(AuthorRequest::Check { doc })) {
        ResponseBody::Author(AuthorResponse::Ready { names }) => names,
        other => panic!("{other:?}"),
    }
}

pub fn fields(e: &mut Engine, doc: u32, page: u32) -> Vec<FormField> {
    match ask(e, RequestBody::ListFormFields { doc, page }) {
        ResponseBody::FormFields { fields } => fields,
        other => panic!("{other:?}"),
    }
}

pub fn named<'a>(list: &'a [FormField], name: &str) -> Vec<&'a FormField> {
    list.iter().filter(|f| f.name == name).collect()
}

pub fn one(list: &[FormField], name: &str) -> FormField {
    let found = named(list, name);
    assert_eq!(
        found.len(),
        1,
        "{name} in {:?}",
        list.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
    found[0].clone()
}

pub fn fill(
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

pub fn signatures(e: &mut Engine, doc: u32) -> (u32, bool) {
    match ask(e, RequestBody::DocInfo { doc }) {
        ResponseBody::DocInfo {
            signatures,
            maybe_signed,
            ..
        } => (signatures, maybe_signed),
        other => panic!("{other:?}"),
    }
}

pub fn widget(page: u32, rect: [f32; 4]) -> DesignWidget {
    DesignWidget {
        page,
        rect,
        export: String::new(),
    }
}

pub fn text(name: &str, rect: [f32; 4]) -> DesignField {
    DesignField {
        name: name.into(),
        kind: DesignKind::Text,
        props: DesignProps::default(),
        widgets: vec![widget(0, rect)],
    }
}

pub fn add(field: DesignField) -> AuthorOp {
    AuthorOp::Add(field)
}

pub fn radio(name: &str, on: &str, y: f32) -> DesignField {
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
pub fn everything() -> Vec<AuthorOp> {
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

pub fn labels(f: &FormField) -> Vec<&str> {
    f.options.iter().map(|o| o.label.as_str()).collect()
}

pub fn picked(f: &FormField) -> Vec<&str> {
    f.options
        .iter()
        .filter(|o| o.selected)
        .map(|o| o.label.as_str())
        .collect()
}
