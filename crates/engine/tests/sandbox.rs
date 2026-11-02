//! The proof that the engine is sandboxed, on every system the tests run on.
//!
//! `engine_selftest` (an example program, built by `cargo test` and not part of any release) starts
//! the way the engine starts, locks itself in the same way, and then tries the things the sandbox
//! must stop. It is started through the same code the app uses to start the engine, so on Windows
//! it gets the restricted token, the job object and the mitigation policies, and on every system
//! it gets the document as an inherited handle. Each attempt is also made by a copy of the program
//! that is not locked in (the control), so that a denial is not an attempt that never could work.
//!
//! The second half runs the real engine: it must report its sandbox, draw pages inside it
//! (including a page with fonts that are not embedded), search and read text, survive being
//! stopped and continued, and refuse to work when it has no document.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, ToSocketAddrs, UdpSocket};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use partridge_engine_client::{
    spawn_engine, ClientError, EngineClient, EngineConfig, SandboxPolicy, SpawnRequest,
};
use partridge_protocol::{read_frame, write_request, ErrorCode, Request, RequestBody, Response};
use partridge_testkit as kit;

fn selftest_program() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap().join("examples");
    dir.join(format!("engine_selftest{}", std::env::consts::EXE_SUFFIX))
}

/// What the layers of the sandbox are called on this system.
fn expected_layer() -> &'static str {
    if cfg!(target_os = "linux") {
        "seccomp"
    } else if cfg!(target_os = "macos") {
        "seatbelt"
    } else {
        "restricted-token"
    }
}

/// What the program printed, and how the process ended.
struct Run {
    lines: Vec<String>,
    exit: String,
}

impl Run {
    fn succeeded(&self) -> bool {
        self.exit.ends_with(": 0")
    }
}

/// Runs `engine_selftest` with the document handed over.
fn run_selftest(document: &std::fs::File, memory_limit: Option<u64>, mode_args: &[String]) -> Run {
    let mut args: Vec<std::ffi::OsString> =
        vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
    args.extend(mode_args.iter().map(Into::into));
    let mut engine = spawn_engine(&SpawnRequest {
        program: &selftest_program(),
        args: &args,
        document: Some(document),
        memory_limit,
        // Required: if the restrictions cannot be applied, the test says why instead of
        // quietly running an unrestricted program.
        policy: SandboxPolicy::Required,
        fonts: &[],
    })
    .expect("the program starts");
    drop(engine.stdin);
    // The output is read by a thread, so that a program that hangs fails the test instead of
    // holding it for ever.
    let collected = Arc::new(Mutex::new(Vec::<u8>::new()));
    let (finished_tx, finished) = mpsc::channel();
    {
        let collected = collected.clone();
        let mut stdout = engine.stdout;
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(n) = stdout.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                collected.lock().unwrap().extend_from_slice(&buffer[..n]);
            }
            let _ = finished_tx.send(());
        });
    }
    let on_time = finished.recv_timeout(Duration::from_secs(90)).is_ok();
    if !on_time {
        let _ = engine.process.kill();
        let _ = engine.process.wait();
        let so_far = String::from_utf8_lossy(&collected.lock().unwrap()).into_owned();
        panic!("the program did not end within 90 seconds. Output so far:\n{so_far}");
    }
    let _ = engine.process.wait();
    let exit = engine
        .process
        .try_wait()
        .ok()
        .flatten()
        .unwrap_or_else(|| "still running".to_string());
    let output = String::from_utf8_lossy(&collected.lock().unwrap()).into_owned();
    Run {
        lines: output.lines().map(str::to_string).collect(),
        exit,
    }
}

/// Runs the probes in a copy of the program that is not locked in, with the environment of this
/// test. Everything it is asked to do works there.
fn run_control(probe_args: &[String]) -> Vec<String> {
    let output = std::process::Command::new(selftest_program())
        .arg("control")
        .args(probe_args)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("the control starts");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

fn line<'a>(lines: &'a [String], name: &str) -> &'a str {
    lines
        .iter()
        .find(|l| l.starts_with(&format!("{name}:")))
        .unwrap_or_else(|| panic!("no line for {name} in {lines:#?}"))
}

/// True when the line says the attempt was refused with the error this system gives for a refusal.
fn refused_with_permission_error(l: &str) -> bool {
    if cfg!(windows) {
        l.contains("os error 5)")
    } else {
        l.contains("os error 1)") || l.contains("os error 13)")
    }
}

/// A TCP listener and a UDP socket on the same loopback port, both without waiting: something to
/// connect to and something to send to.
fn listeners() -> (TcpListener, UdpSocket) {
    for _ in 0..100 {
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = udp.local_addr().unwrap().port();
        if let Ok(tcp) = TcpListener::bind(("127.0.0.1", port)) {
            tcp.set_nonblocking(true).unwrap();
            return (tcp, udp);
        }
    }
    panic!("no port that is free for both TCP and UDP");
}

