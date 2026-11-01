//! The app side of the engine connection.
//!
//! [`EngineClient`] starts `partridge-engine` as a child process, speaks the protocol with it, and
//! keeps going when the engine does not: if the process crashes, hangs, exits or sends something
//! unreadable, the current request fails with [`ClientError::EngineStopped`], the process is
//! cleaned up, and the next request starts a fresh engine. A bad PDF can therefore never take the
//! app down. Documents that were open in the old engine are gone; asking for one returns an
//! `UnknownDocument` error from the new engine.

use std::ffi::OsString;
use std::fmt;
use std::io::BufReader;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use partridge_protocol::{
    read_frame, write_request, ErrorCode, PageSize, Request, RequestBody, Response, ResponseBody,
    PROTOCOL_VERSION,
};

/// How to start and supervise the engine.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Longest the engine may take to answer one request before it is considered hung and killed.
    pub timeout: Duration,
}

impl EngineConfig {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        EngineConfig {
            program: program.into(),
            args: Vec::new(),
            timeout: Duration::from_secs(60),
        }
    }
}

#[derive(Debug)]
pub enum ClientError {
    /// The engine process could not be started, died, hung, or sent unreadable data. It has been
    /// stopped; the next request starts a new one.
    EngineStopped(String),
    /// The engine is fine and refused or failed this one request.
    Engine { code: ErrorCode, message: String },
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::EngineStopped(why) => write!(f, "the PDF engine stopped: {why}"),
            ClientError::Engine { message, .. } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// What the engine reported about itself in the handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineInfo {
    pub engine_version: String,
    pub pdfium_version: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenedDocument {
    pub doc: u32,
    pub pages: Vec<PageSize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

enum Event {
    Response(Response),
    Failed(String),
    Closed,
}

struct Running {
    child: Child,
    stdin: ChildStdin,
    events: Receiver<Event>,
    info: EngineInfo,
}

#[derive(Default)]
struct Inner {
    running: Option<Running>,
    next_id: u32,
    spawns: u64,
}

pub struct EngineClient {
    config: EngineConfig,
    inner: Mutex<Inner>,
}

impl EngineClient {
    /// Creates a client. The engine is started by the first request, not here.
    pub fn new(config: EngineConfig) -> Self {
        EngineClient {
            config,
            inner: Mutex::new(Inner::default()),
        }
    }

    /// How many engine processes this client has started. A jump tells that the engine was replaced.
    pub fn spawn_count(&self) -> u64 {
        self.lock().spawns
    }

    /// Process id of the running engine, if there is one. Used by the measurement tools.
    pub fn pid(&self) -> Option<u32> {
        self.lock().running.as_ref().map(|r| r.child.id())
    }

    /// Starts the engine if needed and reports what it is.
    pub fn info(&self) -> Result<EngineInfo, ClientError> {
        let mut inner = self.lock();
        self.ensure_running(&mut inner)?;
        Ok(inner.running.as_ref().expect("just started").info.clone())
    }

    pub fn open(&self, path: &str, password: Option<&str>) -> Result<OpenedDocument, ClientError> {
        match self.request(RequestBody::Open {
            path: path.to_string(),
            password: password.map(str::to_string),
        })? {
            ResponseBody::Opened { doc, pages } => Ok(OpenedDocument { doc, pages }),
            _ => self.unexpected("open"),
        }
    }

    pub fn render(
        &self,
        doc: u32,
        page: u32,
        width: u32,
        height: u32,
    ) -> Result<Bitmap, ClientError> {
        match self.request(RequestBody::Render {
            doc,
            page,
            width,
            height,
        })? {
            ResponseBody::Rendered {
                width,
                height,
                rgba,
            } => Ok(Bitmap {
                width,
                height,
                rgba,
            }),
            _ => self.unexpected("render"),
        }
    }

    pub fn close(&self, doc: u32) -> Result<(), ClientError> {
        match self.request(RequestBody::Close { doc })? {
            ResponseBody::Closed => Ok(()),
            _ => self.unexpected("close"),
        }
    }

    /// Stops the engine process now. The next request starts a new one.
    pub fn kill(&self) {
        let mut inner = self.lock();
        stop(&mut inner);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves nothing half-done that matters: the engine is
        // restarted on the next request anyway.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// An answer of the wrong kind means the two sides disagree: treat the engine as broken.
    fn unexpected<T>(&self, what: &str) -> Result<T, ClientError> {
        self.kill();
        Err(ClientError::EngineStopped(format!(
            "it gave an unexpected answer to {what}"
        )))
    }

    fn request(&self, body: RequestBody) -> Result<ResponseBody, ClientError> {
        let mut inner = self.lock();
        self.ensure_running(&mut inner)?;
        match self.exchange(&mut inner, body) {
            Ok(ResponseBody::Error { code, message }) => Err(ClientError::Engine { code, message }),
            Ok(other) => Ok(other),
            Err(why) => {
                stop(&mut inner);
                Err(ClientError::EngineStopped(why))
            }
        }
    }

    /// Sends one request and waits for its answer. An `Err` means the connection is unusable.
    fn exchange(&self, inner: &mut Inner, body: RequestBody) -> Result<ResponseBody, String> {
        inner.next_id = inner.next_id.wrapping_add(1);
        let id = inner.next_id;
        let running = inner.running.as_mut().ok_or("it is not running")?;
        write_request(&mut running.stdin, &Request { id, body })
            .map_err(|e| format!("the request could not be sent ({e})"))?;
        // One request is in flight at a time, so the next event is its answer or a failure.
        match running.events.recv_timeout(self.config.timeout) {
            Ok(Event::Response(r)) if r.id == id => Ok(r.body),
            Ok(Event::Response(_)) => Err("it answered a different request".into()),
            Ok(Event::Failed(why)) => Err(format!("its answer was unreadable ({why})")),
            Ok(Event::Closed) | Err(RecvTimeoutError::Disconnected) => {
                Err(match running.child.try_wait() {
                    Ok(Some(status)) => format!("it exited ({status})"),
                    _ => "it exited".to_string(),
                })
            }
            Err(RecvTimeoutError::Timeout) => Err(format!(
                "it did not answer within {} seconds",
                self.config.timeout.as_secs_f32().ceil()
            )),
        }
    }

    fn ensure_running(&self, inner: &mut Inner) -> Result<(), ClientError> {
        if inner.running.is_some() {
            return Ok(());
        }
        let mut command = Command::new(&self.config.program);
        command
            .args(&self.config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console window flashes up next to the app.
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|e| {
            ClientError::EngineStopped(format!(
                "{} could not be started ({e})",
                self.config.program.display()
            ))
        })?;
        inner.spawns += 1;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let (tx, events) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let event = match read_frame(&mut reader) {
                    Ok(Some(frame)) => match Response::decode(frame) {
                        Ok(response) => Event::Response(response),
                        Err(e) => Event::Failed(e.to_string()),
                    },
                    Ok(None) => Event::Closed,
                    Err(e) => Event::Failed(e.to_string()),
                };
                let last = !matches!(event, Event::Response(_));
                if tx.send(event).is_err() || last {
                    return;
                }
            }
        });
        inner.running = Some(Running {
            child,
            stdin,
            events,
            info: EngineInfo {
                engine_version: String::new(),
                pdfium_version: String::new(),
            },
        });

        match self.exchange(
            inner,
            RequestBody::Hello {
                protocol_version: PROTOCOL_VERSION,
            },
        ) {
            Ok(ResponseBody::Hello {
                protocol_version,
                engine_version,
                pdfium_version,
            }) if protocol_version == PROTOCOL_VERSION => {
                inner.running.as_mut().expect("just started").info = EngineInfo {
                    engine_version,
                    pdfium_version,
                };
                Ok(())
            }
            Ok(ResponseBody::Error { code, message }) => {
                stop(inner);
                Err(ClientError::Engine { code, message })
            }
            Ok(_) => {
                stop(inner);
                Err(ClientError::EngineStopped(
                    "it did not greet the app correctly".into(),
                ))
            }
            Err(why) => {
                stop(inner);
                Err(ClientError::EngineStopped(why))
            }
        }
    }
}

