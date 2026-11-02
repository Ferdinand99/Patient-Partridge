//! A stand-in for the engine that misbehaves on purpose in the page tools, for the tests of the
//! client. Usage: `fake_pages_engine <mode>`. Every mode answers `Hello` and `Open` (a document of
//! three Letter pages) correctly. The page tools answer as a good engine would unless the mode says
//! otherwise:
//!
//! - `good`: a delete takes the page away, an insert takes the pieces and adds two pages, a copy
//!   hands out 2 500 000 bytes (byte `i` is `i % 251`) for the pages asked for.
//! - `bad-size`: every change answers with a page of NaN width.
//! - `wrong-kind`: every page tool answers with `Closed`.
//! - `too-many-pages`: a delete answers with more pages than a document may have.
//! - `export-short`, `export-offset`, `export-pages`: a copy that hands a piece a byte too short, a
//!   piece from the wrong offset, or announces another number of pages than were asked for.
//! - `ack-wrong`: an insert acknowledges another number of bytes than were handed over.
//! - `endless`: a delete says it is working for ever.
//! - `hang`: a page tool never answers.

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::io::{self, BufReader, BufWriter};

use partridge_protocol::{
    read_frame, write_frame, write_response, ErrorCode, PageSize, PagesRequest, PagesResponse,
    Request, RequestBody, Response, ResponseBody, MAX_PAGES, PROTOCOL_VERSION,
};

const COPY_SIZE: u32 = 2_500_000;

fn sizes(n: usize) -> Vec<PageSize> {
    vec![
        PageSize {
            width: 612.0,
            height: 792.0,
        };
        n
    ]
}

