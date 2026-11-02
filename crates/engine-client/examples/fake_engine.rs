//! A stand-in for the engine that misbehaves on purpose, for the tests of the client.
//! Usage: `fake_engine <mode> [marker file]`.
//!
//! Modes (every mode answers `Hello` correctly first):
//! - `unasked`: sends a few answers nobody asked for.
//! - `flood`: sends 64 MiB of answers nobody asked for, then exits.
//! - `oversized`: answers `Open` with a 10 MiB frame.
//! - `wrong-id`: answers `Open` with the wrong request id.
//! - `wrong-size`: answers `Render` with a bitmap of another size than asked for.
//! - `hang`: never answers `Open`.
//! - `exit-once`: exits right after the first `Hello` (when the marker file does not exist yet, or is
//!   empty: on Windows the test makes it first, because the engine cannot create files).
//! - `good`: behaves like an engine with one 612 x 792 document.
//! - `no-sandbox`: like `good`, but greets without naming a sandbox.
//! - `save-good`: like `good`, and saves a made-up file of 2 500 000 bytes (byte `i` is `i % 251`).
//! - `save-signed`: like `save-good`, but the document holds two signatures.
//! - `annot-good`: like `good`, and keeps annotations of one page: it lists one note, and adds,
//!   changes, removes and puts back annotations the way the engine does.
//! - `annot-wrong-kind`, `annot-wrong-id`, `annot-wrong-answer`, `annot-big`: like `annot-good`,
//!   but an addition is answered with an annotation of another kind, a change with another id, a
//!   list with something that is not a list, or a list with a 10 MiB frame.
//! - `save-touched`: like `save-good`, but the description says the file was touched while parts of
//!   it were unread.
//! - `save-no-pages`: like `save-good`, but the document describes itself as one of no pages.
//! - `sig-good`, `sig-trusted-no-list`, `sig-trusted-no-name`, `sig-no-list-with-list`,
//!   `sig-wrong-order`, `sig-count`: answer `Signatures` with a report that is fine, or with one that
//!   cannot be true (a trusted signature without a trust list or without the name of the
//!   certificate it leads to, "no trust list" with a list, signatures out of order, more
//!   signatures than the total).
//! - `save-short`, `save-wrong-offset`, `save-big`, `save-error`, `save-lies`: like `save-good`
//!   but a piece is a byte too short, a piece comes from the wrong offset, a piece is a 10 MiB
//!   frame, the save fails with an error, or the end of the file is missing (the last piece is
//!   answered with an error).

// Like the engine, a program without a console on Windows (it is started with a restricted token).
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::io::{self, BufReader, BufWriter};

use partridge_protocol::{
    ability, read_frame, write_frame, write_response, AnnotInfo, AnnotKind, CertificateTime,
    ErrorCode, PageSize, Request, RequestBody, Response, ResponseBody, SignatureInfo,
    SignatureReason, SignatureReport, SignatureState, PROTOCOL_VERSION,
};

/// An annotation the stand-in makes up.
fn made_up(id: u32, kind: AnnotKind, text: &str) -> AnnotInfo {
    AnnotInfo {
        id,
        kind,
        abilities: ability::EDIT_TEXT | ability::REMOVE | ability::RESTORE,
        rect: [10.0, 20.0, 30.0, 40.0],
        color: [255, 235, 59, 255],
        text: text.to_string(),
        author: String::new(),
    }
}

/// The size of the file the stand-in saves.
const SAVE_SIZE: u32 = 2_500_000;

