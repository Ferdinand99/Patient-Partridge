//! Linux: a seccomp-BPF allowlist (always) and Landlock (where the kernel has it).
//!
//! The seccomp filter is the base. It allows the few system calls the engine needs once PDFium is
//! loaded: reading and seeking in the document it already holds, memory, time, futexes, exit. It
//! does not allow opening files, creating sockets, starting processes, `ptrace`, or mapping memory
//! as executable. Everything else gets `EPERM`. A program in the wrong system call ABI is killed.
//!
//! Landlock adds a second wall for the file system (no read, write, execute or create anywhere) and
//! on newer kernels for TCP and for signals and abstract sockets. It is only a second layer: a
//! kernel without it still has the seccomp filter, which already refuses to open any file.
//!
//! Landlock holds for the thread that applies it and the threads that thread starts afterwards,
//! and for no thread that already runs. So it is applied (`restrict_files`) while the process has
//! one thread, before the threads of the engine are started, and the seccomp filter, which is put
//! on every thread at once, comes last. The greeting names Landlock only when it holds for every
//! thread.

use std::mem::size_of;

// seccomp_data layout: nr (u32) at 0, arch (u32) at 4, instruction pointer at 8, args from 16.
const OFFSET_NR: u32 = 0;
const OFFSET_ARCH: u32 = 4;
const fn offset_arg_low(i: u32) -> u32 {
    16 + 8 * i
}
const fn offset_arg_high(i: u32) -> u32 {
    16 + 8 * i + 4
}

#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: u32 = 0xC000_003E;
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: u32 = 0xC000_00B7;

const BPF_LD_W_ABS: u16 = 0x20;
const BPF_JEQ_K: u16 = 0x15;
const BPF_JSET_K: u16 = 0x45;
const BPF_RET_K: u16 = 0x06;

const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;

const PROT_EXEC_BIT: u32 = 4;
const AT_EMPTY_PATH_BIT: u32 = 0x1000;
/// MAP_SHARED (1) and MAP_SHARED_VALIDATE (3) both have bit 0 set. MAP_GROWSDOWN (0x0100, the same
/// on x86_64 and arm64) is refused too: the kernel does not count such a mapping as data, so it
/// would also be a way round RLIMIT_DATA.
const MAP_SHARED_BITS: u32 = 0x01 | 0x0100;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const SockFilter,
}

/// One entry of the allowlist.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Rule {
    /// Always allowed.
    Allow(i64),
    /// Allowed when none of the bits in `mask` is set in the low 32 bits of argument `arg`.
    AllowIfClear { nr: i64, arg: u32, mask: u32 },
    /// Allowed when at least one of the bits in `mask` is set in the low 32 bits of argument `arg`.
    AllowIfSet { nr: i64, arg: u32, mask: u32 },
    /// Allowed when the 64-bit argument `arg` is zero (a null pointer).
    AllowIfNull { nr: i64, arg: u32 },
    /// Allowed when the low 32 bits of argument `arg` are one of `values`.
    AllowIfIn {
        nr: i64,
        arg: u32,
        values: &'static [u32],
    },
    /// Allowed when none of the bits of `mask_a` is set in argument `arg_a` and none of the bits
    /// of `mask_b` in argument `arg_b`.
    AllowIfClearBoth {
        nr: i64,
        arg_a: u32,
        mask_a: u32,
        arg_b: u32,
        mask_b: u32,
    },
}

fn stmt(code: u16, k: u32) -> SockFilter {
    SockFilter {
        code,
        jt: 0,
        jf: 0,
        k,
    }
}

fn jump(code: u16, k: u32, jt: u8, jf: u8) -> SockFilter {
    SockFilter { code, jt, jf, k }
}

