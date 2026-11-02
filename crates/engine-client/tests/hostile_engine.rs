//! The client against an engine that misbehaves (see examples/fake_engine.rs). None of this may
//! hurt the app: every case ends in an error for that request, and the client stays usable.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use partridge_engine_client::{ClientError, EngineClient, EngineConfig, SandboxPolicy};
use partridge_protocol::ErrorCode;

/// `cargo test` builds the examples too; the fake engine sits next to the test programs.
fn fake_engine() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap().join("examples");
    dir.join(format!("fake_engine{}", std::env::consts::EXE_SUFFIX))
}

/// A real file for the client to hand to the stand-in engine, which never reads it.
fn doc_path() -> String {
    static FILE: std::sync::OnceLock<partridge_testkit::TempFile> = std::sync::OnceLock::new();
    FILE.get_or_init(|| {
        partridge_testkit::TempFile::new(
            "doc.pdf",
            b"%PDF-1.7
",
        )
    })
    .path_str()
}

fn client_with(mode: &str, extra: &[&str]) -> EngineClient {
    let mut config = EngineConfig::new(fake_engine());
    config.args = std::iter::once(mode)
        .chain(extra.iter().copied())
        .map(Into::into)
        .collect();
    config.timeout = Duration::from_secs(20);
    EngineClient::new(config)
}

fn assert_stopped<T: std::fmt::Debug>(result: Result<T, ClientError>, needle: &str) {
    match result {
        Err(ClientError::EngineStopped(why)) => {
            assert!(why.contains(needle), "wanted '{needle}' in '{why}'")
        }
        other => panic!("expected EngineStopped, got {other:?}"),
    }
}

#[test]
fn an_engine_that_names_no_sandbox_is_refused_when_one_is_required() {
    let mut config = EngineConfig::new(fake_engine());
    config.args = vec!["no-sandbox".into()];
    config.sandbox = SandboxPolicy::Required;
    let client = EngineClient::new(config);
    match client.open(&doc_path(), None) {
        Err(ClientError::Engine { code, .. }) => assert_eq!(code, ErrorCode::SandboxUnavailable),
        other => panic!("{other:?}"),
    }
    // The same engine is fine for a development build.
    let mut config = EngineConfig::new(fake_engine());
    config.args = vec!["no-sandbox".into()];
    config.sandbox = SandboxPolicy::BestEffort;
    let client = EngineClient::new(config);
    assert!(client.open(&doc_path(), None).is_ok());
}

#[test]
fn the_good_fake_works() {
    let client = client_with("good", &[]);
    let doc = client.open(&doc_path(), None).unwrap();
    assert_eq!(doc.pages.len(), 1);
    assert!(client.render(doc.doc, 0, 40, 50).is_ok());
}

#[test]
fn frames_nobody_asked_for_stop_the_engine() {
    let client = client_with("unasked", &[]);
    // The greeting works; the extra frames come after it.
    client.info().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let err = client.open(&doc_path(), None);
    assert!(matches!(err, Err(ClientError::EngineStopped(_))), "{err:?}");
    // The client is not stuck: a request after that starts a new engine (which again misbehaves
    // after its greeting, but the greeting itself works). The open started one of its own.
    assert_eq!(client.spawn_count(), 2);
    assert!(client.info().is_ok());
    assert_eq!(client.spawn_count(), 3);
}

#[test]
fn a_flood_of_unasked_data_is_cut_off() {
    let client = client_with("flood", &[]);
    client.info().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let started = Instant::now();
    let result = client.open(&doc_path(), None);
    assert!(
        matches!(result, Err(ClientError::EngineStopped(_))),
        "{result:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn an_answer_bigger_than_the_request_can_produce_is_refused() {
    let client = client_with("oversized", &[]);
    client.info().unwrap();
    assert_stopped(client.open(&doc_path(), None), "unreadable");
}

#[test]
fn an_answer_with_the_wrong_id_is_refused() {
    let client = client_with("wrong-id", &[]);
    assert_stopped(client.open(&doc_path(), None), "different request");
}

#[test]
fn a_page_size_that_is_not_a_size_is_refused() {
    let client = client_with("bad-page-size", &[]);
    assert_stopped(client.open(&doc_path(), None), "not a size");
}

#[test]
fn a_bitmap_of_the_wrong_size_is_refused() {
    let client = client_with("wrong-size", &[]);
    let doc = client.open(&doc_path(), None).unwrap();
    assert_stopped(client.render(doc.doc, 0, 40, 50), "unexpected answer");
}

#[test]
fn killing_the_engine_frees_a_request_that_waits_for_it() {
    let client = Arc::new(client_with("hang", &[]));
    client.info().unwrap();
    let waiting = {
        let client = client.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            (client.open(&doc_path(), None), started.elapsed())
        })
    };
    std::thread::sleep(Duration::from_millis(400));
    // This is what the app does when it quits: it must not wait for the stuck request.
    let kill_started = Instant::now();
    client.kill();
    assert!(kill_started.elapsed() < Duration::from_secs(2));
    let (result, waited) = waiting.join().unwrap();
    assert!(
        matches!(result, Err(ClientError::EngineStopped(_))),
        "{result:?}"
    );
    assert!(waited < Duration::from_secs(10), "waited {waited:?}");
}

