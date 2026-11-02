//! Starting the engine on Windows, with the restrictions that make it a sandbox:
//!
//! - **Restricted token.** The engine's primary token is a copy of the app's token with every
//!   privilege removed, every group (and the user) turned into deny-only, and one restricting SID,
//!   the null SID, which no object grants access to. With such a token almost no securable object
//!   can be opened: no file, no registry key, no named pipe, no other process. It does not stop the
//!   network by itself: a process with only this token can still make a socket and connect.
//! - **AppContainer without capabilities** (see `container`). The same two tokens (the engine's
//!   and the one its first thread starts with) are turned into tokens of a package that holds no
//!   capability. The system's network filters refuse every network access of such a process:
//!   connect, send and name lookup, loopback included. This is what blocks the network. The
//!   restricted token stays under it: an object must be open to the restricting SID and to the
//!   package.
//! - **Untrusted integrity level**, below everything the user owns (set by the engine on its own
//!   token just before it gives up the start identity).
//! - **A job object of its own** (so no race with another job): a single process, no child
//!   processes, a memory limit, ended when the app lets go of it, and no access to the desktop,
//!   clipboard, user handles, global atoms or system parameters.
//! - **Process mitigation policies**: no dynamic code, no extension points,
//!   strict handle checks, bottom-up and high-entropy ASLR, heap termination on corruption, no
//!   remote or low-label images, no child processes.
//! - **An explicit handle list**: the engine inherits exactly its standard input, output and error
//!   and the document, and nothing else of the app's handles.
//!
//! A process with that token cannot load its own DLLs, so the first thread of the engine starts out
//! impersonating a copy of the app's own token (the user's rights) that is an AppContainer token of
//! the same package. The engine loads PDFium with that, then calls
//! `RevertToSelf` (see `partridge-engine`), after which the thread has the restricted token like the
//! rest of the process. The engine reads nothing of the document before that.
//!
//! The handles are inheritable only for the moment of `CreateProcess`, and the document handle is a
//! duplicate that is closed again afterwards.