/// Turns the allowlist into a BPF program. Anything not listed gets `EPERM`.
pub(crate) fn build_filter(rules: &[Rule]) -> Vec<SockFilter> {
    let eperm = SECCOMP_RET_ERRNO | libc::EPERM as u32;
    let mut p = vec![
        // A different ABI than the one this table is for (32-bit on a 64-bit kernel) has other
        // system call numbers: kill it.
        stmt(BPF_LD_W_ABS, OFFSET_ARCH),
        jump(BPF_JEQ_K, AUDIT_ARCH, 1, 0),
        stmt(BPF_RET_K, SECCOMP_RET_KILL_PROCESS),
        stmt(BPF_LD_W_ABS, OFFSET_NR),
    ];
    // The x32 ABI has the same architecture value and differs only in bit 30 of the number. Such a
    // number matches no rule below and would get EPERM; it is killed like the other ABIs.
    #[cfg(target_arch = "x86_64")]
    {
        p.push(jump(BPF_JSET_K, 0x4000_0000, 0, 1));
        p.push(stmt(BPF_RET_K, SECCOMP_RET_KILL_PROCESS));
    }
    for rule in rules {
        match *rule {
            Rule::Allow(nr) => {
                p.push(jump(BPF_JEQ_K, nr as u32, 0, 1));
                p.push(stmt(BPF_RET_K, SECCOMP_RET_ALLOW));
            }
            Rule::AllowIfClear { nr, arg, mask } => {
                p.push(jump(BPF_JEQ_K, nr as u32, 0, 4));
                p.push(stmt(BPF_LD_W_ABS, offset_arg_low(arg)));
                p.push(jump(BPF_JSET_K, mask, 1, 0));
                p.push(stmt(BPF_RET_K, SECCOMP_RET_ALLOW));
                p.push(stmt(BPF_RET_K, eperm));
            }
            Rule::AllowIfSet { nr, arg, mask } => {
                p.push(jump(BPF_JEQ_K, nr as u32, 0, 4));
                p.push(stmt(BPF_LD_W_ABS, offset_arg_low(arg)));
                p.push(jump(BPF_JSET_K, mask, 1, 0));
                p.push(stmt(BPF_RET_K, eperm));
                p.push(stmt(BPF_RET_K, SECCOMP_RET_ALLOW));
            }
            Rule::AllowIfIn { nr, arg, values } => {
                let k = values.len();
                p.push(jump(BPF_JEQ_K, nr as u32, 0, (k + 3) as u8));
                p.push(stmt(BPF_LD_W_ABS, offset_arg_low(arg)));
                for (i, value) in values.iter().enumerate() {
                    p.push(jump(BPF_JEQ_K, *value, (k - i) as u8, 0));
                }
                p.push(stmt(BPF_RET_K, eperm));
                p.push(stmt(BPF_RET_K, SECCOMP_RET_ALLOW));
            }
            Rule::AllowIfClearBoth {
                nr,
                arg_a,
                mask_a,
                arg_b,
                mask_b,
            } => {
                p.push(jump(BPF_JEQ_K, nr as u32, 0, 6));
                p.push(stmt(BPF_LD_W_ABS, offset_arg_low(arg_a)));
                p.push(jump(BPF_JSET_K, mask_a, 3, 0));
                p.push(stmt(BPF_LD_W_ABS, offset_arg_low(arg_b)));
                p.push(jump(BPF_JSET_K, mask_b, 1, 0));
                p.push(stmt(BPF_RET_K, SECCOMP_RET_ALLOW));
                p.push(stmt(BPF_RET_K, eperm));
            }
            Rule::AllowIfNull { nr, arg } => {
                p.push(jump(BPF_JEQ_K, nr as u32, 0, 6));
                p.push(stmt(BPF_LD_W_ABS, offset_arg_low(arg)));
                p.push(jump(BPF_JEQ_K, 0, 0, 3));
                p.push(stmt(BPF_LD_W_ABS, offset_arg_high(arg)));
                p.push(jump(BPF_JEQ_K, 0, 0, 1));
                p.push(stmt(BPF_RET_K, SECCOMP_RET_ALLOW));
                p.push(stmt(BPF_RET_K, eperm));
            }
        }
    }
    p.push(stmt(BPF_RET_K, eperm));
    p
}

