//! Test-only program: starts like the engine, locks itself in like the engine, and then tries the
//! things a sandboxed engine must not be able to do. It prints one line per attempt to the
//! protocol output (a pipe the test reads as plain text) and ends.
//!
//! It is an example, not part of the engine: `cargo build --release` does not build it, and the
//! engine binary has no code for it. `cargo test` builds it, and `tests/sandbox.rs` runs it the way
//! the app runs the engine (same start-up, same inherited document, same restrictions).
//!
//! Usage (the engine's options first, then these):
//! `engine_selftest [--pdfium-dir D] [--document N] [--memory-limit B] probe FILE DIR EXE PID PORT TITLE [NAME=VALUE ...]`
//! `engine_selftest [...] memory MIB HOLD_SECONDS [app-only]`
//! `engine_selftest [...] document`
//! `engine_selftest [...] descriptors`
//! `engine_selftest control FILE DIR EXE PID PORT TITLE [NAME=VALUE ...]` runs the probes of `probe` in a program
//! that is not locked in, to show that each attempt is one that works without the sandbox.

// Like the engine, a program without a console on Windows (it is started with a restricted token).
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::io::Write;

use partridge_engine::{parse_args, startup};

/// Linux: the command that chooses the signal a pipe sends (the libc crate has no name for it).
#[cfg(target_os = "linux")]
const F_SETSIG_COMMAND: libc::c_int = 10;

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
            "--pdfium-dir" | "--fonts-dir" | "--document" | "--memory-limit" | "--font"
        ) {
            engine_options.push(word);
            engine_options.extend(raw.next());
        } else {
            mode.push(text);
        }
    }

    // Libraries that the probes use and that a locked-in program could not load are loaded now.
    #[cfg(windows)]
    icmp_api::load();

    // The control is not locked in: the same attempts, to see that they work without a sandbox.
    if mode[0] == "control" {
        let mut say = |line: String| println!("{line}");
        probe(&mode[1..], &mut say);
        say("done".to_string());
        return;
    }

    // `app-only` switches the engine's own memory watch off (macOS), so that only the app's watch
    // from outside can end the program. Set before anything else starts, while there is one thread.
    if mode[0] == "memory" && mode.get(3).is_some_and(|m| m == "app-only") {
        std::env::set_var("PARTRIDGE_TEST_NO_ENGINE_LIMIT", "1");
    }

    let args = parse_args(engine_options).expect("engine options");

    // On Windows the network support is started before the lock-in, the way the engine's own
    // check does it, so that a refusal later comes from the system and not from a library that
    // could not start.
    #[cfg(windows)]
    let _ = std::net::UdpSocket::bind("127.0.0.1:0");

    // Exactly what the engine does, up to the lock-in.
    let startup::Prepared {
        mut engine,
        mut protocol,
        sandbox_problem,
        isolation_problem,
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
    if let Some(why) = &isolation_problem {
        say(format!("isolation: not in force ({why})"));
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
        "memory" => memory(
            mode[1].parse().expect("MiB"),
            mode[2].parse().expect("seconds"),
            &mut say,
        ),
        "document" => document(&mut engine, &mut say),
        "descriptors" => {
            // How many descriptors (handles on Windows) are open: the ones the engine was given
            // and what the system and the libraries hold, which is the same for every run.
            #[cfg(unix)]
            {
                // SAFETY: asking about a descriptor number only reads the descriptor table.
                let open = (0..1024)
                    .filter(|fd| unsafe { libc::fcntl(*fd, libc::F_GETFD) } >= 0)
                    .count();
                say(format!("descriptors: {open}"));
            }
            #[cfg(windows)]
            {
                use windows_sys::Win32::System::Threading::{
                    GetCurrentProcess, GetProcessHandleCount,
                };
                let mut count = 0u32;
                // SAFETY: the pseudo handle of this process and a number to write into.
                let ok = unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) };
                if ok == 0 {
                    say("descriptors: unknown".to_string());
                } else {
                    say(format!("descriptors: {count}"));
                }
            }
        }
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
    let [file, dir, exe, pid, port, title, extra @ ..] = args else {
        panic!("probe needs FILE DIR EXE PID PORT TITLE [NAME=VALUE ...]");
    };
    // Optional targets, each as NAME=VALUE (Windows): `host` is a listener on an address of this
    // computer that is not the loopback one, `dns` a name for the lookup, `v6port` a TCP port on
    // `[::1]`, `remote` an address on another computer that accepts TCP connections (`ADDRESS:PORT`),
    // `pipe` a named pipe with a NULL access list. Without a value the attempt is left out.
    let option = |name: &str| -> Option<String> {
        extra
            .iter()
            .find_map(|e| e.strip_prefix(name).and_then(|v| v.strip_prefix('=')))
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let host_address = option("host");
    let dns_name = option("dns");
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

    // The network: real attempts, the error is the system's.
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
    // A datagram to the UDP socket the test listens on. The socket is bound first, so where the
    // system refuses the bind this line shows the bind and no datagram is sent; the attempts
    // below (`windows_network_probes`) send from a socket that was never bound.
    report(
        "udp_bind_then_send",
        std::net::UdpSocket::bind("0.0.0.0:0")
            .and_then(|s| s.send_to(b"hello", address).map(|_| ())),
    );
    // The same as the TCP connection, to an address of this computer that is not the loopback
    // one: the way to the network card, not only the loopback device.
    if let Some(host) = host_address {
        let host: std::net::SocketAddr = host.parse().expect("HOST_ADDRESS");
        report(
            "connect_host_address",
            std::net::TcpStream::connect_timeout(&host, std::time::Duration::from_secs(2))
                .map(|_| ()),
        );
    }
    // A name lookup: the resolver asks a service of the system, and that asks a server.
    if let Some(name) = dns_name {
        use std::net::ToSocketAddrs;
        report(
            "dns_lookup",
            (name, 443).to_socket_addrs().and_then(|mut found| {
                found
                    .next()
                    .map(|_| ())
                    .ok_or_else(|| std::io::Error::other("no address"))
            }),
        );
    }

    #[cfg(windows)]
    windows_network_probes(&option, port, &mut report);

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

    #[cfg(unix)]
    unix_probes(file, say);
    #[cfg(windows)]
    windows_probes(title, say);
    let _ = title;

    // What the engine can see of the app's environment.
    let mut names: Vec<String> = std::env::vars_os()
        .map(|(n, _)| n.to_string_lossy().into_owned())
        .collect();
    names.sort();
    say(format!("environment: {}", names.join(",")));
}

