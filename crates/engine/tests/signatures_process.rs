//! The signature check with the real engine process, the real PDFium and the sandbox: the CMS
//! parser and the signature arithmetic run in the process that is locked in, and the answer goes
//! through the client, which treats the engine as hostile.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use partridge_engine_client::{
    ClientError, EngineClient, EngineConfig, EnginePool, SandboxPolicy, SaveOptions,
    SignatureReason, SignatureState,
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

#[test]
fn a_verdict_is_never_given_for_bytes_it_was_not_made_from() {
    // Open a signed file, save a copy of it, and ask again. The engine goes on reading the file
    // the document was opened from, so an answer would be about that file while the tab names the
    // copy, and the copy is not that file: PDFium wrote the whole document again, and the
    // signature no longer covers it. The pool refuses to answer once the document was saved, and
    // the copy checked on its own is not intact.
    let dir = Dir::new("saved");
    let pool = EnginePool::new(config(), 4);
    let file = signed_file(&signer(KeyKind::P256), &SignOptions::default());
    let original = dir.write("signed.pdf", &file.bytes);
    let (handle, _) = pool.open(&original, None).unwrap();
    let before = pool.signatures(handle, NOW).unwrap();
    assert_eq!(before.items[0].state, SignatureState::UnknownSigner);

    let target = dir.0.join("copy.pdf");
    let options = SaveOptions {
        allow_signed: true,
        overwrite_changed: false,
    };
    pool.save_as(handle, &target, options).unwrap();
    match pool.signatures(handle, NOW) {
        Err(ClientError::Engine { code, .. }) => {
            assert_eq!(code, partridge_protocol::ErrorCode::SourceChanged)
        }
        other => panic!("a verdict for a document that was saved: {other:?}"),
    }
    // Asking again does not help, and the document itself is still there to be read.
    assert!(pool.signatures(handle, NOW).is_err());
    assert!(pool.render(handle, 0, 100, 130).is_ok());

    // The copy opened as a file of its own gets its own verdict, and it is not "intact".
    let (fresh, _) = pool.open(target.to_str().unwrap(), None).unwrap();
    let copy = pool.signatures(fresh, NOW).unwrap();
    assert!(copy.total + copy.unlisted > 0);
    assert!(copy.items.iter().all(|i| !matches!(
        i.state,
        SignatureState::Trusted | SignatureState::UnknownSigner
    )));
}

#[test]
fn a_save_over_the_same_file_ends_the_verdict_too() {
    let dir = Dir::new("saved-in-place");
    let pool = EnginePool::new(config(), 4);
    let file = signed_file(&signer(KeyKind::P256), &SignOptions::default());
    let path = dir.write("signed.pdf", &file.bytes);
    let (handle, _) = pool.open(&path, None).unwrap();
    assert!(pool.signatures(handle, NOW).is_ok());
    let options = SaveOptions {
        allow_signed: true,
        overwrite_changed: false,
    };
    pool.save(handle, options).unwrap();
    assert!(pool.signatures(handle, NOW).is_err());
    // Another document, not saved, is not affected.
    let other = dir.write("other.pdf", &file.bytes);
    let (second, _) = pool.open(&other, None).unwrap();
    assert!(pool.signatures(second, NOW).is_ok());
}

