//! Test-only program: starts like the engine, locks itself in like the engine, and then tries the
//! things a sandboxed engine must not be able to do. It prints one line per attempt to the
//! protocol output (a pipe the test reads as plain text) and ends.
//!
//! It is an example, not part of the engine: `cargo build --release` does not build it, and the
//! engine binary has no code for it. `cargo test` builds it, and `tests/sandbox.rs` runs it the way
//! the app runs the engine (same start-up, same inherited document, same restrictions).
//!
//! Usage (the engine's options first, then these):
//! `engine_selftest [--pdfium-dir D] [--document N] [--memory-limit B] probe FILE DIR EXE PID PORT`
//! `engine_selftest [...] memory MIB`
//! `engine_selftest [...] document`

// Like the engine, a program without a console on Windows (it is started with a restricted token).
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::io::Write;

use partridge_engine::{parse_args, startup};

fn main() {
    // The engine's own options come from two places: the test puts some in front of the mode,
    // and the app's start-up code (`partridge-engine-client`) adds `--document` and
    // `--memory-limit` at the end. Sort them out.
    let mut engine_options: Vec<std::ffi::OsString> = Vec::new();
    let mut mode: Vec<String> = Vec::new();
    let mut raw = std::env::args_os().skip(1).peekable();
    while let Some(word) = raw.next() {
        let text = word.to_string_lossy().into_owned();
        if matches!(
            text.as_str(),
            "--pdfium-dir" | "--fonts-dir" | "--document" | "--memory-limit"
        ) {
            engine_options.push(word);
            engine_options.extend(raw.next());
        } else {
            mode.push(text);
        }
    }
    let args = parse_args(engine_options).expect("engine options");

    // Exactly what the engine does, up to the lock-in.
    let startup::Prepared {
        mut engine,
        mut protocol,
        sandbox_problem,
        ..
    } = startup::prepare(args);

    let mut say = |line: String| {
        let _ = writeln!(protocol, "{line}");
        let _ = protocol.flush();
    };
    match &sandbox_problem {
        None => say("sandbox: in force".to_string()),
        Some(why) => say(format!("sandbox: NOT IN FORCE ({why})")),
    }
    // Ask the engine what it reports about itself.
    let hello = engine.handle(partridge_protocol::Request {
        id: 1,
        body: partridge_protocol::RequestBody::Hello {
            protocol_version: partridge_protocol::PROTOCOL_VERSION,
        },
    });
    if let partridge_protocol::ResponseBody::Hello { sandbox, .. } = hello.body {
        say(format!("layers: {sandbox}"));
    }

    match mode[0].as_str() {
        "probe" => probe(&mode[1..], &mut say),
        "memory" => memory(mode[1].parse().expect("MiB"), &mut say),
        "document" => document(&mut engine, &mut say),
        _ => unreachable!(),
    }
    say("done".to_string());
}

fn verdict(name: &str, denied: bool, detail: String) -> String {
    format!(
        "{name}: {}{}",
        if denied { "denied" } else { "ALLOWED" },
        if detail.is_empty() {
            String::new()
        } else {
            format!(" ({detail})")
        }
    )
}

fn probe(args: &[String], say: &mut dyn FnMut(String)) {
    let [file, dir, exe, pid, port] = args else {
        panic!("probe needs FILE DIR EXE PID PORT");
    };
    let mut report = |name: &str, result: std::io::Result<()>| {
        let (denied, detail) = match result {
            Ok(()) => (false, String::new()),
            Err(e) => (true, e.to_string()),
        };
        say(verdict(name, denied, detail));
    };

    // Files, by path.
    report("open_file_by_path", std::fs::File::open(file).map(|_| ()));
    report(
        "open_own_executable",
        std::env::current_exe().and_then(|p| std::fs::File::open(p).map(|_| ())),
    );
    report("list_directory", std::fs::read_dir(dir).map(|_| ()));
    report(
        "create_file",
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(std::path::Path::new(dir).join("created-by-the-engine"))
            .map(|_| ()),
    );
    report(
        "write_existing_file",
        std::fs::OpenOptions::new()
            .write(true)
            .open(file)
            .map(|_| ()),
    );
    report(
        "delete_file",
        std::fs::remove_file(std::path::Path::new(dir).join("keep-me")),
    );

    // The network.
    let address: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    report(
        "connect_socket",
        std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_secs(2))
            .map(|_| ()),
    );
    report(
        "bind_udp_socket",
        std::net::UdpSocket::bind("127.0.0.1:0").map(|_| ()),
    );

    // Processes.
    report(
        "start_process",
        std::process::Command::new(exe)
            .arg("--help")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ()),
    );
    report("touch_other_process", touch_process(pid.parse().unwrap()));
    say(verdict(
        "executable_memory",
        executable_memory().is_err(),
        String::new(),
    ));
}