/// What the engine may still do after it is locked in.
pub(crate) fn allowlist() -> Vec<Rule> {
    use libc::*;
    use Rule::*;
    let mut r = vec![
        // Reading the document through the inherited handle, and talking to the app.
        Allow(SYS_read),
        Allow(SYS_pread64),
        Allow(SYS_write),
        Allow(SYS_writev),
        Allow(SYS_lseek),
        Allow(SYS_close),
        // Only the commands the engine uses: F_GETFD, F_SETFD, F_GETFL. Not F_SETOWN, F_SETSIG,
        // F_SETOWN_EX, F_SETFL (with O_ASYNC), F_SETLEASE or F_SETPIPE_SZ: with the first four a
        // pipe sends a signal of the engine's choice to any process of the user.
        AllowIfIn {
            nr: SYS_fcntl,
            arg: 1,
            values: &[F_GETFD as u32, F_SETFD as u32, F_GETFL as u32],
        },
        // Memory. Nothing may become executable after the lock-in, and nothing may be shared or
        // grow down: neither a shared anonymous mapping nor a MAP_GROWSDOWN one counts against
        // RLIMIT_DATA, so each would be a way round the memory limit.
        Allow(SYS_brk),
        AllowIfClearBoth {
            nr: SYS_mmap,
            arg_a: 2,
            mask_a: PROT_EXEC_BIT,
            arg_b: 3,
            mask_b: MAP_SHARED_BITS,
        },
        AllowIfClear {
            nr: SYS_mprotect,
            arg: 2,
            mask: PROT_EXEC_BIT,
        },
        Allow(SYS_munmap),
        Allow(SYS_mremap),
        Allow(SYS_madvise),
        // Threads and time.
        Allow(SYS_futex),
        Allow(SYS_sched_yield),
        Allow(SYS_clock_gettime),
        Allow(SYS_clock_getres),
        Allow(SYS_clock_nanosleep),
        Allow(SYS_nanosleep),
        // A sleep or wait that was interrupted (by SIGSTOP and SIGCONT, or the freezer on suspend)
        // is restarted by the kernel with this call. Without it the engine dies after a suspend.
        Allow(SYS_restart_syscall),
        // Facts about itself.
        Allow(SYS_getpid),
        Allow(SYS_gettid),
        Allow(SYS_getppid),
        Allow(SYS_sched_getaffinity),
        Allow(SYS_uname),
        Allow(SYS_getrandom),
        // Information about an open file (never about a path).
        Allow(SYS_fstat),
        AllowIfSet {
            nr: SYS_newfstatat,
            arg: 3,
            mask: AT_EMPTY_PATH_BIT,
        },
        AllowIfSet {
            nr: SYS_statx,
            arg: 2,
            mask: AT_EMPTY_PATH_BIT,
        },
        // Reading limits is fine; changing them is not.
        AllowIfNull {
            nr: SYS_prlimit64,
            arg: 2,
        },
        // Signal handling of the runtime, and leaving.
        Allow(SYS_rt_sigaction),
        Allow(SYS_rt_sigprocmask),
        Allow(SYS_rt_sigreturn),
        Allow(SYS_sigaltstack),
        Allow(SYS_exit),
        Allow(SYS_exit_group),
    ];
    #[cfg(target_arch = "x86_64")]
    {
        r.push(Allow(SYS_gettimeofday));
        r.push(Allow(SYS_time));
    }
    r
}

fn errno_text(what: &str) -> String {
    format!("{what}: {}", std::io::Error::last_os_error())
}

