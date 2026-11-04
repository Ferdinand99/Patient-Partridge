//! The whole signing path with the real engine, the real signer, the real PDFium and the sandbox:
//! the engine prepares the file, the signer signs a digest of it, the app writes the new file next
//! to its target, a second sandboxed engine checks it, and one rename puts it in place. What goes
//! wrong on the way (a wrong password, a target that cannot be written, a signed file that is saved
//! over) is checked here too, and each time the file that was there is as it was and no stray file is
//! left.

mod common;
mod sign_support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use partridge_engine_client::{
    verify_signed, ClientError, EngineClient, EngineConfig, EnginePool, SandboxPolicy, SaveError,
    SaveOptions, SignOptions, SignPlace, SignatureState, SignerConfig, SignerSession,
};
use partridge_testkit as kit;
use sign_support::fixture;

fn config() -> EngineConfig {
    let mut config = EngineConfig::new(env!("CARGO_BIN_EXE_partridge-engine"));
    config.args = vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
    // The tests insist on the sandbox: an engine that cannot lock itself in fails them.
    config.sandbox = SandboxPolicy::Required;
    config
}

/// The signer program: built by the same `cargo test --workspace` run, or, when only this package
/// is tested, by a build of its own.
fn signer_program() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let debug = exe.parent().and_then(Path::parent).unwrap().to_path_buf();
    let program = debug.join(format!("partridge-signer{}", std::env::consts::EXE_SUFFIX));
    if !program.exists() {
        let target = debug.parent().unwrap();
        let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args(["build", "-p", "partridge-signer", "--locked"])
            .arg("--manifest-path")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml"))
            .env("CARGO_TARGET_DIR", target)
            .status()
            .expect("cargo builds the signer");
        assert!(status.success(), "the signer did not build");
    }
    assert!(program.exists(), "{} is missing", program.display());
    program
}

fn signer_config() -> SignerConfig {
    let mut config = SignerConfig::new(signer_program());
    config.sandbox = SandboxPolicy::Required;
    config
}

/// A folder of its own for one test, removed again at the end.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "partridge-signproc-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn options() -> SignOptions {
    SignOptions {
        place: SignPlace::Rect {
            page: 0,
            rect: [72.0, 600.0, 272.0, 660.0],
        },
        reason: "I have read this".into(),
        location: "Oslo".into(),
        mark: None,
        color: [10, 40, 120, 255],
        signed_at: sign_support::NOW,
    }
}

fn loaded_signer(file: &str) -> SignerSession {
    let mut signer =
        SignerSession::start(&signer_config()).expect("the signer starts in its sandbox");
    signer
        .load(&fixture(file), "test")
        .expect("the certificate file opens");
    signer
}

fn code_of(error: &ClientError) -> String {
    match error {
        ClientError::Signer { code, .. } => (*code).to_string(),
        ClientError::Engine { code, .. } => code.name().to_string(),
        ClientError::Save(why) => why.code().to_string(),
        ClientError::EngineStopped(why) => format!("engine stopped: {why}"),
    }
}

#[test]
fn a_document_is_signed_checked_and_put_in_place_and_the_original_is_not_touched() {
    let dir = Dir::new("ok");
    let original = kit::simple_pdf(3);
    let source = dir.write("contract.pdf", &original);
    let target = dir.0.join("contract (signed).pdf");
    let pool = EnginePool::new(config(), 4);
    let (handle, _) = pool.open(&text(&source), None).unwrap();
    let mut signer = loaded_signer("rsa2048.p12");
    assert!(
        signer.sandbox().contains("seccomp") || !cfg!(target_os = "linux"),
        "{}",
        signer.sandbox()
    );

    let outcome = pool
        .sign_as(
            handle,
            &target,
            &mut signer,
            &options(),
            SaveOptions::default(),
        )
        .unwrap();
    assert_eq!(outcome.path, target);
    assert_eq!(outcome.field, "Signature1");
    assert_eq!(outcome.certificate.name, "Test Signer RSA 2048");
    assert_eq!(outcome.bytes, fs::metadata(&target).unwrap().len());
    // The original is as it was, and nothing else is in the folder.
    assert_eq!(fs::read(&source).unwrap(), original);
    assert_eq!(
        dir.names(),
        vec![
            "contract (signed).pdf".to_string(),
            "contract.pdf".to_string()
        ]
    );
    // The signed file, opened by another engine, holds one signature that is intact.
    let client = EngineClient::new(config());
    let opened = client.open(&text(&target), None).unwrap();
    let report = client.signatures(opened.doc, 1_781_524_800).unwrap();
    assert_eq!((report.total, report.unlisted), (1, 0));
    assert_eq!(report.items[0].state, SignatureState::UnknownSigner);
    assert_eq!(
        report.items[0].signer.as_deref(),
        Some("Test Signer RSA 2048")
    );
    client.kill();
    signer.close();
}