/// Tries to get at another process: a debugger attach is not what a user-owned process cannot do
/// without a sandbox (it can, as a parent or with a loose Yama setting), so the probe sends signal
/// 0, which a process of the same user may always do.
#[cfg(unix)]
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

/// Probes of the Unix systems: the ways round the system call filter that a review found.
#[cfg(unix)]
fn unix_probes(file: &str, say: &mut dyn FnMut(String)) {
    // The signal route: a pipe that is told to signal a process when it is used. Descriptor 0 is
    // the read end of the protocol input. If these succeed, the next read or write on the pipe
    // sends the signal to any process of the user.
    let target = std::process::id() as libc::c_int;
    let fcntl = |cmd: libc::c_int, arg: libc::c_int| -> std::io::Result<()> {
        // SAFETY: plain calls on descriptor 0 with constant commands.
        let rc = unsafe { libc::fcntl(0, cmd, arg) };
        if rc < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    };
    let mut line = |name: &str, r: std::io::Result<()>| {
        let (denied, detail) = match r {
            Ok(()) => (false, String::new()),
            Err(e) => (true, e.to_string()),
        };
        say(verdict(name, denied, detail));
    };
    line("fcntl_setown", fcntl(libc::F_SETOWN, target));
    #[cfg(target_os = "linux")]
    line("fcntl_setsig", fcntl(F_SETSIG_COMMAND, libc::SIGKILL));
    line("fcntl_setfl_async", fcntl(libc::F_SETFL, libc::O_ASYNC));
    // Allowed on purpose: the three commands the engine uses.
    let ok = |r: std::io::Result<()>| if r.is_ok() { "yes" } else { "no" };
    say(format!(
        "fcntl_getfd_getfl: {}",
        ok(fcntl(libc::F_GETFD, 0).and(fcntl(libc::F_GETFL, 0)))
    ));

    // A shared anonymous mapping would not count against the memory limit.
    #[cfg(target_os = "linux")]
    {
        // SAFETY: an anonymous mapping that is unmapped again at once.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                4096,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        let failed = p == libc::MAP_FAILED;
        let e = std::io::Error::last_os_error();
        if !failed {
            // SAFETY: as above.
            unsafe { libc::munmap(p, 4096) };
        }
        say(verdict(
            "shared_anonymous_mapping",
            failed,
            if failed { e.to_string() } else { String::new() },
        ));

        // Information about a path: `newfstatat` with a real name and the flag that is meant for
        // an empty one. The filter cannot read the name. In an empty root there is nothing to find.
        let mut buffer = [0u8; 256];
        let mut found = |name: &str, path: &str| {
            let c = std::ffi::CString::new(path).unwrap();
            // SAFETY: the buffer is larger than `struct stat`.
            let rc = unsafe {
                libc::syscall(
                    libc::SYS_newfstatat,
                    libc::AT_FDCWD,
                    c.as_ptr(),
                    buffer.as_mut_ptr(),
                    libc::AT_EMPTY_PATH,
                )
            };
            let e = std::io::Error::last_os_error();
            say(format!(
                "{name}: {}",
                if rc == 0 {
                    "FOUND".to_string()
                } else {
                    format!("not found ({e})")
                }
            ));
        };
        found("stat_system_file", "/etc/passwd");
        found("stat_document_path", file);
    }
    let _ = file;
}

