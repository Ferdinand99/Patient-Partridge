//! The client against an engine that misbehaves (see examples/fake_engine.rs). None of this may
//! hurt the app: every case ends in an error for that request, and the client stays usable.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use partridge_engine_client::{ClientError, EngineClient, EngineConfig};
use partridge_protocol::ErrorCode;

/// `cargo test` builds the examples too; the fake engine sits next to the test programs.
fn fake_engine() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap().join("examples");
    dir.join(format!("fake_engine{}", std::env::consts::EXE_SUFFIX))
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
fn the_good_fake_works() {
    let client = client_with("good", &[]);
    let doc = client.open("a.pdf", None).unwrap();
    assert_eq!(doc.pages.len(), 1);
    assert!(client.render(doc.doc, 0, 40, 50).is_ok());
}

#[test]
fn frames_nobody_asked_for_stop_the_engine() {
    let client = client_with("unasked", &[]);
    // The greeting works; the extra frames come after it.
    client.info().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let err = client.open("a.pdf", None);
    assert!(matches!(err, Err(ClientError::EngineStopped(_))), "{err:?}");
    // The client is not stuck: a request after that starts a new engine (which again misbehaves
    // after its greeting, but the greeting itself works).
    assert_eq!(client.spawn_count(), 1);
    assert!(client.info().is_ok());
    assert_eq!(client.spawn_count(), 2);
}

#[test]
fn a_flood_of_unasked_data_is_cut_off() {
    let client = client_with("flood", &[]);
    client.info().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let started = Instant::now();
    let result = client.open("a.pdf", None);
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
    assert_stopped(client.open("a.pdf", None), "unreadable");
}

#[test]
fn an_answer_with_the_wrong_id_is_refused() {
    let client = client_with("wrong-id", &[]);
    assert_stopped(client.open("a.pdf", None), "different request");
}

#[test]
fn a_bitmap_of_the_wrong_size_is_refused() {
    let client = client_with("wrong-size", &[]);
    let doc = client.open("a.pdf", None).unwrap();
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
            (client.open("a.pdf", None), started.elapsed())
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
    let doc = client.open("a.pdf", None).unwrap();
    assert_eq!(doc.pages.len(), 1);
    assert_eq!(client.spawn_count(), 2);
    let _ = std::fs::remove_file(&marker);
}

#[test]
fn handles_do_not_survive_an_engine_restart() {
    let client = client_with("good", &[]);
    let old = client.open("a.pdf", None).unwrap();
    client.kill();
    let new = client.open("a.pdf", None).unwrap();
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
