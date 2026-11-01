//! The text requests against an engine that misbehaves (see examples/fake_engine.rs): an answer
//! that is too big, of the wrong kind, or for another request ends that engine, and the client
//! stays usable.

use std::path::PathBuf;
use std::time::Duration;

use partridge_engine_client::{ClientError, EngineClient, EngineConfig, EnginePool};
use partridge_protocol::{ErrorCode, MAX_QUERY_LEN};

fn fake_engine() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap().join("examples");
    dir.join(format!("fake_engine{}", std::env::consts::EXE_SUFFIX))
}

fn config(mode: &str) -> EngineConfig {
    let mut config = EngineConfig::new(fake_engine());
    config.args = vec![mode.into()];
    config.timeout = Duration::from_secs(20);
    config
}

fn client_with(mode: &str) -> EngineClient {
    EngineClient::new(config(mode))
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
fn the_good_fake_answers_the_text_requests() {
    let client = client_with("good");
    let doc = client.open("a.pdf", None).unwrap();
    assert!(client.outline(doc.doc).unwrap().is_empty());
    let text = client.page_text(doc.doc, 0).unwrap();
    assert_eq!((text.text.as_str(), text.rects.len()), ("hi", 2));
    assert!(client.page_links(doc.doc, 0).unwrap().is_empty());
    assert!(client.search_page(doc.doc, 0, "hi").unwrap().is_empty());
}

#[test]
fn an_answer_of_the_wrong_kind_stops_the_engine() {
    let client = client_with("wrong-kind");
    let doc = client.open("a.pdf", None).unwrap();
    assert_stopped(client.outline(doc.doc), "unexpected answer");
    // Opening a file starts a fresh engine; the old handle is stale.
    match client.outline(doc.doc).unwrap_err() {
        ClientError::Engine { code, .. } => assert_eq!(code, ErrorCode::UnknownDocument),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_answer_that_is_too_big_stops_the_engine() {
    let client = client_with("big-text");
    let doc = client.open("a.pdf", None).unwrap();
    assert_stopped(client.page_text(doc.doc, 0), "too large");
    // The engine was replaced: the old handle is stale, and a new open works.
    let doc = client.open("a.pdf", None).unwrap();
    assert_stopped(client.outline(doc.doc), "too large");
    assert_eq!(client.spawn_count(), 2);
}

#[test]
fn an_answer_for_another_request_stops_the_engine() {
    let client = client_with("text-wrong-id");
    let doc = client.open("a.pdf", None).unwrap();
    assert_stopped(client.search_page(doc.doc, 0, "a"), "different request");
}

#[test]
fn a_search_text_that_is_too_long_is_refused_before_anything_is_sent() {
    let client = client_with("good");
    let doc = client.open("a.pdf", None).unwrap();
    let long = "x".repeat(MAX_QUERY_LEN + 1);
    match client.search_page(doc.doc, 0, &long).unwrap_err() {
        ClientError::Engine { code, .. } => assert_eq!(code, ErrorCode::InvalidRequest),
        other => panic!("{other:?}"),
    }
    assert_eq!(client.spawn_count(), 1);
}

#[test]
fn the_pool_works_with_the_fake_and_refuses_handles_it_never_gave_out() {
    let pool = EnginePool::new(config("good"), 3);
    let (a, pages) = pool.open("a.pdf", None).unwrap();
    assert_eq!(pages.len(), 1);
    let (b, _) = pool.open("b.pdf", None).unwrap();
    assert_ne!(a, b);
    assert_eq!(pool.page_text(a, 0).unwrap().text, "hi");
    pool.close(a);
    match pool.page_text(a, 0).unwrap_err() {
        ClientError::Engine { code, .. } => assert_eq!(code, ErrorCode::UnknownDocument),
        other => panic!("{other:?}"),
    }
    // Handles are counted up, never reused.
    let (c, _) = pool.open("c.pdf", None).unwrap();
    assert!(c > b);
}

#[test]
fn a_pool_with_a_missing_engine_reports_it_and_stays_empty() {
    let mut config = EngineConfig::new("/definitely/not/an/engine");
    config.timeout = Duration::from_secs(5);
    let pool = EnginePool::new(config, 3);
    assert!(matches!(
        pool.open("a.pdf", None),
        Err(ClientError::EngineStopped(_))
    ));
    assert!(pool.is_empty());
}
