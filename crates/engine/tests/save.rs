//! What the engine writes when it is asked to serialise a document: the three requests of a save,
//! and what PDFium does to encryption, signatures and edits on the way.
//!
//! These tests run the engine in this process, without the sandbox, because they have to change
//! the open document (through a test-only hook, `Engine::edit_for_tests`; nothing in the protocol
//! changes a document yet). The sandbox and the processes are covered by `tests/save_process.rs`.
//! PDFium wants one user at a time, so the tests take turns on one engine.

use std::fs::File;
use std::sync::{Mutex, MutexGuard, OnceLock};

use partridge_engine::save;
use partridge_engine::Engine;
use partridge_protocol::{
    ErrorCode, Request, RequestBody, ResponseBody, MAX_SAVE_CHUNK, PROTOCOL_VERSION,
};
use partridge_testkit as kit;
use pdfium_render::prelude::*;

/// The engine holds PDFium handles that are not `Send`. Only one test at a time touches it, and
/// PDFium has no thread it must be used from.
struct Shared(Engine);
unsafe impl Send for Shared {}

fn engine() -> MutexGuard<'static, Shared> {
    static ENGINE: OnceLock<Mutex<Shared>> = OnceLock::new();
    ENGINE
        .get_or_init(|| Mutex::new(Shared(Engine::new(&kit::pdfium_dir()))))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn ask(engine: &mut Engine, body: RequestBody) -> ResponseBody {
    engine.handle(Request { id: 1, body }).body
}

fn error_code(body: ResponseBody) -> ErrorCode {
    match body {
        ResponseBody::Error { code, .. } => code,
        other => panic!("expected an error, got {other:?}"),
    }
}

/// Opens `bytes` as a document; returns its id. The file lives as long as the test file value.
fn open(engine: &mut Engine, bytes: &[u8], password: Option<&str>) -> Result<u32, ErrorCode> {
    let file = kit::TempFile::new("save.pdf", bytes);
    engine.set_document(File::open(file.path()).unwrap());
    match ask(
        engine,
        RequestBody::Open {
            password: password.map(str::to_string),
        },
    ) {
        ResponseBody::Opened { doc, .. } => Ok(doc),
        other => Err(error_code(other)),
    }
}

