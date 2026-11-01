//! Keeps the protocol channel clean. The engine talks to the app over standard output, so any
//! stray write to it (a library that prints, a debug line left in a dependency) would turn into
//! garbage in the middle of a frame and end the session. At start the engine takes a private copy
//! of the real output for the protocol and points standard output at the null device, so whatever
//! else writes there goes nowhere.
//!
//! On Windows this covers Rust's `print!` and `println!`, which look the handle up on each call.
//! A C runtime that wrote to its own copy of the handle would not be caught; PDFium's release
//! builds do not print.

use std::io::{self, Write};

/// The writer for the protocol. Call once, before anything else could write to standard output.
pub fn take_protocol_output() -> Box<dyn Write + Send> {
    match redirect() {
        Some(file) => Box::new(file),
        None => Box::new(io::stdout()),
    }
}

#[cfg(unix)]
fn redirect() -> Option<std::fs::File> {
    use std::fs::{File, OpenOptions};
    use std::os::fd::{AsRawFd, FromRawFd};

    extern "C" {
        fn dup(fd: i32) -> i32;
        fn dup2(old: i32, new: i32) -> i32;
    }
    let null = OpenOptions::new().write(true).open("/dev/null").ok()?;
    // SAFETY: plain system calls on file descriptors this function owns or that stay valid for the
    // life of the process. The copy of descriptor 1 is wrapped in a `File` that owns it.
    unsafe {
        let copy = dup(1);
        if copy < 0 {
            return None;
        }
        if dup2(null.as_raw_fd(), 1) < 0 {
            drop(File::from_raw_fd(copy));
            return None;
        }
        Some(File::from_raw_fd(copy))
    }
}

#[cfg(windows)]
fn redirect() -> Option<std::fs::File> {
    use std::ffi::c_void;
    use std::fs::{File, OpenOptions};
    use std::os::windows::io::{AsRawHandle, FromRawHandle};

    extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn SetStdHandle(which: u32, handle: *mut c_void) -> i32;
    }
    // STD_OUTPUT_HANDLE is (DWORD)-11.
    const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;
    let null = OpenOptions::new().write(true).open("NUL").ok()?;
    // SAFETY: documented Win32 calls. The real output handle moves into a `File`; the null device
    // handle is kept open on purpose, because standard output now points at it.
    unsafe {
        let real = GetStdHandle(STD_OUTPUT_HANDLE);
        if real.is_null() || real as isize == -1 {
            return None;
        }
        if SetStdHandle(STD_OUTPUT_HANDLE, null.as_raw_handle() as *mut c_void) == 0 {
            return None;
        }
        std::mem::forget(null);
        Some(File::from_raw_handle(real))
    }
}

#[cfg(not(any(unix, windows)))]
fn redirect() -> Option<std::fs::File> {
    None
}
