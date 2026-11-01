//! Diagnosis aid, run by hand or by CI on Windows only: starts the real engine with parts of the
//! restrictions left out, one at a time, and prints which combinations let the engine start. It
//! only prints. (The switches exist in debug builds only.)

#![cfg(windows)]

use partridge_engine_client::{EngineClient, EngineConfig, SandboxPolicy};
use partridge_testkit as kit;

#[test]
#[ignore = "diagnosis, run explicitly"]
fn which_restrictions_stop_the_engine_from_starting() {
    let file = kit::TempFile::new("matrix.pdf", &kit::simple_pdf(1));
    for skip in [
        "",
        "win32k",
        "mitigations",
        "ui",
        "child",
        "nowindow",
        "starttoken",
        "restricting",
        "win32k,ui",
        "win32k,mitigations,ui,child",
        "win32k,mitigations,ui,child,nowindow",
    ] {
        // SAFETY: this test is the only one in its program.
        unsafe { std::env::set_var("PARTRIDGE_TEST_WINDOWS_SKIP", skip) };
        let mut config = EngineConfig::new(env!("CARGO_BIN_EXE_partridge-engine"));
        config.args = vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
        config.sandbox = SandboxPolicy::Required;
        let client = EngineClient::new(config);
        let result = client.open(&file.path_str(), None).map(|d| {
            format!(
                "opened, {} pages, sandbox {}",
                d.pages.len(),
                client.info().map(|i| i.sandbox).unwrap_or_default()
            )
        });
        println!("skip [{skip}]: {result:?}");
    }
}
