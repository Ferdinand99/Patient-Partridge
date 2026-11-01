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
//! open a file, which must fail. It also tries to make a socket (Winsock is started first, under
//! the start identity, so that the answer is the system's) and to open the network device itself
//! with `NtCreateFile`, but only to find out whether the network is blocked. It is not: the
//! restricted token does not stop the network on Windows, so those two attempts succeed, the layer
//! `no-network` is not reported and the engine goes on.

use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{CloseHandle, FALSE, HANDLE};
use windows_sys::Win32::Networking::WinSock::{
    closesocket, socket, WSAStartup, AF_INET, INVALID_SOCKET, SOCK_DGRAM, WSADATA,
};
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
    }
    Ok(layers)
}

/// Starts Winsock while the thread still has the start identity, so that the network check below
/// can make a real attempt at a socket and the refusal it gets is the system's, not a library that
/// could not start.
fn start_winsock() -> bool {
    // SAFETY: documented call with a live structure.
    unsafe {
        let mut data: WSADATA = zeroed();
        WSAStartup(0x0202, &mut data) == 0
    }
}

/// Opens `\Device\Afd\Endpoint` with `NtCreateFile`, the first thing creating a socket does,
/// without Winsock. Returns the status.
fn open_network_device() -> i32 {
    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *const u16,
    }
    #[repr(C)]
    struct ObjectAttributes {
        length: u32,
        root_directory: *mut c_void,
        object_name: *const UnicodeString,
        attributes: u32,
        security_descriptor: *const c_void,
        security_quality_of_service: *const c_void,
    }
    #[repr(C)]
    struct IoStatusBlock {
        status: isize,
        information: usize,
    }
    #[link(name = "ntdll")]
    extern "system" {
        fn NtCreateFile(
            handle: *mut *mut c_void,
            access: u32,
            attributes: *const ObjectAttributes,
            io: *mut IoStatusBlock,
            allocation: *const i64,
            file_attributes: u32,
            share: u32,
            disposition: u32,
            options: u32,
            ea: *const c_void,
            ea_length: u32,
        ) -> i32;
    }
    let path: Vec<u16> = r"\Device\Afd\Endpoint".encode_utf16().collect();
    let name = UnicodeString {
        length: (path.len() * 2) as u16,
        maximum_length: (path.len() * 2) as u16,
        buffer: path.as_ptr(),
    };
    let attributes = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root_directory: null_mut(),
        object_name: &name,
        attributes: 0x40, // OBJ_CASE_INSENSITIVE
        security_descriptor: std::ptr::null(),
        security_quality_of_service: std::ptr::null(),
    };
    let mut handle: *mut c_void = null_mut();
    let mut io = IoStatusBlock {
        status: 0,
        information: 0,
    };
    // SAFETY: documented native call with live structures; a handle that opens is closed again.
    unsafe {
        let status = NtCreateFile(
            &mut handle,
            0xC010_0000, // GENERIC_READ | GENERIC_WRITE | SYNCHRONIZE
            &attributes,
            &mut io,
            std::ptr::null(),
            0,
            3,
            1, // FILE_OPEN
            0,
            std::ptr::null(),
            0,
        );
        if status >= 0 && !handle.is_null() {
            CloseHandle(handle);
        }
        status
    }
}

/// Tries what the token forbids. If a file can be opened, the sandbox is not in force.
fn verify() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if std::fs::File::open(&exe).is_ok() {
        return Err("a file could still be opened".to_string());
    }
    Ok(())
}

/// Whether the network is out of reach: no socket can be made and the device that sockets are made
/// from cannot be opened. The CI run of this check found that the restricted token does **not** do
/// this on Windows (a socket can be made, and the device can be opened), so the engine does not
/// report the layer and the README says the engine can use the network on Windows. Only an
/// AppContainer stops it; see the README.
fn network_is_blocked(winsock_started: bool) -> bool {
    if winsock_started {
        // SAFETY: documented calls; a socket that is made is closed again.
        unsafe {
            let s = socket(AF_INET as i32, SOCK_DGRAM, 0);
            if s != INVALID_SOCKET {
                closesocket(s);
                return false;
            }
        }
    }
    open_network_device() < 0
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

/// Ends the start-up identity and checks that the restrictions are in force.
pub fn lock_down() -> Result<Vec<&'static str>, String> {
    let winsock_started = start_winsock();
    lower_integrity()?;
    // SAFETY: no arguments. Ends the impersonation of the first thread, if there is one.
    unsafe {
        RevertToSelf();
    }
    let mut layers = check_layers()?;
    verify()?;
    if network_is_blocked(winsock_started) {
        layers.push("no-network");
    }
    // The desktop is an extra layer: the engines run on the user's desktop where the system would
    // not make one.
    if on_own_desktop() {
        layers.push("own-desktop");
    }
    Ok(layers)
}