/// Probes of Windows: the desktop, the clipboard, other windows, and the network below Winsock.
#[cfg(windows)]
fn windows_probes(title: &str, say: &mut dyn FnMut(String)) {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard,
    };
    use windows_sys::Win32::System::StationsAndDesktops::{
        GetThreadDesktop, GetUserObjectInformationW, UOI_NAME,
    };
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FindWindowW, GetForegroundWindow,
    };

    // SAFETY: documented calls with live buffers.
    unsafe {
        // The desktop this program is on.
        let mut name = [0u16; 128];
        let mut needed = 0u32;
        let desktop = GetThreadDesktop(GetCurrentThreadId());
        if !desktop.is_null()
            && GetUserObjectInformationW(
                desktop,
                UOI_NAME,
                name.as_mut_ptr().cast(),
                (name.len() * 2) as u32,
                &mut needed,
            ) != 0
        {
            let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
            say(format!(
                "desktop: {}",
                String::from_utf16_lossy(&name[..end])
            ));
        } else {
            say("desktop: unknown".to_string());
        }

        // A window of the test program, on the user's desktop.
        let wide: Vec<u16> = std::ffi::OsStr::new(title)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let found = !FindWindowW(std::ptr::null(), wide.as_ptr()).is_null();
        say(verdict("find_window_of_the_user", !found, String::new()));
        say(verdict(
            "foreground_window",
            GetForegroundWindow().is_null(),
            String::new(),
        ));
        unsafe extern "system" fn count(_: *mut c_void, total: isize) -> i32 {
            *(total as *mut u32) += 1;
            1
        }
        let mut windows = 0u32;
        EnumWindows(Some(count), &mut windows as *mut u32 as isize);
        say(format!("windows_on_the_desktop: {windows}"));

        // The clipboard: reading what is on it, clearing it, and putting something of our own on
        // it. (Putting something on it needs a window of our own that owns the clipboard.)
        let class = wide_text("STATIC");
        let owner = windows_sys::Win32::UI::WindowsAndMessaging::CreateWindowExW(
            0,
            class.as_ptr(),
            std::ptr::null(),
            0x8000_0000, // WS_POPUP
            0,
            0,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        );
        say(format!(
            "window_created: {}",
            if owner.is_null() { "no" } else { "yes" }
        ));
        let opened = OpenClipboard(owner) != 0;
        let read = opened && !GetClipboardData(13).is_null();
        let cleared = opened && EmptyClipboard() != 0;
        let mut wrote = false;
        if cleared {
            let memory = windows_sys::Win32::System::Memory::GlobalAlloc(
                windows_sys::Win32::System::Memory::GMEM_MOVEABLE,
                2,
            );
            if !memory.is_null() {
                let p = windows_sys::Win32::System::Memory::GlobalLock(memory) as *mut u8;
                *p = b'x';
                *p.add(1) = 0;
                windows_sys::Win32::System::Memory::GlobalUnlock(memory);
                wrote = !windows_sys::Win32::System::DataExchange::SetClipboardData(1, memory)
                    .is_null();
            }
        }
        if opened {
            CloseClipboard();
        }
        say(verdict("clipboard_read", !read, String::new()));
        say(verdict("clipboard_clear", !cleared, String::new()));
        say(verdict("clipboard_write", !wrote, String::new()));
    }

    // The network below Winsock: the device that every socket is made from.
    say(afd_endpoint());
}