/// The three requests of a save, with pieces of `piece` bytes. Returns the whole file.
fn save_all(engine: &mut Engine, doc: u32, piece: u32) -> Vec<u8> {
    let ResponseBody::SaveReady { size } = ask(engine, RequestBody::SaveBegin { doc }) else {
        panic!("no SaveReady");
    };
    let mut out = Vec::new();
    while (out.len() as u32) < size {
        let len = piece.min(size - out.len() as u32);
        match ask(
            engine,
            RequestBody::SaveChunk {
                doc,
                offset: out.len() as u32,
                len,
            },
        ) {
            ResponseBody::SaveData { offset, data } => {
                assert_eq!(offset as usize, out.len());
                assert_eq!(data.len(), len as usize);
                out.extend_from_slice(&data);
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(out.len() as u32, size);
    assert!(matches!(
        ask(engine, RequestBody::SaveEnd { doc }),
        ResponseBody::SaveEnded
    ));
    out
}

fn info(engine: &mut Engine, doc: u32) -> (u32, u8, u32) {
    match ask(engine, RequestBody::DocInfo { doc }) {
        ResponseBody::DocInfo {
            pages,
            security,
            signatures,
            ..
        } => (pages, security, signatures),
        other => panic!("{other:?}"),
    }
}

fn page_text(engine: &mut Engine, doc: u32, page: u32) -> String {
    match ask(engine, RequestBody::PageText { doc, page }) {
        ResponseBody::PageText { text, .. } => text,
        other => panic!("{other:?}"),
    }
}

/// Puts a line of text on the first page, the way an editing feature will.
fn add_text(engine: &mut Engine, doc: u32, text: &str) {
    engine
        .edit_for_tests(doc, |document| {
            let font = document.fonts_mut().helvetica();
            let mut page = document.pages().get(0).unwrap();
            page.objects_mut()
                .create_text_object(
                    PdfPoints::new(100.0),
                    PdfPoints::new(300.0),
                    text,
                    font,
                    PdfPoints::new(18.0),
                )
                .unwrap();
            page.regenerate_content().unwrap();
        })
        .expect("the document is open");
}

#[test]
fn a_saved_copy_of_an_unchanged_document_opens_and_reads_the_same() {
    let mut guard = engine();
    let e = &mut guard.0;
    let original = kit::simple_pdf(3);
    let doc = open(e, &original, None).unwrap();
    let saved = save_all(e, doc, 4096);
    assert!(saved.starts_with(b"%PDF-"));
    let again = open(e, &saved, None).unwrap();
    assert_eq!(info(e, again), (3, 0, 0));
    for page in 0..3 {
        assert_eq!(page_text(e, again, page), page_text(e, doc, page));
    }
}

#[test]
fn a_change_made_before_the_save_is_in_the_saved_file_and_not_in_the_open_original() {
    let mut guard = engine();
    let e = &mut guard.0;
    let original = kit::simple_pdf(2);
    let doc = open(e, &original, None).unwrap();
    assert!(!page_text(e, doc, 0).contains("EDITED-BY-TEST"));
    add_text(e, doc, "EDITED-BY-TEST");
    let saved = save_all(e, doc, MAX_SAVE_CHUNK as u32);
    let again = open(e, &saved, None).unwrap();
    assert!(page_text(e, again, 0).contains("EDITED-BY-TEST"));
    assert_eq!(info(e, again).0, 2);
    // The second page is untouched.
    assert_eq!(page_text(e, again, 1), page_text(e, doc, 1));
    // The bytes of the original file were not what came out.
    assert_ne!(saved, original);
}

#[test]
fn the_pieces_can_be_asked_for_in_any_order_and_again() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::simple_pdf(2), None).unwrap();
    let whole = save_all(e, doc, 1000);
    // A new SaveBegin makes the same bytes again (the document did not change).
    let ResponseBody::SaveReady { size } = ask(e, RequestBody::SaveBegin { doc }) else {
        panic!();
    };
    assert_eq!(size as usize, whole.len());
    let take = |e: &mut Engine, offset: u32, len: u32| match ask(
        e,
        RequestBody::SaveChunk { doc, offset, len },
    ) {
        ResponseBody::SaveData { data, .. } => data,
        other => panic!("{other:?}"),
    };
    let last = take(e, size - 10, 10);
    let first = take(e, 0, 10);
    assert_eq!(last, whole[whole.len() - 10..]);
    assert_eq!(first, whole[..10]);
    assert_eq!(take(e, 5, 10), whole[5..15]);
    // An empty piece at the very end is fine.
    assert!(take(e, size, 0).is_empty());
    ask(e, RequestBody::SaveEnd { doc });
}

#[test]
fn a_request_for_a_piece_that_does_not_exist_is_refused() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::simple_pdf(1), None).unwrap();
    let chunk = |offset, len| RequestBody::SaveChunk { doc, offset, len };
    // No SaveBegin yet.
    assert_eq!(error_code(ask(e, chunk(0, 10))), ErrorCode::InvalidRequest);
    let ResponseBody::SaveReady { size } = ask(e, RequestBody::SaveBegin { doc }) else {
        panic!();
    };
    // One byte past the end, an offset past the end, an offset that would overflow, a piece that
    // is bigger than the cap.
    assert_eq!(
        error_code(ask(e, chunk(size - 9, 10))),
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        error_code(ask(e, chunk(size + 1, 0))),
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        error_code(ask(e, chunk(u32::MAX, 10))),
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        error_code(ask(e, chunk(0, MAX_SAVE_CHUNK as u32 + 1))),
        ErrorCode::InvalidRequest
    );
    // After SaveEnd the buffer is gone.
    ask(e, RequestBody::SaveEnd { doc });
    assert_eq!(error_code(ask(e, chunk(0, 1))), ErrorCode::InvalidRequest);
    // And for a document that is not open.
    for body in [
        RequestBody::DocInfo { doc: 9999 },
        RequestBody::SaveBegin { doc: 9999 },
        RequestBody::SaveChunk {
            doc: 9999,
            offset: 0,
            len: 1,
        },
    ] {
        assert_eq!(error_code(ask(e, body)), ErrorCode::UnknownDocument);
    }
    // Closing the document frees a buffer that was left behind.
    ask(e, RequestBody::SaveBegin { doc });
    ask(e, RequestBody::Close { doc });
    assert_eq!(error_code(ask(e, chunk(0, 1))), ErrorCode::UnknownDocument);
}

