//! The client's annotation requests against the stand-in engine (see examples/fake_engine.rs): what
//! the client refuses before it asks, and what it does with an engine that answers wrongly.

use std::path::PathBuf;
use std::time::Duration;

use partridge_engine_client::{
    AnnotKind, ClientError, EngineClient, EngineConfig, StampKind,
};
use partridge_protocol::{
    ErrorCode, MAX_ANNOT_TEXT, MAX_COORDINATE, MAX_INK_POINTS, MAX_INK_STROKES, MAX_INK_WIDTH,
    MAX_MARKUP_CHARS,
};

fn fake_engine() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap().join("examples");
    dir.join(format!("fake_engine{}", std::env::consts::EXE_SUFFIX))
}

fn doc_path() -> String {
    static FILE: std::sync::OnceLock<partridge_testkit::TempFile> = std::sync::OnceLock::new();
    FILE.get_or_init(|| partridge_testkit::TempFile::new("doc.pdf", b"%PDF-1.7\n"))
        .path_str()
}

fn client_with(mode: &str) -> (EngineClient, u64) {
    let mut config = EngineConfig::new(fake_engine());
    config.args = vec![mode.into()];
    config.timeout = Duration::from_secs(20);
    let client = EngineClient::new(config);
    let doc = client.open(&doc_path(), None).unwrap().doc;
    (client, doc)
}

const COLOR: [u8; 4] = [1, 2, 3, 255];

fn refused<T: std::fmt::Debug>(result: Result<T, ClientError>) {
    match result {
        Err(ClientError::Engine {
            code: ErrorCode::InvalidRequest,
            ..
        }) => {}
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn stopped<T: std::fmt::Debug>(result: Result<T, ClientError>, needle: &str) {
    match result {
        Err(ClientError::EngineStopped(why)) => {
            assert!(why.contains(needle), "wanted '{needle}' in '{why}'")
        }
        other => panic!("expected EngineStopped, got {other:?}"),
    }
}

#[test]
fn the_requests_work_against_a_good_engine() {
    let (c, doc) = client_with("annot-good");
    let list = c.list_annots(doc, 0).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].text, "from the stand-in");
    assert_eq!(
        c.add_markup(doc, 0, AnnotKind::Underline, COLOR, 0, 5).unwrap().kind,
        AnnotKind::Underline
    );
    assert_eq!(c.add_note(doc, 0, 1.0, 2.0, COLOR, "hi").unwrap().text, "hi");
    let strokes = vec![vec![[1.0, 2.0], [3.0, 4.0]]];
    assert_eq!(c.add_ink(doc, 0, COLOR, 2.0, strokes).unwrap().kind, AnnotKind::Ink);
    assert_eq!(
        c.add_stamp(doc, 0, 1.0, 2.0, StampKind::Draft).unwrap().kind,
        AnnotKind::Stamp
    );
    assert_eq!(c.update_annot(doc, 0, 9, Some("x"), None, 0.0, 0.0).unwrap().id, 9);
    c.remove_annot(doc, 0, 9).unwrap();
    assert_eq!(c.restore_annot(doc, 0, 9).unwrap().id, 9);
    // The engine is still the same one after all of it.
    assert_eq!(c.spawn_count(), 1);
}