/// True when a connection is made to the listener within `wait`.
fn accepts_within(listener: &TcpListener, wait: Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        if listener.accept().is_ok() {
            return true;
        }
        if start.elapsed() > wait {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A listener on an address of this computer that is not the loopback one (Windows). It finds the
/// address by asking the system which one it would use to reach another network; nothing is sent.
/// Without a route the check is left out, except on the CI runners, which have one.
fn host_listener() -> Option<(TcpListener, SocketAddr)> {
    if !cfg!(windows) {
        return None;
    }
    let found = (|| {
        let probe = UdpSocket::bind("0.0.0.0:0").ok()?;
        probe.connect("192.0.2.1:9").ok()?;
        let ip = probe.local_addr().ok()?.ip();
        if ip.is_loopback() || ip.is_unspecified() {
            return None;
        }
        let listener = TcpListener::bind((ip, 0)).ok()?;
        listener.set_nonblocking(true).ok()?;
        let address = listener.local_addr().ok()?;
        Some((listener, address))
    })();
    if found.is_none() {
        assert!(
            std::env::var_os("GITHUB_ACTIONS").is_none(),
            "the runner has no address besides the loopback one"
        );
        println!("this computer has no other address: the check of that address is left out");
    }
    found
}

/// A TCP listener and a UDP socket on the same port of `[::1]` (Windows). `None` where the computer
/// has no IPv6 loopback address, except on the CI runners, which have one.
fn v6_listeners() -> Option<(TcpListener, UdpSocket, u16)> {
    if !cfg!(windows) {
        return None;
    }
    for _ in 0..100 {
        let Ok(udp) = UdpSocket::bind("[::1]:0") else {
            break;
        };
        let port = udp.local_addr().unwrap().port();
        if let Ok(tcp) = TcpListener::bind(("::1", port)) {
            tcp.set_nonblocking(true).unwrap();
            return Some((tcp, udp, port));
        }
    }
    assert!(
        std::env::var_os("GITHUB_ACTIONS").is_none(),
        "the runner has no IPv6 loopback address"
    );
    println!("this computer has no IPv6 loopback address: the IPv6 checks are left out");
    None
}

/// An address on another computer that accepts TCP connections: the first IPv4 address of the name
/// (port 443). Nothing is sent to it except the connection. `None` without a network, except on the
/// CI runners, which have one.
fn remote_target(name: Option<&str>) -> Option<String> {
    let found = name.and_then(|name| {
        (name, 443)
            .to_socket_addrs()
            .ok()?
            .find(SocketAddr::is_ipv4)
            .map(|a| a.to_string())
    });
    if found.is_none() {
        assert!(
            std::env::var_os("GITHUB_ACTIONS").is_none(),
            "the runner has no IPv4 address of another computer to try"
        );
        println!("no other computer to try: the check of a remote address is left out");
    }
    found
}

/// A name that only the network can resolve. Without network the lookup is left out, except on
/// the CI runners, which have one.
fn dns_name() -> Option<String> {
    let name = "github.com";
    if (name, 443)
        .to_socket_addrs()
        .is_ok_and(|mut found| found.next().is_some())
    {
        return Some(name.to_string());
    }
    assert!(
        std::env::var_os("GITHUB_ACTIONS").is_none(),
        "{name} does not resolve on the runner"
    );
    println!("{name} does not resolve here: the name lookup check is left out");
    None
}

/// Named pipes whose access list is NULL (open to every program), made by the test. A local proxy
/// would be one. There are two instances: the control takes one and the locked-in program tries the
/// other. (Windows only.)
#[cfg(windows)]
mod pipes {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        InitializeSecurityDescriptor, SetSecurityDescriptorDacl, SECURITY_ATTRIBUTES,
    };

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateNamedPipeW(
            name: *const u16,
            open_mode: u32,
            pipe_mode: u32,
            max_instances: u32,
            out_buffer: u32,
            in_buffer: u32,
            timeout: u32,
            attributes: *const SECURITY_ATTRIBUTES,
        ) -> *mut c_void;
    }

    pub struct Pipes {
        pub name: String,
        handles: Vec<*mut c_void>,
    }

    pub fn start() -> Pipes {
        let name = format!(r"\\.\pipe\partridge-proxy-{}", std::process::id());
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: documented calls; the descriptor outlives the calls that use it.
        unsafe {
            let mut descriptor = [0u64; 8];
            let psd = descriptor.as_mut_ptr().cast::<c_void>();
            assert!(InitializeSecurityDescriptor(psd, 1) != 0);
            // A DACL that is present and NULL: everybody may do everything.
            assert!(SetSecurityDescriptorDacl(psd, 1, std::ptr::null(), 0) != 0);
            let attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: psd,
                bInheritHandle: 0,
            };
            let handles: Vec<_> = (0..2)
                .map(|_| {
                    // PIPE_ACCESS_DUPLEX, a byte pipe that waits, four instances at most.
                    let h = CreateNamedPipeW(wide.as_ptr(), 3, 0, 4, 4096, 4096, 0, &attributes);
                    assert!(h as isize != -1 && !h.is_null(), "no named pipe");
                    h
                })
                .collect();
            Pipes { name, handles }
        }
    }

    impl Drop for Pipes {
        fn drop(&mut self) {
            for h in &self.handles {
                // SAFETY: the handles were made by `start`.
                unsafe { CloseHandle(*h) };
            }
        }
    }
}

