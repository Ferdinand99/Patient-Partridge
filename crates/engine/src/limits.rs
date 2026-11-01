//! A ceiling on the memory the engine may use, so that a PDF built to eat memory ends the engine
//! (which the app survives) instead of slowing down the whole computer.
//!
//! This is a limit, not a sandbox: it does nothing against code that PDFium is tricked into
//! running. On macOS no limit is set (the system does not enforce address-space limits).

/// Most data memory the engine may use: 3 GiB. A 500-page document with a 24 Mpx bitmap needs
/// well under 200 MiB.
pub const MEMORY_LIMIT_BYTES: u64 = 3 * 1024 * 1024 * 1024;

/// Sets the limit for this process. Returns false when the system refused or has no way to do it.
#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
pub fn limit_memory() -> bool {
    #[repr(C)]
    struct Rlimit {
        cur: u64,
        max: u64,
    }
    extern "C" {
        fn setrlimit(resource: i32, limit: *const Rlimit) -> i32;
    }
    // RLIMIT_DATA counts the heap and private writable mappings, but not mapped libraries or
    // address space that PDFium only reserves, which RLIMIT_AS would also count.
    const RLIMIT_DATA: i32 = 2;
    let limit = Rlimit {
        cur: MEMORY_LIMIT_BYTES,
        max: MEMORY_LIMIT_BYTES,
    };
    // SAFETY: plain system call with a pointer to a live struct of the layout the system expects.
    unsafe { setrlimit(RLIMIT_DATA, &limit) == 0 }
}

/// Puts the process in a job object with a limit on the memory one process may commit.
#[cfg(all(windows, target_pointer_width = "64"))]
pub fn limit_memory() -> bool {
    use std::ffi::c_void;
    extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            class: i32,
            info: *const c_void,
            length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn GetCurrentProcess() -> *mut c_void;
    }
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;
    const JOB_OBJECT_LIMIT_PROCESS_MEMORY: u32 = 0x0000_0100;
    // JOBOBJECT_EXTENDED_LIMIT_INFORMATION on 64-bit Windows is 144 bytes: LimitFlags is a u32
    // at offset 16 and ProcessMemoryLimit a u64 at offset 112.
    let mut info = [0u8; 144];
    info[16..20].copy_from_slice(&JOB_OBJECT_LIMIT_PROCESS_MEMORY.to_le_bytes());
    info[112..120].copy_from_slice(&MEMORY_LIMIT_BYTES.to_le_bytes());
    // SAFETY: documented Win32 calls; `info` is a live buffer of the size the class expects.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return false;
        }
        SetInformationJobObject(
            job,
            JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
            info.as_ptr().cast(),
            info.len() as u32,
        ) != 0
            && AssignProcessToJobObject(job, GetCurrentProcess()) != 0
    }
}

/// No limit on this platform.
#[cfg(not(any(
    all(target_os = "linux", target_pointer_width = "64"),
    all(windows, target_pointer_width = "64")
)))]
pub fn limit_memory() -> bool {
    false
}

/// Ends the engine when the app that started it is gone. Closing the engine's input already does
/// that, but only for an engine that reads its input: one stuck inside PDFium never looks at it,
/// so a second thread checks that the parent process is still the same one. (On Windows the app
/// puts the engine in a job object instead, which ends it with the app.)
#[cfg(unix)]
pub fn exit_with_parent() {
    extern "C" {
        fn getppid() -> i32;
        fn _exit(code: i32) -> !;
    }
    // SAFETY: getppid has no arguments and no side effects.
    let parent = unsafe { getppid() };
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        // SAFETY: as above. When the parent is gone this process is adopted by another one and
        // the number changes. `_exit` skips the cleanup code that could itself get stuck.
        if unsafe { getppid() } != parent {
            unsafe { _exit(0) }
        }
    });
}

#[cfg(not(unix))]
pub fn exit_with_parent() {}
