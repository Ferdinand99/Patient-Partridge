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

use partridge_protocol::{
    ErrorCode, LinkTarget, OutlineItem, PageLink, PageSize, Request, RequestBody, Response,
    ResponseBody, TextMatch, MAX_FRAME_LEN, PROTOCOL_VERSION,
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
                RequestBody::Open {
                    path: "/home/user/book.pdf".into(),
                    password: None,
                },
            ),
        ),
        (
            "open_password",
            b(
                3,
                RequestBody::Open {
                    path: "C:\\Users\\ferdi\\bok.pdf".into(),
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
            "error",
            b(
                9,
                ResponseBody::Error {
                    code: ErrorCode::Corrupt,
                    message: "the file is not a valid PDF or is damaged".into(),
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

/// Bytes for the generators of the round-trip targets: four bytes for the id, the byte that picks
/// the kind of message, then bytes from a small fixed generator (no randomness: the same files
/// come out every time).
fn generator_input(kind: u8, salt: u32) -> Vec<u8> {
    let mut state = 0x9E37_79B9u32 ^ salt.wrapping_mul(0x85EB_CA6B);
    let mut out = vec![1, 2, 3, 4, kind];
    for _ in 0..96 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        out.push((state >> 24) as u8);
    }
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

    // Round trips: one input per kind of message.
    for kind in 0..8u8 {
        write(
            &dir,
            "protocol_roundtrip_request",
            &format!("kind{kind}"),
            &generator_input(kind, kind as u32),
        );
    }
    for kind in 0..9u8 {
        write(
            &dir,
            "protocol_roundtrip_response",
            &format!("kind{kind}"),
            &generator_input(kind, 100 + kind as u32),
        );
    }

    // Engine: PDF files from the test kit. Small ones, so the fuzzer can change them quickly.
    let target = "engine_open_render";
    let pdfs: Vec<(&str, Vec<u8>)> = vec![
        ("simple", kit::simple_pdf(1)),
        ("two_pages", kit::simple_pdf(2)),
        ("mixed_sizes", kit::mixed_size_pdf()),
        ("javascript", kit::javascript_pdf()),
        ("features", kit::features_pdf()),
        ("outline_loop", kit::outline_loop_pdf()),
        ("outline_wide", kit::outline_wide_pdf(20)),
        ("outline_deep", kit::outline_deep_pdf(6)),
        ("links", kit::many_links_pdf(12)),
        ("encrypted", kit::encrypted_pdf("secret", 1)),
        ("truncated", kit::truncated_pdf()),
        ("corrupt", kit::corrupt_pdf()),
    ];
    for (name, bytes) in pdfs {
        write(&dir, target, &format!("{name}.pdf"), &bytes);
    }
    println!("seeds written to {}", dir.display());
}
