//! Writes the seed corpus: `fuzz/seeds/<target>/<name>`. Run it from the `fuzz` folder:
//!
//! ```text
//! cargo run --bin make-seeds -- seeds
//! ```
//!
//! The seeds are made here, from the protocol crate and the PDF generator of the test kit, and
//! committed. Nothing is downloaded. Run it again after the protocol changes and commit the
//! difference.

use std::fs;
use std::path::{Path, PathBuf};

use partridge_fuzz::{
    over_cap_request_case, over_cap_response_case, OVER_CAP_REQUESTS, OVER_CAP_RESPONSES,
};
use partridge_protocol::{
    ability, field_flag, AuthorOp, AuthorRequest, AuthorResponse, DesignField, DesignKind,
    DesignProps, DesignWidget, DesignedWidget, WidgetRef, AnnotInfo, AnnotKind, ErrorCode, FontScript, FormField, FormFieldKind,
    FormKind, FormOption, FormValue, LinkTarget, OutlineItem, PageLink, PageSize, PagesRequest,
    PagesResponse, Request, RequestBody, Response, ResponseBody, SignatureFont, SignatureMark,
    StampKind, TextMatch, MAX_FRAME_LEN, MAX_LINKS_PER_PAGE, MAX_MATCHES_PER_PAGE,
    MAX_OUTLINE_DEPTH, MAX_OUTLINE_ITEMS, MAX_OUTLINE_TITLE, MAX_QUERY_LEN, MAX_SAVE_BYTES,
    MAX_SAVE_CHUNK, MAX_URI_LEN, PROTOCOL_VERSION,
};
use partridge_testkit as kit;