fn main() {
    // The client adds `--document <n>` and `--memory-limit <n>` to the command line; they are not
    // for this program.
    let mut words = Vec::new();
    let mut raw = std::env::args().skip(1);
    while let Some(word) = raw.next() {
        if word == "--document" || word == "--memory-limit" {
            raw.next();
        } else {
            words.push(word);
        }
    }
    let mut args = words.into_iter();
    let mode = args.next().unwrap_or_else(|| "good".into());
    let marker = args.next();

    let mut input = BufReader::new(io::stdin().lock());
    let mut output = BufWriter::new(io::stdout().lock());
    let reply = |out: &mut BufWriter<io::StdoutLock>, id: u32, body: ResponseBody| {
        let _ = write_response(out, &Response { id, body });
    };

    while let Ok(Some(frame)) = read_frame(&mut input) {
        let Ok(request) = Request::decode(&frame) else {
            return;
        };
        let id = request.id;
        match request.body {
            RequestBody::Hello { .. } => {
                reply(
                    &mut output,
                    id,
                    ResponseBody::Hello {
                        protocol_version: PROTOCOL_VERSION,
                        engine_version: "fake".into(),
                        pdfium_version: "fake".into(),
                        sandbox: if mode == "no-sandbox" {
                            "none: the stand-in has none".to_string()
                        } else {
                            "fake".into()
                        },
                    },
                );
                match mode.as_str() {
                    "unasked" => {
                        for _ in 0..3 {
                            reply(&mut output, 0, ResponseBody::Closed);
                        }
                    }
                    "flood" => {
                        for _ in 0..64 {
                            reply(
                                &mut output,
                                0,
                                ResponseBody::Rendered {
                                    width: 512,
                                    height: 512,
                                    rgba: vec![0; 512 * 512 * 4],
                                },
                            );
                        }
                        return;
                    }
                    "exit-once" => {
                        if let Some(marker) = &marker {
                            let seen = std::fs::read(marker).is_ok_and(|bytes| !bytes.is_empty());
                            if !seen {
                                let _ = std::fs::write(marker, b"x");
                                return;
                            }
                        }
                    }
                    _ => {}
                }
            }
            RequestBody::Open { .. } => match mode.as_str() {
                "oversized" => {
                    let _ = write_frame(&mut output, &vec![0u8; 10 * 1024 * 1024]);
                }
                "wrong-id" => reply(&mut output, id + 1, opened()),
                "bad-page-size" => reply(
                    &mut output,
                    id,
                    ResponseBody::Opened {
                        doc: 1,
                        pages: vec![PageSize {
                            width: f32::NAN,
                            height: 792.0,
                        }],
                    },
                ),
                "hang" => std::thread::sleep(std::time::Duration::from_secs(3600)),
                _ => reply(&mut output, id, opened()),
            },
            RequestBody::Render { width, height, .. } => {
                let (w, h) = if mode == "wrong-size" {
                    (width, (height / 2).max(1))
                } else {
                    (width, height)
                };
                reply(
                    &mut output,
                    id,
                    ResponseBody::Rendered {
                        width: w,
                        height: h,
                        rgba: vec![0; (w * h * 4) as usize],
                    },
                );
            }
            RequestBody::Close { .. } => reply(&mut output, id, ResponseBody::Closed),
            RequestBody::Outline { .. } => match mode.as_str() {
                "wrong-kind" => reply(&mut output, id, ResponseBody::Closed),
                "big-text" => {
                    let _ = write_frame(&mut output, &vec![0u8; 10 * 1024 * 1024]);
                }
                _ => reply(&mut output, id, ResponseBody::Outline { items: vec![] }),
            },
            RequestBody::PageText { .. } => match mode.as_str() {
                "big-text" => {
                    let _ = write_frame(&mut output, &vec![0u8; 10 * 1024 * 1024]);
                }
                _ => reply(
                    &mut output,
                    id,
                    ResponseBody::PageText {
                        text: "hi".into(),
                        rects: vec![[0.0; 4]; 2],
                    },
                ),
            },
            RequestBody::PageLinks { .. } => {
                reply(&mut output, id, ResponseBody::PageLinks { links: vec![] })
            }
            RequestBody::FontReport => {
                reply(&mut output, id, ResponseBody::FontReport { missed: vec![] })
            }
            RequestBody::Search { .. } => match mode.as_str() {
                "text-wrong-id" => reply(
                    &mut output,
                    id + 1,
                    ResponseBody::Matches { matches: vec![] },
                ),
                _ => reply(&mut output, id, ResponseBody::Matches { matches: vec![] }),
            },
            RequestBody::DocInfo { .. } => reply(
                &mut output,
                id,
                ResponseBody::DocInfo {
                    pages: if mode == "save-no-pages" { 0 } else { 1 },
                    security: 0,
                    signatures: if mode == "save-signed" { 2 } else { 0 },
                    maybe_signed: false,
                    touched: mode == "save-touched",
                },
            ),
            RequestBody::SaveBegin { .. } => match mode.as_str() {
                "save-error" => reply(
                    &mut output,
                    id,
                    ResponseBody::Error {
                        code: ErrorCode::SaveFailed,
                        message: "the stand-in cannot save".into(),
                    },
                ),
                "save-progress" => {
                    // Takes long, and says so: six signs of life, 0.3 s apart.
                    for step in 1..=6u32 {
                        std::thread::sleep(std::time::Duration::from_millis(300));
                        reply(
                            &mut output,
                            id,
                            ResponseBody::Progress { done: step * 1000 },
                        );
                    }
                    reply(&mut output, id, ResponseBody::SaveReady { size: SAVE_SIZE });
                }
                "save-endless" => {
                    // Says it is working, more and more, for a minute: a hostile engine, or a
                    // document whose pages take for ever.
                    for step in 1..=600u32 {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        reply(&mut output, id, ResponseBody::Progress { done: step });
                    }
                    reply(&mut output, id, ResponseBody::SaveReady { size: SAVE_SIZE });
                }
                "save-stuck" => {
                    // Alive, but nothing to show for it: the same number again and again.
                    for _ in 0..8 {
                        std::thread::sleep(std::time::Duration::from_millis(300));
                        reply(&mut output, id, ResponseBody::Progress { done: 5 });
                    }
                    reply(&mut output, id, ResponseBody::SaveReady { size: SAVE_SIZE });
                }
                "save-silent" => {
                    std::thread::sleep(std::time::Duration::from_secs(30));
                }
                "save-slow" => {
                    // Slow, but alive: it answers after a while.
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    reply(&mut output, id, ResponseBody::SaveReady { size: SAVE_SIZE });
                }
                _ => reply(&mut output, id, ResponseBody::SaveReady { size: SAVE_SIZE }),
            },
            RequestBody::SaveChunk { offset, len, .. } => {
                let piece = |from: u32, len: u32| -> Vec<u8> {
                    (from..from + len).map(|i| (i % 251) as u8).collect()
                };
                match mode.as_str() {
                    "save-short" => reply(
                        &mut output,
                        id,
                        ResponseBody::SaveData {
                            offset,
                            data: piece(offset, len - 1),
                        },
                    ),
                    "save-wrong-offset" => reply(
                        &mut output,
                        id,
                        ResponseBody::SaveData {
                            offset: offset + 1,
                            data: piece(offset + 1, len),
                        },
                    ),
                    "save-big" => {
                        let _ = write_frame(&mut output, &vec![0u8; 10 * 1024 * 1024]);
                    }
                    "save-lies" if offset + len == SAVE_SIZE => reply(
                        &mut output,
                        id,
                        ResponseBody::Error {
                            code: ErrorCode::SaveFailed,
                            message: "the end is missing".into(),
                        },
                    ),
                    _ => reply(
                        &mut output,
                        id,
                        ResponseBody::SaveData {
                            offset,
                            data: piece(offset, len),
                        },
                    ),
                }
            }
            RequestBody::SaveEnd { .. } => reply(&mut output, id, ResponseBody::SaveEnded),
            RequestBody::ListAnnots { .. } => match mode.as_str() {
                "annot-wrong-answer" => reply(&mut output, id, ResponseBody::Closed),
                "annot-big" => {
                    let _ = write_frame(&mut output, &vec![0u8; 10 * 1024 * 1024]);
                }
                _ => reply(
                    &mut output,
                    id,
                    ResponseBody::Annots {
                        items: vec![made_up(1, AnnotKind::Note, "from the stand-in")],
                    },
                ),
            },
            RequestBody::AddMarkup { kind, .. } => {
                let kind = if mode == "annot-wrong-kind" {
                    AnnotKind::Ink
                } else {
                    kind
                };
                reply(
                    &mut output,
                    id,
                    ResponseBody::Annot {
                        item: made_up(2, kind, ""),
                    },
                );
            }
            RequestBody::AddNote { text, .. } => {
                let kind = if mode == "annot-wrong-kind" {
                    AnnotKind::Stamp
                } else {
                    AnnotKind::Note
                };
                reply(
                    &mut output,
                    id,
                    ResponseBody::Annot {
                        item: made_up(3, kind, &text),
                    },
                );
            }
            RequestBody::AddInk { .. } => {
                let kind = if mode == "annot-wrong-kind" {
                    AnnotKind::Note
                } else {
                    AnnotKind::Ink
                };
                reply(
                    &mut output,
                    id,
                    ResponseBody::Annot {
                        item: made_up(4, kind, ""),
                    },
                );
            }
            RequestBody::AddStamp { .. } => {
                let kind = if mode == "annot-wrong-kind" {
                    AnnotKind::Note
                } else {
                    AnnotKind::Stamp
                };
                reply(
                    &mut output,
                    id,
                    ResponseBody::Annot {
                        item: made_up(5, kind, ""),
                    },
                );
            }
            RequestBody::UpdateAnnot {
                id: annot, text, ..
            } => {
                let annot = if mode == "annot-wrong-id" {
                    annot + 1
                } else {
                    annot
                };
                reply(
                    &mut output,
                    id,
                    ResponseBody::Annot {
                        item: made_up(annot, AnnotKind::Note, &text.unwrap_or_default()),
                    },
                );
            }
            RequestBody::RemoveAnnot { .. } => reply(&mut output, id, ResponseBody::AnnotRemoved),
            RequestBody::RestoreAnnot { id: annot, .. } => {
                let annot = if mode == "annot-wrong-id" {
                    annot + 1
                } else {
                    annot
                };
                reply(
                    &mut output,
                    id,
                    ResponseBody::Annot {
                        item: made_up(annot, AnnotKind::Note, "restored"),
                    },
                );
            }
            RequestBody::Signatures { .. } => {
                let item = |index: u32, state, reason, anchor: Option<&str>| SignatureInfo {
                    index,
                    state,
                    reason,
                    certificate_time: CertificateTime::Valid,
                    sub_filter: "adbe.pkcs7.detached".into(),
                    byte_range: vec![0, 10, 20, 30],
                    file_len: 50,
                    doc_mdp: 0,
                    certificates: 1,
                    signer: Some("Test Signer".into()),
                    organisation: None,
                    issuer: None,
                    serial: None,
                    fingerprint: None,
                    valid_from: None,
                    valid_to: None,
                    key: None,
                    digest: None,
                    claimed_time: None,
                    stated_reason: None,
                    stated_location: None,
                    trust_anchor: anchor.map(str::to_string),
                };
                let unknown = |index| {
                    item(
                        index,
                        SignatureState::UnknownSigner,
                        SignatureReason::NoTrustList,
                        None,
                    )
                };
                let trusted = |index, anchor| {
                    item(
                        index,
                        SignatureState::Trusted,
                        SignatureReason::Verified,
                        anchor,
                    )
                };
                let (total, anchors, items) = match mode.as_str() {
                    "sig-trusted-no-list" => (1, 0, vec![trusted(0, Some("Root"))]),
                    "sig-trusted-no-name" => (1, 1, vec![trusted(0, None)]),
                    "sig-no-list-with-list" => (1, 1, vec![unknown(0)]),
                    "sig-wrong-order" => (2, 0, vec![unknown(1), unknown(0)]),
                    "sig-count" => (1, 0, vec![unknown(0), unknown(1)]),
                    "sig-good" => (2, 0, vec![unknown(0), unknown(1)]),
                    _ => (0, 0, vec![]),
                };
                reply(
                    &mut output,
                    id,
                    ResponseBody::Signatures {
                        report: SignatureReport {
                            total,
                            unlisted: 0,
                            trust_anchors: anchors,
                            items,
                        },
                    },
                );
            }
            RequestBody::Fingerprint { .. } => reply(
                &mut output,
                id,
                ResponseBody::Fingerprint {
                    pages: if mode == "save-no-pages" { 0 } else { 1 },
                    failed: if mode == "save-unreadable" { 1 } else { 0 },
                    digest: 7,
                },
            ),
            // The form requests have a stand-in of their own (examples/fake_forms_engine.rs).
            RequestBody::FormInfo { .. }
            | RequestBody::ListFormFields { .. }
            | RequestBody::SetFormField { .. } => return,
            // The page tools have a stand-in of their own (fake_pages_engine).
            RequestBody::Pages(_) => reply(
                &mut output,
                id,
                ResponseBody::Error {
                    code: ErrorCode::InvalidRequest,
                    message: "this stand-in has no page tools".into(),
                },
            ),
        }
    }
}

fn opened() -> ResponseBody {
    ResponseBody::Opened {
        doc: 1,
        pages: vec![PageSize {
            width: 612.0,
            height: 792.0,
        }],
    }
}
