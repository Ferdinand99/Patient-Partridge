//! The app opens the font files for each engine and closes them again: a run of engines leaves no
//! descriptor behind in the app. In a test program of its own, so that no other test opens or closes
//! a file while the descriptors are counted.

#![cfg(target_os = "linux")]

use partridge_engine_client::{EngineClient, EngineConfig, SandboxPolicy};
use partridge_testkit as kit;

fn config() -> EngineConfig {
    let mut config = EngineConfig::new(env!("CARGO_BIN_EXE_partridge-engine"));
    config.args = vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
    config.sandbox = SandboxPolicy::Required;
    config
}

/// The app opens the font files for each engine and closes them again: a run of engines leaves no
/// descriptor behind in the app.
#[test]
fn the_app_leaks_no_descriptors_when_it_starts_engines() {
    let open_descriptors = || std::fs::read_dir("/proc/self/fd").unwrap().count();
    let file = kit::TempFile::new("leak.pdf", &kit::simple_pdf(1));
    // One engine first, so that lazily opened things are open already.
    {
        let client = EngineClient::new(config());
        client.open(&file.path_str(), None).unwrap();
        client.kill();
    }
    let before = open_descriptors();
    for _ in 0..20 {
        let client = EngineClient::new(config());
        client.open(&file.path_str(), None).unwrap();
        client.kill();
    }
    let after = open_descriptors();
    assert!(
        after <= before + 2,
        "{before} descriptors before, {after} after"
    );
}
