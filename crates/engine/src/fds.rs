//! Closes the descriptors the engine did not ask for.
//!
//! The app starts the engine with the three standard descriptors and the document on descriptor 3.
//! A descriptor that leaked in from the app (a file, a socket, the window system) would stay
//! usable from inside the sandbox, because the sandbox does not take back what is already open.
//! The engine closes everything above the document before it does anything else.

/// Closes every open descriptor above `highest_kept`.
pub fn close_all_above(highest_kept: i32) {
    #[cfg(target_os = "linux")]
    {
        // close_range(2) exists since Linux 5.9.
        // SAFETY: closes descriptors of this process only.
        let rc = unsafe {
            libc::syscall(
                libc::SYS_close_range,
                (highest_kept + 1) as libc::c_uint,
                libc::c_uint::MAX,
                0 as libc::c_uint,
            )
        };
        if rc == 0 {
            return;
        }
    }
    // The list of open descriptors, read completely before anything is closed.
    let dir = if cfg!(target_os = "linux") {
        "/proc/self/fd"
    } else {
        "/dev/fd"
    };
    let open: Vec<i32> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()))
                .collect()
        })
        .unwrap_or_default();
    if open.is_empty() {
        // No list: close the range that can be in use.
        for fd in (highest_kept + 1)..1024 {
            // SAFETY: closing a descriptor that is not open only returns an error.
            unsafe { libc::close(fd) };
        }
        return;
    }
    for fd in open {
        if fd > highest_kept {
            // SAFETY: as above; the descriptor of the folder listing is already closed or invalid.
            unsafe { libc::close(fd) };
        }
    }
}
