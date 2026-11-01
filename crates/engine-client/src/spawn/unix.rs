//! Starting the engine on Linux and macOS: a normal child process with the document on descriptor
//! 3. Everything else the child could inherit is closed by the engine itself at its start, and the
//! engine locks itself in once it has loaded PDFium.

use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use super::{engine_args, EngineProcess, SpawnRequest, SpawnedEngine, DOCUMENT_DESCRIPTOR};

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
    let stdin = child.stdin.take().expect("stdin is piped");
    let stdout = child.stdout.take().expect("stdout is piped");
    Ok(SpawnedEngine {
        process: Box::new(UnixProcess(child)),
        stdin: Box::new(stdin),
        stdout: Box::new(stdout),
    })
}