#[test]
fn a_document_over_the_size_limit_is_not_handed_over() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::simple_pdf(2), None).unwrap();
    let mut size = 0;
    e.edit_for_tests(doc, |document| {
        let bytes = save::serialise(document).unwrap();
        size = bytes.len();
        // One byte too small is too small; the exact size is fine.
        let err = save::serialise_up_to(document, size - 1, &mut |_| {}).unwrap_err();
        assert_eq!(err.0, ErrorCode::SaveTooLarge);
        assert_eq!(
            save::serialise_up_to(document, size, &mut |_| {})
                .unwrap()
                .len(),
            size
        );
        let err = save::serialise_up_to(document, 10, &mut |_| {}).unwrap_err();
        assert_eq!(err.0, ErrorCode::SaveTooLarge);
    })
    .unwrap();
    assert!(size > 100);
}

#[test]
fn the_info_of_a_document_tells_pages_protection_and_signatures() {
    let mut guard = engine();
    let e = &mut guard.0;
    let plain = open(e, &kit::simple_pdf(4), None).unwrap();
    assert_eq!(info(e, plain), (4, 0, 0));
    let locked = open(e, &kit::encrypted_pdf("secret", 2), Some("secret")).unwrap();
    assert_eq!(info(e, locked), (2, 2, 0));
    let signed = open(e, &kit::signed_pdf(), None).unwrap();
    assert_eq!(info(e, signed), (1, 0, 1));
}

#[test]
fn an_empty_signature_field_is_not_a_signature() {
    let mut guard = engine();
    let e = &mut guard.0;
    // A form with a place to sign that nobody has signed holds no signature.
    let empty = open(e, &kit::forms_empty_signature_pdf(), None).unwrap();
    assert_eq!(info(e, empty), (1, 0, 0));
    // One signed field is one signature.
    let signed = open(e, &kit::forms_signed_pdf(), None).unwrap();
    assert_eq!(info(e, signed), (1, 0, 1));
    // A signed field and an empty one together are still one signature.
    let both = open(e, &kit::forms_signed_and_empty_signature_pdf(), None).unwrap();
    assert_eq!(info(e, both), (1, 0, 1));
}

#[test]
fn a_password_protected_document_stays_protected_when_it_is_saved() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::encrypted_pdf("secret", 2), Some("secret")).unwrap();
    add_text(e, doc, "SECRET-EDIT");
    let saved = save_all(e, doc, 2048);
    // The text must not be readable in the bytes.
    assert!(
        !saved.windows(11).any(|w| w == b"SECRET-EDIT"),
        "the edit is in the saved file in clear text"
    );
    // Without the password the file does not open, with a wrong one neither, with the right one it
    // does, shows the edit, and is protected in the same way.
    assert_eq!(
        open(e, &saved, None).unwrap_err(),
        ErrorCode::PasswordRequired
    );
    assert_eq!(
        open(e, &saved, Some("wrong")).unwrap_err(),
        ErrorCode::PasswordIncorrect
    );
    let again = open(e, &saved, Some("secret")).unwrap();
    assert_eq!(info(e, again), (2, 2, 0));
    assert!(page_text(e, again, 0).contains("SECRET-EDIT"));
}

#[test]
fn a_rewrite_does_not_keep_the_bytes_a_signature_covers() {
    let mut guard = engine();
    let e = &mut guard.0;
    let original = kit::signed_pdf();
    let doc = open(e, &original, None).unwrap();
    assert_eq!(info(e, doc).2, 1);
    let saved = save_all(e, doc, 4096);
    // An incremental save would start with every byte of the original, and a signature over those
    // bytes would still check. PDFium's save without flags writes the file anew.
    assert!(
        !saved.starts_with(&original),
        "the rewrite kept the original bytes: the save is incremental after all"
    );
    assert_ne!(saved.len(), original.len());
    // The signature itself is carried over, so the app can still say the document is signed.
    let again = open(e, &saved, None).unwrap();
    assert_eq!(info(e, again).2, 1);
}

#[test]
fn the_engine_speaks_the_new_protocol_version() {
    let mut guard = engine();
    let e = &mut guard.0;
    assert!(matches!(
        ask(
            e,
            RequestBody::Hello {
                protocol_version: PROTOCOL_VERSION
            }
        ),
        ResponseBody::Hello { .. }
    ));
    assert_eq!(
        error_code(ask(
            e,
            RequestBody::Hello {
                protocol_version: PROTOCOL_VERSION - 1
            }
        )),
        ErrorCode::ProtocolMismatch
    );
}

// ---- what the engine watches while a document is open: a source that fails or changes -----------

use partridge_engine::source::SourceFile;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

/// A file that can be made to fail, and that fails by itself after a number of reads.
struct Flaky {
    file: File,
    broken: Arc<AtomicBool>,
    reads_left: Arc<AtomicUsize>,
}

