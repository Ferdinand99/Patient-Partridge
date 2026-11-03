//! What the signer does between the start of the process and the first request.
//!
//! The same order as the engine, for the same reasons (see `partridge-engine`, `startup`):
//!
//! 1. Close every descriptor that is not one of the standard three.
//! 2. Take a private copy of the protocol output, and point standard output at the null device.
//! 3. Set the memory limit and switch off core files (a core file would hold the key).
//! 4. Warm up what is first used lazily and may need a file (the source of random numbers).
//! 5. Apply the sandbox: the empty root and Landlock while the process has one thread, then the
//!    thread that watches the parent, then the system call filter on all threads.
//!
//! A release build that cannot apply its sandbox does not serve: it answers the greeting with an
//! error that says so and ends. A debug build goes on without one (and says so in the greeting),
//! so the code can be developed on a system that has none.

use std::io::Write;

use partridge_sandbox::{limits, sandbox, stdio};
use ring::rand::{SecureRandom, SystemRandom};

use crate::Signer;

/// Most memory the signer may use: a certificate file is at most a megabyte, and opening it takes
/// a small part of that.
pub const MEMORY_LIMIT_BYTES: u64 = 512 * 1024 * 1024;

/// A signer that is ready for its first request.
pub struct Prepared {
    pub signer: Signer,
    /// The writer for the protocol (standard output has been pointed at the null device).
    pub protocol: Box<dyn Write + Send>,
    /// Why the sandbox is not in force, if it is not.
    pub sandbox_problem: Option<String>,
}

/// Switches for the tests of the start-up itself. They exist in debug builds only, so a release
/// signer cannot be told to skip or fake its sandbox.
#[cfg(debug_assertions)]
fn test_switch(name: &str) -> bool {
    std::env::var_os(name).is_some()
}

#[cfg(not(debug_assertions))]
fn test_switch(_name: &str) -> bool {
    false
}

/// Runs the steps above.
pub fn prepare() -> Prepared {
    #[cfg(unix)]
    partridge_sandbox::fds::close_all_above(2);
    let protocol = stdio::take_protocol_output();
    let limit_set = limits::limit_memory(MEMORY_LIMIT_BYTES);

    // The first use of the random number source can open a file; after the sandbox it could not.
    let mut warm = [0u8; 8];
    let _ = SystemRandom::new().fill(&mut warm);

    // Everything is loaded. The empty root needs a process with one thread, so it comes before the
    // helper thread that watches the parent. Landlock holds for the thread that applies it and the
    // threads started after it, so it comes before them too.
    let _ = sandbox::enter_isolation();
    let _ = sandbox::restrict_files();
    limits::exit_with_parent();

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
    let (report, sandbox_problem) = match locked {
        Ok(applied) => (applied.describe(), None),
        Err(why) => (format!("none: {why}"), Some(why)),
    };
    let mut signer = Signer::new(&report);
    // A release build never goes on without a sandbox. A debug build can be told to act like one.
    let enforce = !cfg!(debug_assertions) || test_switch("PARTRIDGE_TEST_ENFORCE");
    if let (Some(why), true) = (&sandbox_problem, enforce) {
        signer.refuse(&format!("the sandbox could not be applied: {why}"));
    }
    Prepared {
        signer,
        protocol,
        sandbox_problem,
    }
}
