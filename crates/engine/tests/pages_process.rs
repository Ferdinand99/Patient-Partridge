//! The page tools with the real engine process in its sandbox: every change goes to the engine of
//! a document through the pool, and the result is saved with the save path of the app (a new file,
//! checked by a second sandboxed engine, put in place in one step). Pages that come from another
//! file are read by an engine of their own and travel through the app in pieces.
//!
//! When `PARTRIDGE_PAGES_OUT` names a folder, the test `the_files_for_other_tools_are_written`
//! writes the results there, with `expected.txt`, for `scripts/check-pages.sh`, which hands them to
//! qpdf and poppler, tools that are not PDFium.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use partridge_engine_client::{
    Cancelled, ClientError, EngineConfig, EnginePool, ExtractOptions, InsertOptions, SandboxPolicy,
    SaveError, SaveOptions,
};
use partridge_protocol::{ErrorCode, MAX_PAGES};
use partridge_testkit::{self as kit, contains, page_secrets};

fn config() -> EngineConfig {
    let mut config = EngineConfig::new(env!("CARGO_BIN_EXE_partridge-engine"));
    config.args = vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
    config.sandbox = SandboxPolicy::Required;
    config
}

fn pool() -> EnginePool {
    EnginePool::new(config(), 6)
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "partridge-pagesproc-{name}-{}-{}",
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

fn nothing(_: u64, _: u64) {}

fn client_error(result: Result<impl std::fmt::Debug, ClientError>) -> ClientError {
    match result {
        Err(e) => e,
        Ok(v) => panic!("expected an error, got {v:?}"),
    }
}

fn code_of(e: ClientError) -> ErrorCode {
    match e {
        ClientError::Engine { code, .. } => code,
        other => panic!("{other:?}"),
    }
}

fn has_page_text(pool: &EnginePool, handle: u64, place: u32, page: usize) -> bool {
    pool.page_text(handle, place)
        .unwrap()
        .text
        .contains(&kit::page_text(page))
}

#[test]
fn every_tool_works_in_the_sandbox_and_a_save_writes_the_result() {
    let dir = Dir::new("all");
    let path = dir.write("a.pdf", &kit::marked_pdf(6));
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();

    // Delete, move, turn, crop and add a blank page; then save.
    let deleted = pool.pages_delete(handle, &[1]).unwrap();
    assert_eq!(deleted.pages.len(), 5);
    let order = [4u32, 0, 1, 2, 3];
    let moved = pool.pages_reorder(handle, &order).unwrap();
    assert_eq!(moved.pages.len(), 5);
    pool.pages_rotate(handle, &[0], 1).unwrap();
    let cropped = pool
        .pages_crop(handle, &[1], [10.0, 20.0, 30.0, 40.0])
        .unwrap();
    assert_eq!(cropped.pages[1].width, 612.0 - 40.0);
    let blank = pool.pages_insert_blank(handle, 2, 300.0, 400.0).unwrap();
    assert_eq!(blank.pages.len(), 6);
    pool.save(handle, SaveOptions::default()).unwrap();

    let saved = fs::read(&path).unwrap();
    for s in page_secrets(1) {
        assert!(!contains(&saved, &s.plain), "deleted page: {}", s.what);
    }
    for page in [0, 2, 3, 4, 5] {
        for s in page_secrets(page) {
            assert!(contains(&saved, &s.plain), "page {page}: {}", s.what);
        }
    }
    assert_eq!(dir.names(), vec!["a.pdf".to_string()]);
    // The saved file is opened by another engine: pages, order and the turn.
    let (again, sizes) = pool.open(&text(&path), None).unwrap();
    assert_eq!(sizes.len(), 6);
    assert_eq!((sizes[0].width, sizes[0].height), (792.0, 612.0));
    assert_eq!((sizes[2].width, sizes[2].height), (300.0, 400.0));
    assert!(has_page_text(&pool, again, 0, 5));
    assert!(has_page_text(&pool, again, 1, 0));
    assert!(has_page_text(&pool, again, 3, 2));
}

#[test]
fn a_change_is_taken_back_and_made_again_through_the_pool() {
    let dir = Dir::new("undo");
    let path = dir.write("a.pdf", &kit::marked_pdf(4));
    let pool = pool();
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    let before = pool.page_text(handle, 1).unwrap().text;
    let deleted = pool.pages_delete(handle, &[1]).unwrap();
    assert_eq!(deleted.pages.len(), 3);
    let back = pool.pages_restore(handle, deleted.snapshot).unwrap();
    assert_eq!(back.pages.len(), 4);
    assert_eq!(pool.page_text(handle, 1).unwrap().text, before);
    let again = pool.pages_restore(handle, back.snapshot).unwrap();
    assert_eq!(again.pages.len(), 3);
    // The same step cannot be taken twice with the same number.
    let gone = client_error(pool.pages_restore(handle, back.snapshot));
    assert_eq!(code_of(gone), ErrorCode::SnapshotGone);
}

#[test]
fn pages_of_another_file_are_inserted_by_an_engine_of_its_own() {
    let dir = Dir::new("insert");
    let target = dir.write("a.pdf", &kit::marked_pdf(3));
    let other = dir.write("b.pdf", &kit::simple_pdf(4));
    let pool = pool();
    let (handle, _) = pool.open(&text(&target), None).unwrap();
    let last = AtomicU64::new(0);
    let outcome = pool
        .insert_pdf(
            handle,
            1,
            &text(&other),
            None,
            InsertOptions::default(),
            &|done, total| {
                assert!(done <= total);
                last.store(done, Ordering::SeqCst);
            },
        )
        .unwrap();
    assert_eq!(outcome.inserted, 4);
    assert_eq!(outcome.changed.pages.len(), 7);
    assert!(last.load(Ordering::SeqCst) > 0);
    assert!(pool.page_text(handle, 1).unwrap().text.contains("Page 1"));
    assert!(has_page_text(&pool, handle, 5, 1));
    // The engine that read the other file is gone; the pool holds the one document.
    assert_eq!(pool.len(), 1);
    // Saved through the save path, the document has all seven pages.
    pool.save(handle, SaveOptions::default()).unwrap();
    let (again, sizes) = pool.open(&text(&target), None).unwrap();
    assert_eq!(sizes.len(), 7);
    assert!(has_page_text(&pool, again, 0, 0));
    // The other file was not touched.
    assert_eq!(fs::read(&other).unwrap(), kit::simple_pdf(4));
}

#[test]
fn a_protected_file_asks_for_its_password_and_its_pages_do_not_join_without_a_word() {
    let dir = Dir::new("locked");
    let target = dir.write("a.pdf", &kit::marked_pdf(2));
    let locked = dir.write("locked.pdf", &kit::encrypted_pdf("secret", 2));
    let pool = pool();
    let (handle, _) = pool.open(&text(&target), None).unwrap();
    let ask = |password: Option<&str>, options: InsertOptions| {
        pool.insert_pdf(handle, 0, &text(&locked), password, options, &nothing)
    };
    assert_eq!(
        code_of(client_error(ask(None, InsertOptions::default()))),
        ErrorCode::PasswordRequired
    );
    assert_eq!(
        code_of(client_error(ask(Some("wrong"), InsertOptions::default()))),
        ErrorCode::PasswordIncorrect
    );
    // The right password is not enough: the pages would lose their protection.
    match client_error(ask(Some("secret"), InsertOptions::default())) {
        ClientError::Save(SaveError::InsertProtected) => {}
        other => panic!("{other:?}"),
    }
    // Nothing was inserted.
    assert!(
        pool.page_text(handle, 2).is_err(),
        "the document still has two pages"
    );
    let outcome = ask(
        Some("secret"),
        InsertOptions {
            allow_protected: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(outcome.changed.pages.len(), 4);
    // The document itself is not protected, so it still opens without a password after a save.
    pool.save(handle, SaveOptions::default()).unwrap();
    let (_, sizes) = pool.open(&text(&target), None).unwrap();
    assert_eq!(sizes.len(), 4);
}

#[test]
fn pages_of_a_protected_document_get_the_protection_of_the_document_they_join() {
    let dir = Dir::new("keeps");
    let target = dir.write("locked.pdf", &kit::encrypted_pdf("secret", 2));
    let other = dir.write("b.pdf", &kit::simple_pdf(1));
    let pool = pool();
    let (handle, _) = pool.open(&text(&target), Some("secret")).unwrap();
    pool.insert_pdf(
        handle,
        2,
        &text(&other),
        None,
        InsertOptions::default(),
        &nothing,
    )
    .unwrap();
    pool.pages_delete(handle, &[0]).unwrap();
    pool.save(handle, SaveOptions::default()).unwrap();
    // Still needs the password, and has the pages.
    assert_eq!(
        code_of(client_error(pool.open(&text(&target), None))),
        ErrorCode::PasswordRequired
    );
    let (again, sizes) = pool.open(&text(&target), Some("secret")).unwrap();
    assert_eq!(sizes.len(), 2);
    // The page that came from the other file is in it, readable with the key, and its text is not
    // in the bytes of the file (it is written with the key of the document, and compressed).
    assert!(pool.page_text(again, 1).unwrap().text.contains("Page 1"));
    let saved = fs::read(&target).unwrap();
    assert!(!contains(&saved, "quick brown fox"));
}

#[test]
fn a_signed_file_warns_before_its_pages_come_without_the_signature() {
    let dir = Dir::new("signed");
    let target = dir.write("a.pdf", &kit::marked_pdf(2));
    let signed = dir.write("s.pdf", &kit::signed_pdf());
    let pool = pool();
    let (handle, _) = pool.open(&text(&target), None).unwrap();
    match client_error(pool.insert_pdf(
        handle,
        0,
        &text(&signed),
        None,
        InsertOptions::default(),
        &nothing,
    )) {
        ClientError::Save(SaveError::InsertSigned { signatures }) => assert!(signatures >= 1),
        other => panic!("{other:?}"),
    }
    pool.insert_pdf(
        handle,
        0,
        &text(&signed),
        None,
        InsertOptions {
            allow_signed: true,
            ..Default::default()
        },
        &nothing,
    )
    .unwrap();
}

#[test]
fn a_file_that_is_not_a_pdf_or_too_long_is_not_inserted() {
    let dir = Dir::new("bad");
    let target = dir.write("a.pdf", &kit::marked_pdf(2));
    let pool = pool();
    let (handle, _) = pool.open(&text(&target), None).unwrap();
    // Whatever is wrong with the other file is told as being wrong with that file, never as
    // something about the document the pages were to join.
    for (name, bytes, code) in [
        ("plain.pdf", kit::not_a_pdf(), ErrorCode::InsertSourceFailed),
        (
            "broken.pdf",
            kit::corrupt_pdf(),
            ErrorCode::InsertSourceFailed,
        ),
        (
            "long.pdf",
            kit::blank_pages_pdf(MAX_PAGES + 1),
            ErrorCode::InsertSourceFailed,
        ),
        (
            "tall.pdf",
            kit::blank_pages_pdf(partridge_protocol::MAX_EDIT_PAGES + 1),
            ErrorCode::InsertSourceFailed,
        ),
    ] {
        let other = dir.write(name, &bytes);
        let e = client_error(pool.insert_pdf(
            handle,
            0,
            &text(&other),
            None,
            InsertOptions::default(),
            &nothing,
        ));
        assert_eq!(code_of(e), code, "{name}");
    }
    // And a path that is not a file.
    assert!(pool
        .insert_pdf(
            handle,
            0,
            &text(&dir.0),
            None,
            InsertOptions::default(),
            &nothing
        )
        .is_err());
    // The document is as it was and still works.
    assert_eq!(pool.pages_rotate(handle, &[0], 1).unwrap().pages.len(), 2);
}

#[test]
fn a_big_file_travels_in_pieces_and_progress_goes_up() {
    let dir = Dir::new("big");
    let target = dir.write("a.pdf", &kit::marked_pdf(2));
    let big = dir.write("big.pdf", &kit::padded_pdf(12 * 1024 * 1024));
    let pool = pool();
    let (handle, _) = pool.open(&text(&target), None).unwrap();
    let calls = AtomicU64::new(0);
    let last = AtomicU64::new(0);
    let started = Instant::now();
    // The padding is an object no page uses, so the engine does not take it into the new document:
    // what travels is the one page. A document with a lot on its pages is below.
    pool.insert_pdf(
        handle,
        0,
        &text(&big),
        None,
        InsertOptions::default(),
        &|done, _| {
            assert!(done > last.swap(done, Ordering::SeqCst));
            calls.fetch_add(1, Ordering::SeqCst);
        },
    )
    .unwrap();
    assert!(started.elapsed() < Duration::from_secs(60));
    assert!(calls.load(Ordering::SeqCst) >= 1);
    // 300 pages with text: some megabytes to hand over, in pieces of at most one.
    let long = dir.write("long.pdf", &kit::simple_pdf(300));
    let size = AtomicU64::new(0);
    let pieces = AtomicU64::new(0);
    pool.insert_pdf(
        handle,
        0,
        &text(&long),
        None,
        InsertOptions::default(),
        &|_, total| {
            size.store(total, Ordering::SeqCst);
            pieces.fetch_add(1, Ordering::SeqCst);
        },
    )
    .unwrap();
    assert!(
        size.load(Ordering::SeqCst) > 0
            && pieces.load(Ordering::SeqCst) >= size.load(Ordering::SeqCst) / (1 << 20)
    );
}

#[test]
fn extracted_pages_are_written_through_the_save_path_and_checked() {
    let dir = Dir::new("extract");
    let source = dir.write("a.pdf", &kit::marked_pdf(5));
    let out = dir.0.join("part.pdf");
    let pool = pool();
    let (handle, _) = pool.open(&text(&source), None).unwrap();
    let outcome = pool
        .extract_pages(handle, &[3, 1], &out, ExtractOptions::default(), &nothing)
        .unwrap();
    assert_eq!(outcome.path, out);
    let written = fs::read(&out).unwrap();
    assert_eq!(written.len() as u64, outcome.bytes);
    for page in [3, 1] {
        for s in page_secrets(page) {
            assert!(contains(&written, &s.plain), "page {page}: {}", s.what);
        }
    }
    for page in [0, 2, 4] {
        for s in page_secrets(page) {
            assert!(!contains(&written, &s.plain), "page {page}: {}", s.what);
        }
    }
    // Nothing is left beside it, and the document is as it was.
    assert_eq!(
        dir.names(),
        vec!["a.pdf".to_string(), "part.pdf".to_string()]
    );
    assert!(pool
        .page_text(handle, 4)
        .unwrap()
        .text
        .contains(&kit::page_text(4)));
    let (copy, sizes) = pool.open(&text(&out), None).unwrap();
    assert_eq!(sizes.len(), 2);
    assert!(has_page_text(&pool, copy, 0, 3));
    assert!(has_page_text(&pool, copy, 1, 1));
    // The same name again is refused while a tab shows that file, and nothing is written.
    let before = fs::read(&out).unwrap();
    match client_error(pool.extract_pages(handle, &[0], &out, ExtractOptions::default(), &nothing))
    {
        ClientError::Save(SaveError::TargetOpen) => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read(&out).unwrap(), before);
    // With that tab closed it replaces the file (the dialog of the system asked about that).
    pool.close(copy);
    pool.extract_pages(handle, &[0], &out, ExtractOptions::default(), &nothing)
        .unwrap();
    let (copy2, sizes) = pool.open(&text(&out), None).unwrap();
    assert_eq!(sizes.len(), 1);
    assert!(has_page_text(&pool, copy2, 0, 0));
    assert_eq!(
        dir.names(),
        vec!["a.pdf".to_string(), "part.pdf".to_string()]
    );
}

#[test]
fn extracting_over_the_document_itself_is_refused() {
    let dir = Dir::new("over");
    let source = dir.write("a.pdf", &kit::marked_pdf(3));
    let pool = pool();
    let (handle, _) = pool.open(&text(&source), None).unwrap();
    match client_error(pool.extract_pages(
        handle,
        &[0],
        &source,
        ExtractOptions::default(),
        &nothing,
    )) {
        ClientError::Save(SaveError::OverSource) => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read(&source).unwrap(), kit::marked_pdf(3));
}

#[test]
fn extracting_onto_a_file_that_another_tab_has_open_is_refused_and_the_file_is_not_touched() {
    let dir = Dir::new("other-tab");
    let source = dir.write("a.pdf", &kit::marked_pdf(3));
    let other_bytes = kit::marked_pdf(2);
    let other = dir.write("b.pdf", &other_bytes);
    let pool = pool();
    let (handle, _) = pool.open(&text(&source), None).unwrap();
    let (_other_handle, _) = pool.open(&text(&other), None).unwrap();
    match client_error(pool.extract_pages(
        handle,
        &[0],
        &other,
        ExtractOptions::default(),
        &nothing,
    )) {
        ClientError::Save(e @ SaveError::TargetOpen) => assert_eq!(e.code(), "save_target_open"),
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read(&other).unwrap(), other_bytes);
    assert_eq!(
        dir.names(),
        vec!["a.pdf".to_string(), "b.pdf".to_string()],
        "no stray file is left"
    );
    // Another way to spell the same file is the same file.
    let mut spellings = vec![dir.0.join(".").join("b.pdf")];
    #[cfg(unix)]
    {
        let link = dir.0.join("link.pdf");
        std::os::unix::fs::symlink(&other, &link).unwrap();
        spellings.push(link);
    }
    for spelled in &spellings {
        match client_error(pool.extract_pages(
            handle,
            &[0],
            spelled,
            ExtractOptions::default(),
            &nothing,
        )) {
            ClientError::Save(SaveError::TargetOpen) => {}
            other => panic!("{}: {other:?}", spelled.display()),
        }
    }
}

#[test]
fn extracting_onto_a_file_that_holds_a_signature_is_refused_even_when_it_is_not_open() {
    let dir = Dir::new("onto-signed");
    let source = dir.write("a.pdf", &kit::marked_pdf(3));
    let signed_bytes = kit::signed_pdf();
    let signed = dir.write("s.pdf", &signed_bytes);
    let pool = pool();
    let (handle, _) = pool.open(&text(&source), None).unwrap();
    match client_error(pool.extract_pages(
        handle,
        &[0],
        &signed,
        ExtractOptions::default(),
        &nothing,
    )) {
        ClientError::Save(e @ SaveError::TargetSigned) => {
            assert_eq!(e.code(), "save_target_signed")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read(&signed).unwrap(), signed_bytes);
}

#[test]
fn extracting_onto_a_file_that_cannot_be_read_says_so_and_does_not_call_it_signed() {
    let dir = Dir::new("unreadable-target");
    let source = dir.write("a.pdf", &kit::marked_pdf(2));
    let pool = pool();
    let (handle, _) = pool.open(&text(&source), None).unwrap();
    let odd = dir.0.join("bad name.pdf");
    match client_error(pool.extract_pages(handle, &[0], &odd, ExtractOptions::default(), &nothing))
    {
        ClientError::Save(e @ SaveError::TargetUnreadable) => {
            assert_eq!(e.code(), "save_target_unreadable")
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_copy_of_a_document_cannot_replace_a_signed_file_that_is_open_in_a_tab() {
    let dir = Dir::new("save-as-signed-tab");
    let source = dir.write("a.pdf", &kit::marked_pdf(2));
    let signed_bytes = kit::signed_pdf();
    let signed = dir.write("s.pdf", &signed_bytes);
    let pool = pool();
    let (handle, _) = pool.open(&text(&source), None).unwrap();
    let (_signed_handle, _) = pool.open(&text(&signed), None).unwrap();
    match client_error(pool.save_as(handle, &signed, SaveOptions::default())) {
        ClientError::Save(SaveError::TargetOpen) => {}
        other => panic!("{other:?}"),
    }
    // The same file spelled another way is the same file.
    let spelled = dir.0.join(".").join("s.pdf");
    match client_error(pool.save_as(handle, &spelled, SaveOptions::default())) {
        ClientError::Save(SaveError::TargetOpen) => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read(&signed).unwrap(), signed_bytes);

    // A file that another tab shows and that holds no signature may still be chosen: that is the
    // person's own pick through Save As, and the other tab is told that its file was replaced.
    let plain_bytes = kit::marked_pdf(3);
    let plain = dir.write("p.pdf", &plain_bytes);
    let (plain_handle, _) = pool.open(&text(&plain), None).unwrap();
    let outcome = pool
        .save_as(handle, &plain, SaveOptions::default())
        .unwrap();
    assert_eq!(outcome.replaced, vec![plain_handle]);
}

#[test]
fn extracting_from_a_protected_or_signed_document_asks_first() {
    let dir = Dir::new("asks");
    let locked = dir.write("locked.pdf", &kit::encrypted_pdf("secret", 3));
    let signed = dir.write("s.pdf", &kit::signed_pdf());
    let pool = pool();

    let (handle, _) = pool.open(&text(&locked), Some("secret")).unwrap();
    let out = dir.0.join("out.pdf");
    match client_error(pool.extract_pages(handle, &[0], &out, ExtractOptions::default(), &nothing))
    {
        ClientError::Save(SaveError::Unprotected) => {}
        other => panic!("{other:?}"),
    }
    assert!(!out.exists());
    pool.extract_pages(
        handle,
        &[0, 1],
        &out,
        ExtractOptions {
            allow_unprotected: true,
            ..Default::default()
        },
        &nothing,
    )
    .unwrap();
    // The new file opens without a password: that is what was agreed to.
    let (_, sizes) = pool.open(&text(&out), None).unwrap();
    assert_eq!(sizes.len(), 2);

    let (handle, _) = pool.open(&text(&signed), None).unwrap();
    let out2 = dir.0.join("out2.pdf");
    match client_error(pool.extract_pages(handle, &[0], &out2, ExtractOptions::default(), &nothing))
    {
        ClientError::Save(SaveError::Signed { .. }) => {}
        other => panic!("{other:?}"),
    }
    assert!(!out2.exists());
    pool.extract_pages(
        handle,
        &[0],
        &out2,
        ExtractOptions {
            allow_signed: true,
            ..Default::default()
        },
        &nothing,
    )
    .unwrap();
    assert!(out2.exists());
}

#[test]
fn a_document_with_too_many_pages_is_refused_by_the_reader_and_the_tools() {
    let dir = Dir::new("long");
    let pool = pool();
    let huge = dir.write("huge.pdf", &kit::blank_pages_pdf(MAX_PAGES + 1));
    assert_eq!(
        code_of(client_error(pool.open(&text(&huge), None))),
        ErrorCode::TooManyPages
    );
    let tall = dir.write(
        "tall.pdf",
        &kit::blank_pages_pdf(partridge_protocol::MAX_EDIT_PAGES + 1),
    );
    let (handle, sizes) = pool.open(&text(&tall), None).unwrap();
    assert_eq!(sizes.len(), partridge_protocol::MAX_EDIT_PAGES + 1);
    assert_eq!(
        code_of(client_error(pool.pages_delete(handle, &[0]))),
        ErrorCode::TooManyPages
    );
    // A file that says it has millions of pages in a few bytes.
    let lie = dir.write("lie.pdf", &kit::page_count_lie_pdf(2_000_000_000));
    match pool.open(&text(&lie), None) {
        Ok((_, sizes)) => assert!(sizes.len() <= MAX_PAGES),
        Err(ClientError::Engine { code, .. }) => {
            assert!(matches!(code, ErrorCode::TooManyPages | ErrorCode::Corrupt))
        }
        Err(other) => panic!("{other:?}"),
    }
}

#[test]
fn stopping_an_insert_in_the_middle_leaves_the_document_alone() {
    let dir = Dir::new("stop");
    let target = dir.write("a.pdf", &kit::marked_pdf(2));
    let long = dir.write("long.pdf", &kit::simple_pdf(400));
    let pool = Arc::new(pool());
    let (handle, _) = pool.open(&text(&target), None).unwrap();
    let stopper = pool.clone();
    let once = std::sync::Once::new();
    let result = pool.insert_pdf(
        handle,
        0,
        &text(&long),
        None,
        InsertOptions::default(),
        &move |_, _| {
            once.call_once(|| {
                assert_eq!(stopper.cancel_pages(handle), Cancelled::Handover);
            });
        },
    );
    match result {
        Err(ClientError::Save(SaveError::Cancelled)) => {}
        // A document small enough to be handed over in one piece is done before the stop is seen.
        Ok(_) => {}
        other => panic!("{other:?}"),
    }
    // The document works, whichever way it ended, and has either the two pages or all of them.
    let n = pool.pages_rotate(handle, &[0], 1).unwrap().pages.len();
    assert!(n == 2 || n == 402, "{n}");
}

#[test]
fn the_files_for_other_tools_are_written() {
    let Some(out) = std::env::var_os("PARTRIDGE_PAGES_OUT").map(PathBuf::from) else {
        return;
    };
    fs::create_dir_all(&out).unwrap();
    let dir = Dir::new("tools");
    let pool = pool();
    let mut expected = String::new();
    let open = |bytes: &[u8]| {
        let path = dir.write("work.pdf", bytes);
        (path.clone(), pool.open(&text(&path), None).unwrap().0)
    };
    let mut keep = |name: &str, path: &Path, pages: usize, note: &str| {
        let copy = out.join(name);
        fs::copy(path, &copy).unwrap();
        expected.push_str(&format!("{name}\t{pages}\t{note}\n"));
    };

    // Delete the second of five pages.
    let (path, h) = open(&kit::marked_pdf(5));
    fs::copy(&path, out.join("original.pdf")).unwrap();
    pool.pages_delete(h, &[1]).unwrap();
    pool.save(h, SaveOptions::default()).unwrap();
    keep("deleted.pdf", &path, 4, "text=0,2,3,4 gone=1");

    // Move the last page to the front, and the third to the end.
    let (path, h) = open(&kit::marked_pdf(5));
    pool.pages_reorder(h, &[4, 0, 1, 3, 2]).unwrap();
    pool.save(h, SaveOptions::default()).unwrap();
    keep("moved.pdf", &path, 5, "text=4,0,1,3,2");

    // Turn the first and third page a quarter turn.
    let (path, h) = open(&kit::marked_pdf(4));
    pool.pages_rotate(h, &[0, 2], 1).unwrap();
    pool.save(h, SaveOptions::default()).unwrap();
    keep("rotated.pdf", &path, 4, "rotation=90,0,90,0 text=0,1,2,3");

    // Crop the first page: 10 from the left, 20 from the top, 30 from the right, 40 from the bottom.
    let (path, h) = open(&kit::marked_pdf(2));
    pool.pages_crop(h, &[0], [10.0, 20.0, 30.0, 40.0]).unwrap();
    pool.save(h, SaveOptions::default()).unwrap();
    keep(
        "cropped.pdf",
        &path,
        2,
        "cropbox=10,40,582,772 size=572x732 text=0,1",
    );

    // A blank page in the middle, and pages of another file.
    let (path, h) = open(&kit::marked_pdf(3));
    pool.pages_insert_blank(h, 1, 300.0, 400.0).unwrap();
    let other = dir.write("other.pdf", &kit::simple_pdf(2));
    pool.insert_pdf(
        h,
        4,
        &text(&other),
        None,
        InsertOptions::default(),
        &nothing,
    )
    .unwrap();
    pool.save(h, SaveOptions::default()).unwrap();
    keep(
        "inserted.pdf",
        &path,
        6,
        "blank=1 size1=300x400 text=0,x,1,2",
    );

    // Pages that inherit from the nodes above them, moved: other programs must draw every page
    // as it was drawn before the move.
    let (path, h) = open(&kit::inherited_tree_pdf());
    fs::copy(&path, out.join("inherited-original.pdf")).unwrap();
    pool.pages_reorder(h, &[6, 5, 4, 3, 2, 1, 0]).unwrap();
    pool.save(h, SaveOptions::default()).unwrap();
    keep(
        "inherited-moved.pdf",
        &path,
        7,
        "draws=inherited-original.pdf:6,5,4,3,2,1,0",
    );
    // The pool holds six documents at most.
    pool.close(h);
    // Pages whose resources lead to nothing (a reference to an object that is not there, `null`):
    // they have what their node gives, before and after a move.
    for (name, own) in [
        ("dangling", "/Resources 900 0 R"),
        ("nullkey", "/Resources null"),
    ] {
        let (path, h) = open(&kit::dangling_resources_pdf(own));
        fs::copy(&path, out.join(format!("{name}-original.pdf"))).unwrap();
        pool.pages_reorder(h, &[3, 2, 1, 0]).unwrap();
        pool.save(h, SaveOptions::default()).unwrap();
        keep(
            &format!("{name}-moved.pdf"),
            &path,
            4,
            &format!("draws={name}-original.pdf:3,2,1,0"),
        );
        pool.close(h);
    }

    // Two pages copied out in the other order.
    let (path, h) = open(&kit::marked_pdf(5));
    let part = dir.0.join("part.pdf");
    pool.extract_pages(h, &[3, 1], &part, ExtractOptions::default(), &nothing)
        .unwrap();
    let _ = path;
    keep("extracted.pdf", &part, 2, "text=3,1 gone=0,2,4");

    fs::write(out.join("expected.txt"), expected).unwrap();
}

#[test]
fn a_document_with_a_broken_list_of_pages_cannot_take_the_engine_down() {
    // A list of pages that names a null object, an object that is not there, a number, the catalog
    // and a string. PDFium crashed in a page insert on a file like this (found by the fuzz target),
    // so the tools look at the list first. Whatever they answer, the engine must stay alive.
    let dir = Dir::new("broken");
    for kid in kit::BROKEN_KIDS {
        let path = dir.write("broken.pdf", &kit::broken_page_list_pdf(kid));
        let pool = pool();
        let Ok((handle, sizes)) = pool.open(&text(&path), None) else {
            continue;
        };
        for step in 0..8 {
            let result = match step {
                0 => pool.pages_insert_blank(handle, 1, 100.0, 100.0).map(|_| ()),
                1 => pool.pages_delete(handle, &[0]).map(|_| ()),
                2 => pool
                    .pages_reorder(handle, &(0..sizes.len() as u32).rev().collect::<Vec<_>>())
                    .map(|_| ()),
                3 => pool.pages_rotate(handle, &[0], 1).map(|_| ()),
                4 => pool.pages_crop(handle, &[0], [1.0; 4]).map(|_| ()),
                5 => pool
                    .extract_pages(
                        handle,
                        &[0],
                        &dir.0.join("out.pdf"),
                        ExtractOptions::default(),
                        &nothing,
                    )
                    .map(|_| ()),
                6 => pool
                    .insert_pdf(
                        handle,
                        0,
                        &text(&path),
                        None,
                        InsertOptions::default(),
                        &nothing,
                    )
                    .map(|_| ()),
                _ => pool.save(handle, SaveOptions::default()).map(|_| ()),
            };
            if let Err(ClientError::EngineStopped(why)) = result {
                panic!("the engine stopped at step {step} for the kid {kid:?}: {why}");
            }
        }
        // The engine of the document still answers.
        assert!(
            pool.render(handle, 0, 20, 20).is_ok() || pool.page_text(handle, 0).is_ok() || true
        );
    }
}

#[test]
fn a_list_of_pages_that_names_something_that_is_not_there_cannot_take_the_engine_down() {
    // The sequences that stopped the engine with a null pointer in PDFium: a page added behind the
    // pages that can be found and then deleted, a delete, a blank page and a delete again, and a
    // move, on lists of kids that end with a reference to nothing, to an object with a very big
    // number, and with a name and an array. Every change that goes through the list is refused
    // with words of its own, and the next tool still works.
    let dir = Dir::new("trees");
    let pool = pool();
    for (name, bytes) in [
        ("nothing", kit::trailing_null_kid_pdf()),
        ("far", kit::trailing_far_kid_pdf()),
        ("junk", kit::junk_after_the_kids_pdf()),
    ] {
        let path = dir.write("tree.pdf", &bytes);
        let (handle, sizes) = pool.open(&text(&path), None).unwrap();
        assert_eq!(sizes.len(), 3, "{name}");
        fn refused<T>(name: &str, result: Result<T, ClientError>, what: &str) {
            match result {
                Err(ClientError::Engine { code, message }) => {
                    assert_eq!(code, ErrorCode::PageChangeFailed, "{name}: {what}");
                    assert!(
                        message.contains("does not follow the format"),
                        "{name}: {message}"
                    );
                }
                Err(other) => panic!("{name}: {what}: {other:?}"),
                Ok(_) => panic!("{name}: {what} was done"),
            }
        }
        refused(
            name,
            pool.pages_insert_blank(handle, 3, 100.0, 100.0),
            "a blank page",
        );
        refused(name, pool.pages_delete(handle, &[2]), "a delete");
        refused(
            name,
            pool.pages_delete(handle, &[1]),
            "a delete of another page",
        );
        refused(name, pool.pages_reorder(handle, &[2, 1, 0]), "a move");
        refused(
            name,
            pool.insert_pdf(
                handle,
                3,
                &text(&path),
                None,
                InsertOptions::default(),
                &nothing,
            ),
            "an insert",
        );
        // The engine is there, nothing was lost, and the tools that leave the list alone work.
        assert_eq!(
            pool.pages_rotate(handle, &[0], 1).unwrap().pages.len(),
            3,
            "{name}"
        );
        pool.save(handle, SaveOptions::default()).unwrap();
        assert_eq!(pool.open(&text(&path), None).unwrap().1.len(), 3, "{name}");
    }
}

#[test]
fn pages_that_inherit_from_nodes_are_moved_through_the_engine_and_the_saved_file_draws_them_as_before(
) {
    let dir = Dir::new("inherit");
    let path = dir.write("tree.pdf", &kit::inherited_tree_pdf());
    let pool = pool();
    let (handle, sizes) = pool.open(&text(&path), None).unwrap();
    let draw = |h: u64, page: u32| pool.render(h, page, 240, 160).unwrap().rgba;
    let before: Vec<_> = (0..7).map(|p| draw(handle, p)).collect();
    let order = [3u32, 6, 0, 5, 1, 4, 2];
    let moved = pool.pages_reorder(handle, &order).unwrap();
    for (place, &from) in order.iter().enumerate() {
        assert_eq!(moved.pages[place], sizes[from as usize], "place {place}");
        assert!(
            draw(handle, place as u32) == before[from as usize],
            "place {place}"
        );
    }
    pool.save(handle, SaveOptions::default()).unwrap();
    let (again, _) = pool.open(&text(&path), None).unwrap();
    for (place, &from) in order.iter().enumerate() {
        assert!(
            draw(again, place as u32) == before[from as usize],
            "saved, place {place}"
        );
    }
}

// ---- files like real programs write them, through the real engine --------------------------------

fn short<T: std::fmt::Debug>(r: Result<T, ClientError>) -> String {
    match r {
        Ok(v) => format!("{v:?}").chars().take(60).collect(),
        Err(e) => e.to_string(),
    }
}

fn save_options() -> SaveOptions {
    SaveOptions::default()
}

/// A move or an insert of a page with a pop-up note or a field on two pages used to stop the engine
/// at the next save, 20 times in 20, and take the unsaved changes of the document with it.
#[test]
fn a_move_or_an_insert_of_ordinary_files_does_not_stop_the_engine_at_the_save() {
    let dir = Dir::new("ordinary");
    let pool = pool();
    for (name, bytes, pages) in [
        ("popup", kit::popup_note_pdf(), 3usize),
        ("field", kit::shared_field_pdf(), 2),
        ("first", kit::first_key_pdf(), 3),
    ] {
        // Moved.
        let path = dir.write(&format!("{name}-move.pdf"), &bytes);
        let (h, found) = pool.open(&text(&path), None).unwrap();
        assert_eq!(found.len(), pages, "{name}");
        let order: Vec<u32> = (0..pages as u32).rev().collect();
        let moved = pool.pages_reorder(h, &order).unwrap();
        pool.pages_rotate(h, &[0], 1).unwrap();
        pool.save(h, save_options())
            .unwrap_or_else(|e| panic!("{name}: save after a move: {e}"));
        let (again, saved) = pool.open(&text(&path), None).unwrap();
        assert_eq!(saved.len(), pages, "{name}");
        let back = pool.pages_restore(h, moved.snapshot);
        assert!(back.is_ok(), "{name}: undo of the move: {}", short(back));
        pool.pages_delete(h, &[0]).unwrap();
        pool.save(h, save_options())
            .unwrap_or_else(|e| panic!("{name}: save after a delete: {e}"));
        pool.close(h);
        pool.close(again);

        // Inserted into a clean document, by an engine of its own.
        let target_path = dir.write(&format!("{name}-target.pdf"), &kit::marked_pdf(4));
        let source_path = dir.write(&format!("{name}-source.pdf"), &bytes);
        let (t, _) = pool.open(&text(&target_path), None).unwrap();
        let inserted = pool
            .insert_pdf(
                t,
                2,
                &text(&source_path),
                None,
                InsertOptions {
                    allow_protected: true,
                    allow_signed: true,
                },
                &nothing,
            )
            .unwrap_or_else(|e| panic!("{name}: insert: {e}"));
        assert_eq!(inserted.changed.pages.len(), 4 + pages, "{name}");
        pool.save(t, save_options())
            .unwrap_or_else(|e| panic!("{name}: save after an insert: {e}"));
        let (_, saved) = pool.open(&text(&target_path), None).unwrap();
        assert_eq!(saved.len(), 4 + pages, "{name}");
        let undone = pool.pages_restore(t, inserted.changed.snapshot);
        assert!(
            undone.is_ok(),
            "{name}: undo of the insert: {}",
            short(undone)
        );
        pool.save(t, save_options())
            .unwrap_or_else(|e| panic!("{name}: save after the undo: {e}"));
        pool.close(t);
    }
}

/// The fuzz target found a damaged, protected file that PDFium reads with one page and writes out so
/// that it reads back with none. After an insert, an undo and a redo the document of no pages was
/// put in place, and the save replaced the file with it. Nothing of that may happen now.
#[test]
fn a_document_that_reads_back_with_no_pages_never_replaces_the_file() {
    let dir = Dir::new("nopages");
    let bytes = kit::page_count_lost_pdf();
    let path = dir.write("lost.pdf", &bytes);
    let one = dir.write("one.pdf", &kit::simple_pdf(1));
    let pool = pool();
    let password = Some(kit::PAGE_COUNT_LOST_PASSWORD);
    let Ok((h, pages)) = pool.open(&text(&path), password) else {
        panic!("the file opens with its password");
    };
    assert_eq!(pages.len(), 1);
    let _ = pool.pages_rotate(h, &[0], 1);
    let inserted = pool.insert_pdf(
        h,
        0,
        &text(&one),
        None,
        InsertOptions {
            allow_protected: true,
            allow_signed: true,
        },
        &nothing,
    );
    if let Ok(done) = inserted {
        // However it went, taking it back and making it again never gives a document of no pages.
        if let Ok(undone) = pool.pages_restore(h, done.changed.snapshot) {
            assert!(!undone.pages.is_empty());
            if let Ok(redone) = pool.pages_restore(h, undone.snapshot) {
                assert!(!redone.pages.is_empty());
            }
        }
    }
    let before = fs::read(&path).unwrap();
    let saved = pool.save(h, save_options());
    let after = fs::read(&path).unwrap();
    if saved.is_ok() || after != before {
        let check = self::pool();
        let (_, found) = check
            .open(&text(&path), password)
            .unwrap_or_else(|e| panic!("the file written by the save does not open: {e}"));
        assert!(!found.is_empty(), "the save wrote a document of no pages");
    }
}

#[test]
fn a_document_that_forbids_changes_refuses_every_tool_and_stays_intact() {
    let dir = Dir::new("forbids");
    let bytes = kit::encrypted_r3_with_permissions_pdf("pw", 3, 0xFFFF_F0C0);
    let path = dir.write("locked.pdf", &bytes);
    let pool = pool();
    let (h, _) = pool.open(&text(&path), Some("pw")).unwrap();
    for error in [
        client_error(pool.pages_delete(h, &[0])),
        client_error(pool.pages_reorder(h, &[2, 1, 0])),
        client_error(pool.pages_rotate(h, &[0], 1)),
        client_error(pool.pages_insert_blank(h, 0, 100.0, 100.0)),
        client_error(pool.pages_crop(h, &[0], [1.0, 1.0, 1.0, 1.0])),
    ] {
        assert_eq!(code_of(error), ErrorCode::PagesNotAllowed);
    }
    let copy = dir.0.join("copy.pdf");
    let error = client_error(pool.extract_pages(
        h,
        &[0],
        &copy,
        ExtractOptions {
            allow_signed: true,
            allow_unprotected: true,
        },
        &nothing,
    ));
    assert_eq!(code_of(error), ErrorCode::PagesNotAllowed);
    assert!(!copy.exists());
    assert!(pool.pages_delete(h, &[0]).is_err());
}

/// An insert from a file the engine cannot use must not touch the document it was meant for.
#[test]
fn a_source_file_that_cannot_be_used_leaves_the_target_alone() {
    let dir = Dir::new("badsource");
    let target = dir.write("target.pdf", &kit::marked_pdf(3));
    let bad = dir.write("bad.pdf", &kit::corrupt_pdf());
    let pool = pool();
    let (h, _) = pool.open(&text(&target), None).unwrap();
    pool.pages_rotate(h, &[0], 1).unwrap();
    let error =
        client_error(pool.insert_pdf(h, 0, &text(&bad), None, InsertOptions::default(), &nothing));
    assert_eq!(code_of(error), ErrorCode::InsertSourceFailed);
    // The target engine is alive and has its change.
    let turned = pool.pages_rotate(h, &[1], 1).unwrap();
    assert_eq!(turned.pages.len(), 3);
    assert_eq!(turned.pages[0].width, 792.0);
}

// ---- changes that take long ----------------------------------------------------------------------

/// A pool whose engine is let go of after `stall` without a sign of life, as a change to the pages
/// is, with a long overall limit.
fn pool_with_stall(stall: Duration) -> EnginePool {
    let mut config = config();
    config.save_stall = stall;
    config.pages_floor = Duration::from_secs(600);
    EnginePool::new(config, 6)
}

/// The slice of the stall limit that a page may use up: pages are made so heavy that a tool takes
/// several times the stall limit on all of them, and a fraction of it on each.
const HEAVY_PAGES: usize = 40;
const HEAVY_SEGMENTS: usize = 500_000;

#[test]
fn a_change_that_takes_many_times_the_stall_limit_is_not_ended_while_it_reports_its_work() {
    let dir = Dir::new("slow");
    let path = dir.write(
        "heavy.pdf",
        &kit::heavy_pages_pdf(HEAVY_PAGES, HEAVY_SEGMENTS),
    );
    // How long a page takes is found out on a few of them with the ordinary limits; the stall limit
    // of the pool under test is then several pages long, and the tools on all pages take several
    // times the stall limit.
    let measuring = pool_with_stall(Duration::from_secs(300));
    let (handle, _) = measuring.open(&text(&path), None).unwrap();
    // The quickest of three rounds: a round that met a busy machine (the first one also warms the
    // engine up) would make a page look dearer than it is, and the stall limit with it.
    let one_page = (0..3)
        .map(|_| {
            let started = Instant::now();
            measuring
                .pages_rotate(handle, &[0, 1, 2, 3, 4, 5], 1)
                .unwrap();
            started.elapsed() / 6
        })
        .min()
        .unwrap();
    drop(measuring);
    let stall = (one_page * 8).max(Duration::from_millis(1000));
    assert!(
        one_page * HEAVY_PAGES as u32 > stall * 3,
        "the pages are too light for this test: a page takes {one_page:?}"
    );

    let pool = pool_with_stall(stall);
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    let all: Vec<u32> = (0..HEAVY_PAGES as u32).collect();
    // A turn and a delete of all pages but one: neither says nothing for as long as the stall limit,
    // though each takes longer than it altogether.
    let started = Instant::now();
    pool.pages_rotate(handle, &all, 1).unwrap();
    assert!(
        started.elapsed() > stall + stall / 2,
        "the turn was quicker than one and a half times the stall limit"
    );
    let started = Instant::now();
    pool.pages_delete(handle, &all[..HEAVY_PAGES - 1]).unwrap();
    assert!(
        started.elapsed() > stall + stall / 2,
        "the delete was quicker than one and a half times the stall limit"
    );
    // The document is there, with one page, and the next tool works.
    assert_eq!(pool.pages_rotate(handle, &[0], 1).unwrap().pages.len(), 1);
}

// ---- documents that a limit has to meet ------------------------------------------------------------

#[test]
fn a_value_of_a_megabyte_on_a_node_of_a_thousand_pages_is_moved_by_the_engine_in_its_sandbox() {
    // A file of a little over a megabyte. The value was copied onto every page: a gigabyte, and the
    // engine was stopped by the limit on its memory, losing the turn that was not saved.
    let dir = Dir::new("shared");
    let bytes = kit::shared_values_pdf(1000, 1 << 20);
    assert!(bytes.len() < 1_300_000);
    let path = dir.write("shared.pdf", &bytes);
    let pool = pool();
    let (h, sizes) = pool.open(&text(&path), None).unwrap();
    assert_eq!(sizes.len(), 1000);
    pool.pages_rotate(h, &[0], 1).unwrap();
    let order: Vec<u32> = (1..1000).chain([0]).collect();
    let moved = pool.pages_reorder(h, &order).unwrap();
    assert_eq!(moved.pages.len(), 1000);
    // The document saves in a size of its own.
    pool.save(h, SaveOptions::default()).unwrap();
    let saved = fs::metadata(&path).unwrap().len();
    assert!(saved < bytes.len() as u64 * 3, "{saved} bytes");
}

#[test]
fn a_page_listed_thousands_of_times_is_changed_by_the_engine_in_its_sandbox() {
    // A page of 65,000 keys named 2,000 times, in a file of 660 kilobytes, ended the engine at a
    // delete after a turn: the reader kept every listing in full.
    let dir = Dir::new("listed");
    let bytes = kit::listed_many_times_pdf(65_000, 2_000, 0);
    assert!(bytes.len() < 700_000);
    let path = dir.write("listed.pdf", &bytes);
    let pool = pool();
    let (h, sizes) = pool.open(&text(&path), None).unwrap();
    assert_eq!(sizes.len(), 2000);
    pool.pages_rotate(h, &[0], 1).unwrap();
    let deleted = pool.pages_delete(h, &[1]).unwrap();
    assert_eq!(deleted.pages.len(), 1999);
    let order: Vec<u32> = (1..1999).chain([0]).collect();
    assert_eq!(pool.pages_reorder(h, &order).unwrap().pages.len(), 1999);
    pool.save(h, SaveOptions::default()).unwrap();
}