fn requests() -> Vec<(&'static str, Request)> {
    let b = |id, body| Request { id, body };
    vec![
        (
            "hello",
            b(
                1,
                RequestBody::Hello {
                    protocol_version: PROTOCOL_VERSION,
                },
            ),
        ),
        ("open", b(2, RequestBody::Open { password: None })),
        (
            "open_password",
            b(
                3,
                RequestBody::Open {
                    password: Some("secret".into()),
                },
            ),
        ),
        (
            "render",
            b(
                4,
                RequestBody::Render {
                    doc: 1,
                    page: 0,
                    width: 1224,
                    height: 1584,
                },
            ),
        ),
        ("close", b(5, RequestBody::Close { doc: 1 })),
        ("outline", b(6, RequestBody::Outline { doc: 1 })),
        ("page_text", b(7, RequestBody::PageText { doc: 1, page: 2 })),
        (
            "page_links",
            b(8, RequestBody::PageLinks { doc: 1, page: 2 }),
        ),
        ("font_report", b(10, RequestBody::FontReport)),
        (
            "signatures",
            b(
                11,
                RequestBody::Signatures {
                    doc: 1,
                    now: 1_781_524_800,
                },
            ),
        ),
        (
            "search",
            b(
                9,
                RequestBody::Search {
                    doc: 1,
                    page: 2,
                    query: "needle".into(),
                },
            ),
        ),
        ("doc_info", b(10, RequestBody::DocInfo { doc: 1 })),
        ("save_begin", b(11, RequestBody::SaveBegin { doc: 1 })),
        (
            "save_chunk",
            b(
                12,
                RequestBody::SaveChunk {
                    doc: 1,
                    offset: MAX_SAVE_CHUNK as u32,
                    len: MAX_SAVE_CHUNK as u32,
                },
            ),
        ),
        ("save_end", b(13, RequestBody::SaveEnd { doc: 1 })),
        ("fingerprint", b(14, RequestBody::Fingerprint { doc: 1 })),
        (
            "list_annots",
            b(15, RequestBody::ListAnnots { doc: 1, page: 0 }),
        ),
        (
            "add_markup",
            b(
                16,
                RequestBody::AddMarkup {
                    doc: 1,
                    page: 0,
                    kind: AnnotKind::Highlight,
                    color: [255, 235, 59, 255],
                    start: 4,
                    end: 19,
                },
            ),
        ),
        (
            "add_note",
            b(
                17,
                RequestBody::AddNote {
                    doc: 1,
                    page: 0,
                    x: 72.0,
                    y: 100.5,
                    color: [255, 235, 59, 255],
                    text: "A note \u{2713}".into(),
                },
            ),
        ),
        (
            "add_ink",
            b(
                18,
                RequestBody::AddInk {
                    doc: 1,
                    page: 0,
                    color: [0, 0, 255, 255],
                    width: 2.5,
                    strokes: vec![vec![[10.0, 10.0], [20.0, 30.0]], vec![[5.0, 5.0]]],
                },
            ),
        ),
        (
            "add_stamp",
            b(
                19,
                RequestBody::AddStamp {
                    doc: 1,
                    page: 0,
                    x: 100.0,
                    y: 200.0,
                    stamp: StampKind::Approved,
                },
            ),
        ),
        (
            "update_annot",
            b(
                20,
                RequestBody::UpdateAnnot {
                    doc: 1,
                    page: 0,
                    id: 3,
                    text: Some("changed".into()),
                    color: Some([1, 2, 3, 255]),
                    dx: 4.0,
                    dy: -4.0,
                },
            ),
        ),
        (
            "remove_annot",
            b(
                21,
                RequestBody::RemoveAnnot {
                    doc: 1,
                    page: 0,
                    id: 3,
                },
            ),
        ),
        (
            "restore_annot",
            b(
                22,
                RequestBody::RestoreAnnot {
                    doc: 1,
                    page: 0,
                    id: 3,
                },
            ),
        ),
        (
            "add_signature_drawn",
            b(
                26,
                RequestBody::AddSignature {
                    doc: 1,
                    page: 0,
                    rect: [100.0, 100.0, 300.0, 180.0],
                    color: [10, 40, 120, 255],
                    mark: SignatureMark::Strokes {
                        width: 240.0,
                        height: 100.0,
                        line: 4.0,
                        strokes: vec![
                            vec![[10.0, 50.0], [60.0, 20.0], [120.0, 80.0]],
                            vec![[200.0, 60.0]],
                        ],
                    },
                },
            ),
        ),
        (
            "add_signature_typed",
            b(
                27,
                RequestBody::AddSignature {
                    doc: 1,
                    page: 0,
                    rect: [100.0, 100.0, 300.0, 180.0],
                    color: [0, 0, 0, 255],
                    mark: SignatureMark::Typed {
                        font: SignatureFont::GreatVibes,
                        text: "Anna Alvorlig".into(),
                    },
                },
            ),
        ),
        (
            "add_signature_image",
            b(
                28,
                RequestBody::AddSignature {
                    doc: 1,
                    page: 0,
                    rect: [100.0, 100.0, 300.0, 180.0],
                    color: [0, 0, 0, 255],
                    mark: SignatureMark::Image {
                        width: 4,
                        height: 2,
                        rgba: (0..32).map(|i| i as u8 * 8).collect(),
                    },
                },
            ),
        ),
        (
            "place_signature",
            b(
                29,
                RequestBody::PlaceSignature {
                    doc: 1,
                    page: 0,
                    id: 3,
                    rect: [50.0, 60.0, 250.0, 60.0],
                },
            ),
        ),
        ("form_info", b(23, RequestBody::FormInfo { doc: 1 })),
        (
            "list_form_fields",
            b(24, RequestBody::ListFormFields { doc: 1, page: 0 }),
        ),
        (
            "set_form_text",
            b(
                25,
                RequestBody::SetFormField {
                    doc: 1,
                    page: 0,
                    index: 2,
                    name: "fullname".into(),
                    value: FormValue::Text(
                        "Ada
Lovelace"
                            .into(),
                    ),
                },
            ),
        ),
        (
            "set_form_checked",
            b(
                26,
                RequestBody::SetFormField {
                    doc: 1,
                    page: 0,
                    index: 3,
                    name: "agree".into(),
                    value: FormValue::Checked(true),
                },
            ),
        ),
        (
            "set_form_choice",
            b(
                27,
                RequestBody::SetFormField {
                    doc: 1,
                    page: 0,
                    index: 12,
                    name: "toppings".into(),
                    value: FormValue::Choice(vec![0, 2]),
                },
            ),
        ),
        (
            "pages_delete",
            b(
                15,
                RequestBody::Pages(PagesRequest::Delete {
                    doc: 1,
                    pages: vec![0, 4, 5],
                }),
            ),
        ),
        (
            "pages_reorder",
            b(
                16,
                RequestBody::Pages(PagesRequest::Reorder {
                    doc: 1,
                    order: vec![2, 0, 1],
                }),
            ),
        ),
        (
            "pages_rotate",
            b(
                17,
                RequestBody::Pages(PagesRequest::Rotate {
                    doc: 1,
                    pages: vec![1],
                    turns: 3,
                }),
            ),
        ),
        (
            "pages_crop",
            b(
                18,
                RequestBody::Pages(PagesRequest::Crop {
                    doc: 1,
                    pages: vec![0, 1],
                    margins: [10.0, 20.0, 30.0, 40.0],
                }),
            ),
        ),
        (
            "pages_set_boxes",
            b(
                19,
                RequestBody::Pages(PagesRequest::SetBoxes {
                    doc: 1,
                    pages: vec![0],
                    boxes: vec![[0.0, 0.0, 612.0, 792.0]],
                }),
            ),
        ),
        (
            "pages_insert_blank",
            b(
                20,
                RequestBody::Pages(PagesRequest::InsertBlank {
                    doc: 1,
                    at: 2,
                    width: 612.0,
                    height: 792.0,
                }),
            ),
        ),
        (
            "pages_restore",
            b(
                21,
                RequestBody::Pages(PagesRequest::Restore {
                    doc: 1,
                    snapshot: 3,
                }),
            ),
        ),
        (
            "import_begin",
            b(
                22,
                RequestBody::Pages(PagesRequest::ImportBegin { doc: 1, size: 5000 }),
            ),
        ),
        (
            "import_chunk",
            b(
                23,
                RequestBody::Pages(PagesRequest::ImportChunk {
                    doc: 1,
                    offset: 0,
                    data: b"%PDF-1.4".to_vec(),
                }),
            ),
        ),
        (
            "import_end",
            b(
                24,
                RequestBody::Pages(PagesRequest::ImportEnd { doc: 1, at: 0 }),
            ),
        ),
        (
            "import_abort",
            b(25, RequestBody::Pages(PagesRequest::ImportAbort { doc: 1 })),
        ),
        (
            "export_begin",
            b(
                26,
                RequestBody::Pages(PagesRequest::ExportBegin {
                    doc: 1,
                    pages: vec![3, 1],
                }),
            ),
        ),
    ]
}

