//! Windows: the restrictions are placed on the engine by the app when it starts the engine (see the
//! `windows` module of `partridge-engine-client`): an AppContainer token with no capabilities made
//! from a restricted token that has only the null SID as restricting SID, the untrusted integrity
//! level, a job object with no child processes and a memory limit, process mitigation policies, and
//! a desktop of its own.
//!
//! A process with such a token cannot even load its own DLLs, so the engine starts with the first
//! thread impersonating a more powerful token of the same package (the user's own rights, as a
//! restricted token). Loading PDFium and the fonts happens in that state. This function ends it
//! (`RevertToSelf`), so the thread falls back to the locked-down token, and then checks every
//! layer: the token is restricted and an AppContainer token, the integrity level is below medium,
//! the process is in a job, and the mitigation policies are on. It tries to open a file, which must
//! fail. And it tries the network: a TCP connection and a UDP datagram to the loopback address. The
//! AppContainer without capabilities makes the system answer both with "access denied" (what the
//! restricted token alone never did). Anything else, including success, means the network is not
//! blocked, and the engine does not run: a release engine reports the sandbox as unavailable.

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::ptr::null_mut;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, FALSE, HANDLE};
use windows_sys::Win32::Security::{
    GetTokenInformation, IsTokenRestricted, RevertToSelf, SetTokenInformation, TokenIntegrityLevel,
    SID_AND_ATTRIBUTES, TOKEN_ADJUST_DEFAULT, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::StationsAndDesktops::{
    GetThreadDesktop, GetUserObjectInformationW, UOI_NAME,
};
use windows_sys::Win32::System::SystemServices::{
    PROCESS_MITIGATION_DYNAMIC_CODE_POLICY, PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThreadId, GetProcessMitigationPolicy, OpenProcessToken,
    ProcessDynamicCodePolicy, ProcessExtensionPointDisablePolicy,
};

const SE_GROUP_INTEGRITY: u32 = 0x0000_0020;

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

        // TokenIsAppContainer (29): non-zero for an AppContainer token.
        let mut is_container = 0u32;
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            own_token(),
            29,
            (&mut is_container as *mut u32).cast::<c_void>(),
            size_of::<u32>() as u32,
            &mut returned,
        );
        if ok == 0 || is_container == 0 {
            return Err("the engine is not in an AppContainer".to_string());
        }
        layers.push("appcontainer");
    }
    Ok(layers)
}

/// True when the engine runs on the desktop the app made for it, not on the user's.
fn on_own_desktop() -> bool {
    // SAFETY: documented calls with live buffers.
    unsafe {
        let desktop = GetThreadDesktop(GetCurrentThreadId());
        if desktop.is_null() {
            return false;
        }
        let mut name = [0u16; 128];
        let mut needed = 0u32;
        if GetUserObjectInformationW(
            desktop,
            UOI_NAME,
            name.as_mut_ptr().cast(),
            (name.len() * 2) as u32,
            &mut needed,
        ) == 0
        {
            return false;
        }
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        String::from_utf16_lossy(&name[..end]).starts_with("PartridgeEngine")
    }
}

/// Lowers the integrity level of this process to untrusted. This has to happen while the thread
/// still has the start identity: the engine's own token is not open to the restricted one.
fn lower_integrity() -> Result<(), String> {
    // SID S-1-16-0: revision 1, one sub-authority, authority 16, sub-authority 0.
    let mut sid = [1u8, 1, 0, 0, 0, 0, 0, 16, 0, 0, 0, 0];
    // SAFETY: documented Win32 calls; the SID and the label outlive the calls.
    unsafe {
        let mut token: HANDLE = null_mut();
        if OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_DEFAULT | TOKEN_QUERY,
            &mut token,
        ) == 0
        {
            return Err(format!("OpenProcessToken: {}", last_error()));
        }
        let label = TOKEN_MANDATORY_LABEL {
            Label: SID_AND_ATTRIBUTES {
                Sid: sid.as_mut_ptr().cast(),
                Attributes: SE_GROUP_INTEGRITY,
            },
        };
        let ok = SetTokenInformation(
            token,
            TokenIntegrityLevel,
            (&label as *const TOKEN_MANDATORY_LABEL).cast::<c_void>(),
            (size_of::<TOKEN_MANDATORY_LABEL>() + sid.len()) as u32,
        );
        let error = last_error();
        CloseHandle(token);
        if ok == 0 {
            return Err(format!("lowering the integrity level: {error}"));
        }
    }
    Ok(())
}

/// Loads the network support while the thread still has the start identity: Winsock reads its
/// provider list from the registry the first time a socket is made, which the locked-down token
/// cannot do. The checks below then get the answer of the network policy, not of a library that
/// could not start. The results do not matter.
fn start_winsock() {
    let _ = UdpSocket::bind("127.0.0.1:0");
    let _ = TcpListener::bind("127.0.0.1:0");
}

/// `WSAEACCES`: what the system answers when the network policy forbids the attempt.
const WSAEACCES: i32 = 10013;

/// What the system said to a network attempt that was meant to fail.
fn refused_by_policy<T>(what: &str, result: std::io::Result<T>) -> Result<(), String> {
    match result {
        Ok(_) => Err(format!("{what} worked: the network is not blocked")),
        Err(e) if e.raw_os_error() == Some(WSAEACCES) => Ok(()),
        Err(e) => Err(format!(
            "{what} failed, but not with the answer of a blocked network (access denied): {e}"
        )),
    }
}

/// Tries to reach the network and expects the system to forbid it. The address is the loopback
/// one: if the policy did not forbid it, nothing leaves the computer and the answer is "refused
/// by the other end", which is not the answer of a blocked network. (A process in an AppContainer
/// without the `internetClient` capability cannot connect to the loopback address either.)
fn network_is_blocked() -> Result<(), String> {
    let address = SocketAddr::from(([127, 0, 0, 1], 9));
    refused_by_policy(
        "a TCP connection",
        TcpStream::connect_timeout(&address, Duration::from_secs(2)),
    )?;
    // A datagram: either the socket cannot be bound or the send is refused.
    match UdpSocket::bind("127.0.0.1:0") {
        Err(e) if e.raw_os_error() == Some(WSAEACCES) => Ok(()),
        Err(e) => Err(format!("a UDP socket could not be made: {e}")),
        Ok(socket) => refused_by_policy("a UDP datagram", socket.send_to(&[0u8], address)),
    }
}

/// Ends the start-up identity and checks that the restrictions are in force.
pub fn lock_down() -> Result<Vec<&'static str>, String> {
    start_winsock();
    lower_integrity()?;
    // SAFETY: no arguments. Ends the impersonation of the first thread, if there is one.
    unsafe {
        RevertToSelf();
    }
    let mut layers = check_layers()?;
    verify()?;
    // The network is the reason for the AppContainer: it is only reported when the system has just
    // refused real attempts, and an engine that cannot show that does not run.
    network_is_blocked()?;
    layers.push("no-network");
    // The desktop is an extra layer: the engines run on the user's desktop where the system would
    // not make one.
    if on_own_desktop() {
        layers.push("own-desktop");
    }
    Ok(layers)
}

/// Tries what the token forbids. If a file can be opened, the sandbox is not in force. (The
/// engine's own program is open to the AppContainer, so it is the restricted token, with the null
/// SID as its only restricting SID, that refuses this.)
fn verify() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if std::fs::File::open(&exe).is_ok() {
        return Err("a file could still be opened".to_string());
    }
    Ok(())
}
