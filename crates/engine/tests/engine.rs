//! Tests that run the real engine process with the real PDFium library, through the same client
//! the app uses. Needs the PDFium library: run `node scripts/fetch-pdfium.mjs` first, or set
//! `PARTRIDGE_PDFIUM_DIR`.

use std::time::Duration;

use partridge_engine_client::{ClientError, EngineClient, EngineConfig};
use partridge_protocol::ErrorCode;
use partridge_testkit as kit;

fn client() -> EngineClient {
    let mut config = EngineConfig::new(env!("CARGO_BIN_EXE_partridge-engine"));
    config.args = vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
    // The tests insist on the sandbox: an engine that cannot lock itself in fails them.
    config.sandbox = partridge_engine_client::SandboxPolicy::Required;
    EngineClient::new(config)
}

fn code_of(err: ClientError) -> ErrorCode {
    match err {
        ClientError::Engine { code, .. } => code,
        other => panic!("expected an engine error, got {other:?}"),
    }
}

fn message_of(err: &ClientError) -> String {
    err.to_string()
}

fn pixel(bitmap: &partridge_engine_client::Bitmap, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * bitmap.width + x) * 4) as usize;
    [
        bitmap.rgba[i],
        bitmap.rgba[i + 1],
        bitmap.rgba[i + 2],
        bitmap.rgba[i + 3],
    ]
}

#[test]
fn the_engine_greets_with_its_versions() {
    let client = client();
    let info = client.info().unwrap();
    assert_eq!(info.engine_version, env!("CARGO_PKG_VERSION"));
    assert!(info.pdfium_version.starts_with("PDFium"), "{info:?}");
}

#[test]
fn a_missing_pdfium_library_is_a_clear_error() {
    let mut config = EngineConfig::new(env!("CARGO_BIN_EXE_partridge-engine"));
    config.args = vec!["--pdfium-dir".into(), "/no/such/folder".into()];
    let client = EngineClient::new(config);
    let err = client.info().unwrap_err();
    assert_eq!(code_of(err), ErrorCode::EngineUnavailable);
}

#[test]
fn opens_a_document_and_lists_its_pages() {
    let file = kit::TempFile::new("three.pdf", &kit::simple_pdf(3));
    let client = client();
    let doc = client.open(&file.path_str(), None).unwrap();
    assert_eq!(doc.pages.len(), 3);
    for page in &doc.pages {
        assert_eq!((page.width, page.height), (612.0, 792.0));
    }
}

#[test]
fn pages_of_different_sizes_are_reported() {
    let file = kit::TempFile::new("mixed.pdf", &kit::mixed_size_pdf());
    let client = client();
    let doc = client.open(&file.path_str(), None).unwrap();
    let sizes: Vec<(f32, f32)> = doc.pages.iter().map(|p| (p.width, p.height)).collect();
    assert_eq!(
        sizes,
        vec![
            (612.0, 792.0),
            (842.0, 595.0),
            (300.0, 300.0),
            (200.0, 1000.0)
        ]
    );
}

#[test]
fn renders_a_page_at_the_requested_size() {
    let file = kit::TempFile::new("render.pdf", &kit::simple_pdf(2));
    let client = client();
    let doc = client.open(&file.path_str(), None).unwrap();
    // Half size: 306 x 396 pixels.
    let bitmap = client.render(doc.doc, 0, 306, 396).unwrap();
    assert_eq!((bitmap.width, bitmap.height), (306, 396));
    assert_eq!(bitmap.rgba.len(), 306 * 396 * 4);
    // The page background is white and opaque.
    assert_eq!(pixel(&bitmap, 2, 2), [255, 255, 255, 255]);
    // The bar at the bottom of the page is blue: 0.1, 0.3, 0.8 of 255 is about 26, 77, 204.
    let bar = pixel(&bitmap, 150, 396 - 24);
    assert!(
        bar[0] < 60 && (60..100).contains(&bar[1]) && bar[2] > 180 && bar[3] == 255,
        "bar pixel was {bar:?}"
    );
    // There is dark text somewhere: the page is not just white and the bar.
    let dark = bitmap
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] < 80 && p[1] < 80 && p[2] < 80)
        .count();
    assert!(dark > 100, "only {dark} dark pixels");
}

#[test]
fn zoomed_pages_render_at_other_sizes() {
    let file = kit::TempFile::new("zoom.pdf", &kit::simple_pdf(1));
    let client = client();
    let doc = client.open(&file.path_str(), None).unwrap();
    for (w, h) in [(1224, 1584), (100, 130), (612, 792)] {
        let bitmap = client.render(doc.doc, 0, w, h).unwrap();
        assert_eq!((bitmap.width, bitmap.height), (w, h));
    }
}

