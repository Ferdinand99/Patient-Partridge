//! Running the independent tools: qpdf and Poppler. The verifier never loads PDFium and never
//! uses a PDF library of the app, so a bug in the code under test cannot hide in the check.
//!
//! Every call has a time limit, and all the calls for one file share a budget. A tool that runs
//! over is killed with everything it started (its process group on Unix, a job object on Windows;
//! a process that starts a session of its own is out of reach), and the check ends with an error
//! (exit status 2): a file that was not fully checked is never reported clean.

use std::cell::RefCell;
use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::Error;

#[derive(Debug)]
pub struct Output {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

/// The time one call of a tool may take, unless the caller says otherwise.
pub const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(120);

/// The time all the calls for one file may take together, unless the caller says otherwise.
pub const DEFAULT_FILE_BUDGET: Duration = Duration::from_secs(900);

/// The environment variable that sets the time one call may take, in seconds.
pub const TOOL_TIMEOUT_VAR: &str = "VERIFIER_TOOL_TIMEOUT";

/// The environment variable that sets the time all calls for one file may take, in seconds.
pub const FILE_BUDGET_VAR: &str = "VERIFIER_FILE_BUDGET";

/// Seconds from a setting: a number above 0 (a fraction is allowed).
pub fn parse_seconds(text: &str) -> Option<Duration> {
    let v: f64 = text.trim().parse().ok()?;
    (v.is_finite() && v > 0.0 && v < 1e9).then(|| Duration::from_secs_f64(v))
}

fn from_env(var: &str, default: Duration) -> Duration {
    std::env::var(var)
        .ok()
        .and_then(|v| parse_seconds(&v))
        .unwrap_or(default)
}

/// The time one call may take: [`TOOL_TIMEOUT_VAR`], or [`DEFAULT_TOOL_TIMEOUT`].
pub fn tool_timeout_from_env() -> Duration {
    from_env(TOOL_TIMEOUT_VAR, DEFAULT_TOOL_TIMEOUT)
}

/// The time for one file: [`FILE_BUDGET_VAR`], or [`DEFAULT_FILE_BUDGET`].
pub fn file_budget_from_env() -> Duration {
    from_env(FILE_BUDGET_VAR, DEFAULT_FILE_BUDGET)
}

/// The limits of the file that is being checked on this thread.
struct Limits {
    per_call: Duration,
    budget: Duration,
    deadline: Instant,
    /// Why a tool was stopped, once one was.
    stopped: Option<String>,
}

thread_local! {
    static LIMITS: RefCell<Limits> = RefCell::new(Limits {
        per_call: DEFAULT_TOOL_TIMEOUT,
        budget: DEFAULT_FILE_BUDGET,
        deadline: Instant::now() + DEFAULT_FILE_BUDGET,
        stopped: None,
    });
}

/// Starts the clock for one file: a call may take `per_call`, and all calls together `budget`.
pub fn begin(per_call: Duration, budget: Duration) {
    LIMITS.with(|l| {
        *l.borrow_mut() = Limits {
            per_call,
            budget,
            deadline: Instant::now() + budget,
            stopped: None,
        }
    });
}

/// Why a tool was stopped for taking too long, if one was since [`begin`]. A caller must not
/// report a result for the file when there is a reason.
pub fn stopped() -> Option<String> {
    LIMITS.with(|l| l.borrow().stopped.clone())
}

/// The program for `name`, from the environment variable `var` if it is set.
fn program(var: &str, name: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| name.to_owned())
}

/// A Windows job object that holds a tool and everything it starts, so that all of them can be
/// ended at once, whatever they do with their parents. It also ends them when the handle is closed.
#[cfg(windows)]
mod job {
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    pub struct Job(HANDLE);

