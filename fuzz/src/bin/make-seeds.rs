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
    ability, field_flag, AnnotInfo, AnnotKind, ErrorCode, FontScript, FormField, FormFieldKind,
    FormKind, FormOption, FormValue, LinkTarget, OutlineItem, PageLink, PageSize, Request,
    RequestBody, Response, ResponseBody, StampKind, TextMatch, MAX_FRAME_LEN, MAX_LINKS_PER_PAGE,
    MAX_MATCHES_PER_PAGE,
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
        (
            "open",
            b(
                2,
                RequestBody::Open { password: None },
            ),
        ),
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
        ("list_annots", b(15, RequestBody::ListAnnots { doc: 1, page: 0 })),
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
            b(21, RequestBody::RemoveAnnot { doc: 1, page: 0, id: 3 }),
        ),
        (
            "restore_annot",
            b(22, RequestBody::RestoreAnnot { doc: 1, page: 0, id: 3 }),
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
                    value: FormValue::Text("Ada
Lovelace".into()),
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
    ]
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
        ("save_ready", b(11, ResponseBody::SaveReady { size: 70_000 })),
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
        ("progress", b(15, ResponseBody::Progress { done: 1_048_576 })),
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
        ("annot", b(18, ResponseBody::Annot { item: a_note(3, "") })),
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
Lovelace".into(),
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

    // Decoders: one valid message of every kind.
    for (name, request) in requests() {
        write(&dir, "protocol_request_decode", name, &request.encode());
    }
    for (name, response) in responses() {
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
    for kind in 0..25u8 {
        write(
            &dir,
            "protocol_roundtrip_request",
            &format!("kind{kind}"),
            &generator_input(kind, kind as u32),
        );
    }
    for kind in 0..21u8 {
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

    // The metadata check: files in the form PDFium writes (a classic table, `\r\n` line ends), a
    // protected one with an XMP stream that is meant to be encrypted, one that says it is not, and
    // the shapes that a search for words would be fooled by.
    let shaped = |objects: &[&str], trailer: &str| -> Vec<u8> {
        let mut out = b"%PDF-1.7\r\n%\xA1\xB3\xC5\xD7\r\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            out.extend_from_slice(b"\r\n");
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\r\n{body}\r\nendobj\r\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\r\n0 {}\r\n0000000000 65535 f\r\n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n\r\n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\r\n<<{trailer}>>\r\nstartxref\r\n{xref}\r\n%%EOF\r\n").as_bytes(),
        );
        out
    };
    let xmp = "<</Length 12/Subtype/XML/Type/Metadata>>stream\r\n<x:xmpmeta/>\r\nendstream";
    let catalog = "<</Metadata 4 0 R /Pages 3 0 R /Type/Catalog>>";
    let pages = "<</Type/Pages>>";
    let trailer = "/Root 1 0 R /Encrypt 2 0 R /Size 5";
    let metadata_seeds: Vec<(&str, Vec<u8>)> = vec![
        (
            "protected_xmp",
            shaped(&[catalog, "<</Filter/Standard/V 4>>", pages, xmp], trailer),
        ),
        (
            "protected_clear_xmp",
            shaped(
                &[catalog, "<</EncryptMetadata false/Filter/Standard>>", pages, xmp],
                trailer,
            ),
        ),
        (
            "clear_not_xml",
            shaped(
                &[
                    catalog,
                    "<</EncryptMetadata false/Filter/Standard>>",
                    pages,
                    "<</Length 5/Subtype/XML/Type/Metadata>>stream\r\nx\u{9c}abc\r\nendstream",
                ],
                trailer,
            ),
        ),
        (
            "two_streams",
            shaped(
                &[catalog, "<</EncryptMetadata false/Filter/Standard>>", pages, xmp, xmp],
                trailer,
            ),
        ),
        (
            "unprotected_xmp",
            shaped(&[catalog, xmp, pages], "/Root 1 0 R /Size 4"),
        ),
        (
            "name_like_keyword",
            shaped(
                &[
                    catalog,
                    "<</Filter/Standard/V 4>>",
                    pages,
                    "<</A/stream/Length 12/Subtype/XML/Type/Metadata>>stream\r\n<x:xmpmeta/>\r\nendstream",
                ],
                trailer,
            ),
        ),
        (
            "nested_encrypt_metadata",
            shaped(
                &[
                    catalog,
                    "<</Filter/Standard/CF<</StdCF<</EncryptMetadata false>>>>/Note(/EncryptMetadata false)>>",
                    pages,
                    xmp,
                ],
                trailer,
            ),
        ),
        (
            "trailer_in_array",
            shaped(
                &[catalog, "<</EncryptMetadata false/Filter/Standard>>", pages, xmp],
                "/AAA[/Encrypt 5 0 R]/Root 1 0 R /Encrypt 2 0 R /Size 5",
            ),
        ),
        (
            "deep",
            shaped(
                &[
                    catalog,
                    &format!(
                        "<</EncryptMetadata false/X{}{}>>",
                        "[".repeat(40),
                        "]".repeat(40)
                    ),
                    pages,
                    xmp,
                ],
                trailer,
            ),
        ),
    ];
    for (name, bytes) in &metadata_seeds {
        write(&dir, "metadata_check", &format!("{name}.pdf"), bytes);
    }

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
        // A protected file with an XMP stream: encrypted as the dictionary says (a save is refused),
        // and clear as `/EncryptMetadata false` says (a save keeps it).
        ("encrypted_xmp", kit::encrypted_xmp_pdf("secret", false)),
        ("encrypted_xmp_clear", kit::encrypted_xmp_pdf("secret", true)),
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
    println!("seeds written to {}", dir.display());
}