fn signature_report() -> partridge_protocol::SignatureReport {
    use partridge_protocol::{
        CertificateTime, SignatureInfo, SignatureReason, SignatureReport, SignatureState,
    };
    let item = |index: u32, reason: SignatureReason, signer: &str| SignatureInfo {
        index,
        state: reason.state(),
        reason,
        certificate_time: CertificateTime::Valid,
        sub_filter: "adbe.pkcs7.detached".into(),
        byte_range: vec![0, 100, 300, 50],
        file_len: 350,
        doc_mdp: 2,
        certificates: 2,
        signer: Some(signer.into()),
        organisation: Some("Example Ltd".into()),
        issuer: Some("Example CA".into()),
        serial: Some("01ab".into()),
        fingerprint: Some("ab".repeat(32)),
        valid_from: Some("2026-01-31 12:00 UTC".into()),
        valid_to: Some("2027-01-31 12:00 UTC".into()),
        key: Some("ECDSA P-256".into()),
        digest: Some("SHA-256".into()),
        claimed_time: Some("2026-02-01 08:00:00 +01:00".into()),
        stated_reason: Some("I approve".into()),
        stated_location: Some("Oslo".into()),
        trust_anchor: None,
    };
    let _ = SignatureState::Broken;
    SignatureReport {
        total: 2,
        unlisted: 1,
        trust_anchors: 0,
        items: vec![
            item(0, SignatureReason::NoTrustList, "Test Signer"),
            item(1, SignatureReason::DataAfterSignature, "Second Signer"),
        ],
    }
}

fn a_note(id: u32, text: &str) -> AnnotInfo {
    AnnotInfo {
        id,
        kind: AnnotKind::Note,
        abilities: ability::EDIT_TEXT | ability::MOVE | ability::REMOVE,
        rect: [72.0, 100.0, 92.0, 120.0],
        color: [255, 235, 59, 255],
        text: text.into(),
        author: "Someone".into(),
    }
}

fn author_field() -> DesignField {
    DesignField {
        name: "name".into(),
        kind: DesignKind::Text,
        props: DesignProps {
            tooltip: "Your name".into(),
            required: true,
            max_len: 20,
            default_value: "Ola".into(),
            ..DesignProps::default()
        },
        widgets: vec![DesignWidget {
            page: 0,
            rect: [72.0, 100.0, 232.0, 122.0],
            export: String::new(),
        }],
    }
}

fn author_requests() -> Vec<(&'static str, Request)> {
    let b = |id, body| Request { id, body };
    vec![
        ("author_check", b(60, RequestBody::Author(AuthorRequest::Check { doc: 1 }))),
        ("author_list", b(61, RequestBody::Author(AuthorRequest::List { doc: 1, page: 0 }))),
        (
            "author_apply",
            b(
                62,
                RequestBody::Author(AuthorRequest::Apply {
                    doc: 1,
                    ops: vec![
                        AuthorOp::Add(author_field()),
                        AuthorOp::Move {
                            name: "name".into(),
                            widget: 0,
                            page: 0,
                            rect: [80.0, 110.0, 240.0, 132.0],
                        },
                        AuthorOp::Update {
                            name: "name".into(),
                            new_name: "full_name".into(),
                            props: DesignProps::default(),
                            exports: vec![],
                        },
                        AuthorOp::Reorder {
                            page: 0,
                            order: vec![WidgetRef {
                                name: "full_name".into(),
                                widget: 0,
                            }],
                        },
                        AuthorOp::Remove {
                            name: "full_name".into(),
                        },
                    ],
                }),
            ),
        ),
    ]
}

