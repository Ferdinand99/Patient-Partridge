//! Tests of the messages of the form designer: that they come back the same, that the encoder keeps
//! to the caps, and that the decoder refuses what is over them or not made of known values.

use super::*;
use crate::{Request, RequestBody, Response, ResponseBody};

fn props() -> DesignProps {
    DesignProps {
        tooltip: "Your name \u{e6}\u{f8}\u{e5}".into(),
        required: true,
        read_only: false,
        multiline: true,
        password: false,
        comb: true,
        max_len: 12,
        editable: true,
        multiselect: true,
        font_size: 11.5,
        default_value: "Ada\nLovelace".into(),
        choices: vec!["Norway".into(), "\u{c5}land".into()],
        export_value: "Yes".into(),
    }
}

fn widget(page: u32) -> DesignWidget {
    DesignWidget {
        page,
        rect: [10.5, 20.25, 110.0, 40.0],
        export: "M".into(),
    }
}

fn field(kind: DesignKind) -> DesignField {
    DesignField {
        name: "person".into(),
        kind,
        props: props(),
        widgets: vec![widget(0), widget(2)],
    }
}

fn request_back(body: RequestBody) -> RequestBody {
    let bytes = Request { id: 5, body }.encode();
    Request::decode(&bytes).expect("decodes").body
}

fn response_back(body: ResponseBody) -> ResponseBody {
    let bytes = Response { id: 5, body }.encode();
    Response::decode(bytes).expect("decodes").body
}

fn designed(i: u32) -> DesignedWidget {
    DesignedWidget {
        name: format!("f{i}"),
        kind: DesignKind::Combo,
        widget: i,
        widgets: 3,
        rect: [1.0, 2.0, 30.0, 40.0],
        export: "x".into(),
        editable: i.is_multiple_of(2),
        props: props(),
    }
}

#[test]
fn every_designer_request_comes_back_the_same() {
    let requests = [
        AuthorRequest::Check { doc: 3 },
        AuthorRequest::List { doc: 3, page: 9 },
        AuthorRequest::Apply {
            doc: 3,
            ops: vec![
                AuthorOp::Add(field(DesignKind::Text)),
                AuthorOp::Add(field(DesignKind::Signature)),
                AuthorOp::Remove { name: "a".into() },
                AuthorOp::RemoveWidget {
                    name: "a".into(),
                    widget: 2,
                },
                AuthorOp::AddWidget {
                    name: "group".into(),
                    widget: widget(1),
                },
                AuthorOp::Move {
                    name: "a".into(),
                    widget: 1,
                    page: 4,
                    rect: [5.0, 6.0, 70.0, 80.0],
                },
                AuthorOp::Update {
                    name: "a".into(),
                    new_name: "b".into(),
                    props: props(),
                    exports: vec!["S".into(), "M".into(), "L".into()],
                },
                AuthorOp::Reorder {
                    page: 0,
                    order: vec![
                        WidgetRef {
                            name: "b".into(),
                            widget: 0,
                        },
                        WidgetRef {
                            name: "a".into(),
                            widget: 1,
                        },
                    ],
                },
            ],
        },
    ];
    for request in requests {
        let body = RequestBody::Author(request);
        assert_eq!(request_back(body.clone()), body);
    }
}

#[test]
fn every_designer_answer_comes_back_the_same() {
    let ready = ResponseBody::Author(AuthorResponse::Ready {
        names: vec!["a".into(), "\u{c5}land".into()],
    });
    assert_eq!(response_back(ready.clone()), ready);
    let widgets = ResponseBody::Author(AuthorResponse::Widgets {
        items: vec![designed(0), designed(1)],
    });
    assert_eq!(response_back(widgets.clone()), widgets);
    let empty = ResponseBody::Author(AuthorResponse::Widgets { items: vec![] });
    assert_eq!(response_back(empty.clone()), empty);
}