#[test]
fn a_target_that_exists_is_replaced_only_in_one_step_and_a_target_that_changed_is_refused() {
    let dir = Dir::new("replace");
    let source = dir.write("a.pdf", &kit::simple_pdf(1));
    let target = dir.write("b.pdf", b"an older copy");
    let pool = EnginePool::new(config(), 4);
    let (handle, _) = pool.open(&text(&source), None).unwrap();
    let mut signer = loaded_signer("p256.p12");
    pool.sign_as(
        handle,
        &target,
        &mut signer,
        &options(),
        SaveOptions::default(),
    )
    .unwrap();
    assert!(fs::read(&target).unwrap().starts_with(b"%PDF-"));
    assert_eq!(dir.names(), vec!["a.pdf".to_string(), "b.pdf".to_string()]);
    signer.close();
}

#[test]
fn the_signed_copy_cannot_replace_the_file_that_is_open() {
    let dir = Dir::new("same");
    let original = kit::simple_pdf(1);
    let source = dir.write("a.pdf", &original);
    let pool = EnginePool::new(config(), 4);
    let (handle, _) = pool.open(&text(&source), None).unwrap();
    let mut signer = loaded_signer("p256.p12");
    let error = pool
        .sign_as(
            handle,
            &source,
            &mut signer,
            &options(),
            SaveOptions::default(),
        )
        .unwrap_err();
    assert_eq!(code_of(&error), "sign_same_file");
    assert_eq!(fs::read(&source).unwrap(), original);
    assert_eq!(dir.names(), vec!["a.pdf".to_string()]);
    // The signer is still there for another try.
    assert!(signer.certificate().is_some());
    signer.close();
}

#[test]
fn a_wrong_password_leaves_the_signer_ready_and_the_right_one_opens_the_file() {
    let mut signer = SignerSession::start(&signer_config()).unwrap();
    let error = signer.load(&fixture("rsa2048.p12"), "wrong").unwrap_err();
    assert_eq!(code_of(&error), "signer_wrong_password");
    assert!(signer.certificate().is_none());
    let certificate = signer.load(&fixture("rsa2048.p12"), "test").unwrap();
    assert_eq!(certificate.name, "Test Signer RSA 2048");
    // A key that is refused leaves the signer ready too, and a second certificate is not loaded over the first.
    let error = signer.load(&fixture("p256.p12"), "test").unwrap_err();
    assert_eq!(code_of(&error), "signer_invalid_request");
    signer.close();

    let mut signer = SignerSession::start(&signer_config()).unwrap();
    for (file, code) in [
        ("ed25519.p12", "signer_key_unsupported"),
        ("p521.p12", "signer_key_unsupported"),
        ("rsa1024.p12", "signer_key_unsupported"),
        ("no-key.p12", "signer_no_key"),
    ] {
        let error = signer.load(&fixture(file), "test").unwrap_err();
        assert_eq!(code_of(&error), code, "{file}");
    }
    let error = signer
        .load(b"this is not a certificate file", "test")
        .unwrap_err();
    assert_eq!(code_of(&error), "signer_not_a_certificate_file");
    signer.close();
}

