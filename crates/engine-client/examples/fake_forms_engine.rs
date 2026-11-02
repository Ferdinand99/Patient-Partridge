//! A stand-in for the engine that answers the form requests, and misbehaves on purpose, for the
//! tests of the client. Usage: `fake_forms_engine <mode>`.
//!
//! Modes (every mode greets correctly and opens a one page document):
//! - `good`: lists one text field, and answers a set with the same list.
//! - `big`: answers every form request with a frame bigger than a list of fields can be.
//! - `long-name`: answers with a field whose name is a byte over the cap.
//! - `long-value`: answers with a field whose value is a byte over the cap.
//! - `too-many`: answers with a list that claims one field more than a page may have.
//! - `wrong-kind`: answers every form request with the answer to `Close`.
//! - `wrong-id`: answers every form request with another request id.

// Like the engine, a program without a console on Windows (it is started with a restricted token).
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::io::{self, BufReader, BufWriter, Write};

use partridge_protocol::{
    read_frame, write_frame, write_response, FormField, FormFieldKind, FormKind, PageSize, Request,
    RequestBody, Response, ResponseBody, MAX_FORM_FIELDS_PER_PAGE, MAX_FORM_NAME, MAX_FORM_VALUE,
    PROTOCOL_VERSION,
};

fn field() -> FormField {
    FormField {
        index: 0,
        kind: FormFieldKind::Text,
        flags: 0,
        turns: 0,
        rect: [10.0, 10.0, 100.0, 30.0],
        font_size: 0.0,
        name: "name".into(),
        tooltip: String::new(),
        value: "value".into(),
        max_len: 0,
        options: vec![],
    }
}

/// The payload of a `FormFields` answer with one field, written by hand so that a number in it can
/// be wrong. The field has the given length of name and value, and no tooltip and no choices.
fn raw_answer(id: u32, count: u32, name_len: usize, value_len: usize) -> Vec<u8> {
    let mut out = vec![171u8];
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.push(1); // text
    out.extend_from_slice(&0u16.to_le_bytes());
    out.push(0);
    for v in [0.0f32, 0.0, 10.0, 10.0, 0.0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&(name_len as u32).to_le_bytes());
    out.extend(std::iter::repeat_n(b'n', name_len));
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(value_len as u32).to_le_bytes());
    out.extend(std::iter::repeat_n(b'v', value_len));
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out
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
    while let Ok(Some(frame)) = read_frame(&mut input) {
        let Ok(request) = Request::decode(&frame) else {
            return;
        };
        let id = request.id;
        let is_info = matches!(request.body, RequestBody::FormInfo { .. });
        let body = match request.body {
            RequestBody::Hello { .. } => ResponseBody::Hello {
                protocol_version: PROTOCOL_VERSION,
                engine_version: "fake".into(),
                pdfium_version: "fake".into(),
                sandbox: "fake".into(),
            },
            RequestBody::Open { .. } => ResponseBody::Opened {
                doc: 1,
                pages: vec![PageSize {
                    width: 612.0,
                    height: 792.0,
                }],
            },
            RequestBody::FormInfo { .. }
            | RequestBody::ListFormFields { .. }
            | RequestBody::SetFormField { .. } => match mode.as_str() {
                "big" => {
                    let _ = write_frame(&mut output, &vec![0u8; 3 * 1024 * 1024]);
                    let _ = output.flush();
                    continue;
                }
                "long-name" => {
                    let _ = write_frame(&mut output, &raw_answer(id, 1, MAX_FORM_NAME + 1, 1));
                    let _ = output.flush();
                    continue;
                }
                "long-value" => {
                    let _ = write_frame(&mut output, &raw_answer(id, 1, 1, MAX_FORM_VALUE + 1));
                    let _ = output.flush();
                    continue;
                }
                "too-many" => {
                    let count = MAX_FORM_FIELDS_PER_PAGE as u32 + 1;
                    let _ = write_frame(&mut output, &raw_answer(id, count, 1, 1));
                    let _ = output.flush();
                    continue;
                }
                "wrong-kind" => ResponseBody::Closed,
                "wrong-id" => {
                    let _ = write_response(
                        &mut output,
                        &Response {
                            id: id + 1,
                            body: ResponseBody::FormFields { fields: vec![] },
                        },
                    );
                    continue;
                }
                _ if is_info => ResponseBody::FormInfo {
                    kind: FormKind::Acro,
                    has_actions: true,
                },
                _ => ResponseBody::FormFields {
                    fields: vec![field()],
                },
            },
            RequestBody::Close { .. } => ResponseBody::Closed,
            _ => return,
        };
        let _ = write_response(&mut output, &Response { id, body });
    }
}