#[test]
fn an_engine_that_exits_while_idle_is_replaced_quietly() {
    let marker = std::env::temp_dir().join(format!("partridge-fake-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    let client = client_with("exit-once", &[marker.to_str().unwrap()]);
    client.info().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    // The first engine is gone; the next request gets a new one without an error.
    let doc = client.open(&doc_path(), None).unwrap();
    assert_eq!(doc.pages.len(), 1);
    assert_eq!(client.spawn_count(), 2);
    let _ = std::fs::remove_file(&marker);
}

#[test]
fn handles_do_not_survive_an_engine_restart() {
    let client = client_with("good", &[]);
    let old = client.open(&doc_path(), None).unwrap();
    client.kill();
    let new = client.open(&doc_path(), None).unwrap();
    // The fake engine hands out document 1 every time, like a real engine after a restart. The
    // handles still differ.
    assert_ne!(old.doc, new.doc);
    // Closing the old handle must not touch the new document.
    client.close(old.doc).unwrap();
    assert!(client.render(new.doc, 0, 40, 50).is_ok());
    match client.render(old.doc, 0, 40, 50).unwrap_err() {
        ClientError::Engine { code, .. } => assert_eq!(code, ErrorCode::UnknownDocument),
        other => panic!("{other:?}"),
    }
    // None of that started another engine.
    assert_eq!(client.spawn_count(), 2);
}

#[test]
fn a_slow_save_is_not_taken_for_a_hung_engine() {
    // The answer to SaveBegin takes 1.5 s, three times the limit that a render gets here.
    let mut config = EngineConfig::new(fake_engine());
    config.args = vec!["save-slow".into()];
    config.timeout = Duration::from_millis(500);
    let client = EngineClient::new(config);
    let doc = client.open(&doc_path(), None).unwrap();
    let mut size = 0usize;
    let saved = client.save_to(doc.doc, &mut |piece| {
        size += piece.len();
        Ok(())
    });
    assert!(saved.is_ok(), "the engine was killed: {saved:?}");
    assert_eq!(size, 2_500_000);
    // The engine is still the same one.
    assert_eq!(client.spawn_count(), 1);
}

#[test]
fn progress_keeps_a_save_alive_for_longer_than_the_limit_and_silence_does_not() {
    // The save takes 1.8 s with a sign of life every 0.3 s; the engine may be silent for 1 s.
    let mut config = EngineConfig::new(fake_engine());
    config.args = vec!["save-progress".into()];
    config.timeout = Duration::from_millis(500);
    config.save_stall = Duration::from_secs(1);
    let client = EngineClient::new(config);
    let doc = client.open(&doc_path(), None).unwrap();
    assert!(client.save_to(doc.doc, &mut |_| Ok(())).is_ok());
    assert_eq!(client.spawn_count(), 1);

    // Silence for longer than that is a hung engine.
    let mut config = EngineConfig::new(fake_engine());
    config.args = vec!["save-silent".into()];
    config.timeout = Duration::from_millis(500);
    config.save_stall = Duration::from_millis(700);
    let client = EngineClient::new(config);
    let doc = client.open(&doc_path(), None).unwrap();
    let started = Instant::now();
    assert!(client.save_to(doc.doc, &mut |_| Ok(())).is_err());
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn a_sign_of_life_that_shows_no_more_work_than_the_last_is_refused() {
    let mut config = EngineConfig::new(fake_engine());
    config.args = vec!["save-stuck".into()];
    config.timeout = Duration::from_millis(500);
    config.save_stall = Duration::from_secs(1);
    let client = EngineClient::new(config);
    let doc = client.open(&doc_path(), None).unwrap();
    match client.save_to(doc.doc, &mut |_| Ok(())) {
        Err(partridge_engine_client::SaveStreamError::Client(ClientError::EngineStopped(why))) => {
            assert!(why.contains("progress"), "{why}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn signs_of_life_do_not_keep_a_save_open_for_ever() {
    // The engine reports more work every 0.1 s for a minute. Each sign renews the stall limit of
    // 1 s, so only the overall limit (1 s for this small file) can end it.
    let mut config = EngineConfig::new(fake_engine());
    config.args = vec!["save-endless".into()];
    config.timeout = Duration::from_millis(500);
    config.save_stall = Duration::from_secs(1);
    config.save_floor = Duration::from_secs(1);
    config.save_slowest = u64::MAX;
    let client = EngineClient::new(config);
    let doc = client.open(&doc_path(), None).unwrap();
    let started = Instant::now();
    match client.save_to(doc.doc, &mut |_| Ok(())) {
        Err(partridge_engine_client::SaveStreamError::Client(ClientError::EngineStopped(why))) => {
            assert!(
                why.starts_with(partridge_engine_client::TOOK_TOO_LONG),
                "{why}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn the_overall_limit_grows_with_the_size_of_the_file() {
    // A file of 4 MiB at 1 MiB a second, four passes: 16 s more than the floor. A save that takes
    // 1.8 s with signs of life every 0.3 s is far inside it, and a floor of 0 does not matter.
    let mut config = EngineConfig::new(fake_engine());
    config.args = vec!["save-progress".into()];
    config.timeout = Duration::from_millis(500);
    config.save_stall = Duration::from_secs(1);
    config.save_floor = Duration::ZERO;
    config.save_slowest = 1024 * 1024;
    let client = EngineClient::new(config);
    let big = partridge_testkit::TempFile::new("big.pdf", &vec![b'%'; 4 * 1024 * 1024]);
    let doc = client.open(&big.path_str(), None).unwrap();
    assert!(client.save_to(doc.doc, &mut |_| Ok(())).is_ok());
}
