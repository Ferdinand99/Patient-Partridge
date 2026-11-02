//! The signature check with the real engine process, the real PDFium and the sandbox: the CMS
//! parser and the signature arithmetic run in the process that is locked in, and the answer goes
//! through the client, which treats the engine as hostile.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use partridge_engine_client::{
    ClientError, EngineClient, EngineConfig, EnginePool, SandboxPolicy, SignatureReason,
    SignatureState,
};
use partridge_testkit as kit;
use partridge_testkit::signed::*;

fn config() -> EngineConfig {
    let mut config = EngineConfig::new(env!("CARGO_BIN_EXE_partridge-engine"));
    config.args = vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
    // The tests insist on the sandbox: an engine that cannot lock itself in fails them.
    config.sandbox = SandboxPolicy::Required;
    config
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "partridge-sigproc-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn signer(kind: KeyKind) -> Identity {
    Identity::self_signed(kind, &CertSpec::new("Test Signer"))
}

#[test]
fn every_kind_of_signature_is_checked_inside_the_sandbox() {
    let dir = Dir::new("kinds");
    let client = EngineClient::new(config());
    let rsa = shared_rsa();
    let p384 = signer(KeyKind::P384);
    let ed = signer(KeyKind::Ed25519);
    let p256 = signer(KeyKind::P256);
    let cases: Vec<(&str, &Identity, Hash, &str, &str)> = vec![
        (
            "p256.pdf",
            &p256,
            Hash::Sha256,
            "ECDSA P-256",
            "Test Signer",
        ),
        (
            "p384.pdf",
            &p384,
            Hash::Sha384,
            "ECDSA P-384",
            "Test Signer",
        ),
        ("ed.pdf", &ed, Hash::Sha512, "Ed25519", "Test Signer"),
        ("rsa.pdf", rsa, Hash::Sha256, "RSA 2048 bit", "RSA Signer"),
    ];
    for (name, key, hash, text, who) in cases {
        let options = SignOptions {
            cms: CmsOptions {
                hash,
                ..CmsOptions::default()
            },
            ..SignOptions::default()
        };
        let file = signed_file(key, &options);
        let path = dir.write(name, &file.bytes);
        let opened = client.open(&path, None).unwrap();
        let report = client.signatures(opened.doc, NOW).unwrap();
        assert_eq!(
            (report.total, report.unlisted, report.trust_anchors),
            (1, 0, 0),
            "{name}"
        );
        let info = &report.items[0];
        assert_eq!(info.state, SignatureState::UnknownSigner, "{name}");
        assert_eq!(info.reason, SignatureReason::NoTrustList, "{name}");
        assert_eq!(info.key.as_deref(), Some(text));
        assert_eq!(info.signer.as_deref(), Some(who));
        client.close(opened.doc).unwrap();
    }
}

#[test]
fn a_changed_file_and_a_second_signature_are_told_through_the_pool() {
    let dir = Dir::new("pool");
    let pool = EnginePool::new(config(), 4);
    let first = signer(KeyKind::P256);
    let second = Identity::self_signed(KeyKind::P256, &CertSpec::new("Second Signer"));
    let one = signed_file(&first, &SignOptions::default());
    let two = add_signature(&one, &second, &SignOptions::default());
    let edited = change_text_revision(&two.bytes, "Pay 900 to Mallory");

    let (handle, _) = pool.open(&dir.write("two.pdf", &two.bytes), None).unwrap();
    let report = pool.signatures(handle, NOW).unwrap();
    assert_eq!(report.items.len(), 2);
    let states: Vec<_> = report
        .items
        .iter()
        .map(|i| (i.signer.clone().unwrap(), i.state))
        .collect();
    assert!(states.contains(&("Test Signer".to_string(), SignatureState::Changed)));
    assert!(states.contains(&("Second Signer".to_string(), SignatureState::UnknownSigner)));

    let (handle, _) = pool.open(&dir.write("edited.pdf", &edited), None).unwrap();
    let report = pool.signatures(handle, NOW).unwrap();
    assert!(report
        .items
        .iter()
        .all(|i| i.state == SignatureState::Changed));
    assert_eq!(report.items.len(), 2);
}

#[test]
fn hostile_files_do_not_stop_the_engine_or_get_a_tick() {
    let dir = Dir::new("hostile");
    let client = EngineClient::new(config());
    let good = signed_file(&signer(KeyKind::P256), &SignOptions::default());
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    // The signature data cut off and damaged at several places, and a made-up byte range.
    for cut in [10usize, 100, 200, 300] {
        let mut bytes = good.bytes.clone();
        for b in &mut bytes[good.contents_at + 1 + cut * 2..good.contents_end - 1] {
            *b = b'0';
        }
        files.push((format!("cut{cut}.pdf"), bytes));
    }
    for (i, range) in [
        [0u64, 1, 2, 3],
        [0, 0, 0, 0],
        [9_999_999_999, 9_999_999_999, 9_999_999_999, 9_999_999_999],
        [0, 5, 9_999, 5],
    ]
    .into_iter()
    .enumerate()
    {
        let mut bytes = good.bytes.clone();
        set_byte_range(&mut bytes, range);
        files.push((format!("range{i}.pdf"), bytes));
    }
    files.push(("fake.pdf".into(), kit::signed_pdf()));
    files.push(("nested.pdf".into(), kit::signed_nested_pdf()));
    files.push(("nofield.pdf".into(), kit::signed_without_field_pdf()));
    files.push(("many.pdf".into(), fake_signatures_pdf(80)));
    for (name, bytes) in files {
        let path = dir.write(&name, &bytes);
        let opened = client.open(&path, None).unwrap();
        // The engine answers (or says why it cannot) and stays up for the next question.
        match client.signatures(opened.doc, NOW) {
            Ok(report) => {
                for item in &report.items {
                    assert!(
                        !matches!(
                            item.state,
                            SignatureState::Trusted | SignatureState::UnknownSigner
                        ),
                        "{name}: {:?}/{:?}",
                        item.state,
                        item.reason
                    );
                }
            }
            Err(ClientError::Engine { .. }) => {}
            Err(other) => panic!("{name}: {other:?}"),
        }
        assert!(client.render(opened.doc, 0, 100, 130).is_ok(), "{name}");
        client.close(opened.doc).unwrap();
    }
}

#[test]
fn a_file_that_is_not_signed_has_no_signatures() {
    let dir = Dir::new("plain");
    let client = EngineClient::new(config());
    let opened = client
        .open(&dir.write("a.pdf", &kit::simple_pdf(2)), None)
        .unwrap();
    let report = client.signatures(opened.doc, NOW).unwrap();
    assert_eq!(
        (report.total, report.unlisted, report.items.len()),
        (0, 0, 0)
    );
}
