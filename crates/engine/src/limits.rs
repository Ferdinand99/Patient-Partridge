//! A ceiling on the memory the engine may use, so that a PDF built to eat memory ends the engine
//! (which the app survives) instead of slowing down the whole computer.
//!
//! - Linux: `RLIMIT_DATA`, which counts the heap and private writable mappings. Shared mappings
//!   are not counted, so the system call filter refuses them (see `sandbox/linux.rs`).
//! - Windows: the job object the app puts the engine in has a per-process memory limit, which the
//!   kernel enforces (the engine only checks that it is in a job).
//! - macOS: the system does not enforce `RLIMIT_DATA` and `RLIMIT_AS`. The limit that holds is the
//!   one the app keeps: it reads the memory footprint of the engine from outside and ends it
//!   (`partridge-engine-client`, `spawn/unix.rs`). A thread of the engine does the same as a first
//!   line, but code that has taken the engine over can stop that thread.
//!
//! A limit is not a sandbox: it does nothing against code that PDFium is tricked into running.

/// Most data memory the engine may use: 3 GiB. A 500-page document with a 24 Mpx bitmap needs
/// well under 200 MiB.
pub const MEMORY_LIMIT_BYTES: u64 = 3 * 1024 * 1024 * 1024;

/// Sets the limit for this process. Returns false when the system refused or has no way to do it.
#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
pub fn limit_memory(bytes: u64) -> bool {
    // RLIMIT_DATA counts the heap and private writable mappings, but not mapped libraries or
    // address space that PDFium only reserves, which RLIMIT_AS would also count.
    let limit = libc::rlimit {
        rlim_cur: bytes,
        rlim_max: bytes,
    };
    // SAFETY: plain system call with a pointer to a live struct.
    let ok = unsafe { libc::setrlimit(libc::RLIMIT_DATA, &limit) == 0 };
    // No core files: they would hold pages of the document.
    let none = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: as above.
    unsafe { libc::setrlimit(libc::RLIMIT_CORE, &none) };
    ok
}

/// Reads the memory footprint of this process on macOS: the number the system itself uses to decide
/// which process to end when memory is short.
#[cfg(target_os = "macos")]
fn footprint() -> Option<u64> {
    #[repr(C)]
    struct TaskVmInfo {
        virtual_size: u64,
        region_count: i32,
        page_size: i32,
        resident_size: u64,
        resident_size_peak: u64,
        device: u64,
        device_peak: u64,
        internal: u64,
        internal_peak: u64,
        external: u64,
        external_peak: u64,
        reusable: u64,
        reusable_peak: u64,
        purgeable_volatile_pmap: u64,
        purgeable_volatile_resident: u64,
        purgeable_volatile_virtual: u64,
        compressed: u64,
        compressed_peak: u64,
        compressed_lifetime: u64,
        phys_footprint: u64,
        // The rest of the structure, which this code does not read.
        rest: [u8; 128],
    }
    extern "C" {
        static mach_task_self_: u32;
        fn task_info(task: u32, flavor: u32, info: *mut i32, count: *mut u32) -> i32;
    }
    const TASK_VM_INFO: u32 = 22;
    // SAFETY: task_info writes at most `count` words into the buffer, which is that large.
    unsafe {
        let mut info: TaskVmInfo = std::mem::zeroed();
        let mut count = (std::mem::size_of::<TaskVmInfo>() / 4) as u32;
        let rc = task_info(
            mach_task_self_,
            TASK_VM_INFO,
            (&mut info as *mut TaskVmInfo).cast(),
            &mut count,
        );
        (rc == 0).then_some(info.phys_footprint)
    }
}

#[cfg(target_os = "macos")]
pub fn limit_memory(bytes: u64) -> bool {
    let limit = libc::rlimit {
        rlim_cur: bytes,
        rlim_max: bytes,
    };
    // SAFETY: plain system calls with a pointer to a live struct.
    unsafe {
        libc::setrlimit(libc::RLIMIT_DATA, &limit);
        let none = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        libc::setrlimit(libc::RLIMIT_CORE, &none);
    }
    if footprint().is_none() {
        return false;
    }
    // The app watches from outside as well (see `partridge-engine-client`); a test can switch this
    // thread off to show that.
    #[cfg(debug_assertions)]
    if std::env::var_os("PARTRIDGE_TEST_NO_ENGINE_LIMIT").is_some() {
        return true;
    }
    // A first line: a thread that ends the process when it uses more than the limit. It looks every
    // 20 ms. The code inside the engine could stop it, which is why the app does the same.
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(20));
        if footprint().is_some_and(|used| used > bytes) {
            // SAFETY: `_exit` ends the process at once without running any cleanup that could
            // itself get stuck.
            unsafe { libc::_exit(137) }
        }
    });
    true
}

/// The app puts the engine in a job object with a memory limit; here the engine checks that it is
/// in one.
#[cfg(all(windows, target_pointer_width = "64"))]
pub fn limit_memory(_bytes: u64) -> bool {
    use windows_sys::Win32::System::JobObjects::IsProcessInJob;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut in_job = 0;
    // SAFETY: documented Win32 call with a live out parameter.
    unsafe {
        IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut in_job) != 0 && in_job != 0
    }
}

/// No limit on this platform.
#[cfg(not(any(
    all(target_os = "linux", target_pointer_width = "64"),
    target_os = "macos",
    all(windows, target_pointer_width = "64")
)))]
pub fn limit_memory(_bytes: u64) -> bool {
    false
}

/// Ends the engine when the app that started it is gone. Closing the engine's input already does
/// that, but only for an engine that reads its input: one stuck inside PDFium never looks at it,
/// so a second thread checks that the parent process is still the same one. (On Windows the app
/// puts the engine in a job object instead, which ends it with the app.)
#[cfg(unix)]
pub fn exit_with_parent() {
    // SAFETY: getppid has no arguments and no side effects.
    let parent = unsafe { libc::getppid() };
    // The thread says when it has started. The system call filter is put on every thread, and a
    // thread that is still starting up makes calls (to register itself, to set its stack up) that
    // the filter does not allow; it would be killed halfway, and the engine with it.
    let (ready, started) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = ready.send(());
        // If anything in the loop panics (a refused call, say), the engine ends: an engine that
        // has lost its parent check would outlive the app.
        let _ = std::panic::catch_unwind(|| loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            // SAFETY: as above. When the parent is gone this process is adopted by another one and
            // the number changes. `_exit` skips the cleanup code that could itself get stuck.
            if unsafe { libc::getppid() } != parent {
                unsafe { libc::_exit(0) }
            }
        });
        // SAFETY: ends the process without cleanup.
        unsafe { libc::_exit(70) }
    });
    let _ = started.recv();
}

#[cfg(not(unix))]
pub fn exit_with_parent() {}
