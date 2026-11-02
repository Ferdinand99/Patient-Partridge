//! Starting the engine process.
//!
//! The app opens the document and gives the engine an inherited, read-only handle to it. The engine
//! gets exactly four things: standard input and output for the protocol, a null device for standard
//! error, and the document. On Windows the engine is also started with a restricted token, the
//! untrusted integrity level, its own job object and process mitigation policies (see `windows`).
//! On Unix the engine locks itself in after it has loaded what it needs (see `partridge-engine`).

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;

use partridge_protocol::FontScript;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

/// Descriptor number the document has in the engine process (Unix). The engine is told this number
/// with `--document`.
#[cfg(unix)]
pub const DOCUMENT_DESCRIPTOR: i32 = 3;

/// How far the app insists on the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxPolicy {
    /// The engine must run in a sandbox. An engine that does not report one is refused, and on
    /// Windows an engine that cannot be started with its restrictions is not started at all.
    Required,
    /// As `Required` when it works, but an engine without a sandbox is used anyway. For development
    /// and for tests of the client that run stand-ins for the engine.
    BestEffort,
}

impl SandboxPolicy {
    /// `Required` in a release build, `BestEffort` in a debug build.
    pub fn default_for_build() -> SandboxPolicy {
        if cfg!(debug_assertions) {
            SandboxPolicy::BestEffort
        } else {
            SandboxPolicy::Required
        }
    }
}

/// What it takes to start one engine.
pub struct SpawnRequest<'a> {
    pub program: &'a Path,
    pub args: &'a [OsString],
    /// The document, opened for reading by the app. The engine gets its own copy of the handle.
    pub document: Option<&'a File>,
    /// Most memory the engine may use, in bytes. `None` leaves the engine's default.
    pub memory_limit: Option<u64>,
    pub policy: SandboxPolicy,
    /// System fonts, each a single file opened read-only by the app (see `system_fonts`). The
    /// engine gets its own copy of every handle.
    pub fonts: &'a [(FontScript, File)],
}

/// A running engine process.
pub trait EngineProcess: Send {
    fn id(&self) -> u32;
    /// Ends the process. Ending one that has ended is not an error.
    fn kill(&mut self) -> io::Result<()>;
    /// Waits until the process has ended.
    fn wait(&mut self) -> io::Result<()>;
    /// Says how the process ended, or `None` when it still runs.
    fn try_wait(&mut self) -> io::Result<Option<String>>;
    /// Called when the engine has answered its greeting, so it has started up. Windows lets the
    /// engine use all the processors of the app from here on (it starts on one, see `windows`).
    fn started(&mut self) {}
}

/// An engine and its two pipes.
pub struct SpawnedEngine {
    pub process: Box<dyn EngineProcess>,
    pub stdin: Box<dyn Write + Send>,
    pub stdout: Box<dyn Read + Send>,
}

/// Starts the engine. With `Required` the restrictions of the platform are not optional.
pub fn spawn_engine(request: &SpawnRequest<'_>) -> io::Result<SpawnedEngine> {
    #[cfg(unix)]
    {
        unix::spawn(request)
    }
    #[cfg(windows)]
    {
        match windows::spawn(request) {
            Ok(engine) => Ok(engine),
            Err(why) if request.policy == SandboxPolicy::BestEffort => {
                // Development convenience: a plain process, without the restrictions. The engine
                // reports that it has no sandbox, and a debug build of the app goes on.
                let _ = why;
                windows::spawn_plain(request)
            }
            Err(why) => Err(why),
        }
    }
}

/// The command line options that tell the engine about the handle and the limit.
pub(crate) fn engine_args(
    request: &SpawnRequest<'_>,
    document: Option<u64>,
    fonts: &[(FontScript, u64)],
) -> Vec<OsString> {
    let mut args: Vec<OsString> = request.args.to_vec();
    if let Some(document) = document {
        args.push("--document".into());
        args.push(document.to_string().into());
    }
    for (script, number) in fonts {
        args.push("--font".into());
        args.push(format!("{}={number}", script.name()).into());
    }
    if let Some(limit) = request.memory_limit {
        args.push("--memory-limit".into());
        args.push(limit.to_string().into());
    }
    args
}

/// Memory the engine may use when the caller names no limit: 3 GiB.
#[cfg(any(windows, target_os = "macos"))]
pub const DEFAULT_MEMORY_LIMIT: u64 = 3 * 1024 * 1024 * 1024;

/// The variables of the app's environment that the engine gets. Nothing else: not the tokens of the
/// shell the app was started from, not `SSH_AUTH_SOCK` or `DBUS_SESSION_BUS_ADDRESS`, not `HOME`
/// or the user name. Windows needs the system folder to find its own libraries.
#[cfg(windows)]
const ENGINE_ENVIRONMENT: &[&str] = &["SystemRoot", "windir"];
#[cfg(not(windows))]
const ENGINE_ENVIRONMENT: &[&str] = &[];

/// The environment of the engine: the allowed variables of the app's environment, and in debug
/// builds the switches of the tests (`PARTRIDGE_TEST_*`). A release build passes no switch.
pub(crate) fn engine_environment() -> Vec<(OsString, OsString)> {
    std::env::vars_os()
        .filter(|(name, _)| {
            let name = name.to_string_lossy();
            let allowed = ENGINE_ENVIRONMENT
                .iter()
                .any(|a| a.eq_ignore_ascii_case(&name));
            allowed || (cfg!(debug_assertions) && name.starts_with("PARTRIDGE_TEST_"))
        })
        .collect()
}

/// Tests only (debug builds, Windows): lets the AppContainer the engines run in read and write one
/// file. Stand-ins for the engine in the tests of the client do not lock themselves in, and
/// without this a stand-in could not write the marker file some of the tests use.
#[cfg(all(windows, debug_assertions))]
pub fn allow_stand_in_to_use(path: &Path) -> io::Result<()> {
    windows::allow_read_write(path)
}
