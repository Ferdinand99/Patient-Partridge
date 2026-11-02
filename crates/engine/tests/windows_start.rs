//! The engine on Windows starts in a locked-down AppContainer. A process with such a token cannot
//! let the system's loader hand the loading of libraries to its helper threads (only the first
//! thread has the identity that may open files), so every library the engine imports must be one
//! the system maps without opening a file. That holds when the C runtime is linked statically
//! (`.cargo/config.toml`); with the dynamic runtime about nine starts in ten ended with "library
//! not found". These tests start the real engine many times, and many at once.
//!
//! The client starts the engine again when a start ends with "library not found" (see
//! `EngineClient::start`), which would hide exactly the failure these tests look for. So they count
//! the starts that had to be tried again and require that there are none: every start works the
//! first time. The count is printed.

#![cfg(windows)]

use partridge_engine_client::{EngineClient, EngineConfig, SandboxPolicy};
use partridge_testkit as kit;

fn config() -> EngineConfig {
    let mut config = EngineConfig::new(env!("CARGO_BIN_EXE_partridge-engine"));
    config.args = vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
    config.sandbox = SandboxPolicy::Required;
    config
}

/// Starts one engine with a client of its own and returns how many times the start was tried again.
fn start_once(label: &str) -> u64 {
    let client = EngineClient::new(config());
    let info = client
        .info()
        .unwrap_or_else(|e| panic!("{label} failed: {e:?}"));
    for layer in [
        "restricted-token",
        "low-integrity",
        "job",
        "mitigations",
        "appcontainer",
        "no-privileges",
        "own-desktop",
    ] {
        assert!(info.sandbox.contains(layer), "{label}: {info:?}");
    }
    // The block is proved or the greeting says it is not; one of the two, never neither.
    assert!(
        info.sandbox.contains("no-network") || info.sandbox.contains("network-not-proved"),
        "{label}: {info:?}"
    );
    client.start_retries()
}

#[test]
fn the_engine_starts_every_time_with_all_its_layers() {
    let retries: u64 = (0..100)
        .map(|run| start_once(&format!("start {run}")))
        .sum();
    println!("starts: 100, started again after a first failure: {retries}");
    assert_eq!(retries, 0, "{retries} of 100 starts failed the first time");
}

#[test]
fn engines_that_start_together_all_start() {
    let handles: Vec<_> = (0..8)
        .map(|n| {
            std::thread::spawn(move || {
                (0..10)
                    .map(|run| start_once(&format!("thread {n} start {run}")))
                    .sum::<u64>()
            })
        })
        .collect();
    let retries: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
    println!("starts: 80, started again after a first failure: {retries}");
    assert_eq!(retries, 0, "{retries} of 80 starts failed the first time");
}
