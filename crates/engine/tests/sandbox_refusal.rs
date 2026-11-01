//! A release engine that cannot lock itself in must not read documents, and the app must say so.
//!
//! This test pretends to be a release build with the switches that debug builds have for it: the
//! sandbox is forced to fail and the engine is told to act as it does in a release build. It sits in
//! a test program of its own, with one test, because it sets environment variables, which the
//! engines it starts inherit.

use partridge_engine_client::{ClientError, EngineClient, EngineConfig, SandboxPolicy};
use partridge_protocol::ErrorCode;
use partridge_testkit as kit;

fn config(policy: SandboxPolicy) -> EngineConfig {
    let mut config = EngineConfig::new(env!("CARGO_BIN_EXE_partridge-engine"));
    config.args = vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
    config.sandbox = policy;
    config
}

#[test]
fn an_engine_that_cannot_apply_its_sandbox_refuses_to_open_the_document() {
    // SAFETY: the only test in this program, and no other thread reads the environment.
    unsafe {
        std::env::set_var("PARTRIDGE_TEST_FAIL_SANDBOX", "1");
        std::env::set_var("PARTRIDGE_TEST_ENFORCE", "1");
    }
    let client = EngineClient::new(config(SandboxPolicy::Required));
    let file = kit::TempFile::new("refused.pdf", &kit::simple_pdf(1));

    match client.open(&file.path_str(), None) {
        Err(ClientError::Engine { code, message }) => {
            assert_eq!(code, ErrorCode::SandboxUnavailable);
            assert!(message.contains("sandbox"), "{message}");
            assert!(message.contains("forced by a test"), "{message}");
        }
        other => panic!("expected the sandbox error, got {other:?}"),
    }
    // Nothing was opened, and the client is not stuck.
    assert!(matches!(
        client.info(),
        Err(ClientError::Engine {
            code: ErrorCode::SandboxUnavailable,
            ..
        })
    ));

    // A debug build without the switch that makes it act like a release build goes on without a
    // sandbox, and says so in its greeting.
    // SAFETY: as above.
    unsafe {
        std::env::remove_var("PARTRIDGE_TEST_ENFORCE");
    }
    let client = EngineClient::new(config(SandboxPolicy::BestEffort));
    assert_eq!(client.info().unwrap().sandbox, "");
    // The app side refuses such an engine when it insists on a sandbox.
    let client = EngineClient::new(config(SandboxPolicy::Required));
    assert!(matches!(
        client.open(&file.path_str(), None),
        Err(ClientError::Engine {
            code: ErrorCode::SandboxUnavailable,
            ..
        })
    ));
}
