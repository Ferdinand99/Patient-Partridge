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
    let cropped = pool.pages_crop(handle, &[1], [10.0, 20.0, 30.0, 40.0]).unwrap();
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
    assert!(pool
        .page_text(handle, 1)
        .unwrap()
        .text
        .contains("Page 1"));
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
    assert_eq!(pool.page_text(handle, 2).map(|_| ()).unwrap_err().to_string().is_empty(), false);
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
    pool.insert_pdf(handle, 2, &text(&other), None, InsertOptions::default(), &nothing)
        .unwrap();
    pool.pages_delete(handle, &[0]).unwrap();
    pool.save(handle, SaveOptions::default()).unwrap();
    // Still needs the password, and has the pages.
    assert_eq!(
        code_of(client_error(pool.open(&text(&target), None))),
        ErrorCode::PasswordRequired
    );
    let (_, sizes) = pool.open(&text(&target), Some("secret")).unwrap();
    assert_eq!(sizes.len(), 2);
    // The inserted page is not readable in the file without the key.
    let saved = fs::read(&target).unwrap();
    assert!(!contains(&saved, "Line 3 of page 1"));
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
    for (name, bytes, code) in [
        ("plain.pdf", kit::not_a_pdf(), ErrorCode::Corrupt),
        ("broken.pdf", kit::corrupt_pdf(), ErrorCode::Corrupt),
        (
            "long.pdf",
            kit::blank_pages_pdf(MAX_PAGES + 1),
            ErrorCode::TooManyPages,
        ),
        (
            "tall.pdf",
            kit::blank_pages_pdf(partridge_protocol::MAX_EDIT_PAGES + 1),
            ErrorCode::TooManyPages,
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
    assert_eq!(dir.names(), vec!["a.pdf".to_string(), "part.pdf".to_string()]);
    assert_eq!(pool.page_text(handle, 4).unwrap().text.contains(&kit::page_text(4)), true);
    let (copy, sizes) = pool.open(&text(&out), None).unwrap();
    assert_eq!(sizes.len(), 2);
    assert!(has_page_text(&pool, copy, 0, 3));
    assert!(has_page_text(&pool, copy, 1, 1));
    // The same name again replaces the file (the dialog of the system asked about that).
    pool.extract_pages(handle, &[0], &out, ExtractOptions::default(), &nothing)
        .unwrap();
    let (copy2, sizes) = pool.open(&text(&out), None).unwrap();
    assert_eq!(sizes.len(), 1);
    assert!(has_page_text(&pool, copy2, 0, 0));
    assert_eq!(dir.names(), vec!["a.pdf".to_string(), "part.pdf".to_string()]);
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
    keep("cropped.pdf", &path, 2, "cropbox=10,40,582,772 size=572x732 text=0,1");

    // A blank page in the middle, and pages of another file.
    let (path, h) = open(&kit::marked_pdf(3));
    pool.pages_insert_blank(h, 1, 300.0, 400.0).unwrap();
    let other = dir.write("other.pdf", &kit::simple_pdf(2));
    pool.insert_pdf(h, 4, &text(&other), None, InsertOptions::default(), &nothing)
        .unwrap();
    pool.save(h, SaveOptions::default()).unwrap();
    keep("inserted.pdf", &path, 6, "blank=1 size1=300x400 text=0,x,1,2");

    // Two pages copied out in the other order.
    let (path, h) = open(&kit::marked_pdf(5));
    let part = dir.0.join("part.pdf");
    pool.extract_pages(h, &[3, 1], &part, ExtractOptions::default(), &nothing)
        .unwrap();
    let _ = path;
    keep("extracted.pdf", &part, 2, "text=3,1 gone=0,2,4");

    fs::write(out.join("expected.txt"), expected).unwrap();
}