use std::ffi::{c_void, OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::ptr::{null, null_mut};
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::Foundation::{
    CloseHandle, DuplicateHandle, SetHandleInformation, DUPLICATE_SAME_ACCESS, HANDLE,
    HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, CreateRestrictedToken, DuplicateTokenEx, GetTokenInformation,
    LookupPrivilegeValueW, SecurityImpersonation, TokenGroups, TokenImpersonation, TokenPrimary,
    TokenUser, DISABLE_MAX_PRIVILEGE, LUID_AND_ATTRIBUTES, SECURITY_ATTRIBUTES, SID_AND_ATTRIBUTES,
    TOKEN_ADJUST_DEFAULT, TOKEN_ALL_ACCESS, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_GROUPS,
    TOKEN_PRIVILEGES, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicUIRestrictions,
    JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_BASIC_UI_RESTRICTIONS,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOB_OBJECT_UILIMIT_DESKTOP,
    JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
    JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
    JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::StationsAndDesktops::{
    CreateDesktopW, GetProcessWindowStation, GetUserObjectInformationW, UOI_NAME,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessAsUserW, DeleteProcThreadAttributeList, GetCurrentProcess, GetExitCodeProcess,
    InitializeProcThreadAttributeList, OpenProcessToken, ResumeThread, SetThreadToken,
    TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject, CREATE_NO_WINDOW,
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

use partridge_protocol::FontScript;

mod container;

use super::{engine_args, EngineProcess, SpawnRequest, SpawnedEngine};

const PROC_THREAD_ATTRIBUTE_HANDLE_LIST: usize = 0x0002_0002;
const PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY: usize = 0x0002_0007;
const PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY: usize = 0x0002_000E;
const PROCESS_CREATION_CHILD_PROCESS_RESTRICTED: u32 = 0x01;

// Process mitigation policy flags (PROCESS_CREATION_MITIGATION_POLICY_*), spelled out so that the
// meaning of each bit is in this file.
const HEAP_TERMINATE_ALWAYS_ON: u64 = 1 << 12;
const BOTTOM_UP_ASLR_ALWAYS_ON: u64 = 1 << 16;
const HIGH_ENTROPY_ASLR_ALWAYS_ON: u64 = 1 << 20;
const STRICT_HANDLE_CHECKS_ALWAYS_ON: u64 = 1 << 24;
// Bit 28, the win32k system call lockdown, is left out on purpose: PDFium's DLL links user32 and
// gdi32, and loading it fails (DLL initialisation error) in a process that starts with the win32k
// system calls disabled. Switching them off after PDFium is loaded was tried
// (SetProcessMitigationPolicy) and the system refused. It is a known gap, listed in the README.
const EXTENSION_POINT_DISABLE_ALWAYS_ON: u64 = 1 << 32;
const PROHIBIT_DYNAMIC_CODE_ALWAYS_ON: u64 = 1 << 36;
const FONT_DISABLE_ALWAYS_ON: u64 = 1 << 48;
const IMAGE_LOAD_NO_REMOTE_ALWAYS_ON: u64 = 1 << 52;
const IMAGE_LOAD_NO_LOW_LABEL_ALWAYS_ON: u64 = 1 << 56;
const IMAGE_LOAD_PREFER_SYSTEM32_ALWAYS_ON: u64 = 1 << 60;

const MITIGATIONS: u64 = HEAP_TERMINATE_ALWAYS_ON
    | BOTTOM_UP_ASLR_ALWAYS_ON
    | HIGH_ENTROPY_ASLR_ALWAYS_ON
    | STRICT_HANDLE_CHECKS_ALWAYS_ON
    | EXTENSION_POINT_DISABLE_ALWAYS_ON
    | PROHIBIT_DYNAMIC_CODE_ALWAYS_ON
    | FONT_DISABLE_ALWAYS_ON
    | IMAGE_LOAD_NO_REMOTE_ALWAYS_ON
    | IMAGE_LOAD_NO_LOW_LABEL_ALWAYS_ON
    | IMAGE_LOAD_PREFER_SYSTEM32_ALWAYS_ON;

const SE_GROUP_INTEGRITY: u32 = 0x0000_0020;
const SE_GROUP_USE_FOR_DENY_ONLY: u32 = 0x0000_0010;

use super::DEFAULT_MEMORY_LIMIT;

/// The name of the desktop the engines run on ("window station\\desktop"), made once and kept until
/// the app ends. One desktop is shared by all engines: it costs a few MB of the system's desktop
/// heap, not per engine. `None` when the system would not make one; the engines then run on the
/// desktop of the app and do not report the layer.
fn engine_desktop() -> Option<&'static Vec<u16>> {
    static DESKTOP: OnceLock<Option<Vec<u16>>> = OnceLock::new();
    DESKTOP
        .get_or_init(|| {
            // SAFETY: documented Win32 calls with live buffers. The desktop handle is kept open on
            // purpose: the desktop goes away when its last handle and its last user are gone.
            unsafe {
                let station = GetProcessWindowStation();
                if station.is_null() {
                    return None;
                }
                let mut name = [0u16; 256];
                let mut needed = 0u32;
                if GetUserObjectInformationW(
                    station,
                    UOI_NAME,
                    name.as_mut_ptr().cast(),
                    (name.len() * 2) as u32,
                    &mut needed,
                ) == 0
                {
                    return None;
                }
                let station: String = String::from_utf16_lossy(
                    &name[..name.iter().position(|&c| c == 0).unwrap_or(name.len())],
                );
                let desktop_name = format!("PartridgeEngine{}", std::process::id());
                let own = wide(OsStr::new(&desktop_name));
                let desktop =
                    CreateDesktopW(own.as_ptr(), null(), null(), 0, GENERIC_ALL_ACCESS, null());
                if desktop.is_null() {
                    return None;
                }
                // The package of the engine needs access to it, or the engine cannot start.
                if container::allow_desktop(desktop).is_err() {
                    return None;
                }
                Some(wide(OsStr::new(&format!("{station}\\{desktop_name}"))))
            }
        })
        .as_ref()
}

const GENERIC_ALL_ACCESS: u32 = 0x1000_0000;

/// The environment block of the engine: the variables that are allowed, as UTF-16 text with a zero
/// after each and one more at the end. (An empty environment is two zeros.)
fn environment_block() -> Vec<u16> {
    let mut variables: Vec<(String, Vec<u16>)> = super::engine_environment()
        .into_iter()
        .map(|(name, value)| {
            let mut text: Vec<u16> = name.encode_wide().collect();
            text.push(b'=' as u16);
            text.extend(value.encode_wide());
            (name.to_string_lossy().to_uppercase(), text)
        })
        .collect();
    // The system wants them sorted by name.
    variables.sort_by(|a, b| a.0.cmp(&b.0));
    let mut block = Vec::new();
    for (_, text) in variables {
        block.extend(text);
        block.push(0);
    }
    block.push(0);
    if block.len() == 1 {
        block.push(0);
    }
    block
}

/// A handle that is closed when it goes out of scope.
struct Owned(HANDLE);

impl Owned {
    fn new(handle: HANDLE) -> io::Result<Owned> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(Owned(handle))
        }
    }
    fn raw(&self) -> HANDLE {
        self.0
    }
    /// Gives the handle away without closing it.
    fn into_raw(self) -> HANDLE {
        let h = self.0;
        std::mem::forget(self);
        h
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this value and not used afterwards.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

// SAFETY: a Windows handle is a number the kernel looks up; any thread may use or close it.
unsafe impl Send for Owned {}

fn check(ok: i32, what: &str) -> io::Result<()> {
    if ok == 0 {
        let e = io::Error::last_os_error();
        Err(io::Error::new(e.kind(), format!("{what}: {e}")))
    } else {
        Ok(())
    }
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// One argument of a command line, quoted the way `CommandLineToArgvW` and the C runtime read it.
fn quote_arg(arg: &OsStr, out: &mut Vec<u16>) {
    let s: Vec<u16> = arg.encode_wide().collect();
    let needs = s.is_empty()
        || s.iter().any(|&c| {
            c == b' ' as u16 || c == b'\t' as u16 || c == b'"' as u16 || c == b'\n' as u16
        });
    if !needs {
        out.extend_from_slice(&s);
        return;
    }
    out.push(b'"' as u16);
    let backslash = b'\\' as u16;
    let mut i = 0;
    loop {
        let mut backslashes = 0;
        while i < s.len() && s[i] == backslash {
            backslashes += 1;
            i += 1;
        }
        if i == s.len() {
            // Backslashes before the closing quote must not escape it.
            out.extend(std::iter::repeat_n(backslash, backslashes * 2));
            break;
        } else if s[i] == b'"' as u16 {
            out.extend(std::iter::repeat_n(backslash, backslashes * 2 + 1));
            out.push(s[i]);
        } else {
            out.extend(std::iter::repeat_n(backslash, backslashes));
            out.push(s[i]);
        }
        i += 1;
    }
    out.push(b'"' as u16);
}

fn command_line(program: &OsStr, args: &[OsString]) -> Vec<u16> {
    let mut line = Vec::new();
    quote_arg(program, &mut line);
    for arg in args {
        line.push(b' ' as u16);
        quote_arg(arg, &mut line);
    }
    line.push(0);
    line
}

/// A SID built by hand: revision 1, one sub-authority, identifier authority `authority`.
fn simple_sid(authority: u8, sub_authority: u32) -> Vec<u8> {
    let mut sid = vec![1u8, 1, 0, 0, 0, 0, 0, authority];
    sid.extend_from_slice(&sub_authority.to_le_bytes());
    sid
}

/// The null SID, S-1-0-0. No object grants access to it.
fn null_sid() -> Vec<u8> {
    simple_sid(0, 0)
}

fn token_info(token: HANDLE, class: i32) -> io::Result<Vec<u8>> {
    let mut needed = 0u32;
    // SAFETY: the first call only asks for the size.
    unsafe { GetTokenInformation(token, class, null_mut(), 0, &mut needed) };
    let mut buffer = vec![0u8; needed as usize];
    // SAFETY: the buffer is as large as the system asked for.
    check(
        unsafe {
            GetTokenInformation(
                token,
                class,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        },
        "GetTokenInformation",
    )?;
    Ok(buffer)
}

/// Makes the token of the engine and the more powerful one its first thread starts with.
fn make_tokens() -> io::Result<(Owned, Owned)> {
    // SAFETY: documented Win32 calls with live out parameters; every handle is owned.
    unsafe {
        let mut raw: HANDLE = null_mut();
        check(
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT,
                &mut raw,
            ),
            "OpenProcessToken",
        )?;
        let own = Owned::new(raw)?;

        let user_buffer = token_info(own.raw(), TokenUser)?;
        let user = &*(user_buffer.as_ptr() as *const TOKEN_USER);
        let groups_buffer = token_info(own.raw(), TokenGroups)?;
        let groups = &*(groups_buffer.as_ptr() as *const TOKEN_GROUPS);

        // The identity for the start of the engine: the app's own rights, but as a restricted
        // token. (The system lets the first thread of a process with a restricted token
        // impersonate only a restricted token; the restricting SIDs here are all the SIDs the app
        // has, so nothing is lost.)
        let mut everyone = simple_sid(1, 0);
        let mut restricted_code = simple_sid(5, 12);
        let mut start_sids: Vec<SID_AND_ATTRIBUTES> = vec![
            SID_AND_ATTRIBUTES {
                Sid: user.User.Sid,
                Attributes: 0,
            },
            SID_AND_ATTRIBUTES {
                Sid: everyone.as_mut_ptr().cast(),
                Attributes: 0,
            },
            SID_AND_ATTRIBUTES {
                Sid: restricted_code.as_mut_ptr().cast(),
                Attributes: 0,
            },
        ];
        for i in 0..groups.GroupCount as usize {
            let g = &*groups.Groups.as_ptr().add(i);
            if g.Attributes & (SE_GROUP_INTEGRITY | SE_GROUP_USE_FOR_DENY_ONLY) == 0 {
                start_sids.push(SID_AND_ATTRIBUTES {
                    Sid: g.Sid,
                    Attributes: 0,
                });
            }
        }
        let mut start_token: HANDLE = null_mut();
        check(
            CreateRestrictedToken(
                own.raw(),
                0,
                0,
                null(),
                0,
                null(),
                start_sids.len() as u32,
                start_sids.as_ptr(),
                &mut start_token,
            ),
            "CreateRestrictedToken (start)",
        )?;
        let start_token = Owned::new(start_token)?;
        // The user and every group become deny-only, except the integrity entry, which cannot.
        let mut disable: Vec<SID_AND_ATTRIBUTES> = vec![SID_AND_ATTRIBUTES {
            Sid: user.User.Sid,
            Attributes: 0,
        }];
        for i in 0..groups.GroupCount as usize {
            let g = &*groups.Groups.as_ptr().add(i);
            if g.Attributes & (SE_GROUP_INTEGRITY | SE_GROUP_USE_FOR_DENY_ONLY) == 0 {
                disable.push(SID_AND_ATTRIBUTES {
                    Sid: g.Sid,
                    Attributes: 0,
                });
            }
        }
        let mut nobody = null_sid();
        let restricting = [SID_AND_ATTRIBUTES {
            Sid: nobody.as_mut_ptr().cast(),
            Attributes: 0,
        }];

        let mut restricted: HANDLE = null_mut();
        check(
            CreateRestrictedToken(
                own.raw(),
                DISABLE_MAX_PRIVILEGE,
                disable.len() as u32,
                disable.as_ptr(),
                0,
                null(),
                restricting.len() as u32,
                restricting.as_ptr(),
                &mut restricted,
            ),
            "CreateRestrictedToken",
        )?;
        let restricted = Owned::new(restricted)?;

        // A primary token is what a new process needs.
        let mut primary: HANDLE = null_mut();
        check(
            DuplicateTokenEx(
                restricted.raw(),
                TOKEN_ALL_ACCESS,
                null(),
                SecurityImpersonation,
                TokenPrimary,
                &mut primary,
            ),
            "DuplicateTokenEx (primary)",
        )?;
        let lockdown = Owned::new(primary)?;
        remove_bypass_traverse_checking(&lockdown)?;

        // The integrity level stays the app's own for now. The system lets the first thread
        // impersonate a token only if its integrity level is not above that of the process, so
        // the two start out equal, and the engine lowers its own level to untrusted just before
        // it gives up the start identity (see `partridge-engine`).
        drop(nobody);

        // The AppContainer: the same two tokens, as tokens of a package that has no capabilities.
        // The start identity has to be one of the same package, or the system lowers it to
        // "identification" and the loader fails.
        let lockdown = container::lowbox(&lockdown)?;
        let start_box = container::lowbox(&start_token)?;
        let mut imp: HANDLE = null_mut();
        check(
            DuplicateTokenEx(
                start_box.raw(),
                TOKEN_ALL_ACCESS,
                null(),
                SecurityImpersonation,
                TokenImpersonation,
                &mut imp,
            ),
            "DuplicateTokenEx (start, AppContainer)",
        )?;
        let initial = Owned::new(imp)?;
        Ok((lockdown, initial))
    }
}

/// `SE_PRIVILEGE_REMOVED`: the privilege is taken out of the token for good.
const SE_PRIVILEGE_REMOVED: u32 = 0x0000_0004;

/// Takes `SeChangeNotifyPrivilege` ("bypass traverse checking") out of the token of the engine.
/// `DISABLE_MAX_PRIVILEGE` keeps exactly that one privilege. Without it a process cannot open an
/// object whose path runs through directories it has no traverse right on, and with the null SID as
/// restricting SID it has none. Opening `\Device\Afd\Endpoint`, which is the first thing creating
/// any socket does, is such an open. The first thread's start identity keeps the privilege, so
/// loading PDFium and the fonts is not affected.
fn remove_bypass_traverse_checking(token: &Owned) -> io::Result<()> {
    let name = wide(OsStr::new("SeChangeNotifyPrivilege"));
    // SAFETY: documented Win32 calls with live, correctly sized structures.
    unsafe {
        let mut privilege: TOKEN_PRIVILEGES = zeroed();
        privilege.PrivilegeCount = 1;
        check(
            LookupPrivilegeValueW(null(), name.as_ptr(), &mut privilege.Privileges[0].Luid),
            "LookupPrivilegeValue",
        )?;
        privilege.Privileges[0] = LUID_AND_ATTRIBUTES {
            Luid: privilege.Privileges[0].Luid,
            Attributes: SE_PRIVILEGE_REMOVED,
        };
        check(
            AdjustTokenPrivileges(token.raw(), 0, &privilege, 0, null_mut(), null_mut()),
            "AdjustTokenPrivileges",
        )?;
        // The call succeeds with `ERROR_NOT_ALL_ASSIGNED` when the privilege was not in the token.
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(1300) {
            return Err(io::Error::other(
                "AdjustTokenPrivileges: the privilege is not in the token",
            ));
        }
    }
    Ok(())
}

/// The job: one process, a memory limit, no UI access, ended when the handle closes.
fn make_job(memory_limit: u64) -> io::Result<Owned> {
    // SAFETY: documented Win32 calls with live, correctly sized structures.
    unsafe {
        let job = Owned::new(CreateJobObjectW(null(), null()))?;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        limits.BasicLimitInformation.ActiveProcessLimit = 1;
        limits.ProcessMemoryLimit = memory_limit as usize;
        check(
            SetInformationJobObject(
                job.raw(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ),
            "SetInformationJobObject (limits)",
        )?;
        let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS {
            UIRestrictionsClass: JOB_OBJECT_UILIMIT_DESKTOP
                | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
                | JOB_OBJECT_UILIMIT_EXITWINDOWS
                | JOB_OBJECT_UILIMIT_GLOBALATOMS
                | JOB_OBJECT_UILIMIT_HANDLES
                | JOB_OBJECT_UILIMIT_READCLIPBOARD
                | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
                | JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
        };
        check(
            SetInformationJobObject(
                job.raw(),
                JobObjectBasicUIRestrictions,
                (&ui as *const JOBOBJECT_BASIC_UI_RESTRICTIONS).cast(),
                size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
            ),
            "SetInformationJobObject (user interface)",
        )?;
        Ok(job)
    }
}

fn inheritable() -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    }
}

/// A pipe whose `child` end can be inherited and whose `parent` end cannot.
fn make_pipe(child_reads: bool) -> io::Result<(Owned, Owned)> {
    let attributes = inheritable();
    let mut read: HANDLE = null_mut();
    let mut write: HANDLE = null_mut();
    // SAFETY: documented call with live out parameters.
    check(
        unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) },
        "CreatePipe",
    )?;
    let (read, write) = (Owned::new(read)?, Owned::new(write)?);
    let (child, parent) = if child_reads {
        (read, write)
    } else {
        (write, read)
    };
    // SAFETY: the handle is valid and owned.
    check(
        unsafe { SetHandleInformation(parent.raw(), HANDLE_FLAG_INHERIT, 0) },
        "SetHandleInformation",
    )?;
    Ok((child, parent))
}