/// A window and some text on the clipboard of the user's session, made by the test, for the
/// engine to look for. (Windows only.)
#[cfg(windows)]
mod session {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow};

    pub struct Session {
        window: *mut std::ffi::c_void,
        pub clipboard_set: bool,
    }

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub fn start(title: &str) -> Session {
        // SAFETY: documented calls with live buffers.
        unsafe {
            let class = wide("STATIC");
            let name = wide(title);
            let window = CreateWindowExW(
                0,
                class.as_ptr(),
                name.as_ptr(),
                0x8000_0000, // WS_POPUP
                0,
                0,
                10,
                10,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            let mut session = Session {
                window,
                clipboard_set: false,
            };
            session.refill();
            session
        }
    }

    impl Session {
        pub fn has_window(&self) -> bool {
            !self.window.is_null()
        }

        /// Puts the text on the clipboard again (the control empties it).
        pub fn refill(&mut self) {
            // SAFETY: documented calls with live buffers.
            unsafe {
                self.clipboard_set = false;
                if OpenClipboard(std::ptr::null_mut()) != 0 {
                    EmptyClipboard();
                    let text = wide("a secret on the clipboard");
                    let memory = GlobalAlloc(GMEM_MOVEABLE, text.len() * 2);
                    if !memory.is_null() {
                        let p = GlobalLock(memory) as *mut u16;
                        std::ptr::copy_nonoverlapping(text.as_ptr(), p, text.len());
                        GlobalUnlock(memory);
                        self.clipboard_set = !SetClipboardData(13, memory).is_null();
                    }
                    CloseClipboard();
                }
            }
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            if !self.window.is_null() {
                // SAFETY: the window was made by this value.
                unsafe { DestroyWindow(self.window) };
            }
        }
    }
}

