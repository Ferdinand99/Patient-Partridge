//! The engine on Windows starts in a locked-down AppContainer. A process with such a token cannot
//! let the system's loader hand the loading of libraries to helper threads (only the first thread
//! has the identity that may open files), so the app starts the engine on one processor and gives
//! the others back after the greeting. Without that, about nine starts in ten end with "library
//! not found". These tests start the real engine many times, so a start that fails at random
//! fails here too.

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
fn the_engine_starts_every_time_with_all_its_layers() {
    for run in 0..100 {
        // A new client every time: a new engine process every time.
        let info = EngineClient::new(config())
            .info()
            .unwrap_or_else(|e| panic!("start {run} failed: {e:?}"));
        for layer in [
            "restricted-token",
            "low-integrity",
            "job",
            "mitigations",
            "appcontainer",
            "no-network",
            "own-desktop",
        ] {
            assert!(info.sandbox.contains(layer), "start {run}: {info:?}");
        }
    }
}

#[test]
fn engines_that_start_together_all_start() {
    let handles: Vec<_> = (0..8)
        .map(|n| {
            std::thread::spawn(move || {
                for run in 0..10 {
                    let info = EngineClient::new(config())
                        .info()
                        .unwrap_or_else(|e| panic!("thread {n} start {run} failed: {e:?}"));
                    assert!(info.sandbox.contains("appcontainer"), "{info:?}");
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
}