fn open_nul() -> io::Result<Owned> {
    let attributes = inheritable();
    let name = wide(OsStr::new("NUL"));
    // SAFETY: documented call with a live, terminated name.
    Owned::new(unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &attributes,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    })
}

/// A copy of the document handle that the new process may inherit.
fn inheritable_copy(file: &File) -> io::Result<Owned> {
    let mut copy: HANDLE = null_mut();
    // SAFETY: documented call; source and target process are this process.
    check(
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                file.as_raw_handle() as HANDLE,
                GetCurrentProcess(),
                &mut copy,
                0,
                1,
                DUPLICATE_SAME_ACCESS,
            )
        },
        "DuplicateHandle",
    )?;
    Owned::new(copy)
}

/// Only one engine is started at a time, so the handles that are inheritable for a moment are not
/// inherited by a second engine that starts in the same moment.
static START: Mutex<()> = Mutex::new(());

struct WindowsProcess {
    process: Owned,
    id: u32,
    /// Closing the job ends the engine: it must outlive it, and it goes with it.
    _job: Option<Owned>,
}

impl EngineProcess for WindowsProcess {
    fn id(&self) -> u32 {
        self.id
    }
    fn kill(&mut self) -> io::Result<()> {
        // SAFETY: the process handle is valid; ending an ended process fails harmlessly.
        unsafe { TerminateProcess(self.process.raw(), 1) };
        Ok(())
    }
    fn wait(&mut self) -> io::Result<()> {
        // SAFETY: the process handle is valid.
        unsafe { WaitForSingleObject(self.process.raw(), u32::MAX) };
        Ok(())
    }
    fn try_wait(&mut self) -> io::Result<Option<String>> {
        // SAFETY: the process handle is valid.
        unsafe {
            if WaitForSingleObject(self.process.raw(), 0) != WAIT_OBJECT_0 {
                return Ok(None);
            }
            let mut code = 0u32;
            check(
                GetExitCodeProcess(self.process.raw(), &mut code),
                "GetExitCodeProcess",
            )?;
            Ok(Some(format!("exit code: {code}")))
        }
    }
}