/// More ways to reach the network, tried from the locked-in program and from the control. Each
/// returns `Ok` when it worked.
///
/// - Datagrams from sockets that were never bound (`sendto` binds them by itself, so these reach
///   the point that the bind-first attempt never reaches): to the loopback address, to an address on
///   another computer (the TEST-NET-1 address, which the stack sends to the gateway) and to `[::1]`.
/// - A TCP connection to `[::1]`, and to an address on another computer.
/// - A raw socket, an echo request sent from it, and an ICMP echo through the ICMP service of the
///   system.
/// - A path on another computer: the kernel connects to it without any socket of ours.
/// - A named pipe whose access list is NULL, which any program of the user could talk to: a local
///   proxy would be one.
#[cfg(windows)]
fn windows_network_probes(
    option: &dyn Fn(&str) -> Option<String>,
    port: &str,
    report: &mut dyn FnMut(&str, std::io::Result<()>),
) {
    use std::net::{Ipv6Addr, SocketAddr, TcpStream};
    use std::time::{Duration, Instant};

    let timeout = Duration::from_secs(3);
    let port: u16 = port.parse().expect("PORT");
    report(
        "udp_sendto_unbound_loopback",
        sendto_unbound(SocketAddr::from(([127, 0, 0, 1], port))),
    );
    report(
        "udp_sendto_unbound_remote",
        sendto_unbound(SocketAddr::from(([192, 0, 2, 1], 9))),
    );
    if let Some(v6) = option("v6port").and_then(|p| p.parse::<u16>().ok()) {
        let target = SocketAddr::from((Ipv6Addr::LOCALHOST, v6));
        report(
            "connect_v6_loopback",
            TcpStream::connect_timeout(&target, timeout).map(|_| ()),
        );
        report("udp_sendto_unbound_v6", sendto_unbound(target));
    }
    if let Some(remote) = option("remote") {
        let target: SocketAddr = remote.parse().expect("remote");
        report(
            "connect_remote",
            TcpStream::connect_timeout(&target, timeout).map(|_| ()),
        );
        report("icmp_echo_remote", icmp_echo(target.ip()));
    }
    report("raw_socket", raw_socket(None));
    report(
        "raw_socket_send_loopback",
        raw_socket(Some(SocketAddr::from(([127, 0, 0, 1], 0)))),
    );
    report(
        "raw_socket_send_remote",
        raw_socket(Some(SocketAddr::from(([192, 0, 2, 1], 0)))),
    );
    report(
        "icmp_echo_loopback",
        icmp_echo(std::net::IpAddr::from([127, 0, 0, 1])),
    );

    // The server message block client of the kernel, to an address of this computer that is not
    // the loopback one (the loopback one is reached without the network card).
    let share = match option("host").and_then(|h| h.parse::<SocketAddr>().ok()) {
        Some(host) => format!(r"\\{}\C$\Windows\win.ini", host.ip()),
        None => r"\\127.0.0.1\C$\Windows\win.ini".to_string(),
    };
    report("open_network_path", std::fs::File::open(share).map(|_| ()));
    // And to a computer that does not exist. A refusal that comes at once says that no connection
    // was tried: trying one takes seconds. The control does not do this (`smb_remote` is only
    // given to the locked-in program).
    if option("smb_remote").is_some() {
        let start = Instant::now();
        let result = std::fs::File::open(r"\\192.0.2.1\x\y");
        let took = start.elapsed().as_millis();
        report(
            "open_network_path_remote",
            result
                .map(|_| ())
                .map_err(|e| std::io::Error::other(format!("{e}; after {took} ms"))),
        );
    }

    if let Some(pipe) = option("pipe") {
        report(
            "open_local_pipe",
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(pipe)
                .map(|_| ()),
        );
    }
}

