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
            signatures: 0,
            maybe_signed: false,
            touched: false,
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
            signatures: 0,
            maybe_signed: false,
            touched: false,
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
        maybe_signed: false,
        touched: false,
    };
    let check = |path: &Path, password: Option<&str>, expected: DocInfo| {
        verify_written(&config(), path, password, expected, None)
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

    // Somebody else saves a different document there, the way editors do: a new file, renamed
    // over this one. (A rewrite in place is a different case, in the c1 tests: the engine's
    // handle would see the new bytes, and nothing can be saved.)
    let theirs = kit::simple_pdf(5);
    let tmp = dir.write("tmp.pdf", &theirs);
    fs::rename(&tmp, &path).unwrap();

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

// ---- findings of the review of PR 31: each of these failed on the first version --------------

/// Rewrites a file in place: the same file (same inode), new bytes.
fn rewrite_in_place(path: &Path, bytes: &[u8]) {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

#[test]
fn c1_a_file_rewritten_in_place_is_not_saved_over_with_a_mix_of_both() {
    let dir = Dir::new("c1");
    let path = dir.write("a.pdf", &kit::simple_pdf(3));
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();

    // Another program rewrites the file in place: the engine's handle now reads the new bytes.
    let theirs = kit::simple_pdf(5);
    rewrite_in_place(&path, &theirs);

    // Neither Replace nor Save a copy may write what the engine can make of it.
    let replace = SaveOptions {
        overwrite_changed: true,
        ..SaveOptions::default()
    };
    let err = save_error(pool.save(handle, replace));
    assert_eq!(err.code(), "save_source_changed", "{err:?}");
    assert_eq!(
        fs::read(&path).unwrap(),
        theirs,
        "their file was overwritten"
    );
    let copy = dir.0.join("copy.pdf");
    let err = save_error(pool.save_as(handle, &copy, SaveOptions::default()));
    assert_eq!(err.code(), "save_source_changed", "{err:?}");
    assert!(!copy.exists(), "a copy of a mix was written");
    assert_eq!(dir.names(), vec!["a.pdf".to_string()]);
}

#[test]
fn c1_a_same_size_rewrite_with_the_old_time_is_seen_in_the_part_that_was_read() {
    let dir = Dir::new("c1b");
    let original = kit::simple_pdf(2);
    let path = dir.write("a.pdf", &original);
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    // Reading the text of page 1 makes the engine read its content stream.
    assert!(pool.page_text(handle, 0).unwrap().text.contains("Page 1"));

    // The same length, a different content stream, and the time of change put back.
    let mut theirs = original.clone();
    let at = theirs
        .windows(6)
        .position(|w| w == b"Page 1")
        .expect("the text of page 1");
    theirs[at..at + 6].copy_from_slice(b"Page X");
    rewrite_in_place(&path, &theirs);
    let file = fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(before).unwrap();
    drop(file);

    let replace = SaveOptions {
        overwrite_changed: true,
        ..SaveOptions::default()
    };
    let err = save_error(pool.save(handle, replace));
    assert_eq!(err.code(), "save_source_changed", "{err:?}");
}

#[test]
fn m1_a_touch_before_the_first_save_still_lets_the_edits_out_as_a_copy() {
    let dir = Dir::new("m1");
    let original = kit::simple_pdf(3);
    let path = dir.write("a.pdf", &original);
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();

    // Another program only touches the file: the same bytes, a new time of change. Most of the
    // file was never read by the engine when this happens.
    let file = fs::File::options().write(true).open(&path).unwrap();
    file.set_modified(before + std::time::Duration::from_secs(3600))
        .unwrap();
    drop(file);

    // The original is not replaced, whatever the user agreed to ...
    for options in [
        SaveOptions::default(),
        SaveOptions {
            overwrite_changed: true,
            ..SaveOptions::default()
        },
    ] {
        let err = save_error(pool.save(handle, options));
        assert_eq!(err, SaveError::SourceTouched, "{err:?}");
        let err = save_error(pool.save_as(handle, &path, options));
        assert_eq!(err, SaveError::SourceTouched, "{err:?}");
    }
    assert_eq!(fs::read(&path).unwrap(), original);
    // ... and the edits can always leave as a copy, which says the file was touched.
    let copy = dir.0.join("copy.pdf");
    let outcome = pool.save_as(handle, &copy, SaveOptions::default()).unwrap();
    assert!(outcome.touched);
    assert_eq!(describe(&copy, None).pages, 3);
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn m1_a_file_that_was_not_touched_is_replaced_and_says_nothing() {
    let dir = Dir::new("m1b");
    let path = dir.write("a.pdf", &kit::simple_pdf(2));
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    let outcome = pool.save(handle, SaveOptions::default()).unwrap();
    assert!(!outcome.touched);
}

#[test]
fn c1_a_file_replaced_by_a_rename_keeps_the_old_handle_and_a_copy_is_safe() {
    let dir = Dir::new("c1c");
    let path = dir.write("a.pdf", &kit::simple_pdf(3));
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    assert!(pool.page_text(handle, 0).unwrap().text.contains("Page 1"));

    // Somebody saves their own version the way editors do: a new file, renamed over this one.
    let theirs = kit::simple_pdf(5);
    let tmp = dir.write("tmp.pdf", &theirs);
    fs::rename(&tmp, &path).unwrap();

    // Save asks (the file at the path is not the one that was opened) ...
    assert_eq!(
        save_error(pool.save(handle, SaveOptions::default())),
        SaveError::Changed
    );
    // ... and a copy holds the document that was opened, whole.
    let copy = dir.0.join("copy.pdf");
    pool.save_as(handle, &copy, SaveOptions::default()).unwrap();
    assert_eq!(describe(&copy, None).pages, 3);
    assert_eq!(fs::read(&path).unwrap(), theirs);
}

#[test]
fn c2_a_file_cut_off_after_it_was_opened_does_not_become_a_save_with_blank_pages() {
    let dir = Dir::new("c2");
    let original = kit::simple_pdf(6);
    let path = dir.write("a.pdf", &original);
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    // The share drops, or the stick is pulled: what is left of the file is the first part.
    rewrite_in_place(&path, &original[..original.len() * 6 / 10]);

    let replace = SaveOptions {
        overwrite_changed: true,
        ..SaveOptions::default()
    };
    let result = pool.save(handle, replace);
    assert!(result.is_err(), "a save was made from a cut-off file");
    assert_eq!(
        fs::read(&path).unwrap(),
        &original[..original.len() * 6 / 10],
        "the file was replaced"
    );
    assert_eq!(dir.names(), vec!["a.pdf".to_string()]);
}

#[test]
fn i5_a_signature_field_under_a_grouping_field_is_still_a_signed_document() {
    let dir = Dir::new("i5");
    let path = dir.write("a.pdf", &kit::signed_nested_pdf());
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    let err = save_error(pool.save(handle, SaveOptions::default()));
    assert!(matches!(err, SaveError::Signed { .. }), "{err:?}");
    assert_eq!(
        fs::read(&path).unwrap(),
        kit::signed_nested_pdf(),
        "the file was touched"
    );
}

#[cfg(unix)]
fn running_as_root() -> bool {
    // SAFETY: a plain system call without arguments.
    unsafe { libc::geteuid() == 0 }
}

#[cfg(unix)]
#[test]
fn i6_a_file_the_user_cannot_write_is_refused_although_its_folder_is_writable() {
    use std::os::unix::fs::PermissionsExt;
    if running_as_root() {
        eprintln!("skipped: the super user can write every file");
        return;
    }
    let dir = Dir::new("i6");
    let original = kit::simple_pdf(1);
    let path = dir.write("a.pdf", &original);
    // The owner has no write right; the group has one. Some write bit is set, so the old test
    // for "read-only" found nothing wrong.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o464)).unwrap();
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    let err = save_error(pool.save(handle, SaveOptions::default()));
    assert!(matches!(err, SaveError::ReadOnly(_)), "{err:?}");
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(dir.names(), vec!["a.pdf".to_string()]);
}

#[test]
fn i8_a_file_with_two_names_is_not_silently_split_in_two() {
    let dir = Dir::new("i8h");
    let original = kit::simple_pdf(1);
    let path = dir.write("a.pdf", &original);
    let other_name = dir.0.join("b.pdf");
    fs::hard_link(&path, &other_name).unwrap();
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    let err = save_error(pool.save(handle, SaveOptions::default()));
    assert_eq!(err.code(), "save_hard_link", "{err:?}");
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(fs::read(&other_name).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn i8_the_owner_and_the_group_of_a_file_are_kept_where_the_system_allows() {
    use std::os::unix::fs::MetadataExt;
    if !running_as_root() {
        eprintln!("skipped: only the super user can make a file of another owner");
        return;
    }
    let dir = Dir::new("i8o");
    let path = dir.write("a.pdf", &kit::simple_pdf(1));
    std::os::unix::fs::chown(&path, Some(2345), Some(3456)).unwrap();
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    pool.save(handle, SaveOptions::default()).unwrap();
    let meta = fs::metadata(&path).unwrap();
    assert_eq!((meta.uid(), meta.gid()), (2345, 3456));
}

#[test]
fn i5_a_signature_that_no_field_points_to_is_found_by_the_words_in_the_file() {
    let dir = Dir::new("i5b");
    let original = kit::signed_without_field_pdf();
    let path = dir.write("a.pdf", &original);
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    let err = save_error(pool.save(handle, SaveOptions::default()));
    assert!(matches!(err, SaveError::Signed { .. }), "{err:?}");
    assert_eq!(fs::read(&path).unwrap(), original);
    // And the same document with a copy asked for on purpose goes through the question too.
    let copy = dir.0.join("copy.pdf");
    let err = save_error(pool.save_as(handle, &copy, SaveOptions::default()));
    assert!(matches!(err, SaveError::Signed { .. }), "{err:?}");
    assert!(!copy.exists());
}

#[test]
fn c2_the_check_sees_a_file_with_the_same_pages_but_other_content() {
    let dir = Dir::new("c2c");
    let a = dir.write("a.pdf", &kit::simple_pdf(3));
    // Three pages as well, but the text of every page is another text.
    let mut other = kit::simple_pdf(3);
    for at in 0..other.len().saturating_sub(4) {
        if &other[at..at + 4] == b"Line" {
            other[at..at + 4].copy_from_slice(b"Lone");
        }
    }
    let b = dir.write("b.pdf", &other);
    let client = EngineClient::new(config());
    let opened = client.open(&text(&a), None).unwrap();
    let info = client.doc_info(opened.doc).unwrap();
    let print = client.fingerprint(opened.doc).unwrap();
    assert_eq!(print.failed, 0);

    // The file it was made from passes. Another file with the same number of pages does not.
    verify_written(&config(), &a, None, info, Some(&print)).unwrap();
    let err = verify_written(&config(), &b, None, info, Some(&print)).unwrap_err();
    assert!(matches!(err, SaveError::Verify(_)), "{err:?}");
    // Without the fingerprint the old check cannot tell them apart.
    verify_written(&config(), &b, None, info, None).unwrap();
}