#[test]
fn a_locked_in_engine_cannot_open_files_use_the_network_or_start_programs() {
    let document = kit::TempFile::new("selftest.pdf", &kit::fonts_pdf());
    let folder = std::env::temp_dir().join(format!("partridge-sandbox-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let keep = folder.join("keep-me");
    std::fs::write(&keep, b"still here").unwrap();
    // A folder of its own for the control, which really does what it is asked to.
    let control_folder = folder.join("control");
    std::fs::create_dir_all(&control_folder).unwrap();
    std::fs::write(control_folder.join("keep-me"), b"delete me").unwrap();
    let control_document = kit::TempFile::new("selftest-control.pdf", &kit::simple_pdf(1));

    // Something to connect to and something to send to. If the engine could connect or send,
    // these would see it.
    let (listener, udp_listener) = listeners();
    let port = listener.local_addr().unwrap().port();
    // Windows: an address of this computer that is not the loopback one, and a name that only the
    // network can resolve.
    let host = host_listener();
    let dns = dns_name();
    // Windows: the same on IPv6, an address on another computer, and a pipe that is open to all.
    let v6 = v6_listeners();
    let remote = remote_target(dns.as_deref());
    #[cfg(windows)]
    let pipe = pipes::start();

    // A secret in the environment of the app, which the engine must not get.
    std::env::set_var("PARTRIDGE_PLANTED_SECRET", "hunter2");

    let title = format!("Partridge test window {}", std::process::id());
    #[cfg(windows)]
    let mut session = session::start(&title);

    // `slow` adds the attempt that takes seconds when the network is open (the control skips it).
    let probe_args = |file: String, dir: &std::path::Path, slow: bool| -> Vec<String> {
        let mut args = vec![
            "probe".to_string(),
            file,
            dir.to_string_lossy().into_owned(),
            std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            std::process::id().to_string(),
            port.to_string(),
            title.clone(),
        ];
        if let Some((_, address)) = &host {
            args.push(format!("host={address}"));
        }
        if let Some(name) = &dns {
            args.push(format!("dns={name}"));
        }
        if let Some((_, _, v6port)) = &v6 {
            args.push(format!("v6port={v6port}"));
        }
        if let Some(address) = &remote {
            args.push(format!("remote={address}"));
        }
        #[cfg(windows)]
        args.push(format!("pipe={}", pipe.name));
        if slow {
            args.push("smb_remote=1".to_string());
        }
        args
    };

    // The control: the same attempts without the sandbox.
    let control =
        run_control(&probe_args(control_document.path_str(), &control_folder, false)[1..]);
    println!("control: {control:#?}");
    // The control reached every listener: a listener that the control cannot reach would make the
    // checks below pass for nothing.
    assert!(
        accepts_within(&listener, Duration::from_secs(5)),
        "the control did not connect to the TCP listener"
    );
    let mut datagram = [0u8; 16];
    udp_listener
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    // Windows: two datagrams, one from a socket that was bound and one from a socket that was
    // never bound (the system binds that one with the send). Elsewhere one.
    let mut arrived = Vec::new();
    for _ in 0..if cfg!(windows) { 2 } else { 1 } {
        let (length, _) = udp_listener
            .recv_from(&mut datagram)
            .expect("the control's datagram did not arrive");
        arrived.push(datagram[..length].to_vec());
    }
    arrived.sort();
    if cfg!(windows) {
        assert_eq!(arrived, [b"hello".to_vec(), b"hello-unbound".to_vec()]);
    } else {
        assert_eq!(arrived, [b"hello".to_vec()]);
    }
    if let Some((v6_tcp, v6_udp, _)) = &v6 {
        assert!(
            accepts_within(v6_tcp, Duration::from_secs(5)),
            "the control did not connect to the IPv6 listener"
        );
        v6_udp
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let (length, _) = v6_udp
            .recv_from(&mut datagram)
            .expect("the control's IPv6 datagram did not arrive");
        assert_eq!(&datagram[..length], b"hello-unbound");
        v6_udp.set_nonblocking(true).unwrap();
        while v6_tcp.accept().is_ok() {}
    }
    // Nothing else is expected from here on.
    udp_listener.set_nonblocking(true).unwrap();
    while listener.accept().is_ok() {}
    if let Some((host_listener, _)) = &host {
        assert!(
            accepts_within(host_listener, Duration::from_secs(5)),
            "the control did not connect to the address of this computer"
        );
        assert!(
            line(&control, "connect_host_address").contains("ALLOWED"),
            "{control:#?}"
        );
    }
    if dns.is_some() {
        assert!(
            line(&control, "dns_lookup").contains("ALLOWED"),
            "the control cannot resolve the name either: {control:#?}"
        );
    }
    #[cfg(windows)]
    session.refill();

    let handle = std::fs::File::open(document.path()).unwrap();
    let before = std::fs::read(document.path()).unwrap();
    let run = run_selftest(
        &handle,
        None,
        &probe_args(document.path_str(), &folder, true),
    );
    let lines = &run.lines;
    println!("{lines:#?}");

    assert_eq!(line(lines, "sandbox"), "sandbox: in force", "{lines:#?}");
    let layers = line(lines, "layers").to_string();
    assert!(layers.contains(expected_layer()), "{lines:#?}");
    for attempt in [
        "open_file_by_path",
        "open_own_executable",
        "list_directory",
        "create_file",
        "write_existing_file",
        "delete_file",
        "start_process",
        "touch_other_process",
    ] {
        let l = line(lines, attempt);
        assert!(
            l.starts_with(&format!("{attempt}: denied")),
            "{attempt}: {l}"
        );
        assert!(
            refused_with_permission_error(l),
            "{attempt} was refused, but not with a permission error: {l}"
        );
        // Without the sandbox the same attempt works (the control).
        let c = line(&control, attempt);
        assert!(
            c.starts_with(&format!("{attempt}: ALLOWED")),
            "{attempt} does not work in the control either: {c}"
        );
    }
    // The network: real attempts, and the error is the system's (on Windows the program starts
    // Winsock before the lock-in, so a refusal is not "the network support did not start"). A TCP
    // connection and a UDP datagram to listeners the control reached, a connection to an address
    // of this computer that is not the loopback one, and a name lookup.
    #[cfg(windows)]
    {
        // The AppContainer without capabilities is the layer that does this, and the engine
        // reports it only after the system has refused attempts of its own.
        assert!(layers.contains("appcontainer"), "{lines:#?}");
        assert!(layers.contains("no-network"), "{lines:#?}");
    }
    // `udp_bind_then_send` binds first, so where the bind is refused no datagram is sent: it shows
    // the bind. The `udp_sendto_unbound_*` attempts send from a socket that was never bound; their
    // refusal must come from `sendto` itself (a refusal of `socket` would mean no send was tried).
    let mut network_attempts = vec!["connect_socket"];
    if cfg!(windows) {
        network_attempts.extend([
            "udp_bind_then_send",
            "udp_sendto_unbound_loopback",
            "udp_sendto_unbound_remote",
        ]);
    } else {
        network_attempts.push("udp_bind_then_send");
    }
    if host.is_some() {
        network_attempts.push("connect_host_address");
    }
    if dns.is_some() {
        network_attempts.push("dns_lookup");
    }
    if v6.is_some() {
        network_attempts.extend(["connect_v6_loopback", "udp_sendto_unbound_v6"]);
    }
    if remote.is_some() {
        network_attempts.push("connect_remote");
    }
    for attempt in network_attempts {
        let l = line(lines, attempt);
        println!("network: {l}");
        let c = line(&control, attempt);
        assert!(
            c.starts_with(&format!("{attempt}: ALLOWED")),
            "{attempt} does not work in the control either: {c}"
        );
        assert!(l.starts_with(&format!("{attempt}: denied")), "{l}");
        assert!(!l.contains("did not start"), "{l}");
        // Windows: the answer of a network that is closed by policy, not another failure.
        if cfg!(windows) && attempt != "dns_lookup" {
            assert!(l.contains("os error 10013"), "{l}");
        }
        if attempt.starts_with("udp_sendto_unbound") {
            assert!(l.contains("sendto:"), "no datagram was sent: {l}");
        }
    }
    // Windows: a raw socket and an ICMP echo. An account without administrator rights cannot make
    // a raw socket in the control either, and a computer that does not answer an echo gives no
    // reply to the control: then the line is printed and the attempt proves nothing about the
    // engine, which the log shows.
    #[cfg(windows)]
    for attempt in ["raw_socket", "icmp_echo_loopback"] {
        let l = line(lines, attempt);
        let c = line(&control, attempt);
        println!("network: {l} (control: {c})");
        assert!(l.starts_with(&format!("{attempt}: denied")), "{l}");
        if attempt == "icmp_echo_loopback" && c.contains("ALLOWED") {
            assert!(!l.contains("ALLOWED"), "{l}");
        }
    }
    #[cfg(windows)]
    if remote.is_some() {
        let l = line(lines, "icmp_echo_remote");
        println!(
            "network: {l} (control: {})",
            line(&control, "icmp_echo_remote")
        );
        assert!(l.starts_with("icmp_echo_remote: denied"), "{l}");
    }
    #[cfg(windows)]
    {
        let l = line(lines, "open_local_pipe");
        let c = line(&control, "open_local_pipe");
        println!("network: {l} (control: {c})");
        assert!(l.starts_with("open_local_pipe: denied"), "{l}");
        assert!(l.contains("os error 5)"), "{l}");
        assert!(
            c.starts_with("open_local_pipe: ALLOWED"),
            "the control cannot open the pipe either: {c}"
        );
    }
    // Binding a socket is not reaching the network: the systems differ on whether it is refused.
    let bind = line(&control, "bind_udp_socket");
    assert!(bind.starts_with("bind_udp_socket: ALLOWED"), "{bind}");
    println!("network: {}", line(lines, "bind_udp_socket"));
    if !cfg!(windows) {
        assert!(
            line(lines, "bind_udp_socket").starts_with("bind_udp_socket: denied"),
            "{}",
            line(lines, "bind_udp_socket")
        );
    }
    // The system call filter on Linux and the dynamic code policy on Windows stop code from being
    // made at run time. (macOS refuses it on this hardware by its own rule, not by the profile.)
    if !cfg!(target_os = "macos") {
        assert!(
            line(lines, "executable_memory").starts_with("executable_memory: denied"),
            "{}",
            line(lines, "executable_memory")
        );
    }

    // The environment: nothing of the app's, in particular not the planted secret.
    let names = line(lines, "environment")
        .trim_start_matches("environment:")
        .trim()
        .to_string();
    assert!(!names.contains("PARTRIDGE_PLANTED_SECRET"), "{names}");
    for name in names.split(',').filter(|n| !n.is_empty()) {
        assert!(
            name.starts_with("PARTRIDGE_TEST_")
                // The system puts this one in every process on macOS.
                || name == "__CF_USER_TEXT_ENCODING"
                || name.eq_ignore_ascii_case("SystemRoot")
                || name.eq_ignore_ascii_case("windir"),
            "the engine got the variable {name}"
        );
    }
    assert!(
        line(&control, "environment").contains("PARTRIDGE_PLANTED_SECRET"),
        "the control should have the planted variable"
    );

    // Linux: the ways round the filter that a review found.
    #[cfg(target_os = "linux")]
    {
        for attempt in [
            "fcntl_setown",
            "fcntl_setsig",
            "fcntl_setfl_async",
            "shared_anonymous_mapping",
        ] {
            let l = line(lines, attempt);
            assert!(l.starts_with(&format!("{attempt}: denied")), "{l}");
            let c = line(&control, attempt);
            assert!(
                c.starts_with(&format!("{attempt}: ALLOWED")),
                "{attempt} does not work in the control either: {c}"
            );
        }
        assert_eq!(line(lines, "fcntl_getfd_getfl"), "fcntl_getfd_getfl: yes");
        // A path lookup in an empty root finds nothing; without one (user namespaces switched
        // off) the metadata of any path can be asked for, which the README says.
        let found = line(lines, "stat_system_file").contains("FOUND");
        if layers.contains("empty-root") {
            assert!(!found, "{lines:#?}");
            assert!(line(lines, "stat_document_path").contains("not found"));
        } else {
            println!("no empty root here: {}", line(lines, "isolation"));
        }
        assert!(
            line(&control, "stat_system_file").contains("FOUND"),
            "the control cannot see /etc/passwd either"
        );
        if std::env::var_os("PARTRIDGE_TEST_REQUIRE_EMPTY_ROOT").is_some() {
            assert!(layers.contains("empty-root"), "{lines:#?}");
        }
    }

    // Windows: the desktop, the other windows, the clipboard and the network below Winsock.
    #[cfg(windows)]
    {
        assert!(layers.contains("own-desktop"), "{lines:#?}");
        assert!(
            line(lines, "desktop").starts_with("desktop: PartridgeEngine"),
            "{}",
            line(lines, "desktop")
        );
        println!(
            "afd: {} (control: {})",
            line(lines, "afd_endpoint"),
            line(&control, "afd_endpoint")
        );
        let l = line(lines, "open_network_path");
        assert!(l.starts_with("open_network_path: denied"), "{l}");
        assert!(l.contains("os error 5)"), "{l}");
        println!("control: {}", line(&control, "open_network_path"));
        // A path on a computer that does not exist: the refusal comes at once, so no connection
        // was tried (trying one takes seconds).
        let l = line(lines, "open_network_path_remote");
        println!("network: {l}");
        assert!(l.starts_with("open_network_path_remote: denied"), "{l}");
        assert!(l.contains("os error 5)"), "{l}");
        let took: u64 = l
            .rsplit("after ")
            .next()
            .and_then(|t| t.trim_end_matches(" ms)").parse().ok())
            .unwrap_or_else(|| panic!("no time in {l}"));
        assert!(took < 1500, "the refusal took {took} ms: {l}");
        if session.has_window() {
            assert!(line(lines, "find_window_of_the_user").contains("denied"));
            assert!(
                line(&control, "find_window_of_the_user").contains("ALLOWED"),
                "the control cannot see the window either"
            );
        }
        if session.clipboard_set {
            assert!(line(lines, "clipboard_read").contains("denied"));
            assert!(
                line(&control, "clipboard_read").contains("ALLOWED"),
                "the control cannot read the clipboard either"
            );
        }
        println!(
            "clipboard: clear {} / write {} (window made: {})",
            line(lines, "clipboard_clear"),
            line(lines, "clipboard_write"),
            line(lines, "window_created")
        );
        assert!(line(lines, "clipboard_write").contains("denied"));
        assert!(
            line(&control, "clipboard_write").contains("ALLOWED"),
            "the control cannot write the clipboard either: {}",
            line(&control, "clipboard_write")
        );
    }

    assert!(lines.last().is_some_and(|l| l == "done"), "{lines:#?}");
    assert!(run.succeeded(), "the program ended with {}", run.exit);

    // And nothing happened to the world outside.
    assert!(!folder.join("created-by-the-engine").exists());
    assert_eq!(std::fs::read(&keep).unwrap(), b"still here");
    assert_eq!(std::fs::read(document.path()).unwrap(), before);
    // Nothing reached a listener from the engine.
    assert!(
        listener.accept().is_err(),
        "something connected to the listener"
    );
    assert!(
        udp_listener.recv_from(&mut datagram).is_err(),
        "a datagram arrived from the engine"
    );
    if let Some((host_listener, _)) = &host {
        assert!(
            host_listener.accept().is_err(),
            "something connected to the address of this computer"
        );
    }
    if let Some((v6_tcp, v6_udp, _)) = &v6 {
        assert!(
            v6_tcp.accept().is_err(),
            "something connected to the IPv6 listener"
        );
        assert!(
            v6_udp.recv_from(&mut datagram).is_err(),
            "a datagram arrived on IPv6 from the engine"
        );
    }
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn a_locked_in_engine_still_reads_draws_and_searches_the_document_it_was_given() {
    let document = kit::TempFile::new("selftest-doc.pdf", &kit::simple_pdf(2));
    let handle = std::fs::File::open(document.path()).unwrap();
    let run = run_selftest(&handle, None, &["document".to_string()]);
    let lines = &run.lines;
    println!("{lines:#?}");
    assert_eq!(line(lines, "sandbox"), "sandbox: in force", "{lines:#?}");
    assert_eq!(line(lines, "open"), "open: ok, 2 pages");
    assert!(
        line(lines, "render").starts_with("render: ok"),
        "{}",
        line(lines, "render")
    );
    assert_eq!(line(lines, "search"), "search: 30 matches");
    assert!(run.succeeded(), "the program ended with {}", run.exit);
}

/// Runs the program that takes three times the limit and then holds on to it (see `memory` in
/// `engine_selftest`). It is not given the chance to finish first: a limit that is a watch looking
/// every few tens of milliseconds cannot see a program that has taken its memory and ended within
/// one look, and what it promises is that a program that stays over the limit is ended.
fn run_memory_hog(extra: &[&str]) -> (Run, Duration) {
    let document = kit::TempFile::new("selftest-mem.pdf", &kit::simple_pdf(1));
    let handle = std::fs::File::open(document.path()).unwrap();
    // A small limit, so the test does not need gigabytes: the program tries three times as much,
    // and would hold it for 30 seconds.
    let mut mode = vec!["memory".to_string(), "256".to_string(), "30".to_string()];
    mode.extend(extra.iter().map(|s| s.to_string()));
    let started = std::time::Instant::now();
    let run = run_selftest(&handle, Some(256 * 1024 * 1024), &mode);
    let took = started.elapsed();
    println!("{:#?} {} after {took:?}", run.lines, run.exit);
    (run, took)
}

fn assert_ended_by_the_limit(run: &Run, took: Duration) {
    let lines = &run.lines;
    assert!(
        !lines.iter().any(|l| l.contains("SURVIVED")),
        "the limit did not hold: {lines:#?}"
    );
    assert!(!lines.iter().any(|l| l == "done"), "{lines:#?}");
    // It was ended: by a signal or an abort, not by finishing.
    assert!(!run.succeeded(), "the program ended with {}", run.exit);
    // And well before it would have let go of the memory by itself (it holds it for 30 seconds).
    assert!(
        took < Duration::from_secs(20),
        "it took {took:?} to end the program"
    );
}

#[test]
fn the_memory_limit_ends_an_engine_that_uses_too_much() {
    // Whatever enforces the limit on this system: the kernel (Windows, Linux) or the watches on
    // macOS.
    let (run, took) = run_memory_hog(&[]);
    assert_ended_by_the_limit(&run, took);
}

/// macOS: the engine's own watch is switched off, so only the app's watch from outside can end the
/// program. This is the limit that holds: code that has taken the engine over can stop a thread of
/// the engine, but not a signal sent from another process.
#[cfg(target_os = "macos")]
#[test]
fn the_app_ends_an_engine_that_stays_over_the_memory_limit_without_any_help_from_it() {
    let (run, took) = run_memory_hog(&["app-only"]);
    assert_ended_by_the_limit(&run, took);
    // The app's watch ends it with SIGKILL; the engine's own would have exited with status 137.
    assert!(
        run.exit.contains("signal: 9"),
        "the program was not killed from outside: {}",
        run.exit
    );
}

// ---- The real engine ----

fn real_config() -> EngineConfig {
    let mut config = EngineConfig::new(env!("CARGO_BIN_EXE_partridge-engine"));
    config.args = vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
    // The tests insist on the sandbox, so a system that cannot provide one fails here.
    config.sandbox = SandboxPolicy::Required;
    config
}

fn dark_pixels(bitmap: &partridge_engine_client::Bitmap, y0: u32, y1: u32) -> usize {
    let y1 = y1.min(bitmap.height);
    let row = (bitmap.width * 4) as usize;
    bitmap.rgba[y0 as usize * row..y1 as usize * row]
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] < 110 && p[1] < 110 && p[2] < 110)
        .count()
}

#[test]
fn the_real_engine_reports_the_sandbox_it_runs_in() {
    let client = EngineClient::new(real_config());
    let info = client.info().unwrap();
    println!("sandbox: {}", info.sandbox);
    assert!(info.sandbox.contains(expected_layer()), "{info:?}");
    // Where the kernel lists Landlock among its security modules, the engine must use it.
    if cfg!(target_os = "linux") && std::env::var_os("PARTRIDGE_TEST_NO_LANDLOCK").is_none() {
        let lsm = std::fs::read_to_string("/sys/kernel/security/lsm").unwrap_or_default();
        if lsm.split(',').any(|m| m.trim() == "landlock") {
            assert!(info.sandbox.contains("landlock"), "{info:?} with lsm {lsm}");
        }
    }
    if cfg!(target_os = "windows") {
        for layer in ["low-integrity", "job", "mitigations"] {
            assert!(info.sandbox.contains(layer), "{info:?}");
        }
    }
}

#[test]
fn pages_without_embedded_fonts_still_show_their_text_inside_the_sandbox() {
    let file = kit::TempFile::new("fonts.pdf", &kit::fonts_pdf());
    let client = EngineClient::new(real_config());
    let doc = client.open(&file.path_str(), None).unwrap();
    let bitmap = client.render(doc.doc, 0, 612, 792).unwrap();
    for (i, (name, _)) in kit::FONT_LINES.iter().enumerate() {
        // The band of the line: from a little above the capitals to a little below the descenders.
        let baseline = 792.0 - kit::font_line_baseline(i);
        let ink = dark_pixels(&bitmap, (baseline - 18.0) as u32, (baseline + 6.0) as u32);
        // About 35 characters of 20 points leave thousands of dark pixels; 300 is far below any
        // real drawing and far above an empty band.
        assert!(ink > 300, "{name} drew only {ink} dark pixels");
    }
    // The text is there to read and search as well.
    let text = client.page_text(doc.doc, 0).unwrap();
    assert_eq!(
        text.text.matches(kit::FONT_LINE_TEXT).count(),
        kit::FONT_LINES.len(),
        "{}",
        text.text
    );
    let found = client.search_page(doc.doc, 0, "quartz").unwrap();
    assert_eq!(found.len(), kit::FONT_LINES.len());
}

#[test]
fn text_in_a_script_that_needs_a_missing_font_does_not_hurt_the_engine() {
    // Japanese in a font that is not embedded: there is no font for it in the sandbox, so it shows
    // as blanks. Everything else on the page and the engine itself must be fine.
    let file = kit::TempFile::new("cjk.pdf", &kit::cjk_pdf());
    let client = EngineClient::new(real_config());
    let doc = client.open(&file.path_str(), None).unwrap();
    let bitmap = client.render(doc.doc, 0, 612, 792).unwrap();
    // The Latin line is drawn.
    assert!(dark_pixels(&bitmap, 792 - 660, 792 - 635) > 150);
    assert!(client.page_text(doc.doc, 0).is_ok());
    assert!(client.search_page(doc.doc, 0, "Latin").is_ok());
    assert_eq!(client.spawn_count(), 1);
}

/// Sends raw requests to an engine started by hand and returns its answers.
fn talk(requests: Vec<RequestBody>, document: Option<&std::fs::File>) -> Vec<Response> {
    // Run in a thread with a time limit: an engine that hangs must fail the test, not hold it.
    let document = document.map(|d| d.try_clone().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(talk_unbounded(requests, document.as_ref()));
    });
    rx.recv_timeout(Duration::from_secs(90))
        .expect("the engine did not answer within 90 seconds")
}