fn pipe_file(handle: Owned) -> File {
    // SAFETY: the handle is a pipe end that nobody else owns.
    unsafe { File::from_raw_handle(handle.into_raw() as *mut c_void) }
}

/// Starts the engine with all restrictions.
pub(super) fn spawn(request: &SpawnRequest<'_>) -> io::Result<SpawnedEngine> {
    let _only_one = START.lock().unwrap_or_else(|e| e.into_inner());
    container::allow_files(request.program, request.args)?;
    let (lockdown, initial) = make_tokens()?;
    let job = make_job(request.memory_limit.unwrap_or(DEFAULT_MEMORY_LIMIT))?;
    let (child_in, parent_in) = make_pipe(true)?;
    let (child_out, parent_out) = make_pipe(false)?;
    let nul = open_nul()?;
    let document = request.document.map(inheritable_copy).transpose()?;
    let fonts: Vec<(FontScript, Owned)> = request
        .fonts
        .iter()
        // A font that cannot be copied is left out: its script shows as blanks, the document opens.
        .filter_map(|(script, file)| inheritable_copy(file).ok().map(|h| (*script, h)))
        .collect();
    let font_numbers: Vec<(FontScript, u64)> = fonts
        .iter()
        .map(|(script, h)| (*script, h.raw() as usize as u64))
        .collect();

    // The engine inherits these and nothing else.
    let mut inherited: Vec<HANDLE> = vec![child_in.raw(), child_out.raw(), nul.raw()];
    if let Some(document) = &document {
        inherited.push(document.raw());
    }
    inherited.extend(fonts.iter().map(|(_, h)| h.raw()));
    let args = engine_args(
        request,
        document.as_ref().map(|d| d.raw() as usize as u64),
        &font_numbers,
    );
    let mut line = command_line(request.program.as_os_str(), &args);
    let program = wide(request.program.as_os_str());

    // The attribute list: handles to inherit, mitigation policies, no child processes.
    let mut mitigations: u64 = MITIGATIONS;
    let mut child_policy: u32 = PROCESS_CREATION_CHILD_PROCESS_RESTRICTED;
    let mut size = 0usize;
    // SAFETY: the first call only asks how large the list is.
    unsafe { InitializeProcThreadAttributeList(null_mut(), 3, 0, &mut size) };
    let mut list_buffer = vec![0u8; size];
    let list = list_buffer.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
    // SAFETY: the buffer is as large as the system asked for, and stays alive until the list is
    // deleted below; the values it points at live until then too.
    unsafe {
        check(
            InitializeProcThreadAttributeList(list, 3, 0, &mut size),
            "InitializeProcThreadAttributeList",
        )?;
    }
    let result = (|| -> io::Result<(Owned, Owned, u32)> {
        // SAFETY: documented calls; every pointer is to a value that outlives CreateProcess.
        unsafe {
            check(
                UpdateProcThreadAttribute(
                    list,
                    0,
                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                    inherited.as_ptr().cast(),
                    inherited.len() * size_of::<HANDLE>(),
                    null_mut(),
                    null(),
                ),
                "UpdateProcThreadAttribute (handle list)",
            )?;
            check(
                UpdateProcThreadAttribute(
                    list,
                    0,
                    PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY,
                    (&mut mitigations as *mut u64).cast(),
                    size_of::<u64>(),
                    null_mut(),
                    null(),
                ),
                "UpdateProcThreadAttribute (mitigation policy)",
            )?;
            check(
                UpdateProcThreadAttribute(
                    list,
                    0,
                    PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY,
                    (&mut child_policy as *mut u32).cast(),
                    size_of::<u32>(),
                    null_mut(),
                    null(),
                ),
                "UpdateProcThreadAttribute (child processes)",
            )?;

            let mut startup: STARTUPINFOEXW = zeroed();
            startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
            startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            startup.StartupInfo.hStdInput = child_in.raw();
            startup.StartupInfo.hStdOutput = child_out.raw();
            startup.StartupInfo.hStdError = nul.raw();
            startup.lpAttributeList = list;
            // The engine shows nothing, so it gets a desktop of its own: it cannot see the windows
            // of the user, read the screen, or put a window of its own over theirs.
            let mut desktop = engine_desktop().cloned();
            if let Some(name) = desktop.as_mut() {
                startup.StartupInfo.lpDesktop = name.as_mut_ptr();
            }
            let environment = environment_block();
            let mut info: PROCESS_INFORMATION = zeroed();
            check(
                CreateProcessAsUserW(
                    lockdown.raw(),
                    program.as_ptr(),
                    line.as_mut_ptr(),
                    null(),
                    null(),
                    1,
                    EXTENDED_STARTUPINFO_PRESENT
                        | CREATE_SUSPENDED
                        | CREATE_NO_WINDOW
                        | CREATE_UNICODE_ENVIRONMENT,
                    environment.as_ptr().cast(),
                    null(),
                    &startup.StartupInfo,
                    &mut info,
                ),
                "CreateProcessAsUser",
            )?;
            let process = Owned::new(info.hProcess)?;
            let thread = Owned::new(info.hThread)?;
            let id = info.dwProcessId;
            let finish = || -> io::Result<()> {
                // The first thread starts as the app's own identity, so that the engine can load
                // its libraries. The engine ends that with RevertToSelf.
                let thread_handle = thread.raw();
                check(
                    SetThreadToken(&thread_handle, initial.raw()),
                    "SetThreadToken",
                )?;
                check(
                    AssignProcessToJobObject(job.raw(), process.raw()),
                    "AssignProcessToJobObject",
                )?;
                if ResumeThread(thread.raw()) == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            };
            if let Err(e) = finish() {
                TerminateProcess(process.raw(), 1);
                return Err(e);
            }
            Ok((process, thread, id))
        }
    })();
    // SAFETY: the list was initialised above and is not used after this.
    unsafe { DeleteProcThreadAttributeList(list) };
    let (process, _thread, id) = result?;

    // The engine has its copies now.
    drop((child_in, child_out, nul, document, fonts));
    Ok(SpawnedEngine {
        process: Box::new(WindowsProcess {
            process,
            id,
            _job: Some(job),
        }),
        stdin: Box::new(pipe_file(parent_in)) as Box<dyn Write + Send>,
        stdout: Box::new(pipe_file(parent_out)) as Box<dyn Read + Send>,
    })
}

