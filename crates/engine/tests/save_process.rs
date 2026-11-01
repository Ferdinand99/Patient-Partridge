//! The whole save path with the real engine, the real PDFium and the sandbox: the engine
//! serialises, the app writes a new file, a second sandboxed engine checks it, and the new file
//! replaces the old one. What goes wrong on the way (a signature, a password, a changed file, a
//! file that does not pass the check) is checked here too.
//!
//! Nothing in the protocol changes a document yet, so every save here writes the document as it
//! was opened. The saves of changed documents are in `tests/save.rs`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime};

use partridge_engine_client::{
    verify_written, ClientError, DocInfo, EngineClient, EngineConfig, EnginePool, SandboxPolicy,
    SaveError, SaveOptions,
};
use partridge_testkit as kit;

fn config() -> EngineConfig {
    let mut config = EngineConfig::new(env!("CARGO_BIN_EXE_partridge-engine"));
    config.args = vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
    // The tests insist on the sandbox: an engine that cannot lock itself in fails them.
    config.sandbox = SandboxPolicy::Required;
    config
}

fn pool() -> EnginePool {
    EnginePool::new(config(), 4)
}

/// A folder of its own for one test, removed again at the end.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "partridge-saveproc-{name}-{}-{}",
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

fn save_error(result: Result<impl std::fmt::Debug, ClientError>) -> SaveError {
    match result {
        Err(ClientError::Save(e)) => e,
        other => panic!("expected a save error, got {other:?}"),
    }
}

/// Opens `path` in a fresh engine and gives back what a check would see.
fn describe(path: &Path, password: Option<&str>) -> DocInfo {
    let client = EngineClient::new(config());
    let opened = client.open(&text(path), password).unwrap();
    let info = client.doc_info(opened.doc).unwrap();
    assert_eq!(info.pages as usize, opened.pages.len());
    info
}

#[test]
fn a_document_is_saved_in_place_and_the_new_file_opens() {
    let dir = Dir::new("plain");
    let original = kit::simple_pdf(3);
    let path = dir.write("a.pdf", &original);
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();

    let out = pool.save(handle, SaveOptions::default()).unwrap();
    assert_eq!(out.path, path);
    let saved = fs::read(&path).unwrap();
    assert_eq!(saved.len() as u64, out.bytes);
    assert!(saved.starts_with(b"%PDF-"));
    // The file on disk is the new one (PDFium writes the document anew), and nothing is left over.
    assert_ne!(saved, original);
    assert_eq!(dir.names(), vec!["a.pdf".to_string()]);
    assert_eq!(
        describe(&path, None),
        DocInfo {
            pages: 3,
            security: 0,
            signatures: 0
        }
    );

    // The document is still open and usable, although the file it was read from was replaced
    // under it (on Windows too: the engine holds the old file open), and it can be saved again.
    assert!(pool.render(handle, 2, 100, 130).is_ok());
    assert!(pool.page_text(handle, 0).unwrap().text.contains("Page 1"));
    pool.save(handle, SaveOptions::default()).unwrap();
    assert_eq!(describe(&path, None).pages, 3);
    assert_eq!(dir.names(), vec!["a.pdf".to_string()]);
}

#[cfg(unix)]
#[test]
fn the_permissions_of_the_file_survive_a_real_save() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Dir::new("mode");
    let path = dir.write("a.pdf", &kit::simple_pdf(1));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    pool.save(handle, SaveOptions::default()).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn a_read_only_file_is_not_replaced() {
    let dir = Dir::new("readonly");
    let original = kit::simple_pdf(1);
    let path = dir.write("a.pdf", &original);
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&path, perms.clone()).unwrap();

    let err = save_error(pool.save(handle, SaveOptions::default()));

    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(&path, perms).unwrap();
    assert!(matches!(err, SaveError::ReadOnly(_)), "{err:?}");
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(dir.names(), vec!["a.pdf".to_string()]);
}

#[test]
fn a_password_protected_document_is_saved_protected() {
    let dir = Dir::new("locked");
    let path = dir.write("a.pdf", &kit::encrypted_pdf("secret", 2));
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), Some("secret")).unwrap();

    pool.save(handle, SaveOptions::default()).unwrap();

    // The new file needs the password, and the same one.
    let client = EngineClient::new(config());
    match client.open(&text(&path), None) {
        Err(ClientError::Engine { code, .. }) => {
            assert_eq!(code, partridge_protocol::ErrorCode::PasswordRequired)
        }
        other => panic!("the saved file opens without a password: {other:?}"),
    }
    assert_eq!(
        describe(&path, Some("secret")),
        DocInfo {
            pages: 2,
            security: 2,
            signatures: 0
        }
    );
}

