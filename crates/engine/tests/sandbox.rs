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
use std::net::TcpListener;
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

    // Something to connect to. If the engine could connect, this would see it.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();

    // A secret in the environment of the app, which the engine must not get.
    std::env::set_var("PARTRIDGE_PLANTED_SECRET", "hunter2");

    let title = format!("Partridge test window {}", std::process::id());
    #[cfg(windows)]
    let mut session = session::start(&title);

    let probe_args = |file: String, dir: &std::path::Path| -> Vec<String> {
        vec![
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
        ]
    };

    // The control: the same attempts without the sandbox.
    let control = run_control(&probe_args(control_document.path_str(), &control_folder)[1..]);
    println!("control: {control:#?}");
    // The control connected to the listener; that is not what the engine is judged by.
    while listener.accept().is_ok() {}
    #[cfg(windows)]
    session.refill();

    let handle = std::fs::File::open(document.path()).unwrap();
    let before = std::fs::read(document.path()).unwrap();
    let run = run_selftest(&handle, None, &probe_args(document.path_str(), &folder));
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
    // The network: real attempts. Not "the network support did not start": on Windows the
    // program starts Winsock before the lock-in, so the refusal is the system's.
    // On Windows the restricted token does not stop the network (found by this very check, see
    // the README): the engine does not report the layer `no-network` there, and what the attempts
    // got is only printed. Everywhere else they must be refused.
    let network_must_be_blocked = !cfg!(windows) || layers.contains("no-network");
    for attempt in ["connect_socket", "bind_udp_socket"] {
        let l = line(lines, attempt);
        println!("network: {l}");
        let c = line(&control, attempt);
        assert!(
            c.starts_with(&format!("{attempt}: ALLOWED")),
            "{attempt} does not work in the control either: {c}"
        );
        if network_must_be_blocked {
            assert!(l.starts_with(&format!("{attempt}: denied")), "{l}");
            assert!(!l.contains("did not start"), "{l}");
        }
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
        if layers.contains("no-network") {
            assert_eq!(
                line(lines, "afd_endpoint"),
                "afd_endpoint: denied (NTSTATUS 0xC0000022)"
            );
        }
        let l = line(lines, "open_network_path");
        assert!(l.starts_with("open_network_path: denied"), "{l}");
        println!("control: {}", line(&control, "open_network_path"));
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
        assert!(line(lines, "clipboard_write").contains("denied"));
    }

    assert!(lines.last().is_some_and(|l| l == "done"), "{lines:#?}");
    assert!(run.succeeded(), "the program ended with {}", run.exit);

    // And nothing happened to the world outside.
    assert!(!folder.join("created-by-the-engine").exists());
    assert_eq!(std::fs::read(&keep).unwrap(), b"still here");
    assert_eq!(std::fs::read(document.path()).unwrap(), before);
    assert!(
        listener.accept().is_err(),
        "something connected to the listener"
    );
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

#[test]
fn the_memory_limit_ends_an_engine_that_uses_too_much() {
    // On macOS the engine's own watch is switched off for this test: the app has to end the
    // engine from outside, which an engine that has been taken over cannot prevent.
    std::env::set_var("PARTRIDGE_TEST_NO_ENGINE_LIMIT", "1");
    let document = kit::TempFile::new("selftest-mem.pdf", &kit::simple_pdf(1));
    let handle = std::fs::File::open(document.path()).unwrap();
    // A small limit, so the test does not need gigabytes: the program tries three times as much.
    let run = run_selftest(
        &handle,
        Some(256 * 1024 * 1024),
        &["memory".to_string(), "256".to_string()],
    );
    let lines = &run.lines;
    println!("{lines:#?} {}", run.exit);
    assert!(
        !lines.iter().any(|l| l.contains("SURVIVED")),
        "the limit did not hold: {lines:#?}"
    );
    assert!(!lines.iter().any(|l| l == "done"), "{lines:#?}");
    // It was ended: by a signal or an abort, not by finishing.
    assert!(!run.succeeded(), "the program ended with {}", run.exit);
    std::env::remove_var("PARTRIDGE_TEST_NO_ENGINE_LIMIT");
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
