//! Diagnosis aid, run by hand or by CI on Windows only: does an AppContainer (a low box token
//! without capabilities) stop the network where the restricted token does not? It starts the
//! self-test program in its control mode inside an AppContainer and prints what the network
//! attempts got. It only prints.

#![cfg(windows)]

use std::ffi::{c_void, OsStr};
use std::io::Read;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::FromRawHandle;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
    UpdateProcThreadAttribute, WaitForSingleObject, EXTENDED_STARTUPINFO_PRESENT,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

#[test]
#[ignore = "diagnosis, run explicitly"]
fn what_an_appcontainer_does_to_the_network() {
    // A folder the AppContainer may read, with a copy of the program in it.
    let exe = std::env::current_exe().unwrap();
    let source = exe
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join("engine_selftest.exe");
    let folder = std::path::PathBuf::from(r"C:\ProgramData\partridge-appcontainer-test");
    std::fs::create_dir_all(&folder).unwrap();
    let program = folder.join("engine_selftest.exe");
    std::fs::copy(&source, &program).unwrap();
    let grant = std::process::Command::new("icacls")
        .arg(&folder)
        .args(["/grant", "*S-1-15-2-1:(OI)(CI)(RX)"])
        .output()
        .unwrap();
    println!("icacls: {}", String::from_utf8_lossy(&grant.stdout));

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    // SAFETY: documented Win32 calls with live buffers; handles are closed or handed to Files.
    unsafe {
        let name = wide(OsStr::new("PartridgeEngineDiagnosis"));
        let mut sid: *mut c_void = null_mut();
        let hr = CreateAppContainerProfile(
            name.as_ptr(),
            name.as_ptr(),
            name.as_ptr(),
            null(),
            0,
            &mut sid,
        );
        if hr != 0 {
            let hr2 = DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid);
            println!("profile: create {hr:#X}, derive {hr2:#X}");
        }
        assert!(!sid.is_null(), "no AppContainer SID");

        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: 1,
        };
        let mut read: HANDLE = null_mut();
        let mut write: HANDLE = null_mut();
        assert!(CreatePipe(&mut read, &mut write, &attributes, 0) != 0);
        windows_sys::Win32::Foundation::SetHandleInformation(read, 1, 0);

        let mut capabilities = SECURITY_CAPABILITIES {
            AppContainerSid: sid,
            Capabilities: null_mut(),
            CapabilityCount: 0,
            Reserved: 0,
        };
        let mut size = 0usize;
        InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size);
        let mut buffer = vec![0u8; size];
        let list = buffer.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
        assert!(InitializeProcThreadAttributeList(list, 1, 0, &mut size) != 0);
        // PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES
        assert!(
            UpdateProcThreadAttribute(
                list,
                0,
                0x0002_0009,
                (&mut capabilities as *mut SECURITY_CAPABILITIES).cast(),
                size_of::<SECURITY_CAPABILITIES>(),
                null_mut(),
                null(),
            ) != 0
        );
        let mut startup: STARTUPINFOEXW = zeroed();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdOutput = write;
        startup.StartupInfo.hStdError = write;
        startup.lpAttributeList = list;
        let mut line = wide(OsStr::new(&format!(
            "\"{}\" control \"{}\" \"{}\" \"{}\" 1 {port} nothing",
            program.display(),
            folder.join("a.pdf").display(),
            folder.display(),
            program.display(),
        )));
        let mut info: PROCESS_INFORMATION = zeroed();
        let program_w = wide(program.as_os_str());
        let ok = CreateProcessW(
            program_w.as_ptr(),
            line.as_mut_ptr(),
            null(),
            null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT,
            null(),
            null(),
            &startup.StartupInfo,
            &mut info,
        );
        println!(
            "CreateProcessW: {ok}, error {}",
            std::io::Error::last_os_error()
        );
        DeleteProcThreadAttributeList(list);
        windows_sys::Win32::Foundation::CloseHandle(write);
        if ok != 0 {
            let mut text = String::new();
            let mut file = std::fs::File::from_raw_handle(read as *mut c_void);
            let _ = file.read_to_string(&mut text);
            WaitForSingleObject(info.hProcess, 30_000);
            println!("--- the program inside the AppContainer printed:\n{text}");
        }
    }
}
