//! The pool of engines: one engine process per open document. Runs the real engine and the real
//! PDFium library (see `engine.rs` for how to get it).

use partridge_engine_client::{ClientError, EngineConfig, EnginePool};
use partridge_protocol::ErrorCode;
use partridge_testkit as kit;

fn pool(max: usize) -> EnginePool {
    let mut config = EngineConfig::new(env!("CARGO_BIN_EXE_partridge-engine"));
    config.args = vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
    // The tests insist on the sandbox: an engine that cannot lock itself in fails them.
    config.sandbox = partridge_engine_client::SandboxPolicy::Required;
    EnginePool::new(config, max)
}

fn code_of(err: ClientError) -> ErrorCode {
    match err {
        ClientError::Engine { code, .. } => code,
        other => panic!("expected an engine error, got {other:?}"),
    }
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn documents_get_their_own_handles_that_are_never_reused() {
    let a = kit::TempFile::new("a.pdf", &kit::simple_pdf(2));
    let pool = pool(8);
    let (first, pages) = pool.open(&a.path_str(), None).unwrap();
    assert_eq!(pages.len(), 2);
    let (second, _) = pool.open(&a.path_str(), None).unwrap();
    assert_ne!(first, second);
    assert_eq!(pool.len(), 2);

    pool.close(first);
    assert_eq!(pool.len(), 1);
    let (third, _) = pool.open(&a.path_str(), None).unwrap();
    assert!(third > second, "{third} should come after {second}");

    // The closed handle is gone for good, and closing it again does nothing.
    assert_eq!(
        code_of(pool.render(first, 0, 50, 60).unwrap_err()),
        ErrorCode::UnknownDocument
    );
    pool.close(first);
    assert!(pool.render(second, 1, 50, 60).is_ok());
    assert!(pool.render(third, 0, 50, 60).is_ok());
}

#[cfg(unix)]
#[test]
fn every_document_has_its_own_engine_process_and_closing_ends_it() {
    let a = kit::TempFile::new("a.pdf", &kit::simple_pdf(1));
    let pool = pool(8);
    let (one, _) = pool.open(&a.path_str(), None).unwrap();
    let (two, _) = pool.open(&a.path_str(), None).unwrap();
    let (p1, p2) = (pool.pid(one).unwrap(), pool.pid(two).unwrap());
    assert_ne!(p1, p2);
    assert!(alive(p1) && alive(p2));
    pool.close(one);
    assert!(!alive(p1), "the engine of a closed document must end");
    assert!(alive(p2));
    pool.kill_all();
    assert!(!alive(p2));
    assert!(pool.is_empty());
}

#[cfg(unix)]
#[test]
fn a_crashed_engine_costs_only_its_own_document() {
    let a = kit::TempFile::new("a.pdf", &kit::simple_pdf(2));
    let pool = pool(8);
    let (one, _) = pool.open(&a.path_str(), None).unwrap();
    let (two, _) = pool.open(&a.path_str(), None).unwrap();
    // Kill the engine of the first document the hard way, as a crash would.
    let pid = pool.pid(one).unwrap();
    let status = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    // The first document fails, as a document of a dead engine does...
    let err = pool.render(one, 0, 50, 60).unwrap_err();
    assert!(
        matches!(err, ClientError::EngineStopped(_)),
        "unexpected {err:?}"
    );
    // ...and the second one does not notice.
    assert!(pool.render(two, 1, 50, 60).is_ok());
    assert!(pool.page_text(two, 0).is_ok());
    // The dead document stays dead: its handle does not reach anything else.
    assert!(pool.render(one, 0, 50, 60).is_err());
}

#[test]
fn the_number_of_open_documents_is_limited() {
    let a = kit::TempFile::new("a.pdf", &kit::simple_pdf(1));
    let pool = pool(2);
    let (one, _) = pool.open(&a.path_str(), None).unwrap();
    let _two = pool.open(&a.path_str(), None).unwrap();
    assert_eq!(
        code_of(pool.open(&a.path_str(), None).unwrap_err()),
        ErrorCode::InvalidRequest
    );
    assert_eq!(pool.len(), 2);
    pool.close(one);
    assert!(pool.open(&a.path_str(), None).is_ok());
}

#[test]
fn a_file_that_does_not_open_leaves_nothing_behind() {
    let bad = kit::TempFile::new("bad.pdf", &kit::corrupt_pdf());
    let secret = kit::TempFile::new("secret.pdf", &kit::encrypted_pdf("pw", 1));
    let pool = pool(2);
    assert_eq!(
        code_of(pool.open(&bad.path_str(), None).unwrap_err()),
        ErrorCode::Corrupt
    );
    assert_eq!(
        code_of(pool.open(&secret.path_str(), None).unwrap_err()),
        ErrorCode::PasswordRequired
    );
    assert!(pool.is_empty());
    // And the password works on the next try.
    let (handle, _) = pool.open(&secret.path_str(), Some("pw")).unwrap();
    assert_eq!(pool.len(), 1);
    assert!(pool.render(handle, 0, 30, 40).is_ok());
}

#[test]
fn text_requests_go_to_the_right_document() {
    let features = kit::TempFile::new("f.pdf", &kit::features_pdf());
    let plain = kit::TempFile::new("p.pdf", &kit::simple_pdf(1));
    let pool = pool(4);
    let (f, _) = pool.open(&features.path_str(), None).unwrap();
    let (p, _) = pool.open(&plain.path_str(), None).unwrap();
    assert_eq!(pool.outline(f).unwrap().len(), 4);
    assert!(pool.outline(p).unwrap().is_empty());
    assert!(pool.page_text(f, 0).unwrap().text.contains("Partridge"));
    assert_eq!(pool.search_page(p, 0, "lazy dog").unwrap().len(), 30);
    assert_eq!(pool.page_links(f, 0).unwrap().len(), 3);
    assert_eq!(
        code_of(pool.page_links(9999, 0).unwrap_err()),
        ErrorCode::UnknownDocument
    );
}

#[cfg(unix)]
#[test]
fn letting_go_of_every_document_frees_the_pool_for_new_ones() {
    // A reload of the window loses the window's list of documents, while the engines go on. The
    // app then calls `kill_all`; after that all the room is there again.
    let a = kit::TempFile::new("a.pdf", &kit::simple_pdf(1));
    let pool = pool(2);
    let (one, _) = pool.open(&a.path_str(), None).unwrap();
    let (two, _) = pool.open(&a.path_str(), None).unwrap();
    let pids = [pool.pid(one).unwrap(), pool.pid(two).unwrap()];
    assert_eq!(
        code_of(pool.open(&a.path_str(), None).unwrap_err()),
        ErrorCode::InvalidRequest
    );

    pool.kill_all();
    assert!(pool.is_empty());
    assert!(pids.iter().all(|pid| !alive(*pid)));
    // The old handles are dead for good, and new documents get new handles.
    assert_eq!(
        code_of(pool.render(one, 0, 30, 40).unwrap_err()),
        ErrorCode::UnknownDocument
    );
    let (three, _) = pool.open(&a.path_str(), None).unwrap();
    let (four, _) = pool.open(&a.path_str(), None).unwrap();
    assert!(three > two && four > three);
    assert!(pool.render(three, 0, 30, 40).is_ok());
}