#[test]
fn requests_outside_the_limits_are_refused_before_the_engine_is_asked() {
    let (c, doc) = client_with("annot-good");
    // Markup: not markup, no range, backwards, and more than a page holds.
    refused(c.add_markup(doc, 0, AnnotKind::Note, COLOR, 0, 5));
    refused(c.add_markup(doc, 0, AnnotKind::Highlight, COLOR, 5, 5));
    refused(c.add_markup(doc, 0, AnnotKind::Highlight, COLOR, 9, 5));
    refused(c.add_markup(doc, 0, AnnotKind::Highlight, COLOR, 0, MAX_MARKUP_CHARS as u32 + 1));
    assert!(c
        .add_markup(doc, 0, AnnotKind::Highlight, COLOR, 0, MAX_MARKUP_CHARS as u32)
        .is_ok());
    // Text: one byte over the cap is refused (the protocol would cut it), at the cap is fine.
    refused(c.add_note(doc, 0, 1.0, 1.0, COLOR, &"a".repeat(MAX_ANNOT_TEXT + 1)));
    assert!(c
        .add_note(doc, 0, 1.0, 1.0, COLOR, &"a".repeat(MAX_ANNOT_TEXT))
        .is_ok());
    refused(c.update_annot(doc, 0, 1, Some(&"a".repeat(MAX_ANNOT_TEXT + 1)), None, 0.0, 0.0));
    // Places.
    for bad in [f32::NAN, f32::INFINITY, MAX_COORDINATE * 2.0] {
        refused(c.add_note(doc, 0, bad, 1.0, COLOR, ""));
        refused(c.add_stamp(doc, 0, 1.0, bad, StampKind::Final));
        refused(c.update_annot(doc, 0, 1, None, None, bad, 0.0));
        refused(c.add_ink(doc, 0, COLOR, 2.0, vec![vec![[bad, 0.0]]]));
    }
    // Drawings: nothing, an empty stroke, too many strokes, too many points, widths.
    refused(c.add_ink(doc, 0, COLOR, 2.0, vec![]));
    refused(c.add_ink(doc, 0, COLOR, 2.0, vec![vec![]]));
    let many = vec![vec![[1.0, 1.0]]; MAX_INK_STROKES + 1];
    refused(c.add_ink(doc, 0, COLOR, 2.0, many));
    let heavy = vec![vec![[1.0, 1.0]; MAX_INK_POINTS + 1]];
    refused(c.add_ink(doc, 0, COLOR, 2.0, heavy));
    for width in [0.0, MAX_INK_WIDTH + 1.0, f32::NAN] {
        refused(c.add_ink(doc, 0, COLOR, width, vec![vec![[1.0, 1.0]]]));
    }
    let at_cap = vec![vec![[1.0, 1.0]; MAX_INK_POINTS]];
    assert!(c.add_ink(doc, 0, COLOR, MAX_INK_WIDTH, at_cap).is_ok());
    // The refusals did not hurt the connection.
    assert_eq!(c.spawn_count(), 1);
}

#[test]
fn an_engine_that_answers_with_another_kind_of_annotation_is_stopped() {
    let (c, doc) = client_with("annot-wrong-kind");
    stopped(c.add_markup(doc, 0, AnnotKind::Highlight, COLOR, 0, 5), "unexpected");
    let (c, doc) = client_with("annot-wrong-kind");
    stopped(c.add_note(doc, 0, 1.0, 1.0, COLOR, "x"), "unexpected");
    let (c, doc) = client_with("annot-wrong-kind");
    stopped(c.add_ink(doc, 0, COLOR, 2.0, vec![vec![[1.0, 1.0]]]), "unexpected");
    let (c, doc) = client_with("annot-wrong-kind");
    stopped(c.add_stamp(doc, 0, 1.0, 1.0, StampKind::Draft), "unexpected");
}

#[test]
fn an_engine_that_answers_about_another_annotation_is_stopped() {
    let (c, doc) = client_with("annot-wrong-id");
    stopped(c.update_annot(doc, 0, 4, None, None, 1.0, 1.0), "unexpected");
    let (c, doc) = client_with("annot-wrong-id");
    stopped(c.restore_annot(doc, 0, 4), "unexpected");
}

#[test]
fn a_list_that_is_not_a_list_or_is_far_too_big_stops_the_engine() {
    let (c, doc) = client_with("annot-wrong-answer");
    stopped(c.list_annots(doc, 0), "unexpected");
    let (c, doc) = client_with("annot-big");
    stopped(c.list_annots(doc, 0), "");
}

#[test]
fn a_handle_from_a_replaced_engine_cannot_reach_annotations() {
    let (c, doc) = client_with("annot-good");
    c.kill();
    match c.list_annots(doc, 0) {
        Err(ClientError::Engine {
            code: ErrorCode::UnknownDocument,
            ..
        }) => {}
        other => panic!("{other:?}"),
    }
}
