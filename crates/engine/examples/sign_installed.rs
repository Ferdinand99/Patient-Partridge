//! Signs a PDF with the engine and the signer of an installed copy of the app, each in its sandbox,
//! and checks the signed file with a second engine of that copy. Used by the Windows checks
//! (scripts/windows-standard-user, scripts/windows-beta) to prove that the programs an installer
//! put in a folder start there, as the user who runs them, and that the folder's access list lets
//! the sandboxed programs read them.
//!
//! ```text
//! sign_installed <install folder> <input.pdf> <certificate.p12> <password> <output.pdf>
//! ```
//!
//! The install folder holds `partridge-engine`, `partridge-signer` and the PDFium library, as the
//! installer lays them out. Exit code 0 only when the signed file holds one signature that is
//! intact. Not part of the app.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use partridge_engine_client::{
    EngineClient, EngineConfig, EnginePool, SandboxPolicy, SaveOptions, SignOptions, SignPlace,
    SignatureState, SignerConfig, SignerSession,
};

/// 2026-06-15 12:00:00 UTC, the time the tests of signing use.
const NOW: i64 = 1_781_524_800;

fn engine_config(folder: &Path) -> EngineConfig {
    let program = folder.join(format!("partridge-engine{}", std::env::consts::EXE_SUFFIX));
    let mut config = EngineConfig::new(program);
    config.args = vec!["--pdfium-dir".into(), folder.as_os_str().to_owned()];
    // A copy that cannot lock the engine in must fail here, not go on without a sandbox.
    config.sandbox = SandboxPolicy::Required;
    config
}

fn run(args: &[String]) -> Result<(), String> {
    let folder = PathBuf::from(&args[0]);
    let input = &args[1];
    let p12 = std::fs::read(&args[2]).map_err(|e| format!("cannot read {}: {e}", args[2]))?;
    let password = &args[3];
    let output = PathBuf::from(&args[4]);

    let mut signer_config = SignerConfig::new(
        folder.join(format!("partridge-signer{}", std::env::consts::EXE_SUFFIX)),
    );
    signer_config.sandbox = SandboxPolicy::Required;
    let mut signer = SignerSession::start(&signer_config)
        .map_err(|e| format!("the signer did not start: {e:?}"))?;
    println!("signer sandbox: {}", signer.sandbox());
    let certificate = signer
        .load(&p12, password)
        .map_err(|e| format!("the certificate file did not open: {e:?}"))?;
    println!("certificate: {}", certificate.name);

    let pool = EnginePool::new(engine_config(&folder), 2);
    let (handle, pages) = pool
        .open(input, None)
        .map_err(|e| format!("the engine did not open the PDF: {e:?}"))?;
    println!("opened {input}: {} page(s)", pages.len());
    let options = SignOptions {
        place: SignPlace::Rect {
            page: 0,
            rect: [72.0, 600.0, 272.0, 660.0],
        },
        reason: "Installed copy check".into(),
        location: "Oslo".into(),
        mark: None,
        color: [10, 40, 120, 255],
        signed_at: NOW,
    };
    let outcome = pool
        .sign_as(handle, &output, &mut signer, &options, SaveOptions::default())
        .map_err(|e| format!("signing failed: {e:?}"))?;
    println!(
        "signed: {} ({} bytes, field {}, certificate {})",
        outcome.path.display(),
        outcome.bytes,
        outcome.field,
        outcome.certificate.name
    );
    signer.close();
    pool.kill_all();

    // A second engine of the same copy reads the signed file.
    let client = EngineClient::new(engine_config(&folder));
    let opened = client
        .open(&output.to_string_lossy(), None)
        .map_err(|e| format!("the engine did not open the signed file: {e:?}"))?;
    let report = client
        .signatures(opened.doc, NOW)
        .map_err(|e| format!("the signatures could not be read: {e:?}"))?;
    client.kill();
    println!(
        "signatures in the signed file: {} (not listed: {})",
        report.total, report.unlisted
    );
    if report.total != 1 || report.unlisted != 0 {
        return Err("the signed file does not hold exactly one signature".into());
    }
    let item = &report.items[0];
    // The test certificate has no trusted root, so the best state is "signature holds, signer unknown".
    if item.state != SignatureState::UnknownSigner {
        return Err(format!("the signature is not intact: {:?}", item.state));
    }
    println!("signature state: {:?}", item.state);
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 5 {
        eprintln!(
            "usage: sign_installed <install folder> <input.pdf> <certificate.p12> <password> <output.pdf>"
        );
        return ExitCode::from(2);
    }
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("FAILED: {why}");
            ExitCode::FAILURE
        }
    }
}