/// Applies the seccomp filter to every thread of the process.
fn apply_seccomp() -> Result<(), String> {
    let filter = build_filter(&allowlist());
    let prog = SockFprog {
        len: filter.len() as u16,
        filter: filter.as_ptr(),
    };
    const SECCOMP_SET_MODE_FILTER: libc::c_long = 1;
    const SECCOMP_FILTER_FLAG_TSYNC: libc::c_long = 1;
    // SAFETY: plain system calls; `prog` points at a live, correctly laid out program.
    unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(errno_text("no_new_privs"));
        }
        // The filter must cover every thread. There is no fallback to the older call that covers one
        // thread: a helper thread without the filter would hand the attacker a way out. A positive
        // result is the id of a thread that could not be synchronised, and is also a failure.
        let rc = libc::syscall(
            libc::SYS_seccomp,
            SECCOMP_SET_MODE_FILTER,
            SECCOMP_FILTER_FLAG_TSYNC,
            &prog as *const SockFprog,
        );
        if rc == 0 {
            return Ok(());
        }
        if rc > 0 {
            return Err(format!("seccomp: thread {rc} could not be filtered"));
        }
    }
    Err(errno_text("seccomp"))
}

const SYS_LANDLOCK_CREATE_RULESET: libc::c_long = 444;
const SYS_LANDLOCK_RESTRICT_SELF: libc::c_long = 446;
const LANDLOCK_CREATE_RULESET_VERSION: libc::c_ulong = 1;

/// Every file system right Landlock has had, by the version that added it.
fn landlock_fs_rights(abi: i64) -> u64 {
    let mut rights: u64 = (1 << 13) - 1; // version 1: execute .. make symlink
    if abi >= 2 {
        rights |= 1 << 13; // refer
    }
    if abi >= 3 {
        rights |= 1 << 14; // truncate
    }
    if abi >= 5 {
        rights |= 1 << 15; // ioctl on devices
    }
    rights
}

