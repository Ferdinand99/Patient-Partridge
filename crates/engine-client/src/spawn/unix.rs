//! Starting the engine on Linux and macOS: a normal child process with the document on descriptor
//! 3 and an environment that holds nothing of the app's. Everything else the child could inherit is
//! closed by the engine itself at its start, and the engine locks itself in once it has loaded
//! PDFium.
//!
//! On macOS the app also watches the memory of the engine from outside (see `watch_memory`).

use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use super::DOCUMENT_DESCRIPTOR;
use super::{engine_args, engine_environment, EngineProcess, SpawnRequest, SpawnedEngine};

struct UnixProcess(Child);

impl EngineProcess for UnixProcess {
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

/// Ends the engine when it uses more memory than it may, watched from the app. macOS does not
/// enforce `RLIMIT_DATA`, and a limit that a thread inside the engine keeps could be stopped by the
/// code it is meant to hold back (Mach calls are not governed by the profile). The app can look at
/// the footprint of another process with `proc_pid_rusage` and end it with a signal, and nothing
/// the engine does can stop that.
///
/// The watch looks every 25 ms (more when the computer is busy; a look has been seen to come 100 ms
/// late on a loaded CI machine). It promises that an engine that stays over the limit is ended
/// within a fraction of a second. It does not promise anything about memory that is taken and
/// given back within one look: a program can touch several hundred megabytes in 25 ms, so it can
/// go past the limit by that much before it is seen, and one that ends before the look is never
/// seen at all.
#[cfg(target_os = "macos")]
fn watch_memory(pid: u32, limit: u64) {
    extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut std::ffi::c_void) -> i32;
    }
    /// The footprint of the process and its start time, from the first flavour of `rusage_info`.
    /// (Words of eight bytes: 16 bytes of uuid, then times and counters; `ri_phys_footprint` is
    /// word 9 and `ri_proc_start_abstime` word 10.)
    fn usage(pid: u32) -> Option<(u64, u64)> {
        let mut buffer = [0u64; 32];
        // SAFETY: the buffer is larger than the structure of flavour 0.
        let rc = unsafe { proc_pid_rusage(pid as i32, 0, buffer.as_mut_ptr().cast()) };
        (rc == 0).then(|| (buffer[9], buffer[10]))
    }
    std::thread::spawn(move || {
        let mut started: Option<u64> = None;
        let mut failures = 0;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(25));
            let Some((footprint, start)) = usage(pid) else {
                // The numbers may not be there yet (the process is still being started) or any
                // more (it has ended). Look for it a while before giving up; a process that does
                // not exist is gone for sure.
                failures += 1;
                // SAFETY: signal 0 only asks whether the process exists.
                let exists = unsafe { libc::kill(pid as i32, 0) } == 0;
                if !exists || failures > 400 {
                    return;
                }
                continue;
            };
            failures = 0;
            // The same process as before, and not another one that got the number.
            if *started.get_or_insert(start) != start {
                return;
            }
            if footprint > limit {
                // SAFETY: a signal to the process that was just seen to be the engine.
                unsafe { libc::kill(pid as i32, libc::SIGKILL) };
                return;
            }
        }
    });
}

pub(super) fn spawn(request: &SpawnRequest<'_>) -> io::Result<SpawnedEngine> {
    // A copy of the handle on a descriptor above 2, so that the standard descriptors the child
    // gets cannot overwrite it. Copies made this way are not inherited unless asked for.
    let document = request.document.map(|f| f.try_clone()).transpose()?;
    let mut command = Command::new(request.program);
    command
        .args(engine_args(
            request,
            document.as_ref().map(|_| DOCUMENT_DESCRIPTOR as u64),
        ))
        // Nothing of the app's environment: tokens, agent sockets, the user's name.
        .env_clear()
        .envs(engine_environment())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(document) = &document {
        let fd = document.as_raw_fd();
        // SAFETY: the closure runs in the child between fork and exec and only calls `dup2` and
        // `fcntl`, which are safe to call there. It puts the document on its fixed number; if it
        // already has that number it just stays open across exec.
        unsafe {
            command.pre_exec(move || {
                if fd == DOCUMENT_DESCRIPTOR {
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                } else if libc::dup2(fd, DOCUMENT_DESCRIPTOR) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command.spawn()?;
    #[cfg(target_os = "macos")]
    watch_memory(
        child.id(),
        request.memory_limit.unwrap_or(super::DEFAULT_MEMORY_LIMIT),
    );
    let stdin = child.stdin.take().expect("stdin is piped");
    let stdout = child.stdout.take().expect("stdout is piped");
    Ok(SpawnedEngine {
        process: Box::new(UnixProcess(child)),
        stdin: Box::new(stdin),
        stdout: Box::new(stdout),
    })
}