fn main() {
    let mut words = Vec::new();
    let mut raw = std::env::args().skip(1);
    while let Some(word) = raw.next() {
        if word == "--document" || word == "--memory-limit" {
            raw.next();
        } else {
            words.push(word);
        }
    }
    let mode = words.into_iter().next().unwrap_or_else(|| "good".into());
    let mut input = BufReader::new(io::stdin().lock());
    let mut output = BufWriter::new(io::stdout().lock());
    let mut pages = 3usize;
    let mut received = 0u32;
    let reply = |out: &mut BufWriter<io::StdoutLock>, id: u32, body: ResponseBody| {
        let _ = write_response(out, &Response { id, body });
    };
    while let Ok(Some(frame)) = read_frame(&mut input) {
        let Ok(request) = Request::decode(&frame) else {
            return;
        };
        let id = request.id;
        match request.body {
            RequestBody::Hello { .. } => reply(
                &mut output,
                id,
                ResponseBody::Hello {
                    protocol_version: PROTOCOL_VERSION,
                    engine_version: "fake".into(),
                    pdfium_version: "fake".into(),
                    sandbox: "fake".into(),
                },
            ),
            RequestBody::Open { .. } => reply(
                &mut output,
                id,
                ResponseBody::Opened {
                    doc: 1,
                    pages: sizes(pages),
                },
            ),
            RequestBody::Close { .. } => reply(&mut output, id, ResponseBody::Closed),
            RequestBody::DocInfo { .. } => reply(
                &mut output,
                id,
                ResponseBody::DocInfo {
                    pages: pages as u32,
                    security: 0,
                    signatures: 0,
                    maybe_signed: false,
                    touched: false,
                },
            ),
            RequestBody::SaveChunk { offset, len, .. } => {
                let mut start = offset;
                let mut take = len;
                if mode == "export-short" {
                    take = len - 1;
                }
                if mode == "export-offset" {
                    start = offset + 1;
                }
                reply(
                    &mut output,
                    id,
                    ResponseBody::SaveData {
                        offset: start,
                        data: (start..start + take).map(|i| (i % 251) as u8).collect(),
                    },
                );
            }
            RequestBody::SaveEnd { .. } => reply(&mut output, id, ResponseBody::SaveEnded),
            RequestBody::Pages(op) => {
                if mode == "wrong-kind" {
                    reply(&mut output, id, ResponseBody::Closed);
                    continue;
                }
                if mode == "hang" {
                    std::thread::sleep(std::time::Duration::from_secs(3600));
                }
                let changed = |pages: usize, snapshot: u32, count: u32| {
                    ResponseBody::Pages(PagesResponse::Changed {
                        pages: if mode == "bad-size" {
                            vec![PageSize {
                                width: f32::NAN,
                                height: 792.0,
                            }]
                        } else {
                            sizes(pages)
                        },
                        snapshot,
                        count,
                        form_fields: 0,
                    })
                };
                match op {
                    PagesRequest::Delete { pages: list, .. } => {
                        if mode == "endless" {
                            for step in 1..=600u32 {
                                std::thread::sleep(std::time::Duration::from_millis(100));
                                reply(&mut output, id, ResponseBody::Progress { done: step });
                            }
                        }
                        pages = pages.saturating_sub(list.len());
                        if mode == "too-many-pages" {
                            // One page more than a document may have: the frame cannot be read.
                            let mut payload = vec![190u8];
                            payload.extend_from_slice(&id.to_le_bytes());
                            payload.extend_from_slice(&(MAX_PAGES as u32 + 1).to_le_bytes());
                            payload.extend(vec![0u8; (MAX_PAGES + 1) * 8 + 12]);
                            let _ = write_frame(&mut output, &payload);
                        } else {
                            reply(&mut output, id, changed(pages, 1, list.len() as u32));
                        }
                    }
                    PagesRequest::Rotate { pages: list, .. }
                    | PagesRequest::SetBoxes { pages: list, .. } => {
                        reply(&mut output, id, changed(pages, 0, list.len() as u32))
                    }
                    PagesRequest::Reorder { .. } => reply(&mut output, id, changed(pages, 1, 1)),
                    PagesRequest::InsertBlank { .. } => {
                        pages += 1;
                        reply(&mut output, id, changed(pages, 1, 1));
                    }
                    PagesRequest::Restore { .. } => reply(&mut output, id, changed(pages, 2, 0)),
                    PagesRequest::Crop { pages: list, .. } => reply(
                        &mut output,
                        id,
                        ResponseBody::Pages(PagesResponse::Cropped {
                            pages: sizes(pages),
                            before: vec![[0.0, 0.0, 612.0, 792.0]; list.len()],
                            after: vec![[1.0, 1.0, 600.0, 780.0]; list.len()],
                        }),
                    ),
                    PagesRequest::ImportBegin { .. } => {
                        received = 0;
                        reply(
                            &mut output,
                            id,
                            ResponseBody::Pages(PagesResponse::ImportReady),
                        )
                    }
                    PagesRequest::ImportChunk { data, .. } => {
                        received += data.len() as u32;
                        let said = if mode == "ack-wrong" {
                            received + 1
                        } else {
                            received
                        };
                        reply(
                            &mut output,
                            id,
                            ResponseBody::Pages(PagesResponse::ImportAck { received: said }),
                        )
                    }
                    PagesRequest::ImportEnd { .. } => {
                        pages += 2;
                        reply(&mut output, id, changed(pages, 1, 2))
                    }
                    PagesRequest::ImportAbort { .. } => {
                        reply(&mut output, id, ResponseBody::Pages(PagesResponse::Done))
                    }
                    PagesRequest::ExportBegin { pages: list, .. } => {
                        let asked = list.len() as u32;
                        reply(
                            &mut output,
                            id,
                            ResponseBody::Pages(PagesResponse::Exported {
                                size: COPY_SIZE,
                                pages: if mode == "export-pages" {
                                    asked + 1
                                } else {
                                    asked
                                },
                                signatures: 0,
                                source_security: 0,
                                digest: 7,
                            }),
                        )
                    }
                }
            }
            _ => reply(
                &mut output,
                id,
                ResponseBody::Error {
                    code: ErrorCode::InvalidRequest,
                    message: "this stand-in does not do that".into(),
                },
            ),
        }
    }
}
