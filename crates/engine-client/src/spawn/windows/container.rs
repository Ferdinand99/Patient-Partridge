//! The AppContainer layer of the Windows sandbox.
//!
//! An AppContainer token carries a package SID and no capabilities. Windows Filtering Platform
//! blocks every network access of such a token (connect, send, DNS), loopback included, because the
//! token lacks the capabilities that allow a network (`internetClient` and the like). That is the
//! only thing on Windows that stops a process from using the network without a firewall rule, and
//! it needs no administrator, no installer step and no signature.
//!
//! The token is made with `NtCreateLowBoxToken` from the restricted token that the app already
//! builds (the call that `CreateProcess` makes for the `SECURITY_CAPABILITIES` attribute, but on a
//! token of our own), so the restricted token, the job, the mitigation policies and the desktop all
//! stay. Both access checks apply: an object must be open to the restricting SID *and* to the
//! package.
//!
//! **No profile.** The package SID is computed from a fixed name
//! (`DeriveAppContainerSidFromAppContainerName`); `CreateAppContainerProfile` is never called.
//! That call writes under `HKCU\Software\Classes\Local Settings\Software\Microsoft\Windows\
//! CurrentVersion\AppContainer\Mappings` and creates a folder under `%LOCALAPPDATA%\Packages`, which
//! would need clean-up (`DeleteAppContainerProfile`) and would leave traces if the app is killed.
//! A token does not need a registered profile to be an AppContainer, and the network filters only
//! look at the token. So nothing is written to the registry and there is nothing to remove.
//!
//! **Files.** An AppContainer can open only what is open to its package SID or to "all application
//! packages". The system folders are open to the latter; the folder a per-user install lives in is
//! not. So before the first start the app adds one read-and-execute entry for the package SID to
//! the few files the engine loads (its own program, the PDFium library, the `VERSION` file, the
//! fonts folder). The user owns these files, so no administrator is needed. Where the entry cannot
//! be added (a per-machine install in `Program Files`, which already allows "all application
//! packages" to read) nothing is changed. The installers add the entry for this package on the install
//! folder (without inheritance) and on the engine program and the PDFium library
//! (`src-tauri/wix/package-access.wxs`, `src-tauri/windows/installer-hooks.nsh`), because a
//! per-machine install outside `Program Files` belongs to the administrators and the user who runs
//! the program cannot add it; they remove the entry on the folder when the program is uninstalled.
//! The entry the app adds itself is kept: it names a package that exists only as a number, and the
//! next start finds it in place. After the lock-in the engine still cannot
//! open these files, because the restricted token has the null SID as its only restricting SID.

use std::collections::HashSet;
use std::ffi::{c_void, OsStr};
use std::io;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::Foundation::{LocalFree, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, GetSecurityInfo, SetEntriesInAclW, SetNamedSecurityInfoW,
    SetSecurityInfo, EXPLICIT_ACCESS_W, GRANT_ACCESS, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT,
    SE_OBJECT_TYPE, SE_WINDOW_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::Isolation::DeriveAppContainerSidFromAppContainerName;
use windows_sys::Win32::Security::{
    AclSizeInformation, EqualSid, FreeSid, GetAce, GetAclInformation, GetLengthSid,
    ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
};
#[cfg(debug_assertions)]
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_WRITE;
use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_EXECUTE, FILE_GENERIC_READ};

use super::{wide, Owned};

/// The name the package SID is computed from. Fixed, so every start (and every version) of the app
/// gets the same SID.
const PACKAGE_NAME: &str = "Partridge.Engine.Sandbox";

const READ_CONTROL: u32 = 0x0002_0000;
/// The type of an ACE that allows access (`ACCESS_ALLOWED_ACE_TYPE`).
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
/// Read and enumerate the objects of a desktop, make windows and menus, write objects. No hooks,
/// no journal, no switching.
const DESKTOP_RIGHTS: u32 = 0x01 | 0x02 | 0x04 | 0x40 | 0x80 | READ_CONTROL;

/// The package SID, computed once.
pub(super) fn package_sid() -> io::Result<&'static [u8]> {
    static SID: OnceLock<Result<Vec<u8>, String>> = OnceLock::new();
    SID.get_or_init(|| {
        let name = wide(OsStr::new(PACKAGE_NAME));
        let mut sid: *mut c_void = null_mut();
        // SAFETY: documented call with a terminated name and an out pointer; the SID it returns is
        // copied and freed with FreeSid.
        unsafe {
            let hr = DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid);
            if hr < 0 || sid.is_null() {
                return Err(format!(
                    "DeriveAppContainerSidFromAppContainerName: {hr:#010X}"
                ));
            }
            let length = GetLengthSid(sid) as usize;
            let copy = std::slice::from_raw_parts(sid as *const u8, length).to_vec();
            FreeSid(sid);
            Ok(copy)
        }
    })
    .as_ref()
    .map(Vec::as_slice)
    .map_err(|why| io::Error::other(why.clone()))
}