#[test]
fn a_hostile_signature_does_not_hold_up_the_pages() {
    // A megabyte of signature data that is one signed attribute with tens of thousands of values:
    // a decoder that sorts every set it reads takes minutes for it, and the pages of the document
    // wait all that time. The check refuses the shape at once.
    for set in [HostileSet::Values, HostileSet::Attributes] {
        let dir = Dir::new("hostile-set");
        let pool = EnginePool::new(config(), 4);
        let options = SignOptions {
            contents_size: 1024 * 1024 - 64,
            ..SignOptions::default()
        };
        let mut file = signed_file(&signer(KeyKind::P256), &options);
        replace_signature(&mut file, &hostile_set_signature(set, 30_000));
        let path = dir.write("hostile.pdf", &file.bytes);
        let (handle, _) = pool.open(&path, None).unwrap();
        let started = std::time::Instant::now();
        let report = pool.signatures(handle, NOW).unwrap();
        let drawn = pool.render(handle, 0, 100, 130);
        let took = started.elapsed();
        assert!(drawn.is_ok(), "{set:?}");
        assert!(
            took < std::time::Duration::from_secs(20),
            "{set:?}: the check and a page took {took:?}"
        );
        assert_eq!(report.items.len(), 1);
        // Not checked: not intact, and not called broken either.
        assert_eq!(
            report.items[0].state,
            SignatureState::Unsupported,
            "{set:?}"
        );
        assert_eq!(report.items[0].reason, SignatureReason::TooLarge, "{set:?}");
    }
}

#[test]
fn a_set_behind_a_primitive_tag_does_not_hold_up_the_pages() {
    // The decoder reads the content of an implicit field before it looks at the constructed bit of
    // its tag. A quarter of a megabyte behind the tag `0x80` instead of `0xA0` kept the engine
    // busy for minutes and the pages of the document waited all that time. Each field where this
    // can be done is tried, with the tag that hides the set, in the engine as it ships.
    for field in [
        ImplicitField::SignedAttributes,
        ImplicitField::UnsignedAttributes,
        ImplicitField::Certificates,
        ImplicitField::Revocations,
    ] {
        let dir = Dir::new("hidden-set");
        let pool = EnginePool::new(config(), 4);
        let options = SignOptions {
            contents_size: 1024 * 1024 - 64,
            ..SignOptions::default()
        };
        let mut file = signed_file(&signer(KeyKind::P256), &options);
        replace_signature(
            &mut file,
            &hidden_set_signature(field, field.primitive_tag(), 20_000),
        );
        let path = dir.write("hidden.pdf", &file.bytes);
        let (handle, _) = pool.open(&path, None).unwrap();
        let started = std::time::Instant::now();
        let report = pool.signatures(handle, NOW).unwrap();
        let drawn = pool.render(handle, 0, 100, 130);
        let took = started.elapsed();
        assert!(drawn.is_ok(), "{field:?}");
        assert!(
            took < std::time::Duration::from_secs(20),
            "{field:?}: the check and a page took {took:?}"
        );
        assert_eq!(report.items.len(), 1);
        assert_eq!(
            report.items[0].state,
            SignatureState::Unsupported,
            "{field:?}"
        );
        assert_eq!(
            report.items[0].reason,
            SignatureReason::TooLarge,
            "{field:?}"
        );
    }
}

/// Changes the file at `path` in place, the way another program that does not replace the file by
/// a rename would: one digit of the page text, or one byte more at the end.
fn change_in_place(path: &str, original: &[u8], how: &str) {
    use std::io::{Seek, SeekFrom, Write};
    let mut file = fs::OpenOptions::new().write(true).open(path).unwrap();
    match how {
        "digit" => {
            let at = original
                .windows(7)
                .position(|w| w == b"Pay 100")
                .expect("the page text is in the file");
            file.seek(SeekFrom::Start(at as u64 + 4)).unwrap();
            file.write_all(b"9").unwrap();
        }
        _ => {
            file.seek(SeekFrom::End(0)).unwrap();
            file.write_all(b"\n").unwrap();
        }
    }
    file.sync_all().unwrap();
}