fn author_responses() -> Vec<(&'static str, Response)> {
    let b = |id, body| Response { id, body };
    vec![
        (
            "author_ready",
            b(
                60,
                ResponseBody::Author(AuthorResponse::Ready {
                    names: vec!["a".into(), "person.first".into()],
                }),
            ),
        ),
        (
            "author_widgets",
            b(
                61,
                ResponseBody::Author(AuthorResponse::Widgets {
                    items: vec![DesignedWidget {
                        name: "size".into(),
                        kind: DesignKind::Radio,
                        widget: 1,
                        widgets: 3,
                        rect: [10.0, 20.0, 24.0, 34.0],
                        export: "M".into(),
                        editable: true,
                        props: DesignProps {
                            default_value: "M".into(),
                            ..DesignProps::default()
                        },
                    }],
                }),
            ),
        ),
    ]
}

fn responses() -> Vec<(&'static str, Response)> {
    let b = |id, body| Response { id, body };
    vec![
        (
            "hello",
            b(
                1,
                ResponseBody::Hello {
                    protocol_version: PROTOCOL_VERSION,
                    engine_version: "0.0.0".into(),
                    pdfium_version: "PDFium 8125".into(),
                    sandbox: "seccomp+landlock".into(),
                },
            ),
        ),
        (
            "opened",
            b(
                2,
                ResponseBody::Opened {
                    doc: 1,
                    pages: vec![
                        PageSize {
                            width: 612.0,
                            height: 792.0,
                        },
                        PageSize {
                            width: 842.0,
                            height: 595.0,
                        },
                    ],
                },
            ),
        ),
        (
            "rendered",
            b(
                3,
                ResponseBody::Rendered {
                    width: 3,
                    height: 2,
                    rgba: (0..24).collect(),
                },
            ),
        ),
        ("closed", b(4, ResponseBody::Closed)),
        (
            "outline",
            b(
                5,
                ResponseBody::Outline {
                    items: vec![
                        OutlineItem {
                            depth: 0,
                            page: Some(0),
                            title: "Chapter 1".into(),
                        },
                        OutlineItem {
                            depth: 1,
                            page: Some(1),
                            title: "Section 1.1".into(),
                        },
                        OutlineItem {
                            depth: 0,
                            page: None,
                            title: "External \u{2713}".into(),
                        },
                    ],
                },
            ),
        ),
        (
            "page_text",
            b(
                6,
                ResponseBody::PageText {
                    text: "Hi \u{e5}!".into(),
                    rects: vec![
                        [72.0, 700.0, 80.0, 712.0],
                        [80.0, 700.0, 84.0, 712.0],
                        [84.0, 700.0, 90.0, 712.0],
                        [90.0, 700.0, 95.0, 712.0],
                    ],
                },
            ),
        ),
        (
            "page_links",
            b(
                7,
                ResponseBody::PageLinks {
                    links: vec![
                        PageLink {
                            rect: [72.0, 100.0, 200.0, 120.0],
                            target: LinkTarget::Uri("https://example.org/a?b=c".into()),
                        },
                        PageLink {
                            rect: [72.0, 130.0, 200.0, 150.0],
                            target: LinkTarget::Page(2),
                        },
                    ],
                },
            ),
        ),
        (
            "matches",
            b(
                8,
                ResponseBody::Matches {
                    matches: vec![
                        TextMatch { start: 2, len: 6 },
                        TextMatch { start: 40, len: 6 },
                    ],
                },
            ),
        ),
        (
            "font_report",
            b(
                11,
                ResponseBody::FontReport {
                    missed: vec![FontScript::Japanese, FontScript::Thai],
                },
            ),
        ),
        (
            "signatures",
            b(
                13,
                ResponseBody::Signatures {
                    report: signature_report(),
                },
            ),
        ),
        (
            "error",
            b(
                9,
                ResponseBody::Error {
                    code: ErrorCode::Corrupt,
                    message: "the file is not a valid PDF or is damaged".into(),
                },
            ),
        ),
        (
            "doc_info",
            b(
                10,
                ResponseBody::DocInfo {
                    pages: 12,
                    security: 4,
                    signatures: 1,
                    maybe_signed: true,
                    touched: false,
                },
            ),
        ),
        (
            "save_ready",
            b(11, ResponseBody::SaveReady { size: 70_000 }),
        ),
        (
            "save_data",
            b(
                12,
                ResponseBody::SaveData {
                    offset: 65_536,
                    data: (0..=255).collect(),
                },
            ),
        ),
        ("save_ended", b(13, ResponseBody::SaveEnded)),
        (
            "progress",
            b(15, ResponseBody::Progress { done: 1_048_576 }),
        ),
        (
            "fingerprint",
            b(
                16,
                ResponseBody::Fingerprint {
                    pages: 12,
                    failed: 0,
                    digest: 0x0123_4567_89ab_cdef,
                },
            ),
        ),
        (
            "annots",
            b(
                17,
                ResponseBody::Annots {
                    items: vec![a_note(1, "first"), a_note(2, "<b>second</b> \u{2713}")],
                },
            ),
        ),
        (
            "annot",
            b(
                18,
                ResponseBody::Annot {
                    item: a_note(3, ""),
                },
            ),
        ),
        ("annot_removed", b(19, ResponseBody::AnnotRemoved)),
        (
            "error_no_such_annotation",
            b(
                20,
                ResponseBody::Error {
                    code: ErrorCode::NoSuchAnnotation,
                    message: "the annotation is not on the page any more".into(),
                },
            ),
        ),
        (
            "form_info",
            b(
                21,
                ResponseBody::FormInfo {
                    kind: FormKind::Acro,
                    has_actions: true,
                },
            ),
        ),
        (
            "form_fields",
            b(
                22,
                ResponseBody::FormFields {
                    fields: vec![
                        FormField {
                            index: 0,
                            kind: FormFieldKind::Text,
                            flags: field_flag::REQUIRED | field_flag::MULTILINE,
                            turns: 1,
                            rect: [150.0, 68.0, 400.0, 92.0],
                            font_size: 12.0,
                            name: "person.first".into(),
                            tooltip: "First name".into(),
                            value: "Ada
Lovelace"
                                .into(),
                            max_len: 40,
                            options: vec![],
                        },
                        FormField {
                            index: 1,
                            kind: FormFieldKind::Combo,
                            flags: field_flag::EDITABLE,
                            turns: 0,
                            rect: [150.0, 108.0, 350.0, 130.0],
                            font_size: 0.0,
                            name: "country".into(),
                            tooltip: String::new(),
                            value: "Norway".into(),
                            max_len: 0,
                            options: vec![
                                FormOption {
                                    selected: true,
                                    label: "Norway".into(),
                                },
                                FormOption {
                                    selected: false,
                                    label: "Sweden".into(),
                                },
                            ],
                        },
                    ],
                },
            ),
        ),
        (
            "error_save_too_large",
            b(
                14,
                ResponseBody::Error {
                    code: ErrorCode::SaveTooLarge,
                    message: "the document is too big to be saved".into(),
                },
            ),
        ),
        (
            "pages_changed",
            b(
                30,
                ResponseBody::Pages(PagesResponse::Changed {
                    pages: vec![
                        PageSize {
                            width: 612.0,
                            height: 792.0,
                        };
                        3
                    ],
                    snapshot: 2,
                    count: 1,
                    form_fields: 1,
                }),
            ),
        ),
        (
            "pages_cropped",
            b(
                31,
                ResponseBody::Pages(PagesResponse::Cropped {
                    pages: vec![PageSize {
                        width: 500.0,
                        height: 700.0,
                    }],
                    before: vec![[0.0, 0.0, 612.0, 792.0]],
                    after: vec![[10.0, 10.0, 510.0, 710.0]],
                }),
            ),
        ),
        (
            "import_ready",
            b(32, ResponseBody::Pages(PagesResponse::ImportReady)),
        ),
        (
            "import_ack",
            b(
                33,
                ResponseBody::Pages(PagesResponse::ImportAck { received: 8 }),
            ),
        ),
        (
            "exported",
            b(
                34,
                ResponseBody::Pages(PagesResponse::Exported {
                    size: 4096,
                    pages: 2,
                    signatures: 0,
                    source_security: 3,
                    digest: 0xAB,
                }),
            ),
        ),
        (
            "pages_done",
            b(35, ResponseBody::Pages(PagesResponse::Done)),
        ),
        (
            "error_too_many_pages",
            b(
                36,
                ResponseBody::Error {
                    code: ErrorCode::TooManyPages,
                    message: "the document has more pages than the page tools take".into(),
                },
            ),
        ),
    ]
}