/// Sends one datagram from a socket that was never bound: the system binds it with the send. An
/// error says which call failed, so that a refusal of `sendto` is not taken for a refusal of
/// `socket`.
#[cfg(windows)]
fn sendto_unbound(target: std::net::SocketAddr) -> std::io::Result<()> {
    use windows_sys::Win32::Networking::WinSock::{
        closesocket, sendto, socket, WSAGetLastError, INVALID_SOCKET, SOCKADDR,
    };
    // The socket address by hand: family (little endian), port (big endian), address.
    let mut address = Vec::new();
    match target {
        std::net::SocketAddr::V4(v4) => {
            address.extend_from_slice(&2u16.to_le_bytes());
            address.extend_from_slice(&v4.port().to_be_bytes());
            address.extend_from_slice(&v4.ip().octets());
            address.extend_from_slice(&[0u8; 8]);
        }
        std::net::SocketAddr::V6(v6) => {
            address.extend_from_slice(&23u16.to_le_bytes());
            address.extend_from_slice(&v6.port().to_be_bytes());
            address.extend_from_slice(&0u32.to_le_bytes());
            address.extend_from_slice(&v6.ip().octets());
            address.extend_from_slice(&0u32.to_le_bytes());
        }
    }
    let family = if target.is_ipv4() { 2 } else { 23 };
    // SAFETY: documented Winsock calls; Winsock is started (the program made a socket before the
    // lock-in) and the buffers live through the calls.
    unsafe {
        let s = socket(family, 2, 17);
        if s == INVALID_SOCKET {
            let e = std::io::Error::from_raw_os_error(WSAGetLastError());
            return Err(std::io::Error::other(format!("socket: {e}")));
        }
        let sent = sendto(
            s,
            b"hello-unbound".as_ptr(),
            13,
            0,
            address.as_ptr().cast::<SOCKADDR>(),
            address.len() as i32,
        );
        let code = WSAGetLastError();
        closesocket(s);
        if sent < 0 {
            let e = std::io::Error::from_raw_os_error(code);
            return Err(std::io::Error::other(format!("sendto: {e}")));
        }
    }
    Ok(())
}

/// A raw ICMP socket, and an echo request sent from it. `send` is where the attempt ends if the
/// socket could be made. An account without administrator rights cannot make a raw socket anywhere.
/// The error says which call failed.
#[cfg(windows)]
fn raw_socket(send_to: Option<std::net::SocketAddr>) -> std::io::Result<()> {
    use windows_sys::Win32::Networking::WinSock::{
        closesocket, sendto, socket, WSAGetLastError, INVALID_SOCKET, SOCKADDR,
    };
    // SAFETY: documented Winsock calls; the buffers live through the calls.
    unsafe {
        let s = socket(2, 3, 1);
        if s == INVALID_SOCKET {
            let e = std::io::Error::from_raw_os_error(WSAGetLastError());
            return Err(std::io::Error::other(format!("socket: {e}")));
        }
        let result = match send_to {
            None => Ok(()),
            Some(std::net::SocketAddr::V4(v4)) => {
                let mut address = Vec::new();
                address.extend_from_slice(&2u16.to_le_bytes());
                address.extend_from_slice(&0u16.to_be_bytes());
                address.extend_from_slice(&v4.ip().octets());
                address.extend_from_slice(&[0u8; 8]);
                // An ICMP echo request: type 8, code 0, checksum, identifier 1, sequence 1.
                let packet = [8u8, 0, 0xF7, 0xFD, 0, 1, 0, 1];
                let sent = sendto(
                    s,
                    packet.as_ptr(),
                    packet.len() as i32,
                    0,
                    address.as_ptr().cast::<SOCKADDR>(),
                    address.len() as i32,
                );
                let code = WSAGetLastError();
                if sent < 0 {
                    let e = std::io::Error::from_raw_os_error(code);
                    Err(std::io::Error::other(format!("sendto: {e}")))
                } else {
                    Ok(())
                }
            }
            Some(_) => Err(std::io::Error::other("only IPv4")),
        };
        closesocket(s);
        result
    }
}

