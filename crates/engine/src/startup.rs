//! What the engine does between the start of the process and the first request.
//!
//! The order matters, and it is the same for the real engine and for the test-only mode:
//!
//! 1. Close every descriptor that is not the standard three and the document.
//! 2. Take a private copy of the protocol output.
//! 3. Set the memory limit.
//! 4. Load PDFium, the fonts, and warm up the parts of PDFium that start lazily.
//! 5. Apply the sandbox.
//! 6. Only now take hold of the document that the app handed over. Not one byte of it was read
//!    before the sandbox was in force.
//!
//! A release build that cannot apply its sandbox does not serve documents: it answers the greeting
//! with an error that says so and ends. A debug build goes on without a sandbox (and says so in
//! the greeting), so the code can be developed on a system without one.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use partridge_protocol::ErrorCode;

use crate::{limits, sandbox, stdio, Args, Engine};

/// An engine that is ready for its first request.
pub struct Prepared {
    pub engine: Engine,
    /// The writer for the protocol (standard output has been pointed at the null device).
    pub protocol: Box<dyn Write + Send>,
    pub args: Args,
    /// Why the sandbox is not in force, if it is not. Always `None` in a release build that is
    /// serving documents.
    pub sandbox_problem: Option<String>,
    /// Why the empty root (Linux) could not be set up, if it could not. Not an error.
    pub isolation_problem: Option<String>,
}

/// The directory of the running program: where the PDFium library is when no folder is given.
fn own_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
        .unwrap_or_default()
}

#[cfg(unix)]
fn take_document(number: u64) -> Option<File> {
    use std::os::fd::FromRawFd;
    let fd = i32::try_from(number).ok()?;
    // SAFETY: asking about a descriptor number only reads the descriptor table.
    if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
        return None;
    }
    // SAFETY: the app handed this descriptor over for the engine to own; nothing else uses it.
    Some(unsafe { File::from_raw_fd(fd) })
}

#[cfg(windows)]
fn take_document(number: u64) -> Option<File> {
    use std::os::windows::io::FromRawHandle;
    if number == 0 {
        return None;
    }
    // SAFETY: the app handed this handle over for the engine to own; nothing else uses it.
    Some(unsafe { File::from_raw_handle(number as usize as *mut std::ffi::c_void) })
}

#[cfg(not(any(unix, windows)))]
fn take_document(_number: u64) -> Option<File> {
    None
}

/// Quiets the "program stopped working" dialog of Windows when the engine crashes. The app notices
/// the exit by itself and tells the user.
#[cfg(windows)]
fn quiet_crashes() {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        SetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX,
    };
    // SAFETY: SetErrorMode only changes a process flag and has no pointer arguments.
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
    }
}

#[cfg(not(windows))]
fn quiet_crashes() {}

/// Switches for the tests of the start-up itself. They exist in debug builds only, so a release
/// engine cannot be told to skip or fake its sandbox.
#[cfg(debug_assertions)]
fn test_switch(name: &str) -> bool {
    std::env::var_os(name).is_some()
}

#[cfg(not(debug_assertions))]
fn test_switch(_name: &str) -> bool {
    false
}

/// Runs the steps above. `args` is the command line without the program name.
pub fn prepare(args: Args) -> Prepared {
    quiet_crashes();
    #[cfg(unix)]
    crate::fds::close_all_above(args.document.map_or(2, |d| (d as i32).max(2)));
    let protocol = stdio::take_protocol_output();
    let limit_set = limits::limit_memory(args.memory_limit.unwrap_or(limits::MEMORY_LIMIT_BYTES));

    let pdfium_dir = args.pdfium_dir.clone().unwrap_or_else(own_dir);
    let fonts_dir = args
        .fonts_dir
        .clone()
        .unwrap_or_else(|| pdfium_dir.join("fonts"));
    let mut engine = Engine::with_fonts(&pdfium_dir, &fonts_dir);

    // Everything is loaded. The empty root needs a process with one thread, so it comes before the
    // helper thread that watches the parent.
    let isolation_problem = if test_switch("PARTRIDGE_TEST_NO_EMPTY_ROOT") {
        Some("switched off by a test".to_string())
    } else {
        sandbox::enter_isolation().err()
    };
    limits::exit_with_parent();

    // From here on the engine does not open files.
    let locked = if test_switch("PARTRIDGE_TEST_FAIL_SANDBOX") {
        Err("failure forced by a test".to_string())
    } else {
        sandbox::lock_down()
    };
    // A memory limit that could not be set is a sandbox that is not complete.
    let locked = locked.and_then(|applied| {
        if limit_set {
            Ok(applied)
        } else {
            Err("the memory limit could not be set".to_string())
        }
    });
    let sandbox_problem = match locked {
        Ok(applied) => {
            engine.set_sandbox(&applied.describe());
            None
        }
        Err(why) => {
            // The greeting says why, so the app (and a developer) can see the reason.
            engine.set_sandbox(&format!("none: {why}"));
            Some(why)
        }
    };
    // A release build never goes on without a sandbox. A debug build can be told to act like one.
    let enforce = !cfg!(debug_assertions) || test_switch("PARTRIDGE_TEST_ENFORCE");
    match (&sandbox_problem, enforce) {
        (Some(why), true) => engine.refuse(
            ErrorCode::SandboxUnavailable,
            &format!("the sandbox could not be applied: {why}"),
        ),
        // A debug build without a sandbox, or any build with one: the document may be taken.
        _ => {
            if let Some(file) = args.document.and_then(take_document) {
                engine.set_document(file);
            }
        }
    }
    Prepared {
        engine,
        protocol,
        args,
        sandbox_problem,
        isolation_problem,
    }
}