#[test]
fn the_encoder_keeps_the_lists_to_their_caps() {
    let ops: Vec<AuthorOp> = (0..MAX_AUTHOR_OPS + 20)
        .map(|i| AuthorOp::Remove {
            name: format!("f{i}"),
        })
        .collect();
    let RequestBody::Author(AuthorRequest::Apply { ops, .. }) =
        request_back(RequestBody::Author(AuthorRequest::Apply { doc: 1, ops }))
    else {
        panic!("not an apply");
    };
    assert_eq!(ops.len(), MAX_AUTHOR_OPS);

    let mut big = field(DesignKind::Radio);
    big.widgets = (0..MAX_AUTHOR_WIDGETS + 5).map(|_| widget(0)).collect();
    big.props.choices = (0..MAX_FORM_OPTIONS + 5).map(|i| format!("c{i}")).collect();
    big.props.choices[0] = "x".repeat(MAX_FORM_OPTION_LABEL + 50);
    let RequestBody::Author(AuthorRequest::Apply { ops, .. }) =
        request_back(RequestBody::Author(AuthorRequest::Apply {
            doc: 1,
            ops: vec![AuthorOp::Add(big)],
        }))
    else {
        panic!("not an apply");
    };
    let AuthorOp::Add(back) = &ops[0] else {
        panic!("not an add");
    };
    assert_eq!(back.widgets.len(), MAX_AUTHOR_WIDGETS);
    assert_eq!(back.props.choices.len(), MAX_FORM_OPTIONS);
    assert!(back.props.choices[0].len() <= MAX_FORM_OPTION_LABEL);

    let items: Vec<DesignedWidget> = (0..MAX_LISTED_WIDGETS as u32 + 10).map(designed).collect();
    let ResponseBody::Author(AuthorResponse::Widgets { items }) =
        response_back(ResponseBody::Author(AuthorResponse::Widgets { items }))
    else {
        panic!("not widgets");
    };
    assert_eq!(items.len(), MAX_LISTED_WIDGETS);
}

/// The bytes of an `Apply` with one change, to cut and alter.
fn apply_bytes(op: AuthorOp) -> Vec<u8> {
    Request {
        id: 1,
        body: RequestBody::Author(AuthorRequest::Apply {
            doc: 1,
            ops: vec![op],
        }),
    }
    .encode()
}

#[test]
fn the_decoder_refuses_what_is_over_a_cap_or_not_known() {
    // A count of changes over the cap, with no changes behind it.
    let mut bytes = Request {
        id: 1,
        body: RequestBody::Author(AuthorRequest::Apply {
            doc: 1,
            ops: vec![],
        }),
    }
    .encode();
    let at = bytes.len() - 4;
    bytes[at..].copy_from_slice(&(MAX_AUTHOR_OPS as u32 + 1).to_le_bytes());
    assert!(Request::decode(&bytes).is_err());

    // A count of changes that the bytes cannot hold.
    let mut bytes = Request {
        id: 1,
        body: RequestBody::Author(AuthorRequest::Apply {
            doc: 1,
            ops: vec![],
        }),
    }
    .encode();
    let at = bytes.len() - 4;
    bytes[at..].copy_from_slice(&(MAX_AUTHOR_OPS as u32).to_le_bytes());
    assert!(Request::decode(&bytes).is_err());

    // A kind that is not one.
    let mut bytes = apply_bytes(AuthorOp::Add(DesignField {
        name: "n".into(),
        kind: DesignKind::Text,
        props: DesignProps::default(),
        widgets: vec![],
    }));
    // tag, id, doc, count, op tag, name length, name, then the kind.
    let kind_at = 1 + 4 + 4 + 4 + 1 + 4 + 1;
    assert_eq!(bytes[kind_at], DesignKind::Text as u8);
    bytes[kind_at] = 77;
    assert!(Request::decode(&bytes).is_err());

    // A change that is not one.
    let mut bytes = apply_bytes(AuthorOp::Remove { name: "n".into() });
    bytes[1 + 4 + 4 + 4] = 99;
    assert!(Request::decode(&bytes).is_err());

    // A box that is not a number.
    let mut bytes = apply_bytes(AuthorOp::Move {
        name: "n".into(),
        widget: 0,
        page: 0,
        rect: [1.0, 2.0, 3.0, 4.0],
    });
    let rect_at = bytes.len() - 16;
    bytes[rect_at..rect_at + 4].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(Request::decode(&bytes).is_err());
    let mut bytes = apply_bytes(AuthorOp::Move {
        name: "n".into(),
        widget: 0,
        page: 0,
        rect: [1.0, 2.0, 3.0, 4.0],
    });
    let rect_at = bytes.len() - 16;
    bytes[rect_at..rect_at + 4].copy_from_slice(&(MAX_BOX_VALUE * 2.0).to_le_bytes());
    assert!(Request::decode(&bytes).is_err());

    // A flag byte with a bit that means nothing.
    let mut bytes = apply_bytes(AuthorOp::Update {
        name: "n".into(),
        new_name: "n".into(),
        props: DesignProps::default(),
        exports: vec![],
    });
    // tag, id, doc, count, op tag, two names of one byte, the tooltip (empty), then the flags.
    let flags_at = 1 + 4 + 4 + 4 + 1 + 5 + 5 + 4;
    bytes[flags_at + 1] = 0x80;
    assert!(Request::decode(&bytes).is_err());

    // A font size that the encoder would change is refused, so that a request has one encoding.
    for bad in [f32::NAN, f32::INFINITY, -1.0, -1.0e-36] {
        let mut bytes = apply_bytes(AuthorOp::Update {
            name: "n".into(),
            new_name: "n".into(),
            props: DesignProps::default(),
            exports: vec![],
        });
        // The flags are two bytes, the longest text 4 bytes, then the size.
        let size_at = flags_at + 2 + 4;
        bytes[size_at..size_at + 4].copy_from_slice(&bad.to_le_bytes());
        assert!(Request::decode(&bytes).is_err(), "{bad}");
    }

    // An answer with more widgets than it can hold.
    let mut bytes = Response {
        id: 1,
        body: ResponseBody::Author(AuthorResponse::Widgets { items: vec![] }),
    }
    .encode();
    let at = bytes.len() - 4;
    bytes[at..].copy_from_slice(&(MAX_LISTED_WIDGETS as u32 + 1).to_le_bytes());
    assert!(Response::decode(bytes).is_err());
}