    impl Job {
        /// A job that kills its processes when it is closed, with the child in it. `None` when
        /// the system refuses; the caller falls back to ending the tree by process number.
        pub fn holding(child: &Child) -> Option<Job> {
            use std::os::windows::io::AsRawHandle;
            // SAFETY: plain calls into the Win32 API with a handle that this function creates and
            // owns (closed by `Drop`), a zeroed information block of the type the call names, and
            // the raw handle of a child that is alive and owned by `child`.
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    return None;
                }
                let job = Job(h);
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    std::ptr::addr_of!(info).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if set == 0 || AssignProcessToJobObject(h, child.as_raw_handle() as HANDLE) == 0 {
                    return None;
                }
                Some(job)
            }
        }

        /// Ends every process of the job.
        pub fn terminate(&self) {
            // SAFETY: the handle is a job object that this value owns.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is owned by this value and closed once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

/// What ends a tool and everything it started: its process group on Unix, a job object on Windows
/// (or its process tree by `taskkill` where no job could be made).
struct Tree {
    pid: u32,
    #[cfg(windows)]
    job: Option<job::Job>,
}

impl Tree {
    fn of(child: &std::process::Child) -> Tree {
        Tree {
            pid: child.id(),
            #[cfg(windows)]
            job: job::Job::holding(child),
        }
    }

    /// Stops everything that the tool started that is still running.
    fn stop(&self) {
        #[cfg(unix)]
        {
            let _ = Command::new("kill")
                .args(["-KILL", "--", &format!("-{}", self.pid)])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        #[cfg(windows)]
        {
            match &self.job {
                Some(j) => j.terminate(),
                None => {
                    let _ = Command::new("taskkill")
                        .args(["/PID", &self.pid.to_string(), "/T", "/F"])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
            }
        }
        #[cfg(not(any(unix, windows)))]
        let _ = self.pid;
    }
}

/// Stops the child and everything it started.
fn kill(child: &mut std::process::Child, tree: &Tree) {
    tree.stop();
    let _ = child.kill();
    let _ = child.wait();
}

fn stop(why: String) -> Error {
    LIMITS.with(|l| {
        l.borrow_mut().stopped.get_or_insert_with(|| why.clone());
    });
    Error(why)
}

fn run(var: &str, name: &str, args: &[&OsStr]) -> Result<Output, Error> {
    run_program(var, program(var, name), args)
}

fn run_program(var: &str, prog: String, args: &[&OsStr]) -> Result<Output, Error> {
    if let Some(why) = stopped() {
        return Err(Error(why));
    }
    let (per_call, budget, left) = LIMITS.with(|l| {
        let l = l.borrow();
        (
            l.per_call,
            l.budget,
            l.deadline.saturating_duration_since(Instant::now()),
        )
    });
    if left.is_zero() {
        return Err(stop(format!(
            "the time budget of {} s for this file is used up before {prog} could run ({FILE_BUDGET_VAR} or --file-budget sets it); the file was not fully checked",
            budget.as_secs_f64()
        )));
    }
    let (limit, by_budget) = if left < per_call {
        (left, true)
    } else {
        (per_call, false)
    };
    let mut cmd = Command::new(&prog);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| {
        Error(format!(
            "cannot run {prog}: {e}. The verifier needs qpdf and Poppler (pdftotext, pdfimages, pdftoppm) on the PATH, \
             or {var} set to the program"
        ))
    })?;
    let tree = Tree::of(&child);
    // The output is read while the tool runs, or a tool that writes a lot would block.
    let (tx, rx) = mpsc::channel::<(bool, Vec<u8>)>();
    let pipes: [(bool, Option<Box<dyn Read + Send>>); 2] = [
        (
            false,
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        ),
        (
            true,
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
        ),
    ];
    for (is_err, pipe) in pipes {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            let _ = tx.send((is_err, buf));
        });
    }
    drop(tx);
    let started = Instant::now();
    let timed_out = |child: &mut std::process::Child| {
        kill(child, &tree);
        stop(if by_budget {
            format!(
                "the time budget of {} s for this file ran out while {prog} was running, and it was killed ({FILE_BUDGET_VAR} or --file-budget sets the budget); the file was not fully checked",
                budget.as_secs_f64()
            )
        } else {
            format!(
                "{prog} took longer than {} s and was killed ({TOOL_TIMEOUT_VAR} or --tool-timeout sets the limit); the file was not fully checked",
                per_call.as_secs_f64()
            )
        })
    };
    let mut wait = Duration::from_millis(1);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if started.elapsed() >= limit {
                    return Err(timed_out(&mut child));
                }
                std::thread::sleep(wait);
                wait = (wait * 2).min(Duration::from_millis(25));
            }
            Err(e) => {
                kill(&mut child, &tree);
                return Err(Error(format!("cannot wait for {prog}: {e}")));
            }
        }
    };
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    for _ in 0..2 {
        // A grandchild that keeps the pipe open is held to the same limit.
        let left = limit.saturating_sub(started.elapsed());
        match rx.recv_timeout(left) {
            Ok((true, b)) => stderr = b,
            Ok((false, b)) => stdout = b,
            Err(_) => return Err(timed_out(&mut child)),
        }
    }
    // Whatever the tool left running in its group or job (a child that holds no pipe) is stopped
    // too.
    tree.stop();
    Ok(Output {
        status: status.code().unwrap_or(-1),
        stdout,
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

pub fn qpdf(args: &[&OsStr]) -> Result<Output, Error> {
    run("QPDF", "qpdf", args)
}

pub fn pdftotext(args: &[&OsStr]) -> Result<Output, Error> {
    run("PDFTOTEXT", "pdftotext", args)
}

pub fn pdfimages(args: &[&OsStr]) -> Result<Output, Error> {
    run("PDFIMAGES", "pdfimages", args)
}

/// Poppler's renderer. It draws the pages that the redacted regions are checked on.
pub fn pdftoppm(args: &[&OsStr]) -> Result<Output, Error> {
    run("PDFTOPPM", "pdftoppm", args)
}

/// The program that draws a page with PDFium for the x-ray render: `redaction-verify-pdfium`,
/// next to this program (or one folder up, where a test binary lives), or the one named by
/// `VERIFIER_PDFIUM_HELPER`.
fn pdfium_helper() -> String {
    if let Ok(p) = std::env::var("VERIFIER_PDFIUM_HELPER") {
        return p;
    }
    let name = format!("redaction-verify-pdfium{}", std::env::consts::EXE_SUFFIX);
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf))
    {
        for d in [Some(dir.clone()), dir.parent().map(Path::to_path_buf)]
            .into_iter()
            .flatten()
        {
            let c = d.join(&name);
            if c.is_file() {
                return c.to_string_lossy().into_owned();
            }
        }
    }
    name
}