/// The ICMP functions of `iphlpapi.dll`. The library is loaded by name before the lock-in: it is
/// not one of the system's known libraries, and a locked-in program cannot open a library file, so
/// importing it the usual way would make the program fail to start.
#[cfg(windows)]
mod icmp_api {
    use std::ffi::c_void;
    use std::sync::OnceLock;

    pub type Create = unsafe extern "system" fn() -> *mut c_void;
    pub type SendEcho = unsafe extern "system" fn(
        *mut c_void,
        u32,
        *const c_void,
        u16,
        *const c_void,
        *mut c_void,
        u32,
        u32,
    ) -> u32;
    pub type Close = unsafe extern "system" fn(*mut c_void) -> i32;

    pub struct Api {
        pub create: Create,
        pub send_echo: SendEcho,
        pub close: Close,
    }

    static API: OnceLock<Option<Api>> = OnceLock::new();

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryA(name: *const std::ffi::c_char) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void;
    }

    /// Loads the library and finds the three functions. Call it before the lock-in.
    pub fn load() {
        API.get_or_init(|| {
            // SAFETY: documented calls with terminated names; the library stays loaded.
            unsafe {
                let module = LoadLibraryA(c"iphlpapi.dll".as_ptr());
                if module.is_null() {
                    return None;
                }
                let create = GetProcAddress(module, c"IcmpCreateFile".as_ptr());
                let send = GetProcAddress(module, c"IcmpSendEcho".as_ptr());
                let close = GetProcAddress(module, c"IcmpCloseHandle".as_ptr());
                if create.is_null() || send.is_null() || close.is_null() {
                    return None;
                }
                let api = Api {
                    create: std::mem::transmute::<*mut c_void, Create>(create),
                    send_echo: std::mem::transmute::<*mut c_void, SendEcho>(send),
                    close: std::mem::transmute::<*mut c_void, Close>(close),
                };
                // The library loads parts of itself the first time it is used; a locked-in
                // program cannot load them, so the first use is made here.
                let handle = (api.create)();
                if !handle.is_null() && handle as isize != -1 {
                    (api.close)(handle);
                }
                Some(api)
            }
        });
    }

    pub fn get() -> Option<&'static Api> {
        API.get().and_then(Option::as_ref)
    }
}

/// One ICMP echo through the ICMP service of the system (`IcmpSendEcho`), which is not a socket of
/// the program. `Ok` when a reply came.
#[cfg(windows)]
fn icmp_echo(target: std::net::IpAddr) -> std::io::Result<()> {
    let std::net::IpAddr::V4(v4) = target else {
        return Err(std::io::Error::other("only IPv4"));
    };
    let Some(api) = icmp_api::get() else {
        return Err(std::io::Error::other("the ICMP library did not load"));
    };
    // SAFETY: documented calls; the reply buffer is larger than one reply and the data.
    unsafe {
        let handle = (api.create)();
        if handle.is_null() || handle as isize == -1 {
            return Err(std::io::Error::other(format!(
                "IcmpCreateFile: {}",
                std::io::Error::last_os_error()
            )));
        }
        let mut reply = [0u8; 256];
        let data = [b'x'; 8];
        let replies = (api.send_echo)(
            handle,
            u32::from_ne_bytes(v4.octets()),
            data.as_ptr().cast(),
            data.len() as u16,
            std::ptr::null(),
            reply.as_mut_ptr().cast(),
            reply.len() as u32,
            1500,
        );
        let error = std::io::Error::last_os_error();
        (api.close)(handle);
        if replies == 0 {
            return Err(std::io::Error::other(format!("IcmpSendEcho: {error}")));
        }
    }
    Ok(())
}

