//! The app side of the signer process.
//!
//! The signer holds the private key for as long as a signature takes (see `partridge-signer`). It
//! is started the same way the engine is, in the same kind of sandbox (see `spawn`), and treated the
//! same way: it can crash, hang or send rubbish, and none of that may take the app with it. The app
//! gives it a certificate file and the password once, and later the digest of what is to be
//! signed. Nothing else goes through.
//!
//! The password is never kept: the messages that carry it are overwritten after they are sent.

use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use partridge_signer_protocol::{
    read_frame, write_frame, Certificate, ErrorCode, HashKind, Request, Response, MAX_P12_BYTES,
    MAX_PASSWORD_BYTES, PROTOCOL_VERSION,
};
use zeroize::Zeroize;

use crate::spawn::{spawn_engine, EngineProcess, SandboxPolicy, SpawnRequest};
use crate::{sandbox_in_force, ClientError};

/// How to start and supervise the signer.
#[derive(Debug, Clone)]
pub struct SignerConfig {
    pub program: PathBuf,
    /// Longest the signer may take to greet the app or to sign.
    pub timeout: Duration,
    /// Longest the signer may take to open a certificate file. Opening one runs the key derivation
    /// of the file, which is slow on purpose.
    pub load_timeout: Duration,
    /// Whether a signer without a working sandbox is refused. Release builds always insist.
    pub sandbox: SandboxPolicy,
}

impl SignerConfig {
    pub fn new(program: impl Into<PathBuf>) -> SignerConfig {
        SignerConfig {
            program: program.into(),
            timeout: Duration::from_secs(30),
            load_timeout: Duration::from_secs(60),
            sandbox: SandboxPolicy::default_for_build(),
        }
    }
}

/// Most memory the signer may use, in bytes.
const MEMORY_LIMIT: u64 = 512 * 1024 * 1024;

/// What the signer said, or why there is nothing to read.
enum Heard {
    Response(Response),
    Failed(String),
    Closed,
}

/// A running signer.
pub struct SignerSession {
    process: Box<dyn EngineProcess>,
    stdin: Box<dyn Write + Send>,
    heard: Receiver<Heard>,
    config: SignerConfig,
    certificate: Option<Certificate>,
    sandbox: String,
    ended: bool,
}

fn signer_error(code: &'static str, message: impl Into<String>) -> ClientError {
    ClientError::Signer {
        code,
        message: message.into(),
    }
}

/// The code of the window for an error of the signer.
fn code_of(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::NotACertificateFile => "signer_not_a_certificate_file",
        ErrorCode::WrongPassword => "signer_wrong_password",
        ErrorCode::NoKey => "signer_no_key",
        ErrorCode::KeyUnsupported => "signer_key_unsupported",
        ErrorCode::KeyMismatch => "signer_key_mismatch",
        ErrorCode::TooLarge => "signer_too_large",
        ErrorCode::NotLoaded => "signer_not_loaded",
        ErrorCode::InvalidRequest => "signer_invalid_request",
        ErrorCode::SandboxUnavailable => "signer_sandbox_unavailable",
        ErrorCode::Internal => "signer_internal",
        ErrorCode::ProtocolMismatch => "signer_protocol_mismatch",
    }
}

impl SignerSession {
    /// Starts a signer and greets it. The signer must say that it is locked in, unless the policy
    /// allows a development build to go on without.
    ///
    /// On Windows a start that ends before the greeting because the system's loader could not
    /// find a library (exit code `0xC0000135`) or was not let open one (`0xC0000022`) is tried
    /// again, up to five times in all, as the start of the engine is (see `EngineClient`): the
    /// loader hands library loading to helper threads that have no rights in the locked-down
    /// token, and many starts at once can lose that race now and then. Nothing is relaxed by
    /// trying again: every attempt starts the signer with all its restrictions, and a start that
    /// fails for another reason, or every time, is reported as it was.
    pub fn start(config: &SignerConfig) -> Result<SignerSession, ClientError> {
        let mut attempt = 1;
        loop {
            match Self::start_once(config) {
                Err(ClientError::Signer { message, .. })
                    if attempt < 5
                        && cfg!(windows)
                        && (message.contains("exit code: 3221225781")
                            || message.contains("exit code: 3221225506")) =>
                {
                    attempt += 1;
                }
                other => return other,
            }
        }
    }