/// PDFium's renderer, through the helper program: it draws the x-ray copy of the pages.
pub fn pdfium_render(args: &[&OsStr]) -> Result<Output, Error> {
    run_program("VERIFIER_PDFIUM_HELPER", pdfium_helper(), args)
}

/// The folder with the PDFium library when the caller names none: the variable
/// `VERIFIER_PDFIUM_DIR`, or `vendor/pdfium/<platform>` of the repository that this program was
/// built in (the folder that `scripts/fetch-pdfium.mjs` fills).
pub fn default_pdfium_dir() -> Option<std::path::PathBuf> {
    if let Some(d) = std::env::var_os("VERIFIER_PDFIUM_DIR") {
        return Some(std::path::PathBuf::from(d));
    }
    let id = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("macos", "aarch64") => "mac-arm64",
        ("macos", "x86_64") => "mac-x64",
        ("windows", "x86_64") => "win-x64",
        ("windows", "aarch64") => "win-arm64",
        _ => return None,
    };
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/pdfium")
        .join(id);
    dir.is_dir().then_some(dir)
}

/// Versions of the tools, for the report.
pub fn versions() -> Vec<String> {
    let mut out = Vec::new();
    for (var, name, flag) in [
        ("QPDF", "qpdf", "--version"),
        ("PDFTOTEXT", "pdftotext", "-v"),
    ] {
        if let Ok(o) = run(var, name, &[OsStr::new(flag)]) {
            let text = if o.stdout.is_empty() {
                o.stderr
            } else {
                String::from_utf8_lossy(&o.stdout).into_owned()
            };
            if let Some(line) = text.lines().next() {
                out.push(line.to_owned());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_are_a_number_above_zero() {
        assert_eq!(parse_seconds("120"), Some(Duration::from_secs(120)));
        assert_eq!(parse_seconds(" 0.5 "), Some(Duration::from_millis(500)));
        for bad in ["0", "-1", "", "abc", "inf", "NaN", "1e12"] {
            assert_eq!(parse_seconds(bad), None, "{bad}");
        }
    }
}
