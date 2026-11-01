//! The proof that the engine is sandboxed, on every system the tests run on.
//!
//! `engine_selftest` (an example program, built by `cargo test` and not part of any release) starts
//! the way the engine starts, locks itself in the same way, and then tries the things the sandbox
//! must stop. It is started through the same code the app uses to start the engine, so on Windows
//! it gets the restricted token, the job object and the mitigation policies, and on every system
//! it gets the document as an inherited handle.
//!
//! The second half runs the real engine: it must report its sandbox, draw pages inside it
//! (including a page with fonts that are not embedded), search and read text, and refuse to work
//! when it has no document.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::{Duration, Instant};

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

/// Runs `engine_selftest` with the document handed over, and returns what it printed and whether
/// it ended well.
fn run_selftest(
    document: &std::fs::File,
    memory_limit: Option<u64>,
    mode_args: &[String],
) -> (Vec<String>, bool) {
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
    let mut output = String::new();
    let mut raw = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    engine.stdout.read_to_end(&mut raw).ok();
    output.push_str(&String::from_utf8_lossy(&raw));
    assert!(Instant::now() < deadline, "the program took too long");
    let status = engine.process.wait();
    let ended_well = status.is_ok() && engine.process.try_wait().ok().flatten().is_some();
    (output.lines().map(str::to_string).collect(), ended_well)
}

fn line<'a>(lines: &'a [String], name: &str) -> &'a str {
    lines
        .iter()
        .find(|l| l.starts_with(&format!("{name}:")))
        .unwrap_or_else(|| panic!("no line for {name} in {lines:#?}"))
}

#[test]
fn a_locked_in_engine_cannot_open_files_use_the_network_or_start_programs() {
    let document = kit::TempFile::new("selftest.pdf", &kit::fonts_pdf());
    let folder = std::env::temp_dir().join(format!("partridge-sandbox-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let keep = folder.join("keep-me");
    std::fs::write(&keep, b"still here").unwrap();

    // Something to connect to. If the engine could connect, this would see it.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();

    let handle = std::fs::File::open(document.path()).unwrap();
    let before = std::fs::read(document.path()).unwrap();
    let (lines, ended_well) = run_selftest(
        &handle,
        None,
        &[
            "probe".to_string(),
            document.path_str(),
            folder.to_string_lossy().into_owned(),
            std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            std::process::id().to_string(),
            port.to_string(),
        ],
    );
    println!("{lines:#?}");

    assert_eq!(line(&lines, "sandbox"), "sandbox: in force", "{lines:#?}");
    assert!(
        line(&lines, "layers").contains(expected_layer()),
        "{lines:#?}"
    );
    for attempt in [
        "open_file_by_path",
        "open_own_executable",
        "list_directory",
        "create_file",
        "write_existing_file",
        "delete_file",
        "connect_socket",
        "bind_udp_socket",
        "start_process",
        "touch_other_process",
    ] {
        assert!(
            line(&lines, attempt).starts_with(&format!("{attempt}: denied")),
            "{attempt} was not denied: {}",
            line(&lines, attempt)
        );
    }
    // The system call filter on Linux and the dynamic code policy on Windows stop code from being
    // made at run time. (The macOS profile does not govern memory.)
    if !cfg!(target_os = "macos") {
        assert!(
            line(&lines, "executable_memory").starts_with("executable_memory: denied"),
            "{}",
            line(&lines, "executable_memory")
        );
    }
    assert!(lines.last().is_some_and(|l| l == "done"), "{lines:#?}");
    assert!(ended_well);

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
    let (lines, _) = run_selftest(&handle, None, &["document".to_string()]);
    println!("{lines:#?}");
    assert_eq!(line(&lines, "sandbox"), "sandbox: in force", "{lines:#?}");
    assert_eq!(line(&lines, "open"), "open: ok, 2 pages");
    assert!(
        line(&lines, "render").starts_with("render: ok"),
        "{}",
        line(&lines, "render")
    );
    assert_eq!(line(&lines, "search"), "search: 30 matches");
}

#[test]
fn the_memory_limit_ends_an_engine_that_uses_too_much() {
    let document = kit::TempFile::new("selftest-mem.pdf", &kit::simple_pdf(1));
    let handle = std::fs::File::open(document.path()).unwrap();
    // A small limit, so the test does not need gigabytes: the program tries three times as much.
    let (lines, _) = run_selftest(
        &handle,
        Some(256 * 1024 * 1024),
        &["memory".to_string(), "256".to_string()],
    );
    println!("{lines:#?}");
    assert!(
        !lines.iter().any(|l| l.contains("SURVIVED")),
        "the limit did not hold: {lines:#?}"
    );
    assert!(!lines.iter().any(|l| l == "done"), "{lines:#?}");
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