/// Starts the engine as a plain child process, with no restrictions. For development builds only.
pub(super) fn spawn_plain(request: &SpawnRequest<'_>) -> io::Result<SpawnedEngine> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    struct Plain(std::process::Child);
    impl EngineProcess for Plain {
        fn id(&self) -> u32 {
            self.0.id()
        }
        fn kill(&mut self) -> io::Result<()> {
            self.0.kill()
        }
        fn wait(&mut self) -> io::Result<()> {
            self.0.wait().map(|_| ())
        }
        fn try_wait(&mut self) -> io::Result<Option<String>> {
            Ok(self.0.try_wait()?.map(|status| status.to_string()))
        }
    }

    // The standard library lets a child inherit every inheritable handle, so the copy of the
    // document handle is inheritable for the moment of the start and closed again after it.
    let _only_one = START.lock().unwrap_or_else(|e| e.into_inner());
    let document = request.document.map(inheritable_copy).transpose()?;
    let fonts: Vec<(FontScript, Owned)> = request
        .fonts
        .iter()
        // A font that cannot be copied is left out: its script shows as blanks, the document opens.
        .filter_map(|(script, file)| inheritable_copy(file).ok().map(|h| (*script, h)))
        .collect();
    let font_numbers: Vec<(FontScript, u64)> = fonts
        .iter()
        .map(|(script, h)| (*script, h.raw() as usize as u64))
        .collect();
    let mut command = Command::new(request.program);
    command
        .args(engine_args(
            request,
            document.as_ref().map(|d| d.raw() as usize as u64),
            &font_numbers,
        ))
        .env_clear()
        .envs(super::engine_environment())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // CREATE_NO_WINDOW: no console window flashes up next to the app.
        .creation_flags(0x0800_0000);
    let mut child = command.spawn()?;
    let stdin = child.stdin.take().expect("stdin is piped");
    let stdout = child.stdout.take().expect("stdout is piped");
    Ok(SpawnedEngine {
        process: Box::new(Plain(child)),
        stdin: Box::new(stdin),
        stdout: Box::new(stdout),
    })
}

/// See [`allow_stand_in_to_use`](super::allow_stand_in_to_use).
#[cfg(debug_assertions)]
pub(super) fn allow_read_write(path: &std::path::Path) -> io::Result<()> {
    container::allow_read_write(path)
}