#[test]
fn the_check_refuses_a_file_that_lost_its_protection_or_is_not_the_document() {
    let dir = Dir::new("check");
    let plain = dir.write("plain.pdf", &kit::simple_pdf(2));
    let locked = dir.write("locked.pdf", &kit::encrypted_pdf("secret", 2));
    let signed = dir.write("signed.pdf", &kit::signed_pdf());
    let broken = dir.write("broken.pdf", &kit::truncated_pdf());
    let junk = dir.write("junk.pdf", &kit::not_a_pdf());
    let info = |pages, security, signatures| DocInfo {
        pages,
        security,
        signatures,
    };
    let check = |path: &Path, password: Option<&str>, expected: DocInfo| {
        verify_written(&config(), path, password, expected)
    };

    // The right file passes.
    check(&plain, None, info(2, 0, 0)).unwrap();
    check(&locked, Some("secret"), info(2, 2, 0)).unwrap();
    check(&signed, None, info(1, 0, 1)).unwrap();

    // A document that was protected and whose new file is not.
    assert_eq!(
        check(&plain, None, info(2, 2, 0)).unwrap_err(),
        SaveError::ProtectionLost
    );
    // A document opened with a password whose new file opens without one.
    assert_eq!(
        check(&plain, Some("secret"), info(2, 0, 0)).unwrap_err(),
        SaveError::ProtectionLost
    );
    // Another number of pages, another number of signatures, another kind of protection.
    for (path, expected) in [
        (&plain, info(3, 0, 0)),
        (&plain, info(1, 0, 0)),
        (&plain, info(2, 0, 1)),
        (&signed, info(1, 0, 0)),
    ] {
        assert!(
            matches!(check(path, None, expected), Err(SaveError::Verify(_))),
            "{path:?} {expected:?}"
        );
    }
    assert!(matches!(
        check(&locked, Some("secret"), info(2, 3, 0)),
        Err(SaveError::Verify(_))
    ));
    // A wrong password, a file that is not a PDF, and one that is cut off.
    assert!(matches!(
        check(&locked, Some("wrong"), info(2, 2, 0)),
        Err(SaveError::Verify(_))
    ));
    assert!(matches!(
        check(&junk, None, info(1, 0, 0)),
        Err(SaveError::Verify(_))
    ));
    // A cut-off file may be repaired by PDFium into fewer pages; either way it is not the document.
    assert!(matches!(
        check(&broken, None, info(5, 0, 0)),
        Err(SaveError::Verify(_))
    ));
}

#[test]
fn a_signed_document_asks_before_its_signature_is_broken() {
    let dir = Dir::new("signed");
    let original = kit::signed_pdf();
    let path = dir.write("a.pdf", &original);
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();

    let err = save_error(pool.save(handle, SaveOptions::default()));
    assert_eq!(err, SaveError::Signed { signatures: 1 });
    assert_eq!(fs::read(&path).unwrap(), original, "the file was touched");
    assert_eq!(dir.names(), vec!["a.pdf".to_string()]);

    let allowed = SaveOptions {
        allow_signed: true,
        ..SaveOptions::default()
    };
    pool.save(handle, allowed).unwrap();
    let saved = fs::read(&path).unwrap();
    // The file was rewritten, so the bytes a signature covers are not kept ...
    assert!(!saved.starts_with(&original));
    // ... and the signature is still in the document.
    assert_eq!(describe(&path, None).signatures, 1);
}

#[test]
fn a_file_that_someone_else_changed_is_asked_about_not_overwritten() {
    let dir = Dir::new("changed");
    let path = dir.write("a.pdf", &kit::simple_pdf(2));
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();

    // Somebody else saves a different document there.
    let theirs = kit::simple_pdf(5);
    fs::write(&path, &theirs).unwrap();
    let file = fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(SystemTime::now() + Duration::from_secs(10))
        .unwrap();
    drop(file);

    assert_eq!(
        save_error(pool.save(handle, SaveOptions::default())),
        SaveError::Changed
    );
    assert_eq!(fs::read(&path).unwrap(), theirs, "their change was lost");
    assert_eq!(dir.names(), vec!["a.pdf".to_string()]);

    // The user can still say that their version is the one to keep.
    let overwrite = SaveOptions {
        overwrite_changed: true,
        ..SaveOptions::default()
    };
    pool.save(handle, overwrite).unwrap();
    assert_eq!(describe(&path, None).pages, 2);
    // And after that the file is "ours" again: the next save asks nothing.
    pool.save(handle, SaveOptions::default()).unwrap();
}

#[test]
fn save_as_writes_a_new_file_and_the_document_moves_to_it() {
    let dir = Dir::new("as");
    let original = kit::simple_pdf(2);
    let path = dir.write("a.pdf", &original);
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    assert_eq!(pool.source_path(handle), Some(path.clone()));

    let copy = dir.0.join("copy.pdf");
    let out = pool.save_as(handle, &copy, SaveOptions::default()).unwrap();
    assert_eq!(out.path, copy);
    assert_eq!(describe(&copy, None).pages, 2);
    assert_eq!(fs::read(&path).unwrap(), original, "the original changed");
    assert_eq!(pool.source_path(handle), Some(copy.clone()));

    // From now on a plain save goes to the copy.
    fs::remove_file(&path).unwrap();
    pool.save(handle, SaveOptions::default()).unwrap();
    assert_eq!(dir.names(), vec!["copy.pdf".to_string()]);

    // Save As to the file the document belongs to is a plain save.
    let again = pool.save_as(handle, &copy, SaveOptions::default()).unwrap();
    assert_eq!(again.path, copy);
}

#[test]
fn save_as_over_another_file_replaces_it_only_after_the_new_one_checks_out() {
    let dir = Dir::new("over");
    let path = dir.write("a.pdf", &kit::simple_pdf(2));
    let other = dir.write("b.pdf", b"another file the user chose to replace");
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    pool.save_as(handle, &other, SaveOptions::default())
        .unwrap();
    assert_eq!(describe(&other, None).pages, 2);
    assert_eq!(dir.names(), vec!["a.pdf".to_string(), "b.pdf".to_string()]);
}

#[test]
fn a_document_that_is_closed_cannot_be_saved() {
    let dir = Dir::new("closed");
    let path = dir.write("a.pdf", &kit::simple_pdf(1));
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    pool.close(handle);
    match pool.save(handle, SaveOptions::default()) {
        Err(ClientError::Engine { code, .. }) => {
            assert_eq!(code, partridge_protocol::ErrorCode::UnknownDocument)
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(pool.source_path(handle), None);
}