#[test]
fn a_damaged_file_gives_an_error_and_the_next_open_still_works() {
    let client = client();
    let good = kit::TempFile::new("good.pdf", &kit::simple_pdf(1));
    for (name, bytes) in [
        ("corrupt.pdf", kit::corrupt_pdf()),
        ("not-a-pdf.pdf", kit::not_a_pdf()),
        ("empty.pdf", Vec::new()),
    ] {
        let file = kit::TempFile::new(name, &bytes);
        let err = client.open(&file.path_str(), None).unwrap_err();
        assert_eq!(code_of(err), ErrorCode::Corrupt, "{name}");
        // The next file gets an engine of its own and opens fine.
        assert!(client.open(&good.path_str(), None).is_ok());
    }
    // Every document has its own engine process: six opens, six engines.
    assert_eq!(client.spawn_count(), 6);
}

#[test]
fn a_truncated_file_never_takes_the_engine_down() {
    let file = kit::TempFile::new("truncated.pdf", &kit::truncated_pdf());
    let client = client();
    // PDFium may repair the file or refuse it. Either is fine; a crash is not.
    match client.open(&file.path_str(), None) {
        Ok(doc) => {
            for page in 0..doc.pages.len() as u32 {
                let _ = client.render(doc.doc, page, 100, 130);
            }
        }
        Err(err) => assert_eq!(code_of(err), ErrorCode::Corrupt),
    }
    assert_eq!(client.spawn_count(), 1);
    assert!(client.info().is_ok());
}

#[test]
fn a_missing_file_is_an_io_error() {
    let client = client();
    let err = client.open("/definitely/not/a/file.pdf", None).unwrap_err();
    assert_eq!(code_of(err), ErrorCode::Io);
    let dir = std::env::temp_dir();
    let err = client.open(&dir.to_string_lossy(), None).unwrap_err();
    assert_eq!(code_of(err), ErrorCode::Io);
}

#[test]
fn a_password_protected_file_asks_for_a_password() {
    let file = kit::TempFile::new("secret.pdf", &kit::encrypted_pdf("secret", 2));
    let client = client();

    let err = client.open(&file.path_str(), None).unwrap_err();
    assert_eq!(code_of(err), ErrorCode::PasswordRequired);

    let err = client.open(&file.path_str(), Some("wrong")).unwrap_err();
    assert_eq!(code_of(err), ErrorCode::PasswordIncorrect);

    let doc = client.open(&file.path_str(), Some("secret")).unwrap();
    assert_eq!(doc.pages.len(), 2);
    let bitmap = client.render(doc.doc, 1, 120, 156).unwrap();
    assert_eq!(pixel(&bitmap, 5, 5), [255, 255, 255, 255]);
    // The engine opens the one document it was given, once, so each try has an engine of its own.
    assert_eq!(client.spawn_count(), 3);
}

#[test]
fn a_document_with_javascript_opens_like_any_other() {
    let file = kit::TempFile::new("js.pdf", &kit::javascript_pdf());
    let client = client();
    let doc = client.open(&file.path_str(), None).unwrap();
    assert_eq!(doc.pages.len(), 1);
    let bitmap = client.render(doc.doc, 0, 306, 396).unwrap();
    assert_eq!((bitmap.width, bitmap.height), (306, 396));
    assert_eq!(client.spawn_count(), 1);
}

#[test]
fn bad_requests_are_refused_with_the_right_code() {
    let file = kit::TempFile::new("bad.pdf", &kit::simple_pdf(2));
    let client = client();
    let doc = client.open(&file.path_str(), None).unwrap();

    assert_eq!(
        code_of(client.render(doc.doc, 2, 100, 100).unwrap_err()),
        ErrorCode::PageOutOfRange
    );
    assert_eq!(
        code_of(client.render(doc.doc, u32::MAX, 100, 100).unwrap_err()),
        ErrorCode::PageOutOfRange
    );
    assert_eq!(
        code_of(client.render(doc.doc + 100, 0, 100, 100).unwrap_err()),
        ErrorCode::UnknownDocument
    );
    for (w, h) in [(0, 100), (100, 0), (100_000, 10), (16_000, 16_000)] {
        assert_eq!(
            code_of(client.render(doc.doc, 0, w, h).unwrap_err()),
            ErrorCode::InvalidRequest,
            "{w}x{h}"
        );
    }
    // None of that hurt the engine.
    assert!(client.render(doc.doc, 0, 100, 130).is_ok());
    assert_eq!(client.spawn_count(), 1);
}

#[test]
fn closing_a_document_frees_it() {
    let file = kit::TempFile::new("close.pdf", &kit::simple_pdf(1));
    let client = client();
    let doc = client.open(&file.path_str(), None).unwrap();
    client.close(doc.doc).unwrap();
    assert_eq!(
        code_of(client.render(doc.doc, 0, 100, 130).unwrap_err()),
        ErrorCode::UnknownDocument
    );
    // Closing twice is harmless.
    client.close(doc.doc).unwrap();
}

