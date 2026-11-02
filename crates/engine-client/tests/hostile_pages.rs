//! The client and the pool against an engine that misbehaves in the page tools (see
//! examples/fake_pages_engine.rs). None of this may hurt the app: every case ends in an error for
//! that request, the engine is stopped, and the client stays usable.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use partridge_engine_client::{
    Cancelled, ClientError, EngineClient, EngineConfig, EnginePool, ExportError, InsertOptions,
    SaveError,
};
use partridge_protocol::{ErrorCode, MAX_EDIT_PAGES};

fn fake_engine() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap().join("examples");
    dir.join(format!("fake_pages_engine{}", std::env::consts::EXE_SUFFIX))
}

fn doc_path() -> String {
    static FILE: std::sync::OnceLock<partridge_testkit::TempFile> = std::sync::OnceLock::new();
    FILE.get_or_init(|| partridge_testkit::TempFile::new("doc.pdf", b"%PDF-1.7\n"))
        .path_str()
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
fn the_good_stand_in_works_through_every_tool() {
    let client = client_with("good");
    let doc = client.open(&doc_path(), None).unwrap().doc;
    let changed = client.pages_delete(doc, &[1]).unwrap();
    assert_eq!((changed.pages.len(), changed.snapshot, changed.count), (2, 1, 1));
    assert_eq!(client.pages_rotate(doc, &[0], 1).unwrap().snapshot, 0);
    assert_eq!(client.pages_insert_blank(doc, 0, 100.0, 100.0).unwrap().pages.len(), 3);
    assert_eq!(client.pages_reorder(doc, &[2, 1, 0]).unwrap().pages.len(), 3);
    assert_eq!(client.pages_restore(doc, 1).unwrap().snapshot, 2);
    let cropped = client.pages_crop(doc, &[0, 1], [1.0; 4]).unwrap();
    assert_eq!((cropped.before.len(), cropped.after.len()), (2, 2));
    client
        .pages_set_boxes(doc, &[0], &[[0.0, 0.0, 10.0, 10.0]])
        .unwrap();
    // A copy: begin, the pieces, the end.
    let mut got = Vec::new();
    let exported = client
        .export_to(doc, &[0, 2], &mut |_| Ok(()), &mut |piece| {
            got.extend_from_slice(piece);
            Ok(())
        })
        .unwrap();
    assert_eq!(exported.pages, 2);
    assert_eq!(got.len(), 2_500_000);
    assert!(got.iter().enumerate().all(|(i, b)| *b == (i % 251) as u8));
    // An insert.
    client.import_begin(doc, 10).unwrap();
    client.import_piece(doc, 0, &[1, 2, 3, 4]).unwrap();
    client.import_piece(doc, 4, &[1, 2, 3, 4, 5, 6]).unwrap();
    assert_eq!(client.import_end(doc, 1).unwrap().count, 2);
    client.import_abort(doc);
}

#[test]
fn answers_with_pages_that_are_not_sizes_stop_the_engine() {
    let client = client_with("bad-size");
    let doc = client.open(&doc_path(), None).unwrap().doc;
    assert_stopped(client.pages_delete(doc, &[0]), "unexpected answer");
    // The client is not stuck: opening again starts a new engine.
    assert!(client.open(&doc_path(), None).is_ok());
    assert_eq!(client.spawn_count(), 2);
}

#[test]
fn an_answer_of_the_wrong_kind_stops_the_engine() {
    let client = client_with("wrong-kind");
    let doc = client.open(&doc_path(), None).unwrap().doc;
    assert_stopped(client.pages_rotate(doc, &[0], 1), "unexpected answer");
}

#[test]
fn an_answer_with_more_pages_than_a_document_may_have_stops_the_engine() {
    let client = client_with("too-many-pages");
    let doc = client.open(&doc_path(), None).unwrap().doc;
    assert_stopped(client.pages_delete(doc, &[0]), "unreadable");
}

#[test]
fn a_copy_that_breaks_the_rules_of_a_save_stops_the_engine() {
    for (mode, needle) in [
        ("export-short", "unexpected answer"),
        ("export-offset", "unexpected answer"),
        ("export-pages", "unexpected answer"),
    ] {
        let client = client_with(mode);
        let doc = client.open(&doc_path(), None).unwrap().doc;
        match client.export_to(doc, &[0, 1], &mut |_| Ok(()), &mut |_| Ok(())) {
            Err(ExportError::Client(ClientError::EngineStopped(why))) => {
                assert!(why.contains(needle), "{mode}: {why}")
            }
            other => panic!("{mode}: {other:?}"),
        }
    }
}

#[test]
fn an_insert_that_is_acknowledged_wrongly_stops_the_engine() {
    let client = client_with("ack-wrong");
    let doc = client.open(&doc_path(), None).unwrap().doc;
    client.import_begin(doc, 100).unwrap();
    assert_stopped(client.import_piece(doc, 0, &[1, 2, 3]), "unexpected answer");
}

#[test]
fn lists_and_sizes_that_do_not_fit_are_refused_before_anything_is_sent() {
    let client = client_with("good");
    let doc = (1u64 << 32) | 1;
    for result in [
        client.pages_delete(doc, &[]).map(|_| ()),
        client.pages_delete(doc, &vec![0; MAX_EDIT_PAGES + 1]).map(|_| ()),
        client.pages_reorder(doc, &[]).map(|_| ()),
        client.pages_rotate(doc, &[], 1).map(|_| ()),
        client.pages_crop(doc, &[0], [f32::NAN, 0.0, 0.0, 0.0]).map(|_| ()),
        client.pages_set_boxes(doc, &[0, 1], &[[0.0; 4]]).map(|_| ()),
        client.import_begin(doc, 0),
        client.import_begin(doc, u64::MAX),
    ] {
        match result {
            Err(ClientError::Engine { code, .. }) => {
                assert!(
                    matches!(code, ErrorCode::InvalidRequest | ErrorCode::TooManyPages),
                    "{code:?}"
                )
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(client.spawn_count(), 0, "no engine was started for them");
}

#[test]
fn a_tool_that_works_for_ever_is_stopped_by_the_overall_limit() {
    let mut config = config("endless");
    config.save_floor = Duration::from_secs(1);
    config.save_slowest = u64::MAX;
    let client = EngineClient::new(config);
    let doc = client.open(&doc_path(), None).unwrap().doc;
    let started = Instant::now();
    match client.pages_delete(doc, &[0]) {
        Err(ClientError::EngineStopped(why)) => {
            assert!(why.starts_with(partridge_engine_client::TOOK_TOO_LONG), "{why}")
        }
        other => panic!("{other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
}

#[test]
fn a_tool_that_never_answers_is_stopped() {
    let mut config = config("hang");
    config.save_stall = Duration::from_millis(500);
    let client = EngineClient::new(config);
    // `hang` also hangs the open? No: only the page tools.
    let doc = client.open(&doc_path(), None).unwrap().doc;
    let started = Instant::now();
    assert_stopped(client.pages_rotate(doc, &[0], 1), "did not answer");
    assert!(started.elapsed() < Duration::from_secs(10));
}

// ---- the pool ------------------------------------------------------------------------------------

fn pool(mode: &str) -> EnginePool {
    EnginePool::new(config(mode), 4)
}

#[test]
fn pages_are_inserted_from_another_file_and_progress_is_told() {
    let pool = pool("good");
    let (handle, _) = pool.open(&doc_path(), None).unwrap();
    let last = AtomicU64::new(0);
    let calls = AtomicU64::new(0);
    let progress = |done: u64, total: u64| {
        assert_eq!(total, 2_500_000);
        assert!(done > last.swap(done, Ordering::SeqCst), "progress goes up");
        calls.fetch_add(1, Ordering::SeqCst);
    };
    let outcome = pool
        .insert_pdf(handle, 1, &doc_path(), None, InsertOptions::default(), &progress)
        .unwrap();
    assert_eq!(outcome.inserted, 3);
    assert_eq!(outcome.changed.count, 2);
    assert_eq!(last.load(Ordering::SeqCst), 2_500_000);
    assert!(calls.load(Ordering::SeqCst) >= 3, "pieces of at most a MiB");
}

#[test]
fn stopping_the_transfer_of_pages_leaves_the_document_alone() {
    let pool = Arc::new(pool("good"));
    let (handle, _) = pool.open(&doc_path(), None).unwrap();
    let stopper = pool.clone();
    let progress = move |done: u64, _total: u64| {
        if done >= 1_000_000 {
            assert_eq!(stopper.cancel_pages(handle), Cancelled::Handover);
        }
    };
    let started = Instant::now();
    let result = pool.insert_pdf(
        handle,
        0,
        &doc_path(),
        None,
        InsertOptions::default(),
        &progress,
    );
    match result {
        Err(ClientError::Save(SaveError::Cancelled)) => {}
        other => panic!("{other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(10));
    // The document is still there and works.
    assert!(pool.pages_rotate(handle, &[0], 1).is_ok());
    // Nothing is under way any more.
    assert_eq!(pool.cancel_pages(handle), Cancelled::Idle);
}

#[test]
fn a_change_that_hangs_can_be_stopped_by_stopping_the_engine() {
    let mut cfg = config("hang");
    cfg.save_stall = Duration::from_secs(120);
    let pool = Arc::new(EnginePool::new(cfg, 4));
    let (handle, _) = pool.open(&doc_path(), None).unwrap();
    let stopper = pool.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        stopper.cancel_pages(handle)
    });
    let started = Instant::now();
    let result = pool.pages_rotate(handle, &[0], 1);
    assert!(matches!(result, Err(ClientError::EngineStopped(_))), "{result:?}");
    assert_eq!(thread.join().unwrap(), Cancelled::EngineStopped);
    assert!(started.elapsed() < Duration::from_secs(10));
}