impl SourceFile for Flaky {
    fn read_at(&mut self, pos: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.broken.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("the share is gone"));
        }
        if self
            .reads_left
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_err()
        {
            self.broken.store(true, Ordering::SeqCst);
            return Err(std::io::Error::other("the share dropped"));
        }
        self.file.read_at(pos, buf)
    }

    fn state(&mut self) -> std::io::Result<(u64, Option<SystemTime>)> {
        self.file.state()
    }

    fn is_plain_file(&mut self) -> bool {
        true
    }
}

fn open_flaky(
    engine: &mut Engine,
    bytes: &[u8],
    reads: usize,
) -> (u32, Arc<AtomicBool>, kit::TempFile) {
    let file = kit::TempFile::new("flaky.pdf", bytes);
    let broken = Arc::new(AtomicBool::new(false));
    engine.set_document_source(Box::new(Flaky {
        file: File::open(file.path()).unwrap(),
        broken: broken.clone(),
        reads_left: Arc::new(AtomicUsize::new(reads)),
    }));
    let ResponseBody::Opened { doc, .. } = ask(engine, RequestBody::Open { password: None }) else {
        panic!("the document did not open");
    };
    (doc, broken, file)
}

#[test]
fn a_read_that_fails_before_a_save_makes_the_save_refuse() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, broken, _file) = open_flaky(e, &kit::simple_pdf(4), usize::MAX);
    assert!(matches!(
        ask(e, RequestBody::SaveBegin { doc }),
        ResponseBody::SaveReady { .. }
    ));
    // The share drops. PDFium would leave out what it cannot read, and the save would go on.
    broken.store(true, Ordering::SeqCst);
    assert_eq!(
        error_code(ask(e, RequestBody::SaveBegin { doc })),
        ErrorCode::SourceUnreadable
    );
    // The engine's own look failed, not a read of PDFium's, so nothing is missing from the
    // document: when the share is back the save works. (A read of PDFium's that fails stays on
    // record; the test below has one.)
    broken.store(false, Ordering::SeqCst);
    assert!(matches!(
        ask(e, RequestBody::SaveBegin { doc }),
        ResponseBody::SaveReady { .. }
    ));
}

#[test]
fn a_read_that_fails_in_the_middle_of_a_save_makes_the_save_refuse() {
    let mut guard = engine();
    let e = &mut guard.0;
    // Opening reads a few blocks; the file fails a little after that, while the pages are read.
    let (doc, _broken, _file) = open_flaky(e, &kit::simple_pdf(40), 6);
    let answer = ask(e, RequestBody::SaveBegin { doc });
    assert!(
        matches!(
            answer,
            ResponseBody::Error {
                code: ErrorCode::SourceUnreadable,
                ..
            }
        ),
        "{answer:?}"
    );
    // Nothing was handed out.
    assert_eq!(
        error_code(ask(
            e,
            RequestBody::SaveChunk {
                doc,
                offset: 0,
                len: 1
            }
        )),
        ErrorCode::InvalidRequest
    );
}

#[test]
fn a_file_changed_in_place_gives_source_changed_for_every_description_and_save() {
    let mut guard = engine();
    let e = &mut guard.0;
    let original = kit::simple_pdf(3);
    let file = kit::TempFile::new("inplace.pdf", &original);
    e.set_document(File::open(file.path()).unwrap());
    let ResponseBody::Opened { doc, .. } = ask(e, RequestBody::Open { password: None }) else {
        panic!();
    };
    assert!(page_text(e, doc, 0).contains("Page 1"));
    // Another program rewrites the file in place, with the same size.
    let mut theirs = original.clone();
    let at = theirs.windows(6).position(|w| w == b"Page 1").unwrap();
    theirs[at..at + 6].copy_from_slice(b"Page X");
    std::fs::write(file.path(), &theirs).unwrap();
    for body in [
        RequestBody::DocInfo { doc },
        RequestBody::Fingerprint { doc },
        RequestBody::SaveBegin { doc },
    ] {
        assert_eq!(error_code(ask(e, body)), ErrorCode::SourceChanged);
    }
}

#[test]
fn a_save_of_a_bigger_document_reports_its_progress() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::simple_pdf(6000), None).unwrap();
    let mut seen: Vec<u32> = Vec::new();
    let response = e.handle_with_progress(
        Request {
            id: 5,
            body: RequestBody::SaveBegin { doc },
        },
        &mut |done| seen.push(done),
    );
    let ResponseBody::SaveReady { size } = response.body else {
        panic!("{response:?}");
    };
    assert!(
        size > 2 * 1024 * 1024,
        "the test document is too small: {size}"
    );
    assert!(!seen.is_empty(), "no sign of life");
    assert!(seen.windows(2).all(|w| w[0] < w[1]), "{seen:?}");
    assert!(*seen.last().unwrap() <= size);
}

