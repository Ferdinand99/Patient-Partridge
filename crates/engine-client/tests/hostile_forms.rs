//! The form requests against an engine that misbehaves (see examples/fake_forms_engine.rs): an
//! answer that is too big, over a cap, of the wrong kind or for another request ends that engine,
//! and the client stays usable. And a request that is over a cap never leaves the app.

use std::path::PathBuf;
use std::time::Duration;

use partridge_engine_client::{ClientError, EngineClient, EngineConfig, FormKind, FormValue};
use partridge_protocol::{ErrorCode, MAX_FORM_NAME, MAX_FORM_OPTIONS, MAX_FORM_VALUE};

fn fake_engine() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap().join("examples");
    dir.join(format!("fake_forms_engine{}", std::env::consts::EXE_SUFFIX))
}

fn client_with(mode: &str) -> EngineClient {
    let mut config = EngineConfig::new(fake_engine());
    config.args = vec![mode.into()];
    config.timeout = Duration::from_secs(20);
    EngineClient::new(config)
}

/// A real file for the client to hand to the stand-in engine, which never reads it.
fn doc_path() -> String {
    static FILE: std::sync::OnceLock<partridge_testkit::TempFile> = std::sync::OnceLock::new();
    FILE.get_or_init(|| partridge_testkit::TempFile::new("forms-doc.pdf", b"%PDF-1.7\n"))
        .path_str()
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
fn the_good_fake_answers_the_form_requests() {
    let client = client_with("good");
    let doc = client.open(&doc_path(), None).unwrap().doc;
    assert_eq!(client.form_info(doc).unwrap(), (FormKind::Acro, true));
    let fields = client.form_fields(doc, 0).unwrap();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name, "name");
    let after = client
        .set_form_field(doc, 0, 0, "name", &FormValue::Text("x".into()))
        .unwrap();
    assert_eq!(after.len(), 1);
}

#[test]
fn an_answer_bigger_than_a_list_of_fields_can_be_ends_the_engine() {
    let client = client_with("big");
    let doc = client.open(&doc_path(), None).unwrap().doc;
    assert_stopped(client.form_fields(doc, 0), "unreadable");
    // The client is usable: the next document starts a new engine.
    assert!(client.open(&doc_path(), None).is_ok());
    assert_eq!(client.spawn_count(), 2);
}

#[test]
fn a_field_over_a_cap_ends_the_engine() {
    for mode in ["long-name", "long-value", "too-many"] {
        let client = client_with(mode);
        let doc = client.open(&doc_path(), None).unwrap().doc;
        assert_stopped(client.form_fields(doc, 0), "unreadable");
        let doc = client.open(&doc_path(), None).unwrap().doc;
        assert_stopped(
            client.set_form_field(doc, 0, 0, "name", &FormValue::Checked(true)),
            "unreadable",
        );
    }
}

#[test]
fn an_answer_of_the_wrong_kind_or_for_another_request_ends_the_engine() {
    let client = client_with("wrong-kind");
    let doc = client.open(&doc_path(), None).unwrap().doc;
    assert_stopped(client.form_fields(doc, 0), "unexpected answer");
    let doc = client.open(&doc_path(), None).unwrap().doc;
    assert_stopped(client.form_info(doc), "unexpected answer");

    let client = client_with("wrong-id");
    let doc = client.open(&doc_path(), None).unwrap().doc;
    assert_stopped(client.form_fields(doc, 0), "different request");
}

#[test]
fn a_name_or_a_value_over_a_cap_is_refused_before_it_is_sent() {
    let client = client_with("good");
    let doc = client.open(&doc_path(), None).unwrap().doc;
    let spawned = client.spawn_count();
    let refused = |result: Result<_, ClientError>| match result {
        Err(ClientError::Engine { code, .. }) => assert_eq!(code, ErrorCode::InvalidRequest),
        other => panic!("{other:?}"),
    };
    refused(client.set_form_field(
        doc,
        0,
        0,
        &"n".repeat(MAX_FORM_NAME + 1),
        &FormValue::Checked(true),
    ));
    refused(client.set_form_field(
        doc,
        0,
        0,
        "name",
        &FormValue::Text("v".repeat(MAX_FORM_VALUE + 1)),
    ));
    refused(client.set_form_field(
        doc,
        0,
        0,
        "name",
        &FormValue::Choice(vec![0; MAX_FORM_OPTIONS + 1]),
    ));
    // Nothing was sent, so the engine is the same one, and it still answers.
    assert_eq!(client.spawn_count(), spawned);
    assert!(client
        .set_form_field(
            doc,
            0,
            0,
            "name",
            &FormValue::Text("v".repeat(MAX_FORM_VALUE))
        )
        .is_ok());
}