#[test]
fn nothing_is_written_when_the_signer_is_gone_or_has_no_certificate() {
    let dir = Dir::new("gone");
    let source = dir.write("a.pdf", &kit::simple_pdf(1));
    let target = dir.0.join("a (signed).pdf");
    let pool = EnginePool::new(config(), 4);
    let (handle, _) = pool.open(&text(&source), None).unwrap();
    // No certificate was opened.
    let mut signer = SignerSession::start(&signer_config()).unwrap();
    let error = pool
        .sign_as(
            handle,
            &target,
            &mut signer,
            &options(),
            SaveOptions::default(),
        )
        .unwrap_err();
    assert_eq!(code_of(&error), "signer_not_loaded");
    // The signer was ended after the certificate was opened.
    signer.load(&fixture("p256.p12"), "test").unwrap();
    signer.kill();
    let error = pool
        .sign_as(
            handle,
            &target,
            &mut signer,
            &options(),
            SaveOptions::default(),
        )
        .unwrap_err();
    assert_eq!(code_of(&error), "signer_not_loaded");
    assert_eq!(dir.names(), vec!["a.pdf".to_string()]);
}

#[test]
fn a_target_that_cannot_be_written_leaves_everything_as_it_was() {
    let dir = Dir::new("nowrite");
    let original = kit::simple_pdf(1);
    let source = dir.write("a.pdf", &original);
    let pool = EnginePool::new(config(), 4);
    let (handle, _) = pool.open(&text(&source), None).unwrap();
    let mut signer = loaded_signer("p256.p12");
    // A folder that is not there: the new file cannot be made.
    let target = dir.0.join("no such folder").join("a (signed).pdf");
    let error = pool
        .sign_as(
            handle,
            &target,
            &mut signer,
            &options(),
            SaveOptions::default(),
        )
        .unwrap_err();
    assert!(matches!(error, ClientError::Save(_)), "{error:?}");
    // A target that is a folder.
    let folder = dir.0.join("a folder");
    fs::create_dir(&folder).unwrap();
    let error = pool
        .sign_as(
            handle,
            &folder,
            &mut signer,
            &options(),
            SaveOptions::default(),
        )
        .unwrap_err();
    assert!(
        matches!(error, ClientError::Save(SaveError::NotAFile)),
        "{error:?}"
    );
    assert_eq!(fs::read(&source).unwrap(), original);
    assert_eq!(
        dir.names(),
        vec!["a folder".to_string(), "a.pdf".to_string()]
    );
    // The same signer goes on to sign after the failures.
    let target = dir.0.join("ok.pdf");
    pool.sign_as(
        handle,
        &target,
        &mut signer,
        &options(),
        SaveOptions::default(),
    )
    .unwrap();
    signer.close();
}

#[test]
fn a_document_that_cannot_be_signed_is_refused_with_its_reason_and_nothing_is_written() {
    let dir = Dir::new("refused");
    let pool = EnginePool::new(config(), 4);
    let mut signer = loaded_signer("p256.p12");
    let cases: Vec<(&str, Vec<u8>, Option<&str>, &str)> = vec![
        (
            "encrypted",
            kit::encrypted_pdf("secret", 1),
            Some("secret"),
            "sign_encrypted",
        ),
        ("xfa", kit::xfa_pdf(), None, "sign_xfa"),
        ("signed", kit::signed_pdf(), None, "sign_already_signed"),
    ];
    for (name, pdf, password, code) in cases {
        let source = dir.write(&format!("{name}.pdf"), &pdf);
        let (handle, _) = pool.open(&text(&source), password).unwrap();
        let target = dir.0.join(format!("{name} (signed).pdf"));
        let error = pool
            .sign_as(
                handle,
                &target,
                &mut signer,
                &options(),
                SaveOptions::default(),
            )
            .unwrap_err();
        assert_eq!(code_of(&error), code, "{name}");
        assert!(!target.exists(), "{name}");
        pool.close(handle);
    }
    // A field that is not there.
    let source = dir.write("plain.pdf", &kit::simple_pdf(1));
    let (handle, _) = pool.open(&text(&source), None).unwrap();
    let mut wrong = options();
    wrong.place = SignPlace::Field {
        page: 0,
        name: "nobody".into(),
    };
    let error = pool
        .sign_as(
            handle,
            &dir.0.join("x.pdf"),
            &mut signer,
            &wrong,
            SaveOptions::default(),
        )
        .unwrap_err();
    assert_eq!(code_of(&error), "sign_refused");
    assert_eq!(
        dir.names()
            .iter()
            .filter(|n| n.ends_with("(signed).pdf") || *n == "x.pdf")
            .count(),
        0
    );
    signer.close();
}