#[link(name = "ntdll")]
extern "system" {
    fn NtCreateLowBoxToken(
        token: *mut HANDLE,
        existing: HANDLE,
        access: u32,
        attributes: *const c_void,
        package_sid: *const c_void,
        capability_count: u32,
        capabilities: *const c_void,
        handle_count: u32,
        handles: *const HANDLE,
    ) -> i32;
}

const TOKEN_ALL_ACCESS_RIGHTS: u32 = 0x000F_01FF;

/// A primary token that is a copy of `source` (a primary token) turned into an AppContainer token
/// of the package, with no capabilities. The system always makes a primary token of it; an
/// impersonation token is made from that with `DuplicateTokenEx`.
pub(super) fn lowbox(source: &Owned) -> io::Result<Owned> {
    let sid = package_sid()?;
    let mut token: HANDLE = null_mut();
    // SAFETY: documented native call; the SID outlives it, and no attributes, capability or handle
    // is passed.
    let status = unsafe {
        NtCreateLowBoxToken(
            &mut token,
            source.raw(),
            TOKEN_ALL_ACCESS_RIGHTS,
            null(),
            sid.as_ptr().cast(),
            0,
            null(),
            0,
            null(),
        )
    };
    if status < 0 {
        return Err(io::Error::other(format!(
            "NtCreateLowBoxToken: NTSTATUS {status:#010X}"
        )));
    }
    Owned::new(token)
}

fn trustee(sid: &[u8]) -> TRUSTEE_W {
    TRUSTEE_W {
        pMultipleTrustee: null_mut(),
        MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_UNKNOWN,
        ptstrName: sid.as_ptr() as *mut u16,
    }
}

fn win32(code: u32, what: &str) -> io::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{what}: error {code}"),
        ))
    }
}

/// Owns memory the security functions allocated with `LocalAlloc`.
struct Local(*mut c_void);

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from a security function that allocates with LocalAlloc.
            unsafe { LocalFree(self.0) };
        }
    }
}

/// True when an entry of the ACL itself allows all of `rights` to exactly this SID. (Not the
/// effective rights through groups such as "Users" or "Everyone": those do not open anything to an
/// AppContainer, which is checked against its own package SID.)
///
/// # Safety
/// `acl` must be a valid ACL and `sid` a valid SID.
unsafe fn allows(acl: *const ACL, sid: &[u8], rights: u32) -> bool {
    let mut size: ACL_SIZE_INFORMATION = std::mem::zeroed();
    if GetAclInformation(
        acl,
        (&mut size as *mut ACL_SIZE_INFORMATION).cast(),
        std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
        AclSizeInformation,
    ) == 0
    {
        return false;
    }
    let mut granted = 0u32;
    for index in 0..size.AceCount {
        let mut ace: *mut c_void = null_mut();
        if GetAce(acl, index, &mut ace) == 0 || ace.is_null() {
            continue;
        }
        let ace = &*(ace as *const ACCESS_ALLOWED_ACE);
        if ace.Header.AceType != ACCESS_ALLOWED_ACE_TYPE {
            continue;
        }
        let owner = &ace.SidStart as *const u32 as *mut c_void;
        if EqualSid(owner, sid.as_ptr() as *mut c_void) != 0 {
            granted |= ace.Mask;
        }
    }
    granted & rights == rights
}

/// Adds an entry that allows `rights` to the package SID in the DACL that `get` reads, and writes
/// the result with `set`. Does nothing when the package already has those rights.
fn add_entry(
    sid: &[u8],
    rights: u32,
    get: impl FnOnce(*mut *mut ACL, *mut PSECURITY_DESCRIPTOR) -> u32,
    set: impl FnOnce(*const ACL) -> u32,
) -> io::Result<()> {
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    win32(get(&mut dacl, &mut descriptor), "reading the DACL")?;
    let _descriptor = Local(descriptor);

    let who = trustee(sid);
    // A NULL DACL is open to everybody, the package included. (Building a new ACL from one entry
    // would lock everybody else out.)
    if dacl.is_null() {
        return Ok(());
    }
    // SAFETY: the ACL is the one just read; the SID is live.
    if unsafe { allows(dacl, sid, rights) } {
        return Ok(());
    }
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: rights,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: 0,
        Trustee: who,
    };
    let mut merged: *mut ACL = null_mut();
    // SAFETY: one entry, and the old ACL that was just read (or none).
    win32(
        unsafe { SetEntriesInAclW(1, &entry, dacl, &mut merged) },
        "building the DACL",
    )?;
    let _merged = Local(merged.cast());
    win32(set(merged), "writing the DACL")
}

fn on_path(path: &Path, sid: &[u8], rights: u32) -> io::Result<()> {
    let name = wide(path.as_os_str());
    add_entry(
        sid,
        rights,
        |dacl, descriptor| {
            // SAFETY: a terminated name and live out pointers.
            unsafe {
                GetNamedSecurityInfoW(
                    name.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    dacl,
                    null_mut(),
                    descriptor,
                )
            }
        },
        |acl| {
            // SAFETY: a terminated name and the ACL just built.
            unsafe {
                SetNamedSecurityInfoW(
                    name.as_ptr() as *mut u16,
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    acl,
                    null(),
                )
            }
        },
    )
}