impl Drop for EngineClient {
    fn drop(&mut self) {
        let inner = self.inner.get_mut().unwrap_or_else(|e| e.into_inner());
        stop(inner);
    }
}

/// Kills the engine process and waits for it so no zombie is left behind.
fn stop(inner: &mut Inner) {
    if let Some(mut running) = inner.running.take() {
        let _ = running.child.kill();
        let _ = running.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::time::Instant;

    fn is_stopped(err: &ClientError) -> bool {
        matches!(err, ClientError::EngineStopped(_))
    }

    #[test]
    fn a_missing_program_is_reported_not_panicked() {
        let client = EngineClient::new(EngineConfig::new("/definitely/not/an/engine"));
        let err = client.open("a.pdf", None).unwrap_err();
        assert!(is_stopped(&err), "{err:?}");
        // And again: the failure leaves no half-started state behind.
        assert!(is_stopped(&client.info().unwrap_err()));
    }

    #[test]
    fn unreadable_output_stops_the_engine() {
        // The test program itself prints text when asked for help: not a valid frame.
        let mut config = EngineConfig::new(std::env::current_exe().unwrap());
        config.args = vec!["--help".into()];
        let client = EngineClient::new(config);
        let err = client.open("a.pdf", None).unwrap_err();
        assert!(is_stopped(&err), "{err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn an_engine_that_exits_at_once_is_reported() {
        let client = EngineClient::new(EngineConfig::new("true"));
        let err = client.open("a.pdf", None).unwrap_err();
        assert!(is_stopped(&err), "{err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_hung_engine_is_killed_after_the_timeout() {
        let mut config = EngineConfig::new("sleep");
        config.args = vec!["30".into()];
        config.timeout = Duration::from_millis(300);
        let client = EngineClient::new(config);
        let started = Instant::now();
        let err = client.open("a.pdf", None).unwrap_err();
        assert!(is_stopped(&err), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(err.to_string().contains("did not answer"), "{err}");
    }
}
