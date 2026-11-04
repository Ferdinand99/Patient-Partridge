//! The client against a signer that misbehaves (see examples/fake_signer.rs). None of this may hurt
//! the app: every case ends in an error for that request, the signer is ended, and nothing that was
//! typed stays in a message that is kept.

#![cfg(unix)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use partridge_engine_client::{
    ClientError, SandboxPolicy, SignerConfig, SignerHash, SignerSession,
};

/// `cargo test` builds the examples too; the stand-in sits next to the test programs.
fn fake_signer() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join(format!("fake_signer{}", std::env::consts::EXE_SUFFIX))
}

/// The stand-in takes its mode from the name it is started under, because a signer is started
/// without arguments: each mode has a link of its own to the program (a link, and not a file that is
/// written, so that no other test can be holding it open for writing while it starts).
fn config(mode: &str) -> SignerConfig {
    // A folder for each call: tests that run at the same time do not share a link.
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "partridge-fakesigner-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let link = dir.join(format!("fake_signer-{mode}"));
    std::os::unix::fs::symlink(fake_signer(), &link).unwrap();
    let mut config = SignerConfig::new(link);
    config.sandbox = SandboxPolicy::BestEffort;
    config.timeout = Duration::from_secs(5);
    config.load_timeout = Duration::from_secs(5);
    config
}

fn stopped(result: Result<impl std::fmt::Debug, ClientError>) -> String {
    match result {
        Err(ClientError::Signer { code, message }) => {
            assert_eq!(code, "signer_stopped", "{message}");
            message
        }
        other => panic!("expected the signer to be stopped, got {other:?}"),
    }
}

#[test]
fn a_signer_that_behaves_is_greeted_loaded_and_asked_to_sign() {
    let mut signer = SignerSession::start(&config("good")).unwrap();
    assert_eq!(signer.sandbox(), "stand-in");
    let certificate = signer.load(b"anything", "secret").unwrap();
    assert_eq!(certificate.name, "Stand-in");
    assert_eq!(signer.certificate().unwrap().name, "Stand-in");
    let cms = signer.sign_digest(&[1; 32], SignerHash::Sha256).unwrap();
    assert_eq!(cms.len(), 100);
    // A digest of the wrong length is refused before the signer is asked.
    match signer.sign_digest(&[1; 31], SignerHash::Sha256) {
        Err(ClientError::Signer { code, .. }) => assert_eq!(code, "signer_invalid_request"),
        other => panic!("{other:?}"),
    }
    signer.close();
}

#[test]
fn a_signer_without_a_sandbox_is_refused_when_one_is_required() {
    let mut required = config("no-sandbox");
    required.sandbox = SandboxPolicy::Required;
    match SignerSession::start(&required) {
        Err(ClientError::Signer { code, .. }) => assert_eq!(code, "signer_sandbox_unavailable"),
        Err(ClientError::EngineStopped(_)) => {}
        other => panic!("{:?}", other.err()),
    }
    // For a development build the same signer is fine.
    assert!(SignerSession::start(&config("no-sandbox")).is_ok());
}

#[test]
fn another_version_of_the_messages_is_refused() {
    match SignerSession::start(&config("wrong-version")) {
        Err(ClientError::Signer { code, .. }) => assert_eq!(code, "signer_stopped"),
        other => panic!("{:?}", other.err()),
    }
}

#[test]
fn a_wrong_password_is_told_and_the_signer_stays_for_another_try() {
    let mut signer = SignerSession::start(&config("error-load")).unwrap();
    match signer.load(b"x", "nope") {
        Err(ClientError::Signer { code, .. }) => assert_eq!(code, "signer_wrong_password"),
        other => panic!("{other:?}"),
    }
    // The stand-in answers the same again, which shows the signer was not ended.
    match signer.load(b"x", "again") {
        Err(ClientError::Signer { code, .. }) => assert_eq!(code, "signer_wrong_password"),
        other => panic!("{other:?}"),
    }
    assert!(signer.certificate().is_none());
}

#[test]
fn an_answer_that_is_not_a_message_ends_the_signer() {
    let mut signer = SignerSession::start(&config("garbage")).unwrap();
    stopped(signer.load(b"x", "p"));
    // Once ended, nothing goes through.
    stopped(signer.load(b"x", "p"));
}

#[test]
fn an_answer_that_is_too_big_is_not_read() {
    let mut signer = SignerSession::start(&config("huge")).unwrap();
    stopped(signer.load(b"x", "p"));
}

#[test]
fn a_signer_that_never_answers_is_ended_when_the_time_is_up() {
    let mut cfg = config("silent");
    cfg.load_timeout = Duration::from_millis(600);
    let mut signer = SignerSession::start(&cfg).unwrap();
    let started = Instant::now();
    let message = stopped(signer.load(b"x", "p"));
    assert!(message.contains("in time"), "{message}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_signer_that_ends_is_told_from_one_that_answers() {
    match SignerSession::start(&config("exit")) {
        // It may end before or after the greeting is read; either way nothing is signed.
        Ok(mut signer) => {
            stopped(signer.load(b"x", "p"));
        }
        Err(ClientError::Signer { code, .. }) => assert_eq!(code, "signer_stopped"),
        Err(other) => panic!("{other:?}"),
    }
}

#[test]
fn an_answer_of_the_wrong_kind_ends_the_signer() {
    let mut signer = SignerSession::start(&config("wrong-answer")).unwrap();
    stopped(signer.load(b"x", "p"));
}

#[test]
fn a_file_or_a_password_over_the_cap_is_refused_before_the_signer_is_asked() {
    let mut signer = SignerSession::start(&config("good")).unwrap();
    let big = vec![0u8; partridge_engine_client::MAX_CERTIFICATE_FILE + 1];
    match signer.load(&big, "p") {
        Err(ClientError::Signer { code, .. }) => assert_eq!(code, "signer_too_large"),
        other => panic!("{other:?}"),
    }
    match signer.load(b"x", &"p".repeat(5_000)) {
        Err(ClientError::Signer { code, .. }) => assert_eq!(code, "signer_password_too_long"),
        other => panic!("{other:?}"),
    }
    // Both were refused without a word to the signer, which is still there.
    assert!(signer.load(b"x", "p").is_ok());
}

#[test]
fn a_program_that_is_not_there_is_an_error_and_not_a_crash() {
    let mut cfg = config("good");
    cfg.program = PathBuf::from("/nonexistent/partridge-signer");
    match SignerSession::start(&cfg) {
        Err(ClientError::Signer { code, .. }) => assert_eq!(code, "signer_stopped"),
        other => panic!("{:?}", other.err()),
    }
}
