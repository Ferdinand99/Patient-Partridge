//! What the real engine does on a Windows machine whose firewall is in an unusual state: a profile
//! switched off, the firewall service stopped, the filtering engine stopped. CI runs this once per
//! state (the job "Windows, firewall states") and sets `PARTRIDGE_NETWORK_EXPECT`:
//!
//! - `record`: print what happened and assert nothing (used to find out what a state does).
//! - `proved`: the engine must report `no-network`.
//! - `not-proved`: the engine must report `network-not-proved` and no `no-network`.
//! - `any`: either, but never a refusal of the document.
//!
//! In every state except `record` the document must open, because the engine's fail-closed
//! condition is the AppContainer token, not the answer of the firewall to a probe.

#![cfg(windows)]

use partridge_engine_client::{EngineClient, EngineConfig, SandboxPolicy};
use partridge_testkit as kit;

fn config() -> EngineConfig {
    let mut config = EngineConfig::new(env!("CARGO_BIN_EXE_partridge-engine"));
    config.args = vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
    config.sandbox = SandboxPolicy::Required;
    config
}

#[test]
fn the_engine_in_this_firewall_state() {
    let expect = std::env::var("PARTRIDGE_NETWORK_EXPECT").unwrap_or_else(|_| "any".to_string());
    let state = std::env::var("PARTRIDGE_FIREWALL_STATE").unwrap_or_else(|_| "unknown".to_string());
    println!("firewall state: {state}, expectation: {expect}");

    let started = EngineClient::new(config()).info();
    println!("engine greeting: {started:?}");

    let file = kit::TempFile::new("states.pdf", &kit::simple_pdf(2));
    let opened = EngineClient::new(config()).open(&file.path_str(), None);
    println!("document: {opened:?}");

    if expect == "record" {
        return;
    }
    let info = started.unwrap_or_else(|e| panic!("the engine did not start: {e:?}"));
    opened.unwrap_or_else(|e| panic!("the document was not opened: {e:?}"));
    assert!(info.sandbox.contains("appcontainer"), "{info:?}");
    let proved = info.sandbox.split('+').any(|l| l == "no-network");
    let unproved = info.sandbox.contains("network-not-proved");
    assert!(proved != unproved, "exactly one of the two: {info:?}");
    match expect.as_str() {
        "proved" => assert!(proved, "{info:?}"),
        "not-proved" => assert!(unproved, "{info:?}"),
        _ => {}
    }
}
