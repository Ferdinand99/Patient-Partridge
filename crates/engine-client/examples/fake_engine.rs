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
//! - `exit-once`: exits right after the first `Hello` (when the marker file does not exist yet).
//! - `good`: behaves like an engine with one 612 x 792 document.
//! - `no-sandbox`: like `good`, but greets without naming a sandbox.
//! - `save-good`: like `good`, and saves a made-up file of 2 500 000 bytes (byte `i` is `i % 251`).
//! - `save-signed`: like `save-good`, but the document holds two signatures.
//! - `save-short`, `save-wrong-offset`, `save-big`, `save-error`, `save-lies`: like `save-good`
//!   but a piece is a byte too short, a piece comes from the wrong offset, a piece is a 10 MiB
//!   frame, the save fails with an error, or the end of the file is missing (the last piece is
//!   answered with an error).

// Like the engine, a program without a console on Windows (it is started with a restricted token).
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::io::{self, BufReader, BufWriter};
use std::path::Path;

use partridge_protocol::{
    read_frame, write_frame, write_response, ErrorCode, PageSize, Request, RequestBody, Response,
    ResponseBody, PROTOCOL_VERSION,
};

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
                            if !Path::new(marker).exists() {
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
                    pages: 1,
                    security: 0,
                    signatures: if mode == "save-signed" { 2 } else { 0 },
                    maybe_signed: false,
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
            RequestBody::Fingerprint { .. } => reply(
                &mut output,
                id,
                ResponseBody::Fingerprint {
                    pages: 1,
                    failed: if mode == "save-unreadable" { 1 } else { 0 },
                    digest: 7,
                },
            ),
            // The form requests have a stand-in of their own (examples/fake_forms_engine.rs).
            RequestBody::FormInfo { .. }
            | RequestBody::ListFormFields { .. }
            | RequestBody::SetFormField { .. } => return,
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