#[test]
fn a_page_is_not_drawn_from_other_bytes_than_the_ones_that_were_checked() {
    // A signed file is checked, and then something writes into it in place. The engine reads
    // pages as it needs them, so the first page drawn afterwards would come from the new bytes
    // while the verdict, "intact", is about the old ones. The draw is refused with the same answer
    // as any other read of a file that changed, and a new question gets the same.
    for how in ["digit", "byte"] {
        let dir = Dir::new("written-in-place");
        let pool = EnginePool::new(config(), 4);
        let file = signed_file(&signer(KeyKind::P256), &SignOptions::default());
        let path = dir.write("signed.pdf", &file.bytes);
        let (handle, _) = pool.open(&path, None).unwrap();
        let report = pool.signatures(handle, NOW).unwrap();
        assert_eq!(
            report.items[0].state,
            SignatureState::UnknownSigner,
            "{how}"
        );
        change_in_place(&path, &file.bytes, how);

        for attempt in 0..2 {
            match pool.render(handle, 0, 300, 390 + attempt) {
                Err(ClientError::Engine { code, .. }) => {
                    assert_eq!(code, partridge_protocol::ErrorCode::SourceChanged, "{how}")
                }
                other => panic!(
                    "{how}: a page was drawn from changed bytes: {:?}",
                    other.map(|b| b.rgba.len())
                ),
            }
        }
        // Text and a new question are refused as well.
        assert!(
            matches!(
                pool.page_text(handle, 0),
                Err(ClientError::Engine {
                    code: partridge_protocol::ErrorCode::SourceChanged,
                    ..
                })
            ),
            "{how}"
        );
        assert!(
            matches!(
                pool.signatures(handle, NOW),
                Err(ClientError::Engine {
                    code: partridge_protocol::ErrorCode::SourceChanged,
                    ..
                })
            ),
            "{how}"
        );
    }
}

#[test]
fn saving_over_a_file_that_is_open_in_another_tab_ends_the_verdict_of_that_tab() {
    let dir = Dir::new("saved-over-open");
    let pool = EnginePool::new(config(), 4);
    let signed = signed_file(&signer(KeyKind::P256), &SignOptions::default());
    let other = dir.write("other.pdf", &signed.bytes);
    let unrelated = dir.write("unrelated.pdf", &signed.bytes);

    // Two tabs show the same file, and a third shows another one.
    let (saved_from, _) = pool.open(&other, None).unwrap();
    let (same_file, _) = pool.open(&other, None).unwrap();
    let (apart, _) = pool.open(&unrelated, None).unwrap();
    for handle in [saved_from, same_file, apart] {
        assert!(pool.signatures(handle, NOW).is_ok());
    }

    let options = SaveOptions {
        allow_signed: true,
        overwrite_changed: false,
    };
    let outcome = pool.save(saved_from, options).unwrap();
    // The document that was open on the same file is named, the one that was saved and the one on
    // another file are not.
    assert_eq!(outcome.replaced, vec![same_file]);
    // It is no longer answered for: its engine holds the file as it was, and the path is another.
    assert!(matches!(
        pool.signatures(same_file, NOW),
        Err(ClientError::Engine {
            code: partridge_protocol::ErrorCode::SourceChanged,
            ..
        })
    ));
    assert!(pool.signatures(apart, NOW).is_ok());
}

#[test]
fn a_copy_is_not_written_over_a_signed_file_that_another_tab_shows() {
    let dir = Dir::new("copy-over-signed-tab");
    let pool = EnginePool::new(config(), 4);
    let signed = signed_file(&signer(KeyKind::P256), &SignOptions::default());
    let other = dir.write("other.pdf", &signed.bytes);
    let target = dir.write("target.pdf", &signed.bytes);
    let (saved_from, _) = pool.open(&other, None).unwrap();
    let (on_target, _) = pool.open(&target, None).unwrap();
    let options = SaveOptions {
        allow_signed: true,
        overwrite_changed: false,
    };
    let error = pool
        .save_as(saved_from, std::path::Path::new(&target), options)
        .unwrap_err();
    assert!(
        matches!(
            error,
            ClientError::Save(partridge_engine_client::SaveError::TargetOpen)
        ),
        "{error:?}"
    );
    // The file is as it was, and the tab that shows it still has its verdict.
    assert_eq!(fs::read(&target).unwrap(), signed.bytes);
    assert!(pool.signatures(on_target, NOW).is_ok());
}
