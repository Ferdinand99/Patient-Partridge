//! macOS: a Seatbelt profile, applied with `sandbox_init` and denying everything by default.
//!
//! Browsers do the same for their renderer and PDF processes. The App Sandbox with inheritance
//! (an entitlement on the engine binary) was considered and not chosen: it needs the engine to be
//! signed with `com.apple.security.app-sandbox` and `com.apple.security.inherit`, which makes the
//! rules a property of the signature instead of the code, and it can only be tried in a signed
//! build. `sandbox_init` with our own profile works in an unsigned build too, so the same code runs
//! in CI and in the notarised app, and every rule is in this file.
//!
//! The profile allows no file access (the document is an already open descriptor, and reading one
//! is not a file access), no network, no process creation or execution, and no Mach services. It
//! allows reading system information (page size, number of cores).

use std::ffi::{c_char, c_int, CStr, CString};

extern "C" {
    fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> c_int;
    fn sandbox_free_error(errorbuf: *mut c_char);
}

/// The rules. `(deny default)` first: whatever is not listed is refused.
const PROFILE: &str = r#"(version 1)
(deny default)
(allow sysctl-read)
(allow process-info-pidinfo (target self))
(allow process-info-codesignature (target self))
"#;

fn apply_profile() -> Result<(), String> {
    let profile = CString::new(PROFILE).map_err(|e| e.to_string())?;
    let mut error: *mut c_char = std::ptr::null_mut();
    // SAFETY: `profile` is a live C string; flags 0 means the text is a profile, not a name.
    let rc = unsafe { sandbox_init(profile.as_ptr(), 0, &mut error) };
    if rc == 0 {
        return Ok(());
    }
    let text = if error.is_null() {
        "unknown error".to_string()
    } else {
        // SAFETY: on failure `error` points at a C string owned by the library, freed below.
        let text = unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned();
        unsafe { sandbox_free_error(error) };
        text
    };
    Err(format!("sandbox_init: {text}"))
}

/// Tries what the profile forbids. If any of it works, the sandbox is not in force.
fn verify() -> Result<(), String> {
    // SAFETY: plain calls with constant arguments; descriptors that unexpectedly open are closed.
    unsafe {
        let fd = libc::open(c"/usr/bin/true".as_ptr(), libc::O_RDONLY);
        if fd >= 0 {
            libc::close(fd);
            return Err("a file could still be opened".to_string());
        }
        // Making a socket is not governed by the profile; using it is. Binding one must fail.
        let s = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
        if s >= 0 {
            let mut address: libc::sockaddr_in = std::mem::zeroed();
            address.sin_len = std::mem::size_of::<libc::sockaddr_in>() as u8;
            address.sin_family = libc::AF_INET as libc::sa_family_t;
            address.sin_addr.s_addr = u32::from_ne_bytes([127, 0, 0, 1]);
            let bound = libc::bind(
                s,
                (&address as *const libc::sockaddr_in).cast(),
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            );
            libc::close(s);
            if bound == 0 {
                return Err("a socket could still be bound".to_string());
            }
        }
    }
    Ok(())
}

/// Locks the process in and checks that it worked. Returns the layers that are in force.
pub fn lock_down() -> Result<Vec<&'static str>, String> {
    apply_profile()?;
    verify()?;
    Ok(vec!["seatbelt"])
}