fn talk_unbounded(requests: Vec<RequestBody>, document: Option<&std::fs::File>) -> Vec<Response> {
    let config = real_config();
    let mut engine = spawn_engine(&SpawnRequest {
        program: &config.program,
        args: &config.args,
        document,
        memory_limit: None,
        policy: SandboxPolicy::Required,
        fonts: &[],
    })
    .unwrap();
    let mut answers = Vec::new();
    for (i, body) in requests.into_iter().enumerate() {
        write_request(
            &mut engine.stdin,
            &Request {
                id: i as u32 + 1,
                body,
            },
        )
        .unwrap();
        engine.stdin.flush().unwrap();
        let frame = read_frame(&mut engine.stdout).unwrap().unwrap();
        answers.push(Response::decode(frame).unwrap());
    }
    drop(engine.stdin);
    let _ = engine.process.wait();
    answers
}

fn error_code(answer: &Response) -> ErrorCode {
    match &answer.body {
        partridge_protocol::ResponseBody::Error { code, .. } => *code,
        other => panic!("expected an error, got {other:?}"),
    }
}

#[test]
fn an_engine_without_a_document_opens_nothing_and_one_document_opens_once() {
    let hello = RequestBody::Hello {
        protocol_version: partridge_protocol::PROTOCOL_VERSION,
    };
    // No document handed over: there is nothing to open, and the engine cannot go and look for one.
    let answers = talk(
        vec![hello.clone(), RequestBody::Open { password: None }],
        None,
    );
    assert_eq!(error_code(&answers[1]), ErrorCode::InvalidRequest);

    // One document, opened once.
    let file = kit::TempFile::new("once.pdf", &kit::simple_pdf(1));
    let handle = std::fs::File::open(file.path()).unwrap();
    let answers = talk(
        vec![
            hello,
            RequestBody::Open { password: None },
            RequestBody::Open { password: None },
        ],
        Some(&handle),
    );
    assert!(matches!(
        answers[1].body,
        partridge_protocol::ResponseBody::Opened { .. }
    ));
    assert_eq!(error_code(&answers[2]), ErrorCode::InvalidRequest);
}