    fn start_once(config: &SignerConfig) -> Result<SignerSession, ClientError> {
        let spawned = spawn_engine(&SpawnRequest {
            program: &config.program,
            args: &[],
            document: None,
            memory_limit: Some(MEMORY_LIMIT),
            policy: config.sandbox,
            fonts: &[],
        })
        .map_err(|e| {
            signer_error(
                "signer_stopped",
                format!("{} could not be started ({e})", config.program.display()),
            )
        })?;
        let (tx, heard) = mpsc::sync_channel(1);
        let stdout = spawned.stdout;
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let event = match read_frame(&mut reader) {
                    Ok(Some(frame)) => match Response::decode(&frame) {
                        Ok(response) => Heard::Response(response),
                        Err(e) => Heard::Failed(e.to_string()),
                    },
                    Ok(None) => Heard::Closed,
                    Err(e) => Heard::Failed(e.to_string()),
                };
                let last = !matches!(event, Heard::Response(_));
                if tx.send(event).is_err() || last {
                    return;
                }
            }
        });
        let mut session = SignerSession {
            process: spawned.process,
            stdin: spawned.stdin,
            heard,
            config: config.clone(),
            certificate: None,
            sandbox: String::new(),
            ended: false,
        };
        let timeout = config.timeout;
        let greeting = session.exchange(
            &Request::Hello {
                version: PROTOCOL_VERSION,
            }
            .encode(),
            timeout,
        )?;
        match greeting {
            Response::Hello { version, sandbox } if version == PROTOCOL_VERSION => {
                if !sandbox_in_force(&sandbox) && config.sandbox == SandboxPolicy::Required {
                    session.kill();
                    return Err(signer_error(
                        "signer_sandbox_unavailable",
                        format!(
                            "the signer did not report a sandbox, so it was not used ({sandbox})"
                        ),
                    ));
                }
                session.sandbox = sandbox;
                Ok(session)
            }
            Response::Error { code, message } => {
                session.kill();
                Err(signer_error(code_of(code), message))
            }
            _ => {
                session.kill();
                Err(signer_error(
                    "signer_stopped",
                    "the signer did not greet the app correctly",
                ))
            }
        }
    }

    /// The sandbox layers the signer reported.
    pub fn sandbox(&self) -> &str {
        &self.sandbox
    }

    /// The certificate that was loaded, if one was.
    pub fn certificate(&self) -> Option<&Certificate> {
        self.certificate.as_ref()
    }

    /// Sends one message and waits for the answer. Anything wrong with the connection ends the
    /// signer.
    fn exchange(&mut self, message: &[u8], patience: Duration) -> Result<Response, ClientError> {
        if self.ended {
            return Err(signer_error("signer_stopped", "the signer has ended"));
        }
        if let Err(e) = write_frame(&mut self.stdin, message) {
            self.kill();
            return Err(signer_error(
                "signer_stopped",
                format!("the signer could not be reached ({e})"),
            ));
        }
        match self.heard.recv_timeout(patience) {
            Ok(Heard::Response(response)) => Ok(response),
            Ok(Heard::Failed(why)) => {
                self.kill();
                Err(signer_error(
                    "signer_stopped",
                    format!("its answer was unreadable ({why})"),
                ))
            }
            Ok(Heard::Closed) | Err(RecvTimeoutError::Disconnected) => {
                // The process may still be ending: its exit code tells a start that failed in the
                // system's loader from one that failed in the signer.
                let mut how = String::new();
                for _ in 0..40 {
                    if let Ok(Some(status)) = self.process.try_wait() {
                        how = status;
                        break;
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                self.kill();
                Err(signer_error(
                    "signer_stopped",
                    format!("the signer ended ({how})"),
                ))
            }
            Err(RecvTimeoutError::Timeout) => {
                self.kill();
                Err(signer_error(
                    "signer_stopped",
                    "the signer did not answer in time",
                ))
            }
        }
    }

    /// Hands the certificate file and its password to the signer. The answer says what the
    /// certificate holds. A wrong password leaves the signer ready for another try.
    pub fn load(&mut self, p12: &[u8], password: &str) -> Result<Certificate, ClientError> {
        if p12.len() > MAX_P12_BYTES {
            return Err(signer_error(
                "signer_too_large",
                "the certificate file is too big",
            ));
        }
        if password.len() > MAX_PASSWORD_BYTES {
            return Err(signer_error(
                "signer_wrong_password",
                "the password is too long",
            ));
        }
        let mut request = Request::Load {
            p12: p12.to_vec(),
            password: password.as_bytes().to_vec(),
        };
        let mut message = request.encode();
        // The copy of the password in the request is overwritten at once; the one in the message
        // after it has gone out.
        if let Request::Load { p12, password } = &mut request {
            p12.zeroize();
            password.zeroize();
        }
        let timeout = self.config.load_timeout;
        let outcome = self.exchange(&message, timeout);
        // The message holds the password: it is overwritten whatever came of it.
        message.zeroize();
        match outcome? {
            Response::Loaded(certificate) => {
                self.certificate = Some(certificate.clone());
                Ok(certificate)
            }
            Response::Error { code, message } => Err(signer_error(code_of(code), message)),
            _ => {
                self.kill();
                Err(signer_error(
                    "signer_stopped",
                    "the signer gave an answer that does not fit the request",
                ))
            }
        }
    }

    /// The signature over `digest`, which must be as long as the digest of the key.
    pub fn sign_digest(&mut self, digest: &[u8], hash: HashKind) -> Result<Vec<u8>, ClientError> {
        if digest.len() != hash.len() {
            return Err(signer_error(
                "signer_invalid_request",
                "the digest does not have the length that the key needs",
            ));
        }
        let timeout = self.config.timeout;
        match self.exchange(
            &Request::Sign {
                digest: digest.to_vec(),
            }
            .encode(),
            timeout,
        )? {
            Response::Signed { cms } => Ok(cms),
            Response::Error { code, message } => Err(signer_error(code_of(code), message)),
            _ => {
                self.kill();
                Err(signer_error(
                    "signer_stopped",
                    "the signer gave an answer that does not fit the request",
                ))
            }
        }
    }

    /// Lets the key go and ends the signer.
    pub fn close(mut self) {
        if !self.ended {
            let _ = write_frame(&mut self.stdin, &Request::Close.encode());
        }
        self.kill();
    }

    /// Ends the signer now.
    pub fn kill(&mut self) {
        if !self.ended {
            self.ended = true;
            self.certificate = None;
            let _ = self.process.kill();
            let _ = self.process.wait();
        }
    }
}

impl Drop for SignerSession {
    fn drop(&mut self) {
        self.kill();
    }
}