/// A frame: the length, then the payload.
fn framed(payload: &[u8]) -> Vec<u8> {
    let mut out = (payload.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(payload);
    out
}

/// Input of the `protocol_read_frame` target: limit (2 bytes), number of pieces (1 byte), the
/// piece sizes, then the stream.
fn read_frame_input(limit: u16, pieces: &[u8], stream: &[u8]) -> Vec<u8> {
    let mut out = limit.to_le_bytes().to_vec();
    out.push(pieces.len() as u8);
    out.extend_from_slice(pieces);
    out.extend_from_slice(stream);
    out
}

/// Bytes for the generators of the round-trip targets: a byte that keeps the target from taking
/// its over-the-cap branch (7 makes the one-in-eight draw come out false), four bytes for the id,
/// the byte that picks the kind of message, then bytes from a small fixed generator (no
/// randomness: the same files come out every time).
fn generator_input(kind: u8, salt: u32) -> Vec<u8> {
    let mut state = 0x9E37_79B9u32 ^ salt.wrapping_mul(0x85EB_CA6B);
    let mut out = vec![7, 1, 2, 3, 4, kind];
    for _ in 0..96 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        out.push((state >> 24) as u8);
    }
    out
}

/// Replaces every `needle` in `hay` by `with`, which has the same length.
fn replace_same_length(hay: &[u8], needle: &[u8], with: &[u8]) -> Vec<u8> {
    assert_eq!(needle.len(), with.len());
    let mut out = hay.to_vec();
    let mut i = 0;
    while i + needle.len() <= out.len() {
        if &out[i..i + needle.len()] == needle {
            out[i..i + needle.len()].copy_from_slice(with);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

/// The features document with every destination that names a page object (`[4 0 R /Fit]`) changed
/// to name a page number (`[77777 /Fit]`), which PDF allows in a link and PDFium takes as it is.
/// `page` must have five characters, so that the file keeps its length.
fn hostile_destinations(features: &[u8], page: &str) -> Vec<u8> {
    let mut out = features.to_vec();
    for object in ["4", "6", "8"] {
        out = replace_same_length(
            &out,
            format!("[{object} 0 R /Fit]").as_bytes(),
            format!("[{page} /Fit]").as_bytes(),
        );
    }
    assert_ne!(out, features, "no destination was changed");
    out
}

fn write(dir: &Path, target: &str, name: &str, bytes: &[u8]) {
    let dir = dir.join(target);
    fs::create_dir_all(&dir).expect("create seed folder");
    fs::write(dir.join(name), bytes).expect("write seed");
}

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "seeds".to_string()),
    );

    // Signing with a certificate: messages, certificate files and files that PDFium wrote.
    partridge_fuzz::write_sign_seeds(&dir);

    // Decoders: one valid message of every kind.
    for (name, request) in requests().into_iter().chain(author_requests()) {
        write(&dir, "protocol_request_decode", name, &request.encode());
    }
    for (name, response) in responses().into_iter().chain(author_responses()) {
        write(&dir, "protocol_response_decode", name, &response.encode());
    }

    // Frame reader: all requests in one stream, all responses in one stream, and the same
    // streams with a small limit, in small pieces, with interrupted reads, and cut short.
    let request_stream: Vec<u8> = requests()
        .iter()
        .flat_map(|(_, r)| framed(&r.encode()))
        .collect();
    let response_stream: Vec<u8> = responses()
        .iter()
        .flat_map(|(_, r)| framed(&r.encode()))
        .collect();
    let target = "protocol_read_frame";
    write(
        &dir,
        target,
        "requests",
        &read_frame_input(0, &[], &request_stream),
    );
    write(
        &dir,
        target,
        "responses",
        &read_frame_input(0, &[], &response_stream),
    );
    write(
        &dir,
        target,
        "responses_pieces",
        &read_frame_input(0, &[1, 3, 0, 7, 0, 2], &response_stream),
    );
    write(
        &dir,
        target,
        "requests_limit",
        &read_frame_input(20, &[5, 0, 1], &request_stream),
    );
    write(
        &dir,
        target,
        "responses_cut",
        &read_frame_input(0, &[4], &response_stream[..response_stream.len() - 5]),
    );
    write(
        &dir,
        target,
        "huge_announced",
        &read_frame_input(0, &[], &((MAX_FRAME_LEN as u32) + 1).to_le_bytes()),
    );
    write(
        &dir,
        target,
        "announced_but_missing",
        &read_frame_input(0, &[2], &[0, 0, 0, 8, 1, 2, 3]),
    );
    write(&dir, target, "empty", &read_frame_input(0, &[], &[]));
    // A frame bigger than the first chunk the reader makes room for (64 KiB), whole and cut.
    let big = Response {
        id: 1,
        body: ResponseBody::Rendered {
            width: 130,
            height: 130,
            rgba: (0..130 * 130 * 4).map(|i| (i % 253) as u8).collect(),
        },
    };
    let big_stream = framed(&big.encode());
    write(
        &dir,
        target,
        "big_frame",
        &read_frame_input(0, &[], &big_stream),
    );
    write(
        &dir,
        target,
        "big_frame_pieces",
        &read_frame_input(0, &[200, 0, 255, 17], &big_stream),
    );
    // 128 MiB announced, 70 000 bytes sent: the reader must not make room for the 128 MiB.
    let mut announced = (MAX_FRAME_LEN as u32).to_le_bytes().to_vec();
    announced.extend(std::iter::repeat_n(7u8, 70_000));
    write(
        &dir,
        target,
        "huge_announced_big_sent",
        &read_frame_input(0, &[], &announced),
    );

    // Round trips: one input per kind of message.
    for kind in 0..29u8 {
        write(
            &dir,
            "protocol_roundtrip_request",
            &format!("kind{kind}"),
            &generator_input(kind, kind as u32),
        );
    }
    for kind in 0..23u8 {
        write(
            &dir,
            "protocol_roundtrip_response",
            &format!("kind{kind}"),
            &generator_input(kind, 100 + kind as u32),
        );
    }

    // Messages that are one over a cap (the decoder must refuse them), and a few that sit exactly
    // on one. The page text cases are 1.7 MB and are left to the round-trip target.
    for case in 0..OVER_CAP_REQUESTS {
        let (name, payload) = over_cap_request_case(case, 1);
        // Not the picture with a row too many (2 MB): the unit tests of the fuzz crate decode it.
        if payload.len() > 100_000 {
            continue;
        }
        write(
            &dir,
            "protocol_request_decode",
            &format!("over_cap_{}", name.replace(' ', "_")),
            &payload,
        );
    }
    for case in 0..OVER_CAP_RESPONSES {
        let (name, payload, _) = over_cap_response_case(case, 1);
        // Not the page text (1.7 MB) and not the second long string (the same code as the first).
        if payload.len() > 100_000 || name.starts_with("error text") {
            continue;
        }
        write(
            &dir,
            "protocol_response_decode",
            &format!("cap_{}", name.replace(' ', "_")),
            &payload,
        );
    }
    let at_cap = |name: &str, body| {
        write(
            &dir,
            "protocol_response_decode",
            name,
            &Response { id: 1, body }.encode(),
        );
    };
    at_cap(
        "cap_outline_of_the_most_entries",
        ResponseBody::Outline {
            items: (0..MAX_OUTLINE_ITEMS)
                .map(|i| OutlineItem {
                    depth: (i % 16) as u8,
                    page: Some(i as u32),
                    title: String::new(),
                })
                .collect(),
        },
    );
    at_cap(
        "cap_outline_title_of_the_most_bytes",
        ResponseBody::Outline {
            items: vec![OutlineItem {
                depth: MAX_OUTLINE_DEPTH,
                page: None,
                title: "a".repeat(MAX_OUTLINE_TITLE),
            }],
        },
    );
    at_cap(
        "cap_links_of_the_most",
        ResponseBody::PageLinks {
            links: (0..MAX_LINKS_PER_PAGE)
                .map(|i| PageLink {
                    rect: [1.0, 2.0, 3.0, 4.0],
                    target: LinkTarget::Page(i as u32),
                })
                .collect(),
        },
    );
    at_cap(
        "cap_link_address_of_the_most_bytes",
        ResponseBody::PageLinks {
            links: vec![PageLink {
                rect: [1.0, 2.0, 3.0, 4.0],
                target: LinkTarget::Uri("a".repeat(MAX_URI_LEN)),
            }],
        },
    );
    at_cap(
        "cap_matches_of_the_most",
        ResponseBody::Matches {
            matches: (0..MAX_MATCHES_PER_PAGE)
                .map(|i| TextMatch {
                    start: i as u32 * 2,
                    len: 1,
                })
                .collect(),
        },
    );
    at_cap(
        "cap_save_size_of_the_biggest_file",
        ResponseBody::SaveReady {
            size: MAX_SAVE_BYTES as u32,
        },
    );
    at_cap(
        "cap_save_piece_at_the_end_of_the_biggest_file",
        ResponseBody::SaveData {
            offset: MAX_SAVE_BYTES as u32 - 64,
            data: vec![b'z'; 64],
        },
    );
    write(
        &dir,
        "protocol_request_decode",
        "cap_save_piece_of_the_longest_length",
        &Request {
            id: 1,
            body: RequestBody::SaveChunk {
                doc: 1,
                offset: 0,
                len: MAX_SAVE_CHUNK as u32,
            },
        }
        .encode(),
    );
    write(
        &dir,
        "protocol_request_decode",
        "cap_search_text_of_the_most_bytes",
        &Request {
            id: 1,
            body: RequestBody::Search {
                doc: 1,
                page: 0,
                query: "a".repeat(MAX_QUERY_LEN),
            },
        }
        .encode(),
    );

    // Engine: PDF files from the test kit. Small ones, so the fuzzer can change them quickly.
    let target = "engine_open_render";
    // Documents whose destinations are page numbers that do not exist: a link and outline
    // entries that go to page 77 777 of 3, and to a negative page. `[8 0 R /Fit]` and
    // `[77777 /Fit]` have the same length, so the file offsets stay right. The engine must report
    // such links and entries without a page (or not at all).
    let features = kit::features_pdf();
    let pdfs: Vec<(&str, Vec<u8>)> = vec![
        (
            "destinations_past_the_end",
            hostile_destinations(&features, "77777"),
        ),
        (
            "destinations_negative",
            hostile_destinations(&features, "-1111"),
        ),
        ("annotated", kit::annotated_pdf()),
        ("annotations_hostile", kit::hostile_annots_pdf()),
        ("annotations_many", kit::many_annots_pdf(24)),
        ("simple", kit::simple_pdf(1)),
        ("two_pages", kit::simple_pdf(2)),
        ("mixed_sizes", kit::mixed_size_pdf()),
        ("javascript", kit::javascript_pdf()),
        ("features", kit::features_pdf()),
        ("outline_loop", kit::outline_loop_pdf()),
        ("outline_wide", kit::outline_wide_pdf(20)),
        ("outline_deep", kit::outline_deep_pdf(6)),
        ("links", kit::many_links_pdf(12)),
        // One link more than a page may report (about 89 000 bytes, under the engine target's
        // `-max_len` of 100000).
        ("links_over_the_cap", kit::many_links_pdf(501)),
        ("encrypted", kit::encrypted_pdf("secret", 1)),
        // Documents for the page tools: pages with everything a page tool must keep, a page tree
        // that loops or is very deep, a count that lies, odd turns and odd boxes.
        ("marked", kit::marked_pdf(4)),
        ("circular_tree", kit::circular_tree_pdf()),
        ("deep_tree", kit::deep_tree_pdf(300)),
        ("page_count_lie", kit::page_count_lie_pdf(2_000_000_000)),
        ("odd_rotation", kit::odd_rotation_pdf()),
        ("odd_boxes", kit::odd_boxes_pdf()),
        ("many_blank_pages", kit::blank_pages_pdf(300)),
        ("signed", kit::signed_pdf()),
        // What the page tools had to be made safe against: a copy of pages that points into the
        // other document (a note with its pop-up, a field on two pages, an entry with /First), a
        // text field with the look of its value, a tree whose list of kids ends with an entry that
        // is not there, and a damaged protected file that reads back with no pages.
        ("popup_note", kit::popup_note_pdf()),
        ("shared_field", kit::shared_field_pdf()),
        ("first_key", kit::first_key_pdf()),
        ("form_leftovers", kit::form_leftovers_pdf()),
        ("trailing_null_kid", kit::trailing_null_kid_pdf()),
        ("junk_after_the_kids", kit::junk_after_the_kids_pdf()),
        ("trailing_far_kid", kit::trailing_far_kid_pdf()),
        ("far_object_number", kit::far_object_number_pdf()),
        ("inherited_tree", kit::inherited_tree_pdf()),
        ("root_inherited", kit::root_inherited_pdf()),
        (
            "dangling_resources",
            kit::dangling_resources_pdf("/Resources 900 0 R"),
        ),
        (
            "null_resources",
            kit::dangling_resources_pdf("/Resources null"),
        ),
        ("outside_parent", kit::outside_parent_pdf(true, true)),
        ("turned_root", kit::turned_root_pdf()),
        ("listed_many_times", kit::listed_many_times_pdf(200, 300, 0)),
        ("indirect_kids", kit::indirect_kids_pdf()),
        ("words_in_node", kit::words_in_node_pdf(true)),
        ("object_streams", kit::object_streams_pdf()),
        ("inline_dictionaries", kit::inline_dictionaries_pdf()),
        ("page_count_lost", kit::page_count_lost_pdf()),
        ("mixed_sizes", kit::mixed_size_pdf()),
        ("truncated", kit::truncated_pdf()),
        // Forms: every kind of field, a turned page, XFA, scripts and actions, a signature, fields
        // that sit under a parent, and a page with many fields.
        ("form", kit::forms_pdf()),
        ("form_rotated", kit::forms_rotated_pdf(90)),
        ("form_scripts", kit::forms_script_pdf()),
        ("form_xfa", kit::xfa_pdf()),
        ("form_xfa_full", kit::xfa_full_pdf()),
        ("form_signed", kit::forms_signed_pdf()),
        ("form_nested", kit::forms_nested_pdf()),
        ("form_many", kit::forms_many_pdf(40)),
        ("form_actions", kit::forms_actions_pdf()),
        ("form_harmless_actions", kit::forms_harmless_actions_pdf()),
        ("form_multipage", kit::forms_multipage_pdf(false)),
        ("form_multipage_no_page", kit::forms_multipage_pdf(true)),
        ("form_loop", kit::forms_loop_pdf()),
        ("form_parent_event", kit::forms_parent_event_pdf()),
        (
            "form_kids_actions",
            kit::forms_kids_actions_pdf(Some("/K << /S /ResetForm >>"), kit::KidsParent::Absent),
        ),
        ("form_awkward_values", kit::forms_awkward_values_pdf()),
        ("corrupt", kit::corrupt_pdf()),
    ];
    for (name, bytes) in pdfs {
        write(&dir, target, &format!("{name}.pdf"), &bytes);
    }
    // Git must not change the line endings of binary files.
    fs::write(dir.join(".gitattributes"), "* binary\n").expect("write .gitattributes");

    // The writer of form fields: the first bytes pick the document, the rest is the script.
    let mut state = 0x2545_f491u32;
    for fixture in 0..8u8 {
        let mut script = vec![fixture];
        for _ in 0..160 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            script.push((state >> 24) as u8);
        }
        write(&dir, "form_author", &format!("script_{fixture}"), &script);
    }
    println!("seeds written to {}", dir.display());
}
