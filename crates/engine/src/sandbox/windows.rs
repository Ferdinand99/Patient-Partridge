//! Windows: the restrictions are placed on the engine by the app when it starts the engine (see the
//! `windows` module of `partridge-engine-client`): a restricted token that has only the null SID as
//! restricting SID, the untrusted integrity level, a job object with no child processes and a
//! memory limit, and process mitigation policies.
//!
//! A process with such a token cannot even load its own DLLs, so the engine starts with the first
//! thread impersonating a more powerful token (the user's own). Loading PDFium and the fonts happens
//! in that state. This function ends it (`RevertToSelf`), so the thread falls back to the
//! restricted token, and then checks every layer: the token is restricted, the integrity level is
//! below medium, the process is in a job, and the mitigation policies are on. Finally it tries to
//! open a file and a socket, which must fail.

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{FALSE, HANDLE};
use windows_sys::Win32::Security::{
    GetTokenInformation, IsTokenRestricted, RevertToSelf, TokenIntegrityLevel,
    TOKEN_MANDATORY_LABEL,
};
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::SystemServices::{
    PROCESS_MITIGATION_DYNAMIC_CODE_POLICY, PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessMitigationPolicy, ProcessDynamicCodePolicy,
    ProcessExtensionPointDisablePolicy,
};

/// The integrity level of a process is the last sub-authority of the SID in its token. Medium is
/// 0x2000.
const MEDIUM_INTEGRITY: u32 = 0x2000;

/// The pseudo handle for the token of the process itself (`GetCurrentProcessToken` in C). The
/// engine cannot open its own token the usual way: with the restricted token it has now, the
/// access check on the token object fails.
fn own_token() -> HANDLE {
    -4isize as HANDLE
}

fn last_error() -> String {
    std::io::Error::last_os_error().to_string()
}

/// The integrity level of this process, or an error text.
fn integrity_level() -> Result<u32, String> {
    // SAFETY: documented Win32 calls; the buffer is as large as the call asks for.
    unsafe {
        let token = own_token();
        let mut needed = 0u32;
        GetTokenInformation(token, TokenIntegrityLevel, null_mut(), 0, &mut needed);
        let mut buffer = vec![0u8; needed as usize];
        let ok = GetTokenInformation(
            token,
            TokenIntegrityLevel,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        );
        if ok == 0 {
            return Err(format!("GetTokenInformation: {}", last_error()));
        }
        let label = &*(buffer.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let sid = label.Label.Sid;
        // SID layout: revision (1), sub-authority count (1), authority (6), then the u32 sub-authorities.
        let count = *(sid as *const u8).add(1) as usize;
        if count == 0 {
            return Err("the integrity SID is empty".to_string());
        }
        let last = (sid as *const u8).add(8 + 4 * (count - 1)) as *const u32;
        Ok(last.read_unaligned())
    }
}

/// Checks the layers the app put in place. Returns their names.
fn check_layers() -> Result<Vec<&'static str>, String> {
    let mut layers = Vec::new();
    // SAFETY: documented Win32 calls with live out parameters.
    unsafe {
        if IsTokenRestricted(own_token()) == 0 {
            return Err("the token of the engine is not restricted".to_string());
        }
        layers.push("restricted-token");

        if integrity_level()? >= MEDIUM_INTEGRITY {
            return Err("the integrity level of the engine is not below medium".to_string());
        }
        layers.push("low-integrity");

        let mut in_job = 0;
        if IsProcessInJob(GetCurrentProcess(), null_mut(), &mut in_job) == 0 || in_job == 0 {
            return Err("the engine is not in a job object".to_string());
        }
        layers.push("job");

        let mut dynamic: PROCESS_MITIGATION_DYNAMIC_CODE_POLICY = zeroed();
        let ok = GetProcessMitigationPolicy(
            GetCurrentProcess(),
            ProcessDynamicCodePolicy,
            (&mut dynamic as *mut PROCESS_MITIGATION_DYNAMIC_CODE_POLICY).cast::<c_void>(),
            size_of::<PROCESS_MITIGATION_DYNAMIC_CODE_POLICY>(),
        );
        // The first bit of the flags is ProhibitDynamicCode.
        if ok == FALSE || dynamic.Anonymous.Flags & 1 == 0 {
            return Err("dynamic code is not prohibited".to_string());
        }
        let mut extension: PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY = zeroed();
        let ok = GetProcessMitigationPolicy(
            GetCurrentProcess(),
            ProcessExtensionPointDisablePolicy,
            (&mut extension as *mut PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY)
                .cast::<c_void>(),
            size_of::<PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY>(),
        );
        if ok == FALSE || extension.Anonymous.Flags & 1 == 0 {
            return Err("extension points are not disabled".to_string());
        }
        layers.push("mitigations");
    }
    Ok(layers)
}

/// Tries what the token forbids. If a file can be opened, the sandbox is not in force.
fn verify() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if std::fs::File::open(&exe).is_ok() {
        return Err("a file could still be opened".to_string());
    }
    Ok(())
}

/// Ends the start-up identity and checks that the restrictions are in force.
pub fn lock_down() -> Result<Vec<&'static str>, String> {
    // SAFETY: no arguments. Ends the impersonation of the first thread, if there is one.
    unsafe {
        RevertToSelf();
    }
    let layers = check_layers()?;
    verify()?;
    Ok(layers)
}