#[test]
fn the_reads_of_the_guard_report_progress_too() {
    let mut guard = engine();
    let e = &mut guard.0;
    // One page and 6 MiB of padding: the page loops report nothing, so every sign of life below
    // comes from the engine reading the file through the guard.
    let doc = open(e, &kit::padded_pdf(6 * 1024 * 1024), None).unwrap();
    for (name, body) in [
        ("DocInfo", RequestBody::DocInfo { doc }),
        ("Fingerprint", RequestBody::Fingerprint { doc }),
    ] {
        let mut seen: Vec<u32> = Vec::new();
        let response = e.handle_with_progress(Request { id: 9, body }, &mut |done| seen.push(done));
        assert!(
            !matches!(response.body, ResponseBody::Error { .. }),
            "{name}: {response:?}"
        );
        // The first DocInfo reads the whole file, the others read it again to check it.
        assert!(seen.len() >= 5, "{name}: too few signs of life: {seen:?}");
        assert!(seen.windows(2).all(|w| w[0] < w[1]), "{name}: {seen:?}");
    }
}

fn print_of(engine: &mut Engine, doc: u32) -> (u32, u32, u64) {
    match ask(engine, RequestBody::Fingerprint { doc }) {
        ResponseBody::Fingerprint {
            pages,
            failed,
            digest,
        } => (pages, failed, digest),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_fingerprint_of_a_document_is_the_one_of_the_file_it_is_saved_to() {
    let mut guard = engine();
    let e = &mut guard.0;
    for (name, bytes) in [
        ("plain", kit::simple_pdf(5)),
        ("features", kit::features_pdf()),
        ("mixed", kit::mixed_size_pdf()),
        ("links", kit::many_links_pdf(12)),
        ("signed", kit::signed_pdf()),
    ] {
        let doc = open(e, &bytes, None).unwrap();
        let live = print_of(e, doc);
        assert_eq!(live.1, 0, "{name}: pages that could not be read");
        let saved = save_all(e, doc, 65536);
        let again = open(e, &saved, None).unwrap();
        assert_eq!(print_of(e, again), live, "{name}");
    }
    // With a change in memory the file that is saved still matches the document, and not the
    // document as it was.
    let doc = open(e, &kit::simple_pdf(2), None).unwrap();
    let before = print_of(e, doc);
    add_text(e, doc, "A NEW LINE OF TEXT");
    let after = print_of(e, doc);
    assert_ne!(before.2, after.2);
    let saved = save_all(e, doc, 65536);
    let again = open(e, &saved, None).unwrap();
    assert_eq!(print_of(e, again), after);
}

#[test]
fn the_fingerprint_sees_a_page_that_lost_its_text_or_its_size() {
    let mut guard = engine();
    let e = &mut guard.0;
    let original = kit::simple_pdf(3);
    let a = open(e, &original, None).unwrap();
    // The same document with the second page's text turned into other letters ...
    let mut changed = original.clone();
    let at = changed
        .windows(6)
        .position(|w| w == b"Page 2")
        .expect("page 2");
    changed[at..at + 6].copy_from_slice(b"Page Z");
    let b = open(e, &changed, None).unwrap();
    assert_ne!(print_of(e, a).2, print_of(e, b).2);
    // ... and one whose pages have other sizes.
    let c = open(e, &kit::mixed_size_pdf(), None).unwrap();
    assert_ne!(print_of(e, a).2, print_of(e, c).2);
}

#[test]
fn a_signature_field_under_a_grouping_field_is_found_two_ways() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::signed_nested_pdf(), None).unwrap();
    match ask(e, RequestBody::DocInfo { doc }) {
        ResponseBody::DocInfo {
            signatures,
            maybe_signed,
            ..
        } => {
            // The words of a signature are in the file whatever the field tree looks like.
            assert!(maybe_signed);
            eprintln!("signatures found by PDFium and the widgets: {signatures}");
        }
        other => panic!("{other:?}"),
    }
    let plain = open(e, &kit::simple_pdf(2), None).unwrap();
    match ask(e, RequestBody::DocInfo { doc: plain }) {
        ResponseBody::DocInfo {
            signatures,
            maybe_signed,
            ..
        } => assert_eq!((signatures, maybe_signed), (0, false)),
        other => panic!("{other:?}"),
    }
}
