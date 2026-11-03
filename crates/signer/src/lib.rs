//! The Patient Partridge signer.
//!
//! A program of its own that holds a private key for as long as a signature takes and does nothing
//! else. The app gives it a certificate file and its password (`Load`), is told what the
//! certificate says, and later gives it the digest of the part of a PDF that is to be signed
//! (`Sign`). The signer never sees the document, has no file to open and no network to use, and is
//! locked into the sandbox of the operating system before it reads its first request (see
//! `partridge-sandbox`). It ends when the app lets go of it.
//!
//! Why a process of its own and not a part of the app: a certificate file comes from outside and is
//! parsed by code that is young (`p12-keystore` and the crates under it), and the key is the one
//! thing in the program that must not leak. The engine, which reads the PDF, never has the key; the
//! app, which has the window and the updater, never has it either.

mod cms;
mod der;
mod identity;
pub mod startup;

use std::io::{Read, Write};

use partridge_signer_protocol::{
    read_frame, write_frame, ErrorCode, Request, Response, PROTOCOL_VERSION,
};
use zeroize::Zeroize;

pub use identity::{load, Identity, SignerError};

/// Signs the digest of the signed part of a file with the key of `identity`; the answer is the DER
/// of the signature.
pub use cms::sign;

/// What the signer holds between requests.
pub struct Signer {
    sandbox: String,
    identity: Option<Identity>,
    greeted: bool,
    /// When set, the signer answers the greeting with this and does nothing else.
    refusal: Option<String>,
}

impl Signer {
    /// A signer that says `sandbox` (the layers in force) when it is greeted.
    pub fn new(sandbox: &str) -> Signer {
        Signer {
            sandbox: sandbox.to_string(),
            identity: None,
            greeted: false,
            refusal: None,
        }
    }

    /// Makes the signer refuse everything: used when the sandbox could not be applied.
    pub fn refuse(&mut self, why: &str) {
        self.identity = None;
        self.refusal = Some(why.to_string());
    }

    /// True once [`Signer::refuse`] was called: the signer answers the next request and stops.
    pub fn is_refusing(&self) -> bool {
        self.refusal.is_some()
    }

    /// Answers one request. A request before the greeting is refused.
    pub fn handle(&mut self, request: Request) -> Response {
        if let Some(why) = &self.refusal {
            return error(ErrorCode::SandboxUnavailable, why);
        }
        match request {
            Request::Hello { version } => {
                if version != PROTOCOL_VERSION {
                    return error(
                        ErrorCode::ProtocolMismatch,
                        "the app and the signer do not speak the same version",
                    );
                }
                self.greeted = true;
                Response::Hello {
                    version: PROTOCOL_VERSION,
                    sandbox: self.sandbox.clone(),
                }
            }
            _ if !self.greeted => error(ErrorCode::InvalidRequest, "no greeting was sent"),
            Request::Load {
                mut p12,
                mut password,
            } => {
                let outcome = self.load(&p12, &password);
                // Neither is wanted any more, whatever came of it.
                p12.zeroize();
                password.zeroize();
                outcome
            }
            Request::Sign { digest } => match &self.identity {
                None => error(ErrorCode::NotLoaded, "no certificate file is loaded"),
                Some(identity) => match sign(identity, &digest) {
                    Ok(cms) => Response::Signed { cms },
                    Err(e) => error(e.code, &e.message),
                },
            },
            Request::Close => {
                self.identity = None;
                Response::Closed
            }
        }
    }

    fn load(&mut self, p12: &[u8], password: &[u8]) -> Response {
        if self.identity.is_some() {
            return error(
                ErrorCode::InvalidRequest,
                "a certificate file is loaded already",
            );
        }
        let Ok(text) = std::str::from_utf8(password) else {
            return error(ErrorCode::InvalidRequest, "the password is not text");
        };
        match load(p12, text) {
            Ok(identity) => {
                let info = identity.certificate().clone();
                self.identity = Some(identity);
                Response::Loaded(info)
            }
            Err(e) => error(e.code, &e.message),
        }
    }
}

fn error(code: ErrorCode, message: &str) -> Response {
    Response::Error {
        code,
        message: message.to_string(),
    }
}

/// Reads requests from `input` and answers them on `output` until the app closes the connection or
/// says `Close`. A request that cannot be decoded ends the session: it has no way to be answered.
/// Returns the exit code the process should end with.
pub fn serve(input: &mut impl Read, output: &mut impl Write, signer: &mut Signer) -> u8 {
    loop {
        let mut frame = match read_frame(input) {
            Ok(Some(frame)) => frame,
            Ok(None) => return 0,
            Err(_) => return 3,
        };
        let decoded = Request::decode(&frame);
        // The frame holds the password; it is overwritten before anything else happens.
        frame.zeroize();
        let Ok(request) = decoded else { return 3 };
        let closing = matches!(request, Request::Close);
        let response = signer.handle(request);
        if write_frame(output, &response.encode()).is_err() {
            return 4;
        }
        if signer.is_refusing() {
            return 5;
        }
        if closing {
            return 0;
        }
    }
}