/// What the engine lists comes from a file, so every part of an answer can hold a value no one
/// meant. Whatever the encoder writes, the decoder has to accept (the app takes an answer it cannot
/// decode for a broken engine and ends it).
#[test]
fn an_answer_the_encoder_writes_is_one_the_decoder_accepts() {
    let odd_boxes: [[f32; 4]; 6] = [
        [0.0, 0.0, 1.0e9, 20.0],
        [-1.0e9, -1.0e30, 5.0, 5.0],
        [f32::NAN, 0.0, 1.0, 1.0],
        [f32::INFINITY, f32::NEG_INFINITY, 1.0, 1.0],
        [f32::MAX, f32::MIN, 0.0, 0.0],
        [MAX_BOX_VALUE, -MAX_BOX_VALUE, 0.0, 0.0],
    ];
    let mut items = Vec::new();
    for (i, rect) in odd_boxes.iter().enumerate() {
        let mut w = designed(i as u32);
        w.rect = *rect;
        // A size of the text and strings that are longer than the caps.
        w.props.font_size = [-3.0, f32::NAN, f32::INFINITY, -0.0, 1.0e30, 12.0][i];
        w.props.tooltip = "t\u{e6}".repeat(MAX_FORM_TOOLTIP);
        w.props.default_value = "d\u{f8}".repeat(MAX_FORM_VALUE);
        w.props.choices = (0..MAX_FORM_OPTIONS + 40)
            .map(|n| format!("{n}\u{e5}").repeat(MAX_FORM_OPTION_LABEL))
            .collect();
        w.export = "e\u{e5}".repeat(MAX_EXPORT_VALUE);
        w.name = "n\u{e6}".repeat(MAX_FORM_NAME);
        items.push(w);
    }
    let bytes = Response {
        id: 7,
        body: ResponseBody::Author(AuthorResponse::Widgets { items }),
    }
    .encode();
    let back = Response::decode(bytes).expect("what was encoded decodes");
    match back.body {
        ResponseBody::Author(AuthorResponse::Widgets { items }) => {
            assert_eq!(items.len(), odd_boxes.len());
            for w in &items {
                assert!(w
                    .rect
                    .iter()
                    .all(|v| v.is_finite() && v.abs() <= MAX_BOX_VALUE));
                assert!(w.props.font_size.is_finite() && w.props.font_size >= 0.0);
                assert!(w.props.choices.len() <= MAX_FORM_OPTIONS);
            }
            // A box in range is not changed.
            assert_eq!(items[5].rect, [MAX_BOX_VALUE, -MAX_BOX_VALUE, 0.0, 0.0]);
            assert_eq!(items[0].rect, [0.0, 0.0, MAX_BOX_VALUE, 20.0]);
        }
        other => panic!("{other:?}"),
    }
    // The names a document has: more than the cap, and names longer than the cap.
    let names: Vec<String> = (0..MAX_AUTHOR_NAMES + 10)
        .map(|n| format!("{n}\u{e6}").repeat(MAX_FORM_NAME))
        .collect();
    let bytes = Response {
        id: 8,
        body: ResponseBody::Author(AuthorResponse::Ready { names }),
    }
    .encode();
    match Response::decode(bytes)
        .expect("what was encoded decodes")
        .body
    {
        ResponseBody::Author(AuthorResponse::Ready { names }) => {
            assert_eq!(names.len(), MAX_AUTHOR_NAMES);
            assert!(names.iter().all(|n| n.len() <= MAX_FORM_NAME));
        }
        other => panic!("{other:?}"),
    }
    // A request that was made with a place nobody can use is cut too.
    let request = Request {
        id: 1,
        body: RequestBody::Author(AuthorRequest::Apply {
            doc: 1,
            ops: vec![AuthorOp::Move {
                name: "a".into(),
                widget: 0,
                page: 0,
                rect: [f32::NAN, 1.0e12, -1.0e12, 3.0],
            }],
        }),
    }
    .encode();
    assert!(Request::decode(&request).is_ok());
}
