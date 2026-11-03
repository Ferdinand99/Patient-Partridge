//! A stand-in for the signer that misbehaves on purpose, for the tests of the client.
//! Usage: `fake_signer <mode>`.
//!
//! Every mode greets correctly first, except where the mode says otherwise.
//!
//! - `good`: opens any file with any password and signs any digest with 100 bytes of nothing in
//!   particular.
//! - `no-sandbox`: greets without naming a sandbox.
//! - `wrong-version`: greets with another version of the messages.
//! - `error-load`: answers `Load` with "wrong password".
//! - `garbage`: answers `Load` with bytes that are not a message.
//! - `huge`: answers `Load` with a frame that announces more than a frame may hold.
//! - `silent`: never answers `Load`.
//! - `exit`: ends right after the greeting.
//! - `wrong-answer`: answers `Load` with a signature, and `Sign` with a certificate.
//! - `long-signature`: its signature is longer than the certificate said it would be.

// Like the engine, a program without a console on Windows (it is started with a restricted token).
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::io::{self, BufReader, BufWriter, Write};

use partridge_signer_protocol::{
    read_frame, usage, write_frame, Certificate, ErrorCode, KeyKind, Request, Response,
    PROTOCOL_VERSION,
};

fn certificate() -> Certificate {
    Certificate {
        name: "Stand-in".into(),
        issuer: "Stand-in".into(),
        self_signed: true,
        serial: "01".into(),
        fingerprint: "cd".repeat(32),
        not_before: 1_700_000_000,
        not_after: 1_900_000_000,
        key: KeyKind::EcdsaP256,
        usage: usage::HAS_EXTENSION | usage::DIGITAL_SIGNATURE,
        chain: 1,
        signature_bytes: 900,
    }
}

fn send(output: &mut impl Write, response: Response) {
    write_frame(output, &response.encode()).expect("stdout");
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "good".into());
    let mut input = BufReader::new(io::stdin().lock());
    let mut output = BufWriter::new(io::stdout().lock());
    loop {
        let Ok(Some(frame)) = read_frame(&mut input) else {
            return;
        };
        let Ok(request) = Request::decode(&frame) else {
            return;
        };
        match request {
            Request::Hello { .. } => {
                send(
                    &mut output,
                    Response::Hello {
                        version: if mode == "wrong-version" {
                            PROTOCOL_VERSION + 1
                        } else {
                            PROTOCOL_VERSION
                        },
                        sandbox: if mode == "no-sandbox" {
                            "none: a stand-in".into()
                        } else {
                            "stand-in".into()
                        },
                    },
                );
                if mode == "exit" {
                    return;
                }
            }
            Request::Load { .. } => match mode.as_str() {
                "error-load" => send(
                    &mut output,
                    Response::Error {
                        code: ErrorCode::WrongPassword,
                        message: "the password is wrong".into(),
                    },
                ),
                "garbage" => {
                    write_frame(&mut output, &[0xEE, 1, 2, 3]).expect("stdout");
                }
                "huge" => {
                    output
                        .write_all(&(3u32 * 1024 * 1024).to_le_bytes())
                        .expect("stdout");
                    output.flush().expect("stdout");
                }
                "silent" => std::thread::sleep(std::time::Duration::from_secs(600)),
                "wrong-answer" => send(&mut output, Response::Signed { cms: vec![1; 10] }),
                _ => send(&mut output, Response::Loaded(certificate())),
            },
            Request::Sign { .. } => match mode.as_str() {
                "wrong-answer" => send(&mut output, Response::Loaded(certificate())),
                "long-signature" => send(
                    &mut output,
                    Response::Signed {
                        cms: vec![0x30; 2_000],
                    },
                ),
                _ => send(
                    &mut output,
                    Response::Signed {
                        cms: vec![0x30; 100],
                    },
                ),
            },
            Request::Close => {
                send(&mut output, Response::Closed);
                return;
            }
        }
    }
}