/// Landlock: no file system access at all (a ruleset with no rule allows nothing), no TCP, and no
/// signals or abstract sockets to other domains. Returns what it could enforce, or why not.
fn apply_landlock() -> Result<&'static str, String> {
    #[repr(C)]
    struct Attr {
        handled_access_fs: u64,
        handled_access_net: u64,
        scoped: u64,
    }
    // SAFETY: querying the version takes no pointer.
    let abi = unsafe {
        libc::syscall(
            SYS_LANDLOCK_CREATE_RULESET,
            std::ptr::null::<Attr>(),
            0usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    if abi < 1 {
        return Err(errno_text("landlock is not available"));
    }
    let attr = Attr {
        handled_access_fs: landlock_fs_rights(abi),
        handled_access_net: if abi >= 4 { 0b11 } else { 0 },
        scoped: if abi >= 6 { 0b11 } else { 0 },
    };
    // The kernel accepts the attribute in the size it knows, with the newer fields left out.
    let size = match abi {
        1..=3 => 8,
        4..=5 => 16,
        _ => size_of::<Attr>(),
    };
    // SAFETY: `attr` is a live struct and `size` never exceeds it.
    let fd = unsafe {
        libc::syscall(
            SYS_LANDLOCK_CREATE_RULESET,
            &attr as *const Attr,
            size,
            0 as libc::c_ulong,
        )
    };
    if fd < 0 {
        return Err(errno_text("landlock ruleset"));
    }
    // SAFETY: `fd` is the ruleset descriptor just made; closing it after use is correct.
    unsafe {
        let rc = libc::syscall(
            SYS_LANDLOCK_RESTRICT_SELF,
            fd as libc::c_int,
            0 as libc::c_ulong,
        );
        libc::close(fd as libc::c_int);
        if rc != 0 {
            return Err(errno_text("landlock restrict"));
        }
    }
    Ok("landlock")
}

/// Tries what the sandbox forbids. If any of it works, the sandbox is not in force.
fn verify() -> Result<(), String> {
    // SAFETY: plain calls with constant arguments; descriptors that unexpectedly open are closed.
    unsafe {
        let fd = libc::open(c"/proc/self/exe".as_ptr(), libc::O_RDONLY);
        if fd >= 0 {
            libc::close(fd);
            return Err("a file could still be opened".to_string());
        }
        let fd = libc::open(c"/".as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY);
        if fd >= 0 {
            libc::close(fd);
            return Err("a folder could still be opened".to_string());
        }
        let s = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
        if s >= 0 {
            libc::close(s);
            return Err("a socket could still be created".to_string());
        }
    }
    Ok(())
}

/// Whether `enter_empty_root` worked. Read by `lock_down` for the report.
static EMPTY_ROOT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[repr(C)]
struct CapHeader {
    version: u32,
    pid: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CapData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

/// Puts this process in a user namespace and a mount namespace whose root is an empty file system,
/// so that a path lookup finds nothing. The system call filter cannot read a path, so it cannot tell
/// `newfstatat(AT_FDCWD, "/home/u/.ssh/id_ed25519", AT_EMPTY_PATH)` from a call on an open file;
/// in an empty root that call finds no file to describe.
///
/// This is an extra layer: where the system does not allow it (user namespaces switched off, a
/// container profile that refuses `unshare` or `mount`) the engine goes on without it and does not
/// report the layer. It must be called while the process has one thread, because a user namespace
/// cannot be entered by a process with more.
pub fn enter_empty_root() -> Result<(), String> {
    use std::ffi::CString;
    // Ubuntu 24.04 and later let a program make a user namespace but move it to an AppArmor
    // profile that has no rights in it, and that profile is also applied to the descriptors the
    // program already holds: the document would stop being readable. The switch that does this is
    // read first, and the namespace is not tried where it is on.
    if std::fs::read_to_string("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")
        .is_ok_and(|v| v.trim() == "1")
    {
        return Err("user namespaces are restricted by AppArmor on this system".to_string());
    }
    if std::fs::read_to_string("/proc/sys/kernel/unprivileged_userns_clone")
        .is_ok_and(|v| v.trim() == "0")
    {
        return Err("user namespaces are switched off on this system".to_string());
    }
    let path = |p: &str| CString::new(p).expect("no nul in a constant");
    // SAFETY: plain system calls with constant arguments and live C strings.
    unsafe {
        let uid = libc::getuid();
        let gid = libc::getgid();
        if libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS) != 0 {
            return Err(errno_text("unshare"));
        }
        // From here the process is in the new user namespace. It must say who it is.
        std::fs::write("/proc/self/setgroups", "deny").map_err(|e| format!("setgroups: {e}"))?;
        std::fs::write("/proc/self/uid_map", format!("{uid} {uid} 1"))
            .map_err(|e| format!("uid_map: {e}"))?;
        std::fs::write("/proc/self/gid_map", format!("{gid} {gid} 1"))
            .map_err(|e| format!("gid_map: {e}"))?;
        // Nothing of what happens to the mounts below may reach the real ones.
        if libc::mount(
            std::ptr::null(),
            c"/".as_ptr(),
            std::ptr::null(),
            libc::MS_REC | libc::MS_PRIVATE,
            std::ptr::null(),
        ) != 0
        {
            return Err(errno_text("mount private"));
        }
        // An empty file system on a folder that every system has, and that becomes the root.
        let mut mounted = None;
        for dir in ["/tmp", "/mnt", "/run", "/var", "/srv", "/opt"] {
            let d = path(dir);
            if libc::mount(
                c"tmpfs".as_ptr(),
                d.as_ptr(),
                c"tmpfs".as_ptr(),
                libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
                c"size=4k,mode=0555".as_ptr().cast(),
            ) == 0
            {
                mounted = Some(d);
                break;
            }
        }
        let Some(dir) = mounted else {
            return Err(errno_text("mount tmpfs"));
        };
        if libc::chdir(dir.as_ptr()) != 0 {
            return Err(errno_text("chdir"));
        }
        // pivot_root(".", "."): the old root is stacked under the new one, and then detached.
        if libc::syscall(libc::SYS_pivot_root, c".".as_ptr(), c".".as_ptr()) != 0 {
            return Err(errno_text("pivot_root"));
        }
        if libc::umount2(c".".as_ptr(), libc::MNT_DETACH) != 0 {
            return Err(errno_text("umount"));
        }
        if libc::chdir(c"/".as_ptr()) != 0 {
            return Err(errno_text("chdir /"));
        }
        // The new root may not be changed, and the process gives up the capabilities it has in the
        // namespace: the system call filter forbids `mount` and the like anyway.
        libc::mount(
            std::ptr::null(),
            c"/".as_ptr(),
            std::ptr::null(),
            libc::MS_REMOUNT | libc::MS_BIND | libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV,
            std::ptr::null(),
        );
        let header = CapHeader {
            version: 0x2008_0522,
            pid: 0,
        };
        let data = [CapData {
            effective: 0,
            permitted: 0,
            inheritable: 0,
        }; 2];
        if libc::syscall(libc::SYS_capset, &header as *const CapHeader, data.as_ptr()) != 0 {
            return Err(errno_text("capset"));
        }
    }
    EMPTY_ROOT.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// Set once [`restrict_files`] was called, so that [`lock_down`] does not do it a second time.
static LANDLOCK_TRIED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True when Landlock is in force for every thread of the process: it was applied while the
/// process had one thread, so each thread started after it has it too.
static LANDLOCK_ON_ALL_THREADS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// True when the process runs one thread only. In an empty root the answer is known without a look
/// (a process with more than one thread cannot get one, and nothing has started a thread since);
/// otherwise the threads are counted. When that cannot be done the answer is no.
fn single_threaded() -> bool {
    if EMPTY_ROOT.load(std::sync::atomic::Ordering::SeqCst) {
        return true;
    }
    std::fs::read_dir("/proc/self/task").is_ok_and(|tasks| tasks.count() == 1)
}

/// Puts the process under Landlock where the kernel has it (no file system access at all, no TCP,
/// no signals or abstract sockets to other domains). Call it while the process runs one thread, so
/// that every thread it starts later is under it too: `landlock_restrict_self` acts on the calling
/// thread and on the threads that thread starts afterwards, and on no other thread that already
/// runs (the seccomp filter has `TSYNC` for that; Landlock has no such flag). So this comes before
/// the threads of the engine are started, and after [`enter_empty_root`], because a process under
/// Landlock may not change its mounts. Starting a thread needs no file access.
///
/// The layer is reported (see [`lock_down`]) only when it is in force for every thread: when the
/// process already ran more than one thread, Landlock is still applied (to this thread and the
/// ones started later), and it is not reported.
pub fn restrict_files() -> Result<(), String> {
    LANDLOCK_TRIED.store(true, std::sync::atomic::Ordering::SeqCst);
    // Core files could hold pages of the document; and nobody should attach to the engine. Landlock
    // needs no_new_privs, and the capabilities are gone after the empty root.
    // SAFETY: plain call.
    unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(errno_text("no_new_privs"));
        }
    }
    #[cfg(debug_assertions)]
    {
        if std::env::var_os("PARTRIDGE_TEST_NO_LANDLOCK").is_some() {
            return Err("switched off by a test".to_string());
        }
    }
    let alone = single_threaded();
    apply_landlock()?;
    LANDLOCK_ON_ALL_THREADS.store(alone, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// Locks the process in and checks that it worked. Returns the layers that are in force.
pub fn lock_down() -> Result<Vec<&'static str>, String> {
    let mut layers = Vec::new();
    // Landlock is normally in force already (see `restrict_files`). Where nobody asked for it yet
    // it is applied now, and then it cannot be in force for every thread.
    if !LANDLOCK_TRIED.load(std::sync::atomic::Ordering::SeqCst) {
        let _ = restrict_files();
    }
    // Core files could hold pages of the document; and nobody should attach to the engine.
    // SAFETY: plain call.
    unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(errno_text("no_new_privs"));
        }
    }
    if LANDLOCK_ON_ALL_THREADS.load(std::sync::atomic::Ordering::SeqCst) {
        layers.push("landlock");
    }
    apply_seccomp()?;
    layers.insert(0, "seccomp");
    verify()?;
    // The descriptors the engine was given must still work after all this (an AppArmor profile or
    // another module could take that away). The input is checked here; the document is taken only
    // when this has passed.
    // SAFETY: plain call on descriptor 0 with a live buffer.
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        if libc::fstat(0, &mut st) != 0 {
            return Err(errno_text("the standard input stopped working"));
        }
    }
    if EMPTY_ROOT.load(std::sync::atomic::Ordering::SeqCst) {
        // A path lookup must find nothing at all. `std::fs::metadata` would get EPERM from the
        // filter whether the root is empty or not, so this uses `newfstatat` with AT_EMPTY_PATH,
        // the one form of it that the filter lets through.
        // SAFETY: plain calls with constant arguments and live buffers.
        unsafe {
            let mut st: libc::stat = std::mem::zeroed();
            for name in [c"/etc", c"/proc"] {
                let r = libc::syscall(
                    libc::SYS_newfstatat,
                    libc::AT_FDCWD,
                    name.as_ptr(),
                    &mut st as *mut libc::stat,
                    libc::AT_EMPTY_PATH,
                );
                if r == 0 {
                    return Err("the empty root is not empty".to_string());
                }
            }
        }
        layers.push("empty-root");
    }
    Ok(layers)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Linux commands that the libc crate does not name.
    const F_SETSIG_COMMAND: i32 = 10;
    const F_SETOWN_EX_COMMAND: i32 = 15;

    /// Runs the filter on one system call, the way the kernel does, and gives the answer.
    fn run(nr: i64, args: [u64; 6]) -> u32 {
        let program = build_filter(&allowlist());
        let mut a: u32 = 0;
        let mut pc = 0usize;
        let data = |offset: u32| -> u32 {
            match offset {
                OFFSET_NR => nr as u32,
                OFFSET_ARCH => AUDIT_ARCH,
                o if o >= 16 => {
                    let index = ((o - 16) / 8) as usize;
                    let half = (o - 16) % 8;
                    let v = args[index];
                    if half == 0 {
                        v as u32
                    } else {
                        (v >> 32) as u32
                    }
                }
                _ => 0,
            }
        };
        loop {
            let ins = program[pc];
            pc += 1;
            match ins.code {
                BPF_LD_W_ABS => a = data(ins.k),
                BPF_JEQ_K => pc += if a == ins.k { ins.jt } else { ins.jf } as usize,
                BPF_JSET_K => pc += if a & ins.k != 0 { ins.jt } else { ins.jf } as usize,
                BPF_RET_K => return ins.k,
                other => panic!("instruction {other:#x} is not simulated"),
            }
        }
    }

    const ALLOW: u32 = SECCOMP_RET_ALLOW;
    const EPERM_ANSWER: u32 = SECCOMP_RET_ERRNO | libc::EPERM as u32;

    #[test]
    fn fcntl_is_allowed_for_the_three_commands_the_engine_uses_and_for_nothing_else() {
        let fcntl = |cmd: i32| run(libc::SYS_fcntl, [3, cmd as u64, 0, 0, 0, 0]);
        for ok in [libc::F_GETFD, libc::F_SETFD, libc::F_GETFL] {
            assert_eq!(fcntl(ok), ALLOW, "command {ok}");
        }
        // The route to a signal for any process of the user, and the other ways round.
        for bad in [
            libc::F_SETOWN,
            F_SETSIG_COMMAND,
            F_SETOWN_EX_COMMAND,
            libc::F_SETFL,
            libc::F_SETLEASE,
            libc::F_SETPIPE_SZ,
            libc::F_DUPFD,
            libc::F_DUPFD_CLOEXEC,
            libc::F_SETLK,
        ] {
            assert_eq!(fcntl(bad), EPERM_ANSWER, "command {bad}");
        }
    }

    #[test]
    fn memory_may_not_be_executable_or_shared() {
        let mmap = |prot: i32, flags: i32| {
            run(
                libc::SYS_mmap,
                [0, 4096, prot as u64, flags as u64, u64::MAX, 0],
            )
        };
        let private = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS;
        assert_eq!(mmap(libc::PROT_READ | libc::PROT_WRITE, private), ALLOW);
        assert_eq!(
            mmap(libc::PROT_READ | libc::PROT_EXEC, private),
            EPERM_ANSWER
        );
        assert_eq!(
            mmap(
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_ANONYMOUS
            ),
            EPERM_ANSWER
        );
        assert_eq!(
            mmap(libc::PROT_READ, libc::MAP_SHARED_VALIDATE),
            EPERM_ANSWER
        );
        // A private mapping that grows down is not counted as data either.
        assert_eq!(
            mmap(
                libc::PROT_READ | libc::PROT_WRITE,
                private | libc::MAP_GROWSDOWN
            ),
            EPERM_ANSWER
        );
    }

    #[test]
    fn an_interrupted_sleep_may_be_restarted() {
        assert_eq!(run(libc::SYS_restart_syscall, [0; 6]), ALLOW);
    }

    #[test]
    fn calls_that_open_connect_or_run_get_eperm_in_the_simulation() {
        for nr in [
            libc::SYS_openat,
            libc::SYS_socket,
            libc::SYS_execve,
            libc::SYS_ptrace,
            libc::SYS_kill,
            libc::SYS_tgkill,
        ] {
            assert_eq!(run(nr, [0; 6]), EPERM_ANSWER, "call {nr}");
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn the_x32_abi_is_killed() {
        assert_eq!(
            run(libc::SYS_read | 0x4000_0000, [0; 6]),
            SECCOMP_RET_KILL_PROCESS
        );
    }

    #[test]
    fn the_filter_starts_with_the_architecture_check_and_ends_with_a_denial() {
        let filter = build_filter(&allowlist());
        assert_eq!(filter[0].k, OFFSET_ARCH);
        assert_eq!(filter[2].k, SECCOMP_RET_KILL_PROCESS);
        let last = filter.last().unwrap();
        assert_eq!(last.code, BPF_RET_K);
        assert_eq!(last.k, SECCOMP_RET_ERRNO | libc::EPERM as u32);
        // A BPF program may have 4096 instructions at most.
        assert!(filter.len() < 4096, "{}", filter.len());
    }

    #[test]
    fn every_jump_stays_inside_the_program() {
        let filter = build_filter(&allowlist());
        for (i, ins) in filter.iter().enumerate() {
            if ins.code == BPF_JEQ_K || ins.code == BPF_JSET_K {
                assert!(i + 1 + (ins.jt as usize) < filter.len(), "jt at {i}");
                assert!(i + 1 + (ins.jf as usize) < filter.len(), "jf at {i}");
            }
        }
    }

    #[test]
    fn the_list_has_no_system_call_that_opens_runs_or_connects() {
        use libc::*;
        let listed: Vec<i64> = allowlist()
            .iter()
            .map(|r| match *r {
                Rule::Allow(nr)
                | Rule::AllowIfClear { nr, .. }
                | Rule::AllowIfSet { nr, .. }
                | Rule::AllowIfNull { nr, .. }
                | Rule::AllowIfIn { nr, .. }
                | Rule::AllowIfClearBoth { nr, .. } => nr,
            })
            .collect();
        for forbidden in [
            SYS_openat,
            SYS_execve,
            SYS_execveat,
            SYS_socket,
            SYS_connect,
            SYS_bind,
            SYS_ptrace,
            SYS_clone,
            SYS_kill,
            SYS_tkill,
            SYS_tgkill,
            SYS_mkdirat,
            SYS_unlinkat,
            SYS_renameat,
            SYS_seccomp,
            SYS_prctl,
            SYS_ioctl,
            SYS_process_vm_readv,
            SYS_process_vm_writev,
        ] {
            assert!(
                !listed.contains(&forbidden),
                "{forbidden} must not be allowed"
            );
        }
    }
}