#[test]
fn a_client_holds_one_document_and_opening_another_replaces_it() {
    // Many documents at once is the job of the pool (tests/pool.rs): one engine each.
    let file = kit::TempFile::new("many.pdf", &kit::simple_pdf(1));
    let client = client();
    let ids: Vec<u64> = (0..5)
        .map(|_| client.open(&file.path_str(), None).unwrap().doc)
        .collect();
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), 5);
    for id in &ids[..4] {
        assert_eq!(
            code_of(client.render(*id, 0, 100, 130).unwrap_err()),
            ErrorCode::UnknownDocument
        );
    }
    assert!(client.render(ids[4], 0, 100, 130).is_ok());
    assert_eq!(client.spawn_count(), 5);
}

#[test]
fn the_app_survives_an_engine_crash_and_gets_a_fresh_engine() {
    let file = kit::TempFile::new("crash.pdf", &kit::simple_pdf(2));
    let client = client();
    let doc = client.open(&file.path_str(), None).unwrap();
    assert!(client.render(doc.doc, 0, 100, 130).is_ok());
    assert_eq!(client.spawn_count(), 1);

    // Simulate the engine dying (as it would on a file that crashes the parser).
    client.kill();

    // The old document is gone with the old process. The client says so without starting a new
    // engine; nothing panics.
    let err = client.render(doc.doc, 0, 100, 130).unwrap_err();
    assert_eq!(code_of(err), ErrorCode::UnknownDocument);
    assert_eq!(client.spawn_count(), 1);

    // Reopening works, with a fresh engine.
    let doc = client.open(&file.path_str(), None).unwrap();
    assert_eq!(client.spawn_count(), 2);
    assert!(client.render(doc.doc, 1, 100, 130).is_ok());
}

#[test]
fn an_old_handle_cannot_touch_a_document_of_the_new_engine() {
    let a = kit::TempFile::new("a.pdf", &kit::simple_pdf(2));
    let b = kit::TempFile::new("b.pdf", &kit::simple_pdf(7));
    let client = client();
    let old = client.open(&a.path_str(), None).unwrap();
    client.kill();
    // The new engine numbers its documents from 1 again, as the old one did.
    let new = client.open(&b.path_str(), None).unwrap();
    assert_ne!(old.doc, new.doc);
    assert_eq!(new.pages.len(), 7);

    // What the app does when it replaces a document: close the old one. This must be a no-op.
    client.close(old.doc).unwrap();
    assert!(client.render(new.doc, 6, 100, 130).is_ok());
    assert_eq!(
        code_of(client.render(old.doc, 0, 100, 130).unwrap_err()),
        ErrorCode::UnknownDocument
    );
    assert_eq!(client.spawn_count(), 2);
}

/// If the app dies without closing the engine's input (a crash, or a kill), an engine that is
/// stuck inside PDFium would never see the end of its input. It watches its parent instead.
#[cfg(unix)]
#[test]
fn the_engine_ends_when_its_parent_is_gone() {
    use std::process::{Command, Stdio};

    // A shell starts the engine in the background with an input that never ends, prints the
    // engine's pid, waits a second so the engine is up and has noted its parent, and exits. The
    // engine is left without its parent.
    let script = format!(
        "sleep 60 2>/dev/null | \"{}\" --pdfium-dir \"{}\" >/dev/null 2>&1 & echo $!; sleep 1",
        env!("CARGO_BIN_EXE_partridge-engine"),
        kit::pdfium_dir().display()
    );
    let out = Command::new("sh")
        .args(["-c", &script])
        .stdout(Stdio::piped())
        .output()
        .unwrap();
    let pid = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(pid.parse::<u32>().is_ok(), "pid was {pid:?}");

    let alive = || {
        let ps = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&ps.stdout).trim().to_string();
        // Gone, or a zombie that only waits to be reaped.
        !state.is_empty() && !state.starts_with('Z')
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while alive() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!alive(), "the engine {pid} is still running");
}

#[test]
fn an_engine_that_dies_during_a_request_is_reported_and_replaced() {
    // Point the client at a program that exits straight away. The request fails with
    // EngineStopped (not a panic), and there is a readable explanation for the user.
    let mut config = EngineConfig::new(env!("CARGO_BIN_EXE_partridge-engine"));
    config.args = vec!["--no-such-flag".into()];
    let client = EngineClient::new(config);
    let err = client.info().unwrap_err();
    assert!(matches!(err, ClientError::EngineStopped(_)), "{err:?}");
    assert!(!message_of(&err).is_empty());
    // It tries again on the next request instead of staying broken for good.
    let _ = client.info();
    assert_eq!(client.spawn_count(), 2);
}
