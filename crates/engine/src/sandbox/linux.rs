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
        // A different ABI than the one this table is for (32-bit or x32 on a 64-bit kernel) has
        // other system call numbers: kill it.
        stmt(BPF_LD_W_ABS, OFFSET_ARCH),
        jump(BPF_JEQ_K, AUDIT_ARCH, 1, 0),
        stmt(BPF_RET_K, SECCOMP_RET_KILL_PROCESS),
        stmt(BPF_LD_W_ABS, OFFSET_NR),
    ];
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
        Allow(SYS_fcntl),
        // Memory. Nothing may become executable after the lock-in.
        Allow(SYS_brk),
        AllowIfClear {
            nr: SYS_mmap,
            arg: 2,
            mask: PROT_EXEC_BIT,
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
        let rc = libc::syscall(
            libc::SYS_seccomp,
            SECCOMP_SET_MODE_FILTER,
            SECCOMP_FILTER_FLAG_TSYNC,
            &prog as *const SockFprog,
        );
        if rc == 0 {
            return Ok(());
        }
        // A kernel without `seccomp(2)` (before 3.17) still has the older call. It covers the
        // calling thread only, which is all of this process once its helper thread has ended.
        let rc = libc::prctl(libc::PR_SET_SECCOMP, 2, &prog as *const SockFprog);
        if rc == 0 {
            return Ok(());
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

/// Locks the process in and checks that it worked. Returns the layers that are in force.
pub fn lock_down() -> Result<Vec<&'static str>, String> {
    let mut layers = Vec::new();
    // Core files could hold pages of the document; and nobody should attach to the engine.
    // SAFETY: plain call.
    unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(errno_text("no_new_privs"));
        }
    }
    // Landlock first, while the system calls it needs are still allowed.
    #[cfg(debug_assertions)]
    let try_landlock = std::env::var_os("PARTRIDGE_TEST_NO_LANDLOCK").is_none();
    #[cfg(not(debug_assertions))]
    let try_landlock = true;
    if try_landlock {
        if let Ok(name) = apply_landlock() {
            layers.push(name);
        }
    }
    apply_seccomp()?;
    layers.insert(0, "seccomp");
    verify()?;
    Ok(layers)
}

#[cfg(test)]
mod tests {
    use super::*;

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
                | Rule::AllowIfNull { nr, .. } => nr,
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
