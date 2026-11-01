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
        let err = save::serialise_up_to(document, size - 1).unwrap_err();
        assert_eq!(err.0, ErrorCode::SaveTooLarge);
        assert_eq!(save::serialise_up_to(document, size).unwrap().len(), size);
        let err = save::serialise_up_to(document, 10).unwrap_err();
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