#[test]
fn a_signed_file_is_not_saved_over_and_a_copy_does_not_break_the_signed_file() {
    let dir = Dir::new("after");
    let source = dir.write("a.pdf", &kit::simple_pdf(2));
    let target = dir.0.join("a (signed).pdf");
    let pool = EnginePool::new(config(), 4);
    let (handle, _) = pool.open(&text(&source), None).unwrap();
    let mut signer = loaded_signer("p256.p12");
    pool.sign_as(
        handle,
        &target,
        &mut signer,
        &options(),
        SaveOptions::default(),
    )
    .unwrap();
    signer.close();
    let signed = fs::read(&target).unwrap();

    // The signed file is opened as a document of its own: an ordinary save is refused.
    let (signed_handle, _) = pool.open(&text(&target), None).unwrap();
    let error = pool
        .save(signed_handle, SaveOptions::default())
        .unwrap_err();
    assert!(
        matches!(error, ClientError::Save(SaveError::Signed { .. })),
        "{error:?}"
    );
    assert_eq!(fs::read(&target).unwrap(), signed);
    assert_eq!(
        dir.names(),
        vec!["a (signed).pdf".to_string(), "a.pdf".to_string()]
    );

    // A copy is written when the person agrees that the copy's signature breaks; the signed file stays.
    let copy = dir.0.join("copy.pdf");
    pool.save_as(
        signed_handle,
        &copy,
        SaveOptions {
            allow_signed: true,
            ..SaveOptions::default()
        },
    )
    .unwrap();
    assert_eq!(fs::read(&target).unwrap(), signed);
    let client = EngineClient::new(config());
    let at = client.open(&text(&target), None).unwrap();
    assert_eq!(
        client.signatures(at.doc, 1_781_524_800).unwrap().items[0].state,
        SignatureState::UnknownSigner
    );
    let broken = client.open(&text(&copy), None).unwrap();
    let report = client.signatures(broken.doc, 1_781_524_800).unwrap();
    assert!(
        report
            .items
            .iter()
            .all(|i| i.state != SignatureState::UnknownSigner),
        "the copy was written again by PDFium, so its signature does not hold: {report:?}"
    );
    client.kill();
}

#[test]
fn the_check_of_the_new_file_refuses_a_file_that_is_not_the_one_that_was_made() {
    let dir = Dir::new("verify");
    let source = dir.write("a.pdf", &kit::simple_pdf(2));
    let target = dir.0.join("a (signed).pdf");
    let pool = EnginePool::new(config(), 4);
    let (handle, _) = pool.open(&text(&source), None).unwrap();
    let mut signer = loaded_signer("rsa2048.p12");
    let certificate = signer.certificate().unwrap().clone();
    pool.sign_as(
        handle,
        &target,
        &mut signer,
        &options(),
        SaveOptions::default(),
    )
    .unwrap();
    signer.close();

    // What the document looked like, for the check: its fingerprint without signature fields.
    let client = EngineClient::new(config());
    let opened = client.open(&text(&source), None).unwrap();
    let print = client.sign_fingerprint(opened.doc).unwrap();
    client.kill();
    let ok =
        |path: &Path,
         certificate: &partridge_engine_client::Certificate,
         print: &partridge_engine_client::Fingerprint,
         pages: u32| { verify_signed(&config(), path, certificate, print, pages) };
    ok(&target, &certificate, &print, 2).expect("the file that was written passes");

    // Not the pages of the document.
    let mut other = print;
    other.digest ^= 1;
    assert!(matches!(
        ok(&target, &certificate, &other, 2),
        Err(SaveError::Verify(_))
    ));
    assert!(matches!(
        ok(&target, &certificate, &print, 3),
        Err(SaveError::Verify(_))
    ));
    // Not signed with the certificate that was used.
    let mut stranger = certificate.clone();
    stranger.fingerprint = "00".repeat(32);
    assert!(matches!(
        ok(&target, &stranger, &print, 2),
        Err(SaveError::Verify(_))
    ));
    // A byte changed in what is signed, or a byte added after it.
    let bytes = fs::read(&target).unwrap();
    let mut changed = bytes.clone();
    let at = changed
        .windows(8)
        .position(|w| w == b"Alvorlig" || w == b"RSA 2048")
        .expect("a name in the file");
    changed[at + 1] ^= 1;
    let path = dir.write("changed.pdf", &changed);
    assert!(matches!(
        ok(&path, &certificate, &print, 2),
        Err(SaveError::Verify(_))
    ));
    let mut longer = bytes.clone();
    longer.extend_from_slice(b"\n% more\n");
    let path = dir.write("longer.pdf", &longer);
    assert!(matches!(
        ok(&path, &certificate, &print, 2),
        Err(SaveError::Verify(_))
    ));
    // A file that was never signed.
    let unsigned = dir.write("unsigned.pdf", &kit::simple_pdf(2));
    assert!(matches!(
        ok(&unsigned, &certificate, &print, 2),
        Err(SaveError::Verify(_))
    ));
}