#[test]
fn a_client_with_a_missing_engine_program_reports_it() {
    let mut config = real_config();
    config.program = PathBuf::from("/definitely/not/here/partridge-engine");
    let client = EngineClient::new(config);
    let file = kit::TempFile::new("x.pdf", &kit::simple_pdf(1));
    let err = client.open(&file.path_str(), None).unwrap_err();
    assert!(matches!(err, ClientError::EngineStopped(_)), "{err:?}");
}

/// A laptop that goes to sleep stops every process the same way: the kernel freezes them with a
/// signal, and a sleep that was cut short is restarted with `restart_syscall`. A filter that does
/// not allow that call kills the helper thread of the engine, and with it the engine.
#[cfg(unix)]
#[test]
fn an_engine_that_is_stopped_and_continued_keeps_working() {
    let file = kit::TempFile::new("stop.pdf", &kit::simple_pdf(2));
    let client = EngineClient::new(real_config());
    let doc = client.open(&file.path_str(), None).unwrap();
    let pid = client.pid().unwrap() as libc::pid_t;
    for _ in 0..3 {
        // SAFETY: signals to the engine process of this test.
        unsafe { libc::kill(pid, libc::SIGSTOP) };
        // Longer than the helper thread sleeps between two looks at its parent (500 ms).
        std::thread::sleep(Duration::from_millis(1300));
        // SAFETY: as above.
        unsafe { libc::kill(pid, libc::SIGCONT) };
        std::thread::sleep(Duration::from_millis(1300));
        let bitmap = client.render(doc.doc, 1, 120, 156).unwrap();
        assert_eq!((bitmap.width, bitmap.height), (120, 156));
    }
    assert_eq!(client.spawn_count(), 1, "the engine had to be restarted");
}