fn on_handle(handle: HANDLE, kind: SE_OBJECT_TYPE, sid: &[u8], rights: u32) -> io::Result<()> {
    add_entry(
        sid,
        rights,
        |dacl, descriptor| {
            // SAFETY: a live handle and live out pointers.
            unsafe {
                GetSecurityInfo(
                    handle,
                    kind,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    dacl,
                    null_mut(),
                    descriptor,
                )
            }
        },
        |acl| {
            // SAFETY: a live handle and the ACL just built.
            unsafe {
                SetSecurityInfo(
                    handle,
                    kind,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    acl,
                    null(),
                )
            }
        },
    )
}

/// Lets the AppContainer use the desktop the engines run on.
pub(super) fn allow_desktop(desktop: HANDLE) -> io::Result<()> {
    on_handle(desktop, SE_WINDOW_OBJECT, package_sid()?, DESKTOP_RIGHTS)
}

/// The most font files the engine reads from a folder (as in the engine).
const MAX_FONT_FILES: usize = 64;

/// What the engine opens by path before it locks itself in: its own program, the PDFium folder
/// (which it looks at) with the library and the `VERSION` file in it, and the fonts folder with
/// its font files. `args` are the engine's options.
fn engine_paths(program: &Path, args: &[std::ffi::OsString]) -> Vec<PathBuf> {
    let option = |name: &str| -> Option<PathBuf> {
        args.windows(2)
            .find(|pair| pair[0] == name)
            .map(|pair| PathBuf::from(&pair[1]))
    };
    let own_dir = program.parent().map(Path::to_path_buf).unwrap_or_default();
    let pdfium_dir = option("--pdfium-dir").unwrap_or(own_dir);
    let fonts_dir = option("--fonts-dir").unwrap_or_else(|| pdfium_dir.join("fonts"));

    let mut paths = vec![
        // The folder of the program too: the loader looks there first for the libraries it loads.
        program.parent().map(Path::to_path_buf).unwrap_or_default(),
        program.to_path_buf(),
        pdfium_dir.clone(),
        pdfium_dir.join("pdfium.dll"),
        pdfium_dir.join("VERSION"),
    ];
    if let Ok(entries) = std::fs::read_dir(&fonts_dir) {
        paths.push(fonts_dir.clone());
        let mut fonts: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc"))
                    .unwrap_or(false)
            })
            .collect();
        fonts.sort();
        fonts.truncate(MAX_FONT_FILES);
        paths.extend(fonts);
    }
    paths
}

/// Makes what the engine loads readable for the package. A path that does not exist, or whose
/// entry cannot be added, is left as it is: either the engine does not need it, or the system
/// already allows "all application packages" to read it, or the engine does not start and the
/// start fails closed. Each path is handled once per run of the app.
pub(super) fn allow_files(program: &Path, args: &[std::ffi::OsString]) -> io::Result<()> {
    static DONE: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    let sid = package_sid()?;
    let mut done = DONE
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for path in engine_paths(program, args) {
        if done.contains(&path) || !path.exists() {
            continue;
        }
        // Read, list and run: nothing that writes.
        if on_path(&path, sid, FILE_GENERIC_READ | FILE_GENERIC_EXECUTE).is_ok() {
            done.insert(path);
        }
    }
    Ok(())
}

/// For tests of the client that run stand-ins for the engine, which are in the same AppContainer
/// but do not lock themselves in: lets the package read and write one file the test made.
#[cfg(debug_assertions)]
pub(super) fn allow_read_write(path: &Path) -> io::Result<()> {
    on_path(path, package_sid()?, FILE_GENERIC_READ | FILE_GENERIC_WRITE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;

    /// The installers name the package by its number. It must be the one the app computes.
    #[test]
    fn the_installers_name_the_package_the_app_uses() {
        let sid = package_sid().expect("the package SID");
        let mut text: *mut u16 = null_mut();
        // SAFETY: a valid SID and an out pointer; the string is freed with LocalFree.
        let ok = unsafe { ConvertSidToStringSidW(sid.as_ptr() as *mut c_void, &mut text) };
        assert_ne!(ok, 0, "ConvertSidToStringSidW");
        let _free = Local(text.cast());
        // SAFETY: the system returned a terminated string.
        let name = unsafe {
            let mut length = 0;
            while *text.add(length) != 0 {
                length += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(text, length))
        };
        assert!(name.starts_with("S-1-15-2-"), "{name}");
        for file in [
            include_str!("../../../../../src-tauri/wix/package-access.wxs"),
            include_str!("../../../../../src-tauri/windows/installer-hooks.nsh"),
        ] {
            assert!(
                file.contains(&name),
                "an installer file does not name {name}"
            );
            // No entry for all application packages (S-1-15-2-1) on a path.
            assert!(
                !file.contains("*S-1-15-2-1:"),
                "an installer file grants all application packages"
            );
        }
    }
}