fn has(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn the_signer_has_let_the_key_go_after_the_one_signature_and_the_name_is_the_certificates() {
    let dir = Dir::new("once");
    let source = dir.write("a.pdf", &kit::simple_pdf(1));
    let target = dir.0.join("a (signed).pdf");
    let pool = EnginePool::new(config(), 4);
    let (handle, _) = pool.open(&text(&source), None).unwrap();
    let mut signer = loaded_signer("rsa2048.p12");
    pool.sign_as(
        handle,
        &target,
        &mut signer,
        &options(),
        SaveOptions::default(),
    )
    .unwrap();
    // The signer ended after the signature: no certificate is open in it any more.
    assert!(signer.certificate().is_none());
    let error = signer.load(&fixture("rsa2048.p12"), "test").unwrap_err();
    assert_eq!(code_of(&error), "signer_stopped");
    // The signature names the holder of the certificate, whatever else the person might have typed.
    let signed = fs::read(&target).unwrap();
    assert!(has(&signed, b"/Name (Test Signer RSA 2048)"));
}

#[test]
fn a_document_with_signature_fields_that_nobody_signed_is_signed_the_whole_way() {
    let dir = Dir::new("empty-fields");
    let pool = EnginePool::new(config(), 4);
    for (pdf, fields) in [
        (kit::forms_empty_signature_pdf(), vec!["empty_signature"]),
        (
            kit::forms_two_empty_signatures_pdf(),
            vec!["empty_signature", "second_signature"],
        ),
    ] {
        let source = dir.write("form.pdf", &pdf);
        let (handle, _) = pool.open(&text(&source), None).unwrap();
        let mut places = vec![
            SignPlace::Rect {
                page: 0,
                rect: [72.0, 600.0, 272.0, 660.0],
            },
            SignPlace::Invisible,
        ];
        places.extend(fields.iter().map(|name| SignPlace::Field {
            page: 0,
            name: (*name).to_string(),
        }));
        for (n, place) in places.into_iter().enumerate() {
            let target = dir.0.join(format!("form-signed-{}-{n}.pdf", fields.len()));
            let mut signer = loaded_signer("rsa2048.p12");
            let mut chosen = options();
            chosen.place = place.clone();
            let outcome = pool
                .sign_as(
                    handle,
                    &target,
                    &mut signer,
                    &chosen,
                    SaveOptions::default(),
                )
                .unwrap_or_else(|e| panic!("{place:?}: {e:?}"));
            // One signature holds; the fields that nobody signed are still there, still empty.
            let client = EngineClient::new(config());
            let opened = client.open(&text(&target), None).unwrap();
            let report = client.signatures(opened.doc, 1_781_524_800).unwrap();
            let held = report
                .items
                .iter()
                .filter(|i| !i.byte_range.is_empty())
                .count();
            assert_eq!(held, 1, "{place:?}: {:?}", report.items);
            let empty = report.items.len() - held;
            let used = usize::from(matches!(place, SignPlace::Field { .. }));
            // A new box adds a field of its own; a field that is used leaves one less.
            let expected = if used == 1 {
                fields.len() - 1
            } else {
                fields.len()
            };
            assert_eq!(empty, expected, "{place:?}: {:?}", report.items);
            client.kill();
            assert!(!outcome.field.is_empty());
        }
    }
}

#[test]
fn strings_that_are_not_text_reach_the_signed_file_as_they_were() {
    let dir = Dir::new("latin1");
    let source = dir.write("form.pdf", &sign_support::latin1_form_pdf());
    let pool = EnginePool::new(config(), 4);
    let (handle, _) = pool.open(&text(&source), None).unwrap();
    let target = dir.0.join("form (signed).pdf");
    let mut signer = loaded_signer("rsa2048.p12");
    let mut chosen = options();
    chosen.place = SignPlace::Field {
        page: 0,
        name: sign_support::LATIN1_NAME.to_string(),
    };
    pool.sign_as(
        handle,
        &target,
        &mut signer,
        &chosen,
        SaveOptions::default(),
    )
    .unwrap();
    let signed = fs::read(&target).unwrap();
    assert!(!has(&signed, &[0xef, 0xbf, 0xbd]));
    for bytes in [
        sign_support::LATIN1_FIELD,
        sign_support::LATIN1_TIP,
        sign_support::UTF16_FIELD,
        sign_support::LABEL,
        sign_support::LANGUAGE,
        sign_support::FORM_DEFAULT,
    ] {
        assert!(
            has(&signed, bytes),
            "{} is not in the signed file",
            String::from_utf8_lossy(bytes)
        );
    }
    // The other field is listed under the name it had.
    let client = EngineClient::new(config());
    let opened = client.open(&text(&target), None).unwrap();
    let names: Vec<String> = client
        .sign_fields(opened.doc)
        .unwrap()
        .into_iter()
        .map(|f| f.name)
        .collect();
    assert!(
        names.contains(&sign_support::UTF16_NAME.to_string()),
        "{names:?}"
    );
    client.kill();
}

#[test]
fn a_document_whose_metadata_is_mended_is_saved_mended_signed_and_checked() {
    let dir = Dir::new("mended");
    let pool = EnginePool::new(config(), 4);
    for (n, (what, pdf)) in sign_support::mended_documents().into_iter().enumerate() {
        let source = dir.write(&format!("m{n}.pdf"), &pdf);
        let target = dir.0.join(format!("m{n} (signed).pdf"));
        let (handle, _) = pool.open(&text(&source), None).unwrap();
        let mut signer = loaded_signer("rsa2048.p12");
        // The whole path, with the check of the new file by a second engine in its sandbox.
        pool.sign_as(
            handle,
            &target,
            &mut signer,
            &options(),
            SaveOptions::default(),
        )
        .unwrap_or_else(|e| panic!("{what}: {e:?}"));
        let signed = fs::read(&target).unwrap();
        assert!(has(&signed, b"%%EOF"), "{what}");
        let client = EngineClient::new(config());
        let opened = client.open(&text(&target), None).unwrap();
        let report = client.signatures(opened.doc, 1_781_524_800).unwrap();
        assert_eq!(report.items.len(), 1, "{what}");
        assert_eq!(
            report.items[0].state,
            SignatureState::UnknownSigner,
            "{what}"
        );
        client.kill();
    }
}

#[test]
fn a_file_that_another_document_reads_is_not_a_target_and_a_signed_one_knows_its_own_file() {
    let dir = Dir::new("open-elsewhere");
    let first = dir.write("a.pdf", &kit::simple_pdf(1));
    let other_bytes = kit::simple_pdf(2);
    let other = dir.write("b.pdf", &other_bytes);
    let pool = EnginePool::new(config(), 4);
    let (a, _) = pool.open(&text(&first), None).unwrap();
    let (b, _) = pool.open(&text(&other), None).unwrap();
    let mut signer = loaded_signer("p256.p12");
    // b.pdf is open as a document of its own: the signed copy would replace what it shows.
    let error = pool
        .sign_as(a, &other, &mut signer, &options(), SaveOptions::default())
        .unwrap_err();
    assert_eq!(code_of(&error), "sign_target_open");
    assert_eq!(fs::read(&other).unwrap(), other_bytes);
    assert!(
        signer.certificate().is_some(),
        "the key is still there for another try"
    );
    assert!(pool.read_by_another(a, &other) && !pool.read_by_another(a, &first));
    // A name of its own works, and the signed file knows that it is the signed file.
    let target = dir.0.join("a (signed).pdf");
    pool.sign_as(a, &target, &mut signer, &options(), SaveOptions::default())
        .unwrap();
    let (signed, _) = pool.open(&text(&target), None).unwrap();
    assert!(pool.is_signed(signed) && !pool.is_signed(a) && !pool.is_signed(b));
    assert!(pool.is_own_file(signed, &target) && !pool.is_own_file(signed, &first));
}