/// Opens `\Device\Afd\Endpoint` with `NtCreateFile`, which is what creating a socket does first.
#[cfg(windows)]
fn afd_endpoint() -> String {
    use std::ffi::c_void;
    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *const u16,
    }
    #[repr(C)]
    struct ObjectAttributes {
        length: u32,
        root_directory: *mut c_void,
        object_name: *const UnicodeString,
        attributes: u32,
        security_descriptor: *const c_void,
        security_quality_of_service: *const c_void,
    }
    #[repr(C)]
    struct IoStatusBlock {
        status: isize,
        information: usize,
    }
    #[link(name = "ntdll")]
    extern "system" {
        fn NtCreateFile(
            handle: *mut *mut c_void,
            access: u32,
            attributes: *const ObjectAttributes,
            io: *mut IoStatusBlock,
            allocation: *const i64,
            file_attributes: u32,
            share: u32,
            disposition: u32,
            options: u32,
            ea: *const c_void,
            ea_length: u32,
        ) -> i32;
    }
    let path: Vec<u16> = r"\Device\Afd\Endpoint".encode_utf16().collect();
    let name = UnicodeString {
        length: (path.len() * 2) as u16,
        maximum_length: (path.len() * 2) as u16,
        buffer: path.as_ptr(),
    };
    let attributes = ObjectAttributes {
        length: std::mem::size_of::<ObjectAttributes>() as u32,
        root_directory: std::ptr::null_mut(),
        object_name: &name,
        attributes: 0x40, // OBJ_CASE_INSENSITIVE
        security_descriptor: std::ptr::null(),
        security_quality_of_service: std::ptr::null(),
    };
    let mut handle: *mut c_void = std::ptr::null_mut();
    let mut io = IoStatusBlock {
        status: 0,
        information: 0,
    };
    // GENERIC_READ | GENERIC_WRITE | SYNCHRONIZE; share read and write; FILE_OPEN.
    // SAFETY: documented native call with live structures.
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            0xC010_0000,
            &attributes,
            &mut io,
            std::ptr::null(),
            0,
            3,
            1,
            0,
            std::ptr::null(),
            0,
        )
    };
    if status >= 0 && !handle.is_null() {
        // SAFETY: the handle was just opened.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
    }
    // 0xC0000022 is STATUS_ACCESS_DENIED.
    let text = format!("NTSTATUS {:#010X}", status as u32);
    if status as u32 == 0xC000_0022 {
        verdict("afd_endpoint", true, text)
    } else {
        format!(
            "afd_endpoint: {} ({text})",
            if status >= 0 { "ALLOWED" } else { "other" }
        )
    }
}

#[cfg(windows)]
fn wide_text(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
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

/// Uses more memory than the limit allows, and keeps it. If this returns, the limit did not work.
///
/// The program holds the memory for up to `hold_seconds` instead of ending as soon as it has it.
/// Where the limit is a watch that looks every few tens of milliseconds (macOS), a program that
/// takes a few hundred megabytes in less time than that and then ends is never seen, which is no
/// failure of the limit: it is the one thing the watch does not promise. What it promises is that
/// a program that stays over the limit is ended, and that is what this program is.
fn memory(limit_mib: usize, hold_seconds: u64, say: &mut dyn FnMut(String)) {
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
    say(format!("memory: holding it all ({} MiB)", held.len() * 16));
    let until = std::time::Instant::now() + std::time::Duration::from_secs(hold_seconds);
    while std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(10));
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