/// Tries to get at another process: a debugger attach on Linux, a signal on macOS, a memory read
/// handle on Windows.
#[cfg(target_os = "linux")]
fn touch_process(pid: u32) -> std::io::Result<()> {
    // SAFETY: plain system call; a successful attach would be a failure of the sandbox.
    let rc = unsafe { libc::ptrace(libc::PTRACE_ATTACH as _, pid as libc::pid_t, 0, 0) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "macos")]
fn touch_process(pid: u32) -> std::io::Result<()> {
    // SAFETY: signal 0 only checks whether the process can be signalled.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn touch_process(pid: u32) -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_VM_READ};
    // SAFETY: documented call; a handle that does open is closed again.
    unsafe {
        let handle = OpenProcess(PROCESS_VM_READ, 0, pid);
        if handle.is_null() {
            Err(std::io::Error::last_os_error())
        } else {
            CloseHandle(handle);
            Ok(())
        }
    }
}

/// Tries to map memory that can run code.
#[cfg(unix)]
fn executable_memory() -> Result<(), ()> {
    // SAFETY: an anonymous mapping that is unmapped again at once.
    unsafe {
        let p = libc::mmap(
            std::ptr::null_mut(),
            4096,
            libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        );
        if p == libc::MAP_FAILED {
            Err(())
        } else {
            libc::munmap(p, 4096);
            Ok(())
        }
    }
}

#[cfg(windows)]
fn executable_memory() -> Result<(), ()> {
    use windows_sys::Win32::System::Memory::{
        VirtualAlloc, VirtualFree, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READWRITE,
    };
    // SAFETY: documented calls; the region is released again at once.
    unsafe {
        let p = VirtualAlloc(
            std::ptr::null(),
            4096,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_EXECUTE_READWRITE,
        );
        if p.is_null() {
            Err(())
        } else {
            VirtualFree(p, 0, MEM_RELEASE);
            Ok(())
        }
    }
}

/// Uses more memory than the limit allows. If this returns, the limit did not work.
fn memory(limit_mib: usize, say: &mut dyn FnMut(String)) {
    let mut held: Vec<Vec<u8>> = Vec::new();
    // Up to three times the limit, so a limit that works ends the program well before.
    for step in 1..=(limit_mib * 3 / 16) {
        let mut chunk = vec![0u8; 16 * 1024 * 1024];
        // Touch every page, so the memory is really used.
        for i in (0..chunk.len()).step_by(4096) {
            chunk[i] = 1;
        }
        held.push(chunk);
        if step % 4 == 0 {
            say(format!("memory: holding {} MiB", step * 16));
        }
    }
    say(format!("memory: SURVIVED with {} MiB", held.len() * 16));
}

/// Opens the inherited document and draws and searches it, inside the sandbox.
fn document(engine: &mut partridge_engine::Engine, say: &mut dyn FnMut(String)) {
    use partridge_protocol::{Request, RequestBody, ResponseBody};
    let mut id = 10;
    let mut ask = |body: RequestBody| {
        id += 1;
        engine.handle(Request { id, body }).body
    };
    match ask(RequestBody::Open { password: None }) {
        ResponseBody::Opened { doc, pages } => {
            say(format!("open: ok, {} pages", pages.len()));
            match ask(RequestBody::Render {
                doc,
                page: 0,
                width: 240,
                height: 320,
            }) {
                ResponseBody::Rendered { rgba, .. } => {
                    let ink = rgba.chunks(4).filter(|p| p[0] < 128).count();
                    say(format!("render: ok, {ink} dark pixels"));
                }
                other => say(format!("render: FAILED {other:?}")),
            }
            match ask(RequestBody::Search {
                doc,
                page: 0,
                query: "fox".into(),
            }) {
                ResponseBody::Matches { matches } => {
                    say(format!("search: {} matches", matches.len()))
                }
                other => say(format!("search: FAILED {other:?}")),
            }
        }
        other => say(format!("open: FAILED {other:?}")),
    }
}
