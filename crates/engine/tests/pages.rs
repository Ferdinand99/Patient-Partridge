//! The page tools, with the real PDFium: what a delete, a move, a rotation, a crop, an insert and
//! an extract do to a document, read back from the bytes a save writes.
//!
//! These tests run the engine in this process, without the sandbox (the sandbox and the processes
//! are covered by `tests/pages_process.rs`). PDFium wants one user at a time, so the tests take
//! turns on one engine.

use std::fs::File;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use partridge_engine::pages::PageTools;
use partridge_engine::Engine;
use partridge_protocol::{
    ErrorCode, LinkTarget, OutlineItem, PageLink, PageSize, PagesRequest, PagesResponse, Request,
    RequestBody, ResponseBody, MAX_EDIT_PAGES, MAX_PAGES, MAX_SAVE_CHUNK,
};
use partridge_testkit::{self as kit, contains, page_secrets, page_text, TempFile};
use pdfium_render::prelude::*;

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

fn code_of(body: ResponseBody) -> ErrorCode {
    match body {
        ResponseBody::Error { code, .. } => code,
        other => panic!("expected an error, got {other:?}"),
    }
}

fn open(engine: &mut Engine, bytes: &[u8], password: Option<&str>) -> Result<u32, ErrorCode> {
    let file = TempFile::new("pages.pdf", bytes);
    engine.set_document(File::open(file.path()).unwrap());
    match ask(
        engine,
        RequestBody::Open {
            password: password.map(str::to_string),
        },
    ) {
        ResponseBody::Opened { doc, .. } => Ok(doc),
        other => Err(code_of(other)),
    }
}

/// Opens a document and also returns the file it lives in, which must outlive the test.
fn open_doc(engine: &mut Engine, bytes: &[u8]) -> (u32, TempFile) {
    let file = TempFile::new("pages.pdf", bytes);
    engine.set_document(File::open(file.path()).unwrap());
    match ask(engine, RequestBody::Open { password: None }) {
        ResponseBody::Opened { doc, .. } => (doc, file),
        other => panic!("{other:?}"),
    }
}

fn page_op(engine: &mut Engine, op: PagesRequest) -> Result<PagesResponse, ErrorCode> {
    match ask(engine, RequestBody::Pages(op)) {
        ResponseBody::Pages(r) => Ok(r),
        other => Err(code_of(other)),
    }
}

/// (sizes, snapshot, count) of a `Changed` answer.
fn changed(r: Result<PagesResponse, ErrorCode>) -> (Vec<PageSize>, u32, u32) {
    match r.expect("the page change failed") {
        PagesResponse::Changed {
            pages,
            snapshot,
            count,
            ..
        } => (pages, snapshot, count),
        other => panic!("{other:?}"),
    }
}

fn save_all(engine: &mut Engine, doc: u32) -> Vec<u8> {
    let ResponseBody::SaveReady { size } = ask(engine, RequestBody::SaveBegin { doc }) else {
        panic!("no SaveReady");
    };
    let mut out = Vec::new();
    while (out.len() as u32) < size {
        let len = (MAX_SAVE_CHUNK as u32).min(size - out.len() as u32);
        match ask(
            engine,
            RequestBody::SaveChunk {
                doc,
                offset: out.len() as u32,
                len,
            },
        ) {
            ResponseBody::SaveData { data, .. } => out.extend_from_slice(&data),
            other => panic!("{other:?}"),
        }
    }
    ask(engine, RequestBody::SaveEnd { doc });
    out
}

fn text(engine: &mut Engine, doc: u32, page: u32) -> String {
    match ask(engine, RequestBody::PageText { doc, page }) {
        ResponseBody::PageText { text, .. } => text,
        other => panic!("{other:?}"),
    }
}

/// Which marked page (counted from 0 in the document that was opened) each page is now, read from
/// the text.
fn order_of(engine: &mut Engine, doc: u32, pages: usize) -> Vec<usize> {
    (0..pages as u32)
        .map(|p| {
            let t = text(engine, doc, p);
            let start = t.find("PAGE-TEXT-").expect("a page without its text") + 10;
            t[start..].split('-').next().unwrap().parse().unwrap()
        })
        .collect()
}

fn outline(engine: &mut Engine, doc: u32) -> Vec<OutlineItem> {
    match ask(engine, RequestBody::Outline { doc }) {
        ResponseBody::Outline { items } => items,
        other => panic!("{other:?}"),
    }
}

fn links(engine: &mut Engine, doc: u32, page: u32) -> Vec<PageLink> {
    match ask(engine, RequestBody::PageLinks { doc, page }) {
        ResponseBody::PageLinks { links } => links,
        other => panic!("{other:?}"),
    }
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

fn fingerprint(engine: &mut Engine, doc: u32) -> (u32, u32, u64) {
    match ask(engine, RequestBody::Fingerprint { doc }) {
        ResponseBody::Fingerprint {
            pages,
            failed,
            digest,
        } => (pages, failed, digest),
        other => panic!("{other:?}"),
    }
}

/// Counts and values read straight from PDFium, for the things the protocol does not report.
fn notes_on(engine: &mut Engine, doc: u32, page: u32) -> usize {
    engine
        .edit_for_tests(doc, |d| {
            d.pages()
                .get(page as PdfPageIndex)
                .unwrap()
                .annotations()
                .iter()
                .filter(|a| a.annotation_type() == PdfPageAnnotationType::Text)
                .count()
        })
        .unwrap()
}

fn field_values_on(engine: &mut Engine, doc: u32, page: u32) -> Vec<String> {
    engine
        .edit_for_tests(doc, |d| {
            let p = d.pages().get(page as PdfPageIndex).unwrap();
            p.annotations()
                .iter()
                .filter_map(|a| {
                    a.as_form_field()
                        .and_then(|f| f.as_text_field().and_then(|t| t.value()))
                })
                .collect()
        })
        .unwrap()
}

fn assert_has(saved: &[u8], page: usize) {
    for s in page_secrets(page) {
        assert!(contains(saved, &s.plain), "page {page} lost its {}", s.what);
    }
}

fn assert_gone(saved: &[u8], page: usize) {
    for s in page_secrets(page) {
        assert!(
            !contains(saved, &s.plain),
            "the file still holds the {} of deleted page {page}",
            s.what
        );
    }
}

// ---- delete ------------------------------------------------------------------------------------

#[test]
fn a_deleted_page_is_gone_from_the_bytes_of_the_saved_file() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::marked_pdf(4));
    // The control: before anything is deleted, the saved file holds the secrets of every page. If
    // PDFium compressed or hid them, the check below would pass for the wrong reason.
    let before = save_all(e, doc);
    for page in 0..4 {
        assert_has(&before, page);
    }

    let (sizes, snapshot, count) = changed(page_op(
        e,
        PagesRequest::Delete {
            doc,
            pages: vec![1],
        },
    ));
    assert_eq!((sizes.len(), count), (3, 1));
    assert_ne!(snapshot, 0, "a copy to go back to was kept");

    let after = save_all(e, doc);
    assert_gone(&after, 1);
    for page in [0, 2, 3] {
        assert_has(&after, page);
    }
    assert!(after.len() < before.len(), "the file did not get smaller");
    // The pages that are left are the right ones, in order.
    assert_eq!(order_of(e, doc, 3), vec![0, 2, 3]);
}

#[test]
fn the_form_fields_of_a_deleted_page_are_counted_and_their_text_is_cleared() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::marked_pdf(3));
    let PagesResponse::Changed { form_fields, .. } = page_op(
        e,
        PagesRequest::Delete {
            doc,
            pages: vec![1, 2],
        },
    )
    .unwrap() else {
        panic!("no answer");
    };
    assert_eq!(form_fields, 2);
    let saved = save_all(e, doc);
    // What was typed is gone. The field itself is not: PDFium keeps the list of fields of a form,
    // and the window tells the user. This is asserted so that a PDFium that drops the field (or
    // keeps the text) is noticed.
    for page in [1, 2] {
        assert!(
            !contains(&saved, &format!("VALUE-{page}-SECRET")),
            "text of page {page}"
        );
        assert!(
            contains(&saved, &format!("field{page}")),
            "name of field {page}"
        );
    }
    assert!(contains(&saved, "VALUE-0-SECRET"));
}

#[test]
fn deleting_several_pages_removes_all_of_them() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::marked_pdf(6));
    changed(page_op(
        e,
        PagesRequest::Delete {
            doc,
            // Unsorted and with a repeat: the engine sorts and counts each page once.
            pages: vec![4, 1, 1, 0],
        },
    ));
    let saved = save_all(e, doc);
    for page in [0, 1, 4] {
        assert_gone(&saved, page);
    }
    for page in [2, 3, 5] {
        assert_has(&saved, page);
    }
    assert_eq!(order_of(e, doc, 3), vec![2, 3, 5]);
}

#[test]
fn what_stays_after_a_delete_still_works() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::marked_pdf(4));
    changed(page_op(
        e,
        PagesRequest::Delete {
            doc,
            pages: vec![1],
        },
    ));
    // Bookmarks: those of the pages that stay lead to them; the one of the deleted page stays in
    // the list and leads nowhere.
    let items = outline(e, doc);
    let pages: Vec<(String, Option<u32>)> = items.into_iter().map(|i| (i.title, i.page)).collect();
    assert_eq!(
        pages,
        vec![
            ("Chapter 0".into(), Some(0)),
            ("Chapter 1".into(), None),
            ("Chapter 2".into(), Some(1)),
            ("Chapter 3".into(), Some(2)),
        ]
    );
    // Links: page 0 linked to the deleted page and now links to nothing that can be shown; page 2
    // (now 1) links to the last page (now 2); the last page links to the first.
    assert!(links(e, doc, 0).is_empty());
    assert_eq!(links(e, doc, 1)[0].target, LinkTarget::Page(2));
    assert_eq!(links(e, doc, 2)[0].target, LinkTarget::Page(0));
    // Notes and form fields stay on their pages.
    assert_eq!(notes_on(e, doc, 0), 1);
    assert_eq!(
        field_values_on(e, doc, 1),
        vec!["VALUE-2-SECRET".to_string()]
    );
    // The information dictionary and the viewer settings are kept.
    let saved = save_all(e, doc);
    assert!(contains(&saved, "INFO-TITLE-MARKER"));
    assert!(contains(&saved, "/HideToolbar"));
}

#[test]
fn the_last_page_cannot_be_deleted_and_bad_lists_are_refused() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::marked_pdf(3));
    for (pages, code) in [
        (vec![0, 1, 2], ErrorCode::InvalidRequest),
        (vec![], ErrorCode::InvalidRequest),
        (vec![3], ErrorCode::PageOutOfRange),
        (vec![u32::MAX], ErrorCode::PageOutOfRange),
    ] {
        assert_eq!(
            page_op(e, PagesRequest::Delete { doc, pages }).unwrap_err(),
            code
        );
    }
    // Nothing changed.
    assert_eq!(order_of(e, doc, 3), vec![0, 1, 2]);
    assert_eq!(
        page_op(
            e,
            PagesRequest::Delete {
                doc: 999,
                pages: vec![0]
            }
        )
        .unwrap_err(),
        ErrorCode::UnknownDocument
    );
}

// ---- reorder -----------------------------------------------------------------------------------

#[test]
fn pages_move_to_their_new_places() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::marked_pdf(5));
    let (sizes, snapshot, _) = changed(page_op(
        e,
        PagesRequest::Reorder {
            doc,
            order: vec![4, 2, 0, 3, 1],
        },
    ));
    assert_eq!(sizes.len(), 5);
    assert_ne!(snapshot, 0);
    assert_eq!(order_of(e, doc, 5), vec![4, 2, 0, 3, 1]);
    // A move copies nothing, so every link, bookmark and field goes with its page: the page at
    // place `p` was page `order[p]`, and its link names the page that followed it (the last links
    // to the first), which is now at the place where `order` holds that number.
    let order = [4usize, 2, 0, 3, 1];
    let place_of = |page: usize| order.iter().position(|&p| p == page).unwrap() as u32;
    for (place, &page) in order.iter().enumerate() {
        assert_eq!(
            links(e, doc, place as u32)[0].target,
            LinkTarget::Page(place_of((page + 1) % 5)),
            "the link of the page at place {place}"
        );
    }
    let items = outline(e, doc);
    for (page, item) in items.iter().enumerate() {
        assert_eq!(
            item.page,
            Some(place_of(page)),
            "the bookmark of page {page}"
        );
    }
    // Everything of every page is still in the file, once.
    let saved = save_all(e, doc);
    for page in 0..5 {
        assert_has(&saved, page);
    }
    // Notes and fields went with their pages.
    for place in 0..5 {
        assert_eq!(notes_on(e, doc, place), 1, "the note of place {place}");
        assert_eq!(field_values_on(e, doc, place).len(), 1);
    }
    assert_eq!(
        field_values_on(e, doc, 0),
        vec!["VALUE-4-SECRET".to_string()]
    );
}

#[test]
fn a_move_changes_the_order_only_and_keeps_links_and_bookmarks() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::marked_pdf(4));
    // The last page goes to the front; the other three keep their order.
    let (_, _, count) = changed(page_op(
        e,
        PagesRequest::Reorder {
            doc,
            order: vec![3, 0, 1, 2],
        },
    ));
    assert_eq!(count, 1, "one page had to move");
    assert_eq!(order_of(e, doc, 4), vec![3, 0, 1, 2]);
    // Page 0 (now at place 1) still links to page 1 (now at place 2), page 1 to page 2 (place 3),
    // and the page that moved (the last, now first) keeps its link to the first page (place 1),
    // which links to it from the end.
    assert_eq!(links(e, doc, 1)[0].target, LinkTarget::Page(2));
    assert_eq!(links(e, doc, 2)[0].target, LinkTarget::Page(3));
    assert_eq!(links(e, doc, 3)[0].target, LinkTarget::Page(0));
    assert_eq!(links(e, doc, 0)[0].target, LinkTarget::Page(1));
    // Every bookmark leads to its page, also that of the page that moved.
    let items = outline(e, doc);
    assert_eq!(items[0].page, Some(1));
    assert_eq!(items[1].page, Some(2));
    assert_eq!(items[2].page, Some(3));
    assert_eq!(items[3].page, Some(0));
}

#[test]
fn a_reorder_that_changes_nothing_changes_nothing() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::marked_pdf(3));
    let (sizes, snapshot, count) = changed(page_op(
        e,
        PagesRequest::Reorder {
            doc,
            order: vec![0, 1, 2],
        },
    ));
    assert_eq!((sizes.len(), snapshot, count), (3, 0, 0));
}

#[test]
fn a_new_order_must_name_every_page_once() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::marked_pdf(3));
    for order in [
        vec![0, 1],
        vec![0, 1, 1],
        vec![0, 1, 3],
        vec![0, 1, 2, 3],
        vec![],
    ] {
        assert_eq!(
            page_op(e, PagesRequest::Reorder { doc, order }).unwrap_err(),
            ErrorCode::InvalidRequest
        );
    }
    assert_eq!(order_of(e, doc, 3), vec![0, 1, 2]);
}

#[test]
fn reversing_a_long_document_finishes_in_reasonable_time() {
    let mut guard = engine();
    let e = &mut guard.0;
    let n = 1500;
    let (doc, _file) = open_doc(e, &kit::marked_pdf(n));
    let started = Instant::now();
    let order: Vec<u32> = (0..n as u32).rev().collect();
    changed(page_op(e, PagesRequest::Reorder { doc, order }));
    assert!(
        started.elapsed() < Duration::from_secs(120),
        "{:?}",
        started.elapsed()
    );
    assert!(text(e, doc, 0).contains(&page_text(n - 1)));
    assert!(text(e, doc, n as u32 - 1).contains(&page_text(0)));
}

// ---- rotate ------------------------------------------------------------------------------------

#[test]
fn rotating_swaps_the_sides_and_three_quarter_turns_undo_one() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::mixed_size_pdf());
    // Pages: 612x792, 842x595, 300x300, 200x1000.
    let (sizes, snapshot, count) = changed(page_op(
        e,
        PagesRequest::Rotate {
            doc,
            pages: vec![0, 3],
            turns: 1,
        },
    ));
    assert_eq!(
        (snapshot, count),
        (0, 2),
        "a turn needs no copy of the document"
    );
    assert_eq!((sizes[0].width, sizes[0].height), (792.0, 612.0));
    assert_eq!((sizes[1].width, sizes[1].height), (842.0, 595.0));
    assert_eq!((sizes[3].width, sizes[3].height), (1000.0, 200.0));
    // Three more quarter turns on the same pages bring them back.
    let (sizes, ..) = changed(page_op(
        e,
        PagesRequest::Rotate {
            doc,
            pages: vec![0, 3],
            turns: 3,
        },
    ));
    assert_eq!((sizes[0].width, sizes[0].height), (612.0, 792.0));
    assert_eq!((sizes[3].width, sizes[3].height), (200.0, 1000.0));
    // Two quarter turns keep the sides and turn the page over.
    let (sizes, ..) = changed(page_op(
        e,
        PagesRequest::Rotate {
            doc,
            pages: vec![0],
            turns: 2,
        },
    ));
    assert_eq!((sizes[0].width, sizes[0].height), (612.0, 792.0));
    // The turn is written by a save and read back.
    let saved = save_all(e, doc);
    let (again, _file2) = open_doc(e, &saved);
    assert_eq!(
        engine_page_rotations(e, again),
        vec![2, 0, 0, 0],
        "rotations in quarter turns after a save"
    );
}

fn engine_page_rotations(engine: &mut Engine, doc: u32) -> Vec<u8> {
    engine
        .edit_for_tests(doc, |d| {
            d.pages()
                .iter()
                .map(|p| match p.rotation().unwrap() {
                    PdfPageRenderRotation::None => 0,
                    PdfPageRenderRotation::Degrees90 => 1,
                    PdfPageRenderRotation::Degrees180 => 2,
                    PdfPageRenderRotation::Degrees270 => 3,
                })
                .collect()
        })
        .unwrap()
}

#[test]
fn bad_turns_and_pages_are_refused_and_change_nothing() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::mixed_size_pdf());
    for (pages, turns, code) in [
        (vec![0], 0u8, ErrorCode::InvalidRequest),
        (vec![0], 4, ErrorCode::InvalidRequest),
        (vec![0], 255, ErrorCode::InvalidRequest),
        (vec![], 1, ErrorCode::InvalidRequest),
        (vec![0, 4], 1, ErrorCode::PageOutOfRange),
    ] {
        assert_eq!(
            page_op(e, PagesRequest::Rotate { doc, pages, turns }).unwrap_err(),
            code
        );
    }
    assert_eq!(engine_page_rotations(e, doc), vec![0, 0, 0, 0]);
}

#[test]
fn odd_rotation_entries_are_turned_to_a_real_rotation() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::odd_rotation_pdf());
    let n = 9;
    let started = Instant::now();
    let (sizes, ..) = changed(page_op(
        e,
        PagesRequest::Rotate {
            doc,
            pages: (0..n).collect(),
            turns: 1,
        },
    ));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(sizes.len() as u32, n);
    for r in engine_page_rotations(e, doc) {
        assert!(r < 4);
    }
    // And the document can still be saved and opened again.
    let saved = save_all(e, doc);
    assert!(saved.starts_with(b"%PDF-"));
    let (again, _f) = open_doc(e, &saved);
    assert_eq!(info(e, again).0, n);
}

// ---- crop --------------------------------------------------------------------------------------

fn turned_pdf(turns: u32) -> Vec<u8> {
    // One Letter page with text, turned `turns` quarter turns by its own entry.
    let spec = kit::PageSpec {
        width: 612,
        height: 792,
        content: Some("BT /F1 24 Tf 100 600 Td (CROPME) Tj ET\n".into()),
        extra: format!("/Rotate {}", turns * 90),
    };
    kit::pdf_from_specs(&[spec])
}

#[test]
fn a_crop_takes_the_margins_off_the_page_as_it_is_drawn_whatever_its_turn() {
    let mut guard = engine();
    let e = &mut guard.0;
    for turns in 0..4 {
        let (doc, _file) = open_doc(e, &turned_pdf(turns));
        let word = |e: &mut Engine| match ask(e, RequestBody::PageText { doc, page: 0 }) {
            ResponseBody::PageText { rects, text } => rects[text.find("CROPME").unwrap()],
            other => panic!("{other:?}"),
        };
        // A crop with no margins leaves the page as it is, and says how big it is drawn.
        let PagesResponse::Cropped { pages: whole, .. } = page_op(
            e,
            PagesRequest::Crop {
                doc,
                pages: vec![0],
                margins: [0.0; 4],
            },
        )
        .unwrap() else {
            panic!("no crop answer");
        };
        assert_eq!(
            (whole[0].width, whole[0].height),
            if turns % 2 == 0 {
                (612.0, 792.0)
            } else {
                (792.0, 612.0)
            }
        );
        let first = word(e);
        let PagesResponse::Cropped {
            pages,
            before,
            after,
        } = page_op(
            e,
            PagesRequest::Crop {
                doc,
                pages: vec![0],
                margins: [10.0, 20.0, 30.0, 40.0],
            },
        )
        .unwrap()
        else {
            panic!("no crop answer");
        };
        // The drawn page lost exactly the margins: left and right from the width, top and bottom
        // from the height.
        assert_eq!(
            pages[0].width,
            whole[0].width - 40.0,
            "width at {turns} turns"
        );
        assert_eq!(
            pages[0].height,
            whole[0].height - 60.0,
            "height at {turns} turns"
        );
        assert_ne!(before[0], after[0]);
        // The text moved by the left and the top margin on the drawn page, so each margin was
        // taken from its own side.
        let moved = word(e);
        assert!(
            (first[0] - moved[0] - 10.0).abs() < 0.5,
            "left, {turns} turns: {first:?} {moved:?}"
        );
        assert!(
            (first[1] - moved[1] - 20.0).abs() < 0.5,
            "top, {turns} turns: {first:?} {moved:?}"
        );
    }
}

#[test]
fn a_crop_hides_the_edges_and_removes_nothing() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::marked_pdf(2));
    let print_before = fingerprint(e, doc);
    let PagesResponse::Cropped { .. } = page_op(
        e,
        PagesRequest::Crop {
            doc,
            pages: vec![0, 1],
            margins: [200.0, 200.0, 200.0, 200.0],
        },
    )
    .unwrap() else {
        panic!("no crop answer");
    };
    let saved = save_all(e, doc);
    // The words, the picture and the note of the page are all in the file ...
    for page in 0..2 {
        assert_has(&saved, page);
    }
    // ... the media box did not change, and the crop box is there ...
    assert!(
        contains(&saved, "/MediaBox[ 0 0 612 792]") || contains(&saved, "/MediaBox [0 0 612 792]")
    );
    assert!(contains(&saved, "/CropBox"));
    // ... and the text is still on the page for anything that reads it.
    assert!(text(e, doc, 0).contains(&page_text(0)));
    // The page is smaller as it is drawn, so a fingerprint (which includes the size) differs.
    assert_ne!(fingerprint(e, doc), print_before);
}

#[test]
fn a_crop_can_be_taken_back_and_made_again_with_the_boxes_it_reported() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::mixed_size_pdf());
    let start = fingerprint(e, doc);
    let PagesResponse::Cropped {
        pages: cropped,
        before,
        after,
    } = page_op(
        e,
        PagesRequest::Crop {
            doc,
            pages: vec![0, 2],
            margins: [5.0, 6.0, 7.0, 8.0],
        },
    )
    .unwrap()
    else {
        panic!("no crop answer");
    };
    let mid = fingerprint(e, doc);
    assert_ne!(mid, start);
    let (back, ..) = changed(page_op(
        e,
        PagesRequest::SetBoxes {
            doc,
            pages: vec![0, 2],
            boxes: before,
        },
    ));
    assert_eq!(fingerprint(e, doc), start, "the crop was taken back");
    assert_eq!((back[0].width, back[0].height), (612.0, 792.0));
    let (again, ..) = changed(page_op(
        e,
        PagesRequest::SetBoxes {
            doc,
            pages: vec![0, 2],
            boxes: after,
        },
    ));
    assert_eq!(fingerprint(e, doc), mid, "the crop was made again");
    assert_eq!(again, cropped);
}

#[test]
fn a_crop_that_leaves_too_little_is_refused_for_every_page_and_changes_none() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::mixed_size_pdf());
    let start = fingerprint(e, doc);
    // Fine for the first page (612 wide) and too much for the third (300 wide).
    assert_eq!(
        page_op(
            e,
            PagesRequest::Crop {
                doc,
                pages: vec![0, 2],
                margins: [150.0, 0.0, 150.0, 0.0],
            },
        )
        .unwrap_err(),
        ErrorCode::InvalidRequest
    );
    assert_eq!(fingerprint(e, doc), start, "no page was cropped");
    for margins in [
        [-1.0, 0.0, 0.0, 0.0],
        [f32::NAN, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, f32::INFINITY],
        [100000.0, 0.0, 0.0, 0.0],
        // Leaves exactly nothing; and less than the smallest side.
        [306.0, 0.0, 306.0, 0.0],
        [0.0, 396.0, 0.0, 389.0],
    ] {
        let r = page_op(
            e,
            PagesRequest::Crop {
                doc,
                pages: vec![0],
                margins,
            },
        );
        assert_eq!(r.unwrap_err(), ErrorCode::InvalidRequest, "{margins:?}");
    }
    assert_eq!(fingerprint(e, doc), start);
    // Boxes to set must be usable too.
    for b in [
        [10.0, 10.0, 5.0, 50.0],
        [10.0, 10.0, 10.0, 50.0],
        [0.0, 0.0, 20000.0, 20000.0],
        [f32::NAN, 0.0, 10.0, 10.0],
    ] {
        let r = page_op(
            e,
            PagesRequest::SetBoxes {
                doc,
                pages: vec![0],
                boxes: vec![b],
            },
        );
        assert_eq!(r.unwrap_err(), ErrorCode::InvalidRequest, "{b:?}");
    }
    assert_eq!(fingerprint(e, doc), start);
}

#[test]
fn odd_boxes_are_cropped_or_refused_but_never_crash_or_hang() {
    let mut guard = engine();
    let e = &mut guard.0;
    let cases = kit::odd_boxes();
    let (doc, _file) = open_doc(e, &kit::odd_boxes_pdf());
    let started = Instant::now();
    for (i, (label, _)) in cases.iter().enumerate() {
        let r = page_op(
            e,
            PagesRequest::Crop {
                doc,
                pages: vec![i as u32],
                margins: [1.0, 1.0, 1.0, 1.0],
            },
        );
        match r {
            Ok(PagesResponse::Cropped { pages, .. }) => {
                let p = pages[i];
                assert!(p.width.is_finite() && p.height.is_finite(), "{label}");
                assert!(p.width > 0.0 && p.height > 0.0, "{label}");
            }
            Ok(other) => panic!("{label}: {other:?}"),
            Err(code) => assert!(
                matches!(
                    code,
                    ErrorCode::InvalidRequest | ErrorCode::PageChangeFailed
                ),
                "{label}: {code:?}"
            ),
        }
        // Whatever happened, the other tools still work on that page, and a save works.
        let _ = page_op(
            e,
            PagesRequest::Rotate {
                doc,
                pages: vec![i as u32],
                turns: 1,
            },
        );
    }
    assert!(started.elapsed() < Duration::from_secs(20));
    let saved = save_all(e, doc);
    let (again, _f) = open_doc(e, &saved);
    assert_eq!(info(e, again).0 as usize, cases.len());
}

// ---- insert a blank page -----------------------------------------------------------------------

#[test]
fn a_blank_page_goes_where_it_is_asked() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::marked_pdf(3));
    let (sizes, snapshot, count) = changed(page_op(
        e,
        PagesRequest::InsertBlank {
            doc,
            at: 1,
            width: 300.0,
            height: 200.0,
        },
    ));
    assert_eq!((sizes.len(), count), (4, 1));
    assert_ne!(snapshot, 0);
    assert_eq!((sizes[1].width, sizes[1].height), (300.0, 200.0));
    assert!(text(e, doc, 1).trim().is_empty());
    assert_eq!(order_of_skipping(e, doc, 4, 1), vec![0, 1, 2]);
    // At the start and at the end.
    changed(page_op(
        e,
        PagesRequest::InsertBlank {
            doc,
            at: 0,
            width: 100.0,
            height: 100.0,
        },
    ));
    changed(page_op(
        e,
        PagesRequest::InsertBlank {
            doc,
            at: 5,
            width: 100.0,
            height: 100.0,
        },
    ));
    assert_eq!(info(e, doc).0, 6);
    assert_eq!(
        page_op(
            e,
            PagesRequest::InsertBlank {
                doc,
                at: 7,
                width: 100.0,
                height: 100.0
            }
        )
        .unwrap_err(),
        ErrorCode::PageOutOfRange
    );
    for (w, h) in [
        (0.0, 100.0),
        (100.0, 7.9),
        (14401.0, 100.0),
        (f32::NAN, 100.0),
        (100.0, f32::INFINITY),
    ] {
        assert_eq!(
            page_op(
                e,
                PagesRequest::InsertBlank {
                    doc,
                    at: 0,
                    width: w,
                    height: h
                }
            )
            .unwrap_err(),
            ErrorCode::InvalidRequest,
            "{w} {h}"
        );
    }
    assert_eq!(info(e, doc).0, 6);
}

fn order_of_skipping(engine: &mut Engine, doc: u32, pages: usize, skip: usize) -> Vec<usize> {
    (0..pages as u32)
        .filter(|p| *p as usize != skip)
        .map(|p| {
            let t = text(engine, doc, p);
            let start = t.find("PAGE-TEXT-").unwrap() + 10;
            t[start..].split('-').next().unwrap().parse().unwrap()
        })
        .collect()
}

// ---- going back --------------------------------------------------------------------------------

#[test]
fn a_change_is_taken_back_exactly_and_made_again() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::marked_pdf(4));
    let start = fingerprint(e, doc);
    let (_, back_to_start, _) = changed(page_op(
        e,
        PagesRequest::Delete {
            doc,
            pages: vec![0, 3],
        },
    ));
    let after_delete = fingerprint(e, doc);
    assert_ne!(after_delete, start);
    // Go back: the document is what it was, down to every secret of every page in the file.
    let (sizes, redo, _) = changed(page_op(
        e,
        PagesRequest::Restore {
            doc,
            snapshot: back_to_start,
        },
    ));
    assert_eq!(sizes.len(), 4);
    assert_eq!(fingerprint(e, doc), start);
    let saved = save_all(e, doc);
    for page in 0..4 {
        assert_has(&saved, page);
    }
    assert_eq!(outline(e, doc)[3].page, Some(3));
    assert_eq!(links(e, doc, 0)[0].target, LinkTarget::Page(1));
    // And forward again, with the number the engine gave for the state it left.
    let (sizes, undo_again, _) = changed(page_op(
        e,
        PagesRequest::Restore {
            doc,
            snapshot: redo,
        },
    ));
    assert_eq!(sizes.len(), 2);
    assert_eq!(fingerprint(e, doc), after_delete);
    assert_gone(&save_all(e, doc), 0);
    // The number of a copy works once.
    assert_eq!(
        page_op(
            e,
            PagesRequest::Restore {
                doc,
                snapshot: redo
            }
        )
        .unwrap_err(),
        ErrorCode::SnapshotGone
    );
    assert_ne!(undo_again, 0);
    // A number that never existed.
    assert_eq!(
        page_op(
            e,
            PagesRequest::Restore {
                doc,
                snapshot: 9999
            }
        )
        .unwrap_err(),
        ErrorCode::SnapshotGone
    );
}

#[test]
fn a_reorder_and_a_blank_page_are_taken_back_too() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::marked_pdf(4));
    let start = fingerprint(e, doc);
    let (_, undo_blank, _) = changed(page_op(
        e,
        PagesRequest::InsertBlank {
            doc,
            at: 2,
            width: 100.0,
            height: 100.0,
        },
    ));
    let (_, undo_move, _) = changed(page_op(
        e,
        PagesRequest::Reorder {
            doc,
            order: vec![4, 3, 2, 1, 0],
        },
    ));
    changed(page_op(
        e,
        PagesRequest::Restore {
            doc,
            snapshot: undo_move,
        },
    ));
    changed(page_op(
        e,
        PagesRequest::Restore {
            doc,
            snapshot: undo_blank,
        },
    ));
    assert_eq!(fingerprint(e, doc), start);
    assert_eq!(order_of(e, doc, 4), vec![0, 1, 2, 3]);
}

#[test]
fn the_engine_keeps_few_copies_and_lets_the_oldest_go() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::marked_pdf(8));
    let size = save_all(e, doc).len();
    // Room for two copies of the document, and no more than two steps.
    e.set_page_tools_for_tests(doc, PageTools::with_limits(size * 2 + size / 2, 2));
    let mut tokens = Vec::new();
    for _ in 0..4 {
        let (_, token, _) = changed(page_op(
            e,
            PagesRequest::Delete {
                doc,
                pages: vec![0],
            },
        ));
        tokens.push(token);
    }
    // The first two are gone, the last two work.
    for old in &tokens[..2] {
        assert_eq!(
            page_op(
                e,
                PagesRequest::Restore {
                    doc,
                    snapshot: *old
                }
            )
            .unwrap_err(),
            ErrorCode::SnapshotGone
        );
    }
    assert_eq!(info(e, doc).0, 4);
    changed(page_op(
        e,
        PagesRequest::Restore {
            doc,
            snapshot: tokens[3],
        },
    ));
    assert_eq!(info(e, doc).0, 5);
}

#[test]
fn a_document_too_big_to_keep_a_copy_of_can_still_be_changed_but_not_taken_back() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open_doc(e, &kit::marked_pdf(4));
    // A limit smaller than the document.
    e.set_page_tools_for_tests(doc, PageTools::with_limits(100, 10));
    let (sizes, snapshot, _) = changed(page_op(
        e,
        PagesRequest::Delete {
            doc,
            pages: vec![1],
        },
    ));
    assert_eq!((sizes.len(), snapshot), (3, 0));
    assert_gone(&save_all(e, doc), 1);
}

// ---- insert pages from another document ---------------------------------------------------------

/// Hands the bytes of a document to the engine the way the app does: begin, pieces, end.
fn insert_document(
    engine: &mut Engine,
    doc: u32,
    bytes: &[u8],
    at: u32,
    piece: usize,
) -> Result<(Vec<PageSize>, u32, u32), ErrorCode> {
    match page_op(
        engine,
        PagesRequest::ImportBegin {
            doc,
            size: bytes.len() as u32,
        },
    )? {
        PagesResponse::ImportReady => {}
        other => panic!("{other:?}"),
    }
    let mut offset = 0usize;
    while offset < bytes.len() {
        let end = (offset + piece).min(bytes.len());
        match page_op(
            engine,
            PagesRequest::ImportChunk {
                doc,
                offset: offset as u32,
                data: bytes[offset..end].to_vec(),
            },
        )? {
            PagesResponse::ImportAck { received } => assert_eq!(received as usize, end),
            other => panic!("{other:?}"),
        }
        offset = end;
    }
    match page_op(engine, PagesRequest::ImportEnd { doc, at })? {
        PagesResponse::Changed {
            pages,
            snapshot,
            count,
            ..
        } => Ok((pages, snapshot, count)),
        other => panic!("{other:?}"),
    }
}

/// The bytes of a document made of some pages of another, as the app gets them from the engine
/// that holds the other file.
fn export(engine: &mut Engine, doc: u32, pages: Vec<u32>) -> (Vec<u8>, PagesResponse) {
    let response = page_op(engine, PagesRequest::ExportBegin { doc, pages }).unwrap();
    let PagesResponse::Exported { size, .. } = response else {
        panic!("{response:?}");
    };
    let mut out = Vec::new();
    while (out.len() as u32) < size {
        let len = (MAX_SAVE_CHUNK as u32).min(size - out.len() as u32);
        match ask(
            engine,
            RequestBody::SaveChunk {
                doc,
                offset: out.len() as u32,
                len,
            },
        ) {
            ResponseBody::SaveData { data, .. } => out.extend_from_slice(&data),
            other => panic!("{other:?}"),
        }
    }
    ask(engine, RequestBody::SaveEnd { doc });
    (out, response)
}

#[test]
fn pages_of_another_document_are_inserted_in_the_place_asked() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (target, _f1) = open_doc(e, &kit::marked_pdf(3));
    let (source, _f2) = open_doc(e, &kit::simple_pdf(2));
    let (bytes, _) = export(e, source, vec![0, 1]);
    let (sizes, snapshot, count) = insert_document(e, target, &bytes, 1, 700).unwrap();
    assert_eq!((sizes.len(), count), (5, 2));
    assert_ne!(snapshot, 0);
    assert!(text(e, target, 1).contains("Page 1"));
    assert!(text(e, target, 2).contains("Page 2"));
    assert!(text(e, target, 0).contains(&page_text(0)));
    assert!(text(e, target, 3).contains(&page_text(1)));
    // The pages that were there keep their notes, fields and links.
    assert_eq!(notes_on(e, target, 0), 1);
    assert_eq!(
        field_values_on(e, target, 3),
        vec!["VALUE-1-SECRET".to_string()]
    );
    assert_eq!(links(e, target, 0)[0].target, LinkTarget::Page(3));
    // The inserted pages are in the saved file, and taking the insert back removes them again.
    let saved = save_all(e, target);
    assert!(saved.starts_with(b"%PDF-"));
    changed(page_op(
        e,
        PagesRequest::Restore {
            doc: target,
            snapshot,
        },
    ));
    assert_eq!(info(e, target).0, 3);
    assert_eq!(order_of(e, target, 3), vec![0, 1, 2]);
}

#[test]
fn an_insert_at_the_start_and_at_the_end_work() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (target, _f1) = open_doc(e, &kit::marked_pdf(2));
    let (source, _f2) = open_doc(e, &kit::simple_pdf(1));
    let (bytes, _) = export(e, source, vec![0]);
    insert_document(e, target, &bytes, 0, MAX_SAVE_CHUNK).unwrap();
    insert_document(e, target, &bytes, 3, 64).unwrap();
    assert_eq!(info(e, target).0, 4);
    assert!(text(e, target, 0).contains("Page 1"));
    assert!(text(e, target, 3).contains("Page 1"));
    assert_eq!(
        insert_document(e, target, &bytes, 5, 1000).unwrap_err(),
        ErrorCode::PageOutOfRange
    );
}

#[test]
fn a_document_to_insert_that_is_not_usable_changes_nothing() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _f) = open_doc(e, &kit::marked_pdf(3));
    let start = fingerprint(e, doc);
    for bytes in [
        kit::not_a_pdf(),
        kit::corrupt_pdf(),
        kit::encrypted_pdf("secret", 2),
        vec![0u8; 10],
    ] {
        assert_eq!(
            insert_document(e, doc, &bytes, 0, 100).unwrap_err(),
            ErrorCode::ImportFailed
        );
        assert_eq!(fingerprint(e, doc), start);
    }
    // Pieces in the wrong order, too many, too few, and none at all.
    let good = kit::simple_pdf(1);
    page_op(
        e,
        PagesRequest::ImportBegin {
            doc,
            size: good.len() as u32,
        },
    )
    .unwrap();
    assert_eq!(
        page_op(
            e,
            PagesRequest::ImportChunk {
                doc,
                offset: 5,
                data: vec![1, 2, 3]
            }
        )
        .unwrap_err(),
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        page_op(
            e,
            PagesRequest::ImportChunk {
                doc,
                offset: 0,
                data: vec![0; good.len() + 1]
            }
        )
        .unwrap_err(),
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        page_op(e, PagesRequest::ImportEnd { doc, at: 0 }).unwrap_err(),
        ErrorCode::InvalidRequest,
        "the document is not complete"
    );
    // The failed end dropped what was gathered: a piece now has no insert to belong to.
    assert_eq!(
        page_op(
            e,
            PagesRequest::ImportChunk {
                doc,
                offset: 0,
                data: vec![1]
            }
        )
        .unwrap_err(),
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        page_op(e, PagesRequest::ImportBegin { doc, size: 0 }).unwrap_err(),
        ErrorCode::InvalidRequest
    );
    page_op(e, PagesRequest::ImportAbort { doc }).unwrap();
    assert_eq!(fingerprint(e, doc), start);
}

#[test]
fn a_document_with_too_many_pages_is_not_inserted() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _f) = open_doc(e, &kit::blank_pages_pdf(MAX_EDIT_PAGES - 1));
    let start = fingerprint(e, doc);
    let two = kit::blank_pages_pdf(2);
    assert_eq!(
        insert_document(e, doc, &two, 0, 100_000).unwrap_err(),
        ErrorCode::TooManyPages
    );
    assert_eq!(fingerprint(e, doc), start);
    assert_eq!(
        page_op(
            e,
            PagesRequest::InsertBlank {
                doc,
                at: 0,
                width: 100.0,
                height: 100.0
            }
        )
        .map(|_| ()),
        Ok(())
    );
    assert_eq!(
        page_op(
            e,
            PagesRequest::InsertBlank {
                doc,
                at: 0,
                width: 100.0,
                height: 100.0
            }
        )
        .unwrap_err(),
        ErrorCode::TooManyPages
    );
}

// ---- extract -----------------------------------------------------------------------------------

#[test]
fn extracted_pages_are_a_new_document_and_the_old_one_is_not_changed() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _f) = open_doc(e, &kit::marked_pdf(5));
    let start = fingerprint(e, doc);
    let (bytes, response) = export(e, doc, vec![3, 1, 4]);
    let PagesResponse::Exported {
        size,
        pages,
        signatures,
        source_security,
        digest,
    } = response
    else {
        panic!();
    };
    assert_eq!(size as usize, bytes.len());
    assert_eq!((pages, signatures, source_security), (3, 0, 0));
    assert_eq!(fingerprint(e, doc), start, "the document was not changed");
    // The new document holds the pages asked for, in that order, and nothing of the others.
    for page in [3, 1, 4] {
        assert_has(&bytes, page);
    }
    for page in [0, 2] {
        assert_gone(&bytes, page);
    }
    // Opened again it has the digest the engine promised.
    let (again, _f2) = open_doc(e, &bytes);
    assert_eq!(order_of(e, again, 3), vec![3, 1, 4]);
    let (p, failed, d) = fingerprint(e, again);
    assert_eq!((p, failed, d), (3, 0, digest));
    // What belongs to a page goes with it, except what refers to it from outside: the note and the
    // link stay, and the form field is a widget on the page that is no longer part of a form (the
    // new document has no form), so it cannot be filled in any more.
    assert_eq!(notes_on(e, again, 0), 1);
    assert!(field_values_on(e, again, 0).is_empty());
    // A link to a page that is not in the new document is dropped, and so is one to a page that
    // comes later in it. (Pages 3, 1 and 4: page 3 links to 4, which comes later; page 1 links to 2,
    // which is not there; page 4 links to 0, which is not there.)
    for place in 0..3 {
        assert!(links(e, again, place).is_empty(), "place {place}");
    }
    // A link to a page that comes earlier in the new document is kept.
    let (kept, _) = export(e, doc, vec![2, 1]);
    let (again2, _f3) = open_doc(e, &kept);
    assert_eq!(links(e, again2, 1)[0].target, LinkTarget::Page(0));
    assert!(links(e, again2, 0).is_empty());
}

#[test]
fn bad_page_lists_are_refused_when_extracting() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _f) = open_doc(e, &kit::marked_pdf(3));
    for (pages, code) in [
        (vec![], ErrorCode::InvalidRequest),
        (vec![3], ErrorCode::PageOutOfRange),
        (vec![0, u32::MAX], ErrorCode::PageOutOfRange),
    ] {
        assert_eq!(
            page_op(e, PagesRequest::ExportBegin { doc, pages }).unwrap_err(),
            code
        );
    }
}

#[test]
fn extracting_from_a_signed_document_reports_what_the_new_one_has() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _f) = open_doc(e, &kit::signed_pdf());
    assert!(info(e, doc).2 >= 1, "the source is signed");
    let (bytes, response) = export(e, doc, vec![0]);
    let PagesResponse::Exported { signatures, .. } = response else {
        panic!();
    };
    // Whatever the number, it is the number the written file will show: the app compares it.
    let (again, _f2) = open_doc(e, &bytes);
    assert_eq!(info(e, again).2, signatures);
}

// ---- passwords ---------------------------------------------------------------------------------

#[test]
fn a_protected_document_stays_protected_through_every_page_change() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (file, doc) = {
        let file = TempFile::new("locked.pdf", &kit::encrypted_pdf("secret", 4));
        e.set_document(File::open(file.path()).unwrap());
        let doc = match ask(
            e,
            RequestBody::Open {
                password: Some("secret".into()),
            },
        ) {
            ResponseBody::Opened { doc, .. } => doc,
            other => panic!("{other:?}"),
        };
        (file, doc)
    };
    let _keep = file;
    let (_, security, _) = info(e, doc);
    assert_ne!(security, 0);
    let (_, undo, _) = changed(page_op(
        e,
        PagesRequest::Delete {
            doc,
            pages: vec![0],
        },
    ));
    changed(page_op(
        e,
        PagesRequest::Reorder {
            doc,
            order: vec![2, 0, 1],
        },
    ));
    changed(page_op(
        e,
        PagesRequest::Rotate {
            doc,
            pages: vec![1],
            turns: 1,
        },
    ));
    changed(page_op(
        e,
        PagesRequest::InsertBlank {
            doc,
            at: 1,
            width: 100.0,
            height: 100.0,
        },
    ));
    let saved = save_all(e, doc);
    assert_eq!(info(e, doc).0, 4);
    // The saved file still needs the password, and opens with it.
    assert_eq!(
        open(e, &saved, None).unwrap_err(),
        ErrorCode::PasswordRequired
    );
    assert_eq!(
        open(e, &saved, Some("wrong")).unwrap_err(),
        ErrorCode::PasswordIncorrect
    );
    let again = open(e, &saved, Some("secret")).unwrap();
    assert_eq!(info(e, again).0, 4);
    // Going back loads a copy of the document, which needs the password too: the engine has it.
    changed(page_op(
        e,
        PagesRequest::Restore {
            doc,
            snapshot: undo,
        },
    ));
    assert_eq!(info(e, doc).0, 4);
    assert_ne!(info(e, doc).1, 0);
}

#[test]
fn extracted_pages_of_a_protected_document_are_not_protected_and_the_answer_says_so() {
    let mut guard = engine();
    let e = &mut guard.0;
    let file = TempFile::new("locked.pdf", &kit::encrypted_pdf("secret", 3));
    e.set_document(File::open(file.path()).unwrap());
    let ResponseBody::Opened { doc, .. } = ask(
        e,
        RequestBody::Open {
            password: Some("secret".into()),
        },
    ) else {
        panic!();
    };
    let (bytes, response) = export(e, doc, vec![0, 1]);
    let PagesResponse::Exported {
        source_security, ..
    } = response
    else {
        panic!();
    };
    assert_ne!(source_security, 0, "the source is protected");
    // The new document opens without a password: this is what the app must warn about.
    let again = open(e, &bytes, None).unwrap();
    assert_eq!(info(e, again), (2, 0, 0));
}

// ---- the signature, the busy engine and the limits -----------------------------------------------

#[test]
fn a_signed_document_is_still_seen_as_signed_after_a_page_change() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _f) = open_doc(e, &kit::signed_pdf());
    let (_, _, sigs) = info(e, doc);
    assert!(sigs >= 1);
    changed(page_op(
        e,
        PagesRequest::Rotate {
            doc,
            pages: vec![0],
            turns: 1,
        },
    ));
    assert_eq!(info(e, doc).2, sigs);
    let saved = save_all(e, doc);
    // The signature entry is in the file, and the app warns before the rewrite (the bytes it
    // covers cannot be the same).
    assert!(contains(&saved, "/ByteRange"));
}

#[test]
fn nothing_changes_while_a_save_or_a_copy_holds_the_bytes() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _f) = open_doc(e, &kit::marked_pdf(3));
    let ResponseBody::SaveReady { .. } = ask(e, RequestBody::SaveBegin { doc }) else {
        panic!();
    };
    for op in [
        PagesRequest::Delete {
            doc,
            pages: vec![0],
        },
        PagesRequest::Rotate {
            doc,
            pages: vec![0],
            turns: 1,
        },
        PagesRequest::Reorder {
            doc,
            order: vec![2, 1, 0],
        },
        PagesRequest::InsertBlank {
            doc,
            at: 0,
            width: 100.0,
            height: 100.0,
        },
        PagesRequest::ExportBegin {
            doc,
            pages: vec![0],
        },
    ] {
        assert_eq!(page_op(e, op).unwrap_err(), ErrorCode::Busy);
    }
    ask(e, RequestBody::SaveEnd { doc });
    changed(page_op(
        e,
        PagesRequest::Rotate {
            doc,
            pages: vec![0],
            turns: 1,
        },
    ));
}

#[test]
fn a_document_with_too_many_pages_is_not_opened_and_a_long_one_is_not_edited() {
    let mut guard = engine();
    let e = &mut guard.0;
    // One page over what the reader opens.
    assert_eq!(
        open(e, &kit::blank_pages_pdf(MAX_PAGES + 1), None).unwrap_err(),
        ErrorCode::TooManyPages
    );
    // A tree that says it holds millions of pages in a few bytes.
    for claim in [
        1_000_000u64,
        2_000_000_000,
        u64::from(u32::MAX),
        u64::MAX / 2,
    ] {
        match open(e, &kit::page_count_lie_pdf(claim), None) {
            Ok(doc) => assert!(info(e, doc).0 as usize <= MAX_PAGES),
            Err(code) => assert!(
                matches!(code, ErrorCode::TooManyPages | ErrorCode::Corrupt),
                "{claim}: {code:?}"
            ),
        }
    }
    // Over what the page tools take, under what the reader opens: the reader works, the tools
    // refuse.
    let n = MAX_EDIT_PAGES + 1;
    let (doc, _f) = open_doc(e, &kit::blank_pages_pdf(n));
    assert_eq!(info(e, doc).0 as usize, n);
    for op in [
        PagesRequest::Delete {
            doc,
            pages: vec![0],
        },
        PagesRequest::Rotate {
            doc,
            pages: vec![0],
            turns: 1,
        },
        PagesRequest::Reorder {
            doc,
            order: (0..n as u32).rev().collect(),
        },
        PagesRequest::ExportBegin {
            doc,
            pages: vec![0],
        },
    ] {
        assert_eq!(page_op(e, op).unwrap_err(), ErrorCode::TooManyPages);
    }
}

#[test]
fn the_longest_document_the_tools_take_can_be_changed() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _f) = open_doc(e, &kit::blank_pages_pdf(MAX_EDIT_PAGES));
    let started = Instant::now();
    changed(page_op(
        e,
        PagesRequest::Delete {
            doc,
            pages: (0..5000).collect(),
        },
    ));
    assert_eq!(info(e, doc).0 as usize, MAX_EDIT_PAGES - 5000);
    // A shuffle in which every other page moves.
    let n = info(e, doc).0;
    let mut order: Vec<u32> = (0..n).collect();
    for pair in order.chunks_mut(2) {
        pair.reverse();
    }
    changed(page_op(e, PagesRequest::Reorder { doc, order }));
    assert!(
        started.elapsed() < Duration::from_secs(180),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_circular_page_tree_and_a_very_deep_one_do_not_hang_the_tools() {
    let mut guard = engine();
    let e = &mut guard.0;
    for bytes in [kit::circular_tree_pdf(), kit::deep_tree_pdf(2000)] {
        let started = Instant::now();
        let Ok(doc) = open(e, &bytes, None) else {
            continue;
        };
        let n = info(e, doc).0;
        if n >= 1 {
            let _ = page_op(
                e,
                PagesRequest::Rotate {
                    doc,
                    pages: vec![0],
                    turns: 1,
                },
            );
            let _ = page_op(
                e,
                PagesRequest::Crop {
                    doc,
                    pages: vec![0],
                    margins: [1.0; 4],
                },
            );
            let _ = page_op(
                e,
                PagesRequest::InsertBlank {
                    doc,
                    at: 0,
                    width: 100.0,
                    height: 100.0,
                },
            );
            let _ = page_op(
                e,
                PagesRequest::ExportBegin {
                    doc,
                    pages: vec![0],
                },
            );
            ask(e, RequestBody::SaveEnd { doc });
            if n >= 2 {
                let _ = page_op(
                    e,
                    PagesRequest::Delete {
                        doc,
                        pages: vec![0],
                    },
                );
                let n = info(e, doc).0;
                let _ = page_op(
                    e,
                    PagesRequest::Reorder {
                        doc,
                        order: (0..n).rev().collect(),
                    },
                );
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "{:?}",
            started.elapsed()
        );
        // The engine still answers.
        let _ = info(e, doc);
    }
}

// ---- a change that fails half way ---------------------------------------------------------------

#[test]
fn a_change_that_fails_half_way_leaves_the_document_as_it_was_when_a_copy_was_kept() {
    let mut guard = engine();
    let e = &mut guard.0;
    for (name, make, step) in [
        (
            "delete",
            (|doc| PagesRequest::Delete {
                doc,
                pages: vec![1, 2, 3],
            }) as fn(u32) -> PagesRequest,
            2usize,
        ),
        (
            "move",
            |doc| PagesRequest::Reorder {
                doc,
                order: vec![4, 3, 2, 1, 0],
            },
            1,
        ),
        (
            "blank page",
            |doc| PagesRequest::InsertBlank {
                doc,
                at: 2,
                width: 100.0,
                height: 100.0,
            },
            0,
        ),
    ] {
        let (doc, _f) = open_doc(e, &kit::marked_pdf(5));
        let start = fingerprint(e, doc);
        e.set_page_tools_for_tests(doc, PageTools::default().failing_at(step));
        assert_eq!(
            page_op(e, make(doc)).unwrap_err(),
            ErrorCode::PageChangeFailed,
            "{name}"
        );
        // The pages are exactly what they were, in the file too, and the tools go on working.
        assert_eq!(fingerprint(e, doc), start, "{name}");
        assert_eq!(order_of(e, doc, 5), vec![0, 1, 2, 3, 4], "{name}");
        let saved = save_all(e, doc);
        for page in 0..5 {
            assert_has(&saved, page);
        }
        changed(page_op(
            e,
            PagesRequest::Rotate {
                doc,
                pages: vec![0],
                turns: 1,
            },
        ));
    }
}

#[test]
fn a_change_that_fails_half_way_without_a_copy_closes_the_document_instead_of_keeping_half_of_it() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _f) = open_doc(e, &kit::marked_pdf(5));
    // No room for a copy, so there is nothing to go back to when the change fails.
    e.set_page_tools_for_tests(doc, PageTools::with_limits(100, 10).failing_at(1));
    assert_eq!(
        page_op(
            e,
            PagesRequest::Delete {
                doc,
                pages: vec![1, 2, 3]
            }
        )
        .unwrap_err(),
        ErrorCode::UnknownDocument
    );
    // What was left, with some of the pages gone, can neither be read nor saved.
    for body in [
        RequestBody::DocInfo { doc },
        RequestBody::SaveBegin { doc },
        RequestBody::Fingerprint { doc },
        RequestBody::PageText { doc, page: 0 },
    ] {
        assert_eq!(code_of(ask(e, body)), ErrorCode::UnknownDocument);
    }
}

// ---- files like real programs write them --------------------------------------------------------
//
// A pop-up note and a field shown on two pages both have a `/Parent`, the key that PDFium's page copy
// leaves alone. A copy of such a page used to point into the document it came from, which was let
// go of, and the next save read freed memory.

/// Every document that has something PDFium's page copy does not carry over.
fn copy_trouble() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("pop-up note", kit::popup_note_pdf()),
        ("field on two pages", kit::shared_field_pdf()),
        ("first key", kit::first_key_pdf()),
    ]
}

fn page_count(engine: &mut Engine, doc: u32) -> u32 {
    info(engine, doc).0
}

#[test]
fn a_move_of_pages_with_notes_and_shared_fields_saves_goes_back_and_goes_on() {
    let mut guard = engine();
    let e = &mut guard.0;
    for (name, bytes) in copy_trouble() {
        let (doc, _f) = open_doc(e, &bytes);
        let n = page_count(e, doc);
        let order: Vec<u32> = (0..n).rev().collect();
        let (_, snapshot, _) = changed(page_op(
            e,
            PagesRequest::Reorder {
                doc,
                order: order.clone(),
            },
        ));
        assert_ne!(snapshot, 0, "{name}");
        let saved = save_all(e, doc);
        let (again, _f2) = open_doc(e, &saved);
        assert_eq!(page_count(e, again), n, "{name}: the saved file");
        // Back, forward, a delete, and a save after each.
        let (_, redo, _) = changed(page_op(e, PagesRequest::Restore { doc, snapshot }));
        assert_eq!(page_count(e, doc), n, "{name}");
        save_all(e, doc);
        changed(page_op(
            e,
            PagesRequest::Restore {
                doc,
                snapshot: redo,
            },
        ));
        save_all(e, doc);
        changed(page_op(
            e,
            PagesRequest::Delete {
                doc,
                pages: vec![0],
            },
        ));
        let saved = save_all(e, doc);
        let (again, _f3) = open_doc(e, &saved);
        assert_eq!(page_count(e, again), n - 1, "{name}: after the delete");
    }
}

#[test]
fn inserting_pages_with_notes_and_shared_fields_saves_goes_back_and_goes_on() {
    let mut guard = engine();
    let e = &mut guard.0;
    for (name, bytes) in copy_trouble() {
        let (target, _f) = open_doc(e, &kit::marked_pdf(4));
        let (source, _f2) = open_doc(e, &bytes);
        let n = page_count(e, source);
        let (copy, _) = export(e, source, (0..n).collect());
        let (sizes, snapshot, count) = insert_document(e, target, &copy, 2, 4096).unwrap();
        assert_eq!((sizes.len() as u32, count), (4 + n, n), "{name}");
        // The save used to stop the engine here.
        let saved = save_all(e, target);
        let (again, _f3) = open_doc(e, &saved);
        assert_eq!(page_count(e, again), 4 + n, "{name}: the saved file");
        let (_, redo, _) = changed(page_op(
            e,
            PagesRequest::Restore {
                doc: target,
                snapshot,
            },
        ));
        assert_eq!(page_count(e, target), 4, "{name}");
        save_all(e, target);
        changed(page_op(
            e,
            PagesRequest::Restore {
                doc: target,
                snapshot: redo,
            },
        ));
        assert_eq!(page_count(e, target), 4 + n, "{name}");
        changed(page_op(
            e,
            PagesRequest::Delete {
                doc: target,
                pages: vec![0],
            },
        ));
        changed(page_op(
            e,
            PagesRequest::InsertBlank {
                doc: target,
                at: 0,
                width: 100.0,
                height: 100.0,
            },
        ));
        let saved = save_all(e, target);
        let (again, _f4) = open_doc(e, &saved);
        assert_eq!(page_count(e, again), 4 + n, "{name}: after more changes");
    }
}

#[test]
fn every_order_of_five_pages_is_reached_and_a_move_keeps_links_and_bookmarks() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _f) = open_doc(e, &kit::marked_pdf(5));
    // All 120 orders, one after the other: the order the pages are in now is read from their text,
    // and the next one is asked for in terms of it.
    let mut orders: Vec<Vec<usize>> = Vec::new();
    fn permute(rest: &mut Vec<usize>, so_far: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        if rest.is_empty() {
            out.push(so_far.clone());
            return;
        }
        for i in 0..rest.len() {
            let page = rest.remove(i);
            so_far.push(page);
            permute(rest, so_far, out);
            so_far.pop();
            rest.insert(i, page);
        }
    }
    permute(&mut (0..5).collect(), &mut Vec::new(), &mut orders);
    assert_eq!(orders.len(), 120);
    let mut now: Vec<usize> = (0..5).collect();
    for want in orders {
        let order: Vec<u32> = want
            .iter()
            .map(|page| now.iter().position(|p| p == page).unwrap() as u32)
            .collect();
        changed(page_op(e, PagesRequest::Reorder { doc, order }));
        now = order_of(e, doc, 5);
        assert_eq!(now, want);
    }
    // Whatever the order, every page still has its link, its bookmark, its note and its field.
    let last = now.clone();
    let place_of = |page: usize| last.iter().position(|&p| p == page).unwrap() as u32;
    for (place, &page) in last.iter().enumerate() {
        assert_eq!(
            links(e, doc, place as u32)[0].target,
            LinkTarget::Page(place_of((page + 1) % 5))
        );
        assert_eq!(notes_on(e, doc, place as u32), 1);
        assert_eq!(field_values_on(e, doc, place as u32).len(), 1);
    }
    for (page, item) in outline(e, doc).iter().enumerate() {
        assert_eq!(item.page, Some(place_of(page)));
    }
    let saved = save_all(e, doc);
    for page in 0..5 {
        assert_has(&saved, page);
    }
}

#[test]
fn a_larger_move_with_runs_of_pages_that_stay_is_exact() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _f) = open_doc(e, &kit::marked_pdf(9));
    // A fixed sequence of mixed orders: runs that stay, pairs that swap, one page far away, a
    // reverse, then back to the start.
    let wants: [[usize; 9]; 6] = [
        [0, 1, 2, 3, 4, 5, 6, 7, 8],
        [3, 4, 0, 1, 2, 8, 5, 6, 7],
        [8, 1, 2, 3, 4, 5, 6, 7, 0],
        [1, 0, 3, 2, 5, 4, 7, 6, 8],
        [8, 7, 6, 5, 4, 3, 2, 1, 0],
        [0, 1, 2, 3, 4, 5, 6, 7, 8],
    ];
    let mut now: Vec<usize> = (0..9).collect();
    for want in wants {
        let order: Vec<u32> = want
            .iter()
            .map(|page| now.iter().position(|p| p == page).unwrap() as u32)
            .collect();
        let moved_expected = {
            let stable = partridge_engine::pages::stable_places(&order);
            stable.iter().filter(|s| !**s).count() as u32
        };
        let r = page_op(e, PagesRequest::Reorder { doc, order });
        if want.iter().enumerate().all(|(i, &p)| now[i] == p) {
            // Already in that order: nothing is done.
            assert_eq!(changed(r).1, 0);
        } else {
            let (_, _, count) = changed(r);
            assert_eq!(count, moved_expected);
        }
        now = order_of(e, doc, 9);
        assert_eq!(now, want);
    }
}

// ---- a copy that does not read back as what was kept ----------------------------------------------

#[test]
fn a_copy_that_reads_back_with_other_pages_is_refused_and_the_document_stays() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _f) = open_doc(e, &kit::marked_pdf(3));
    let bytes = save_all(e, doc);
    // The copy is a good document of three pages, said to have five (as a copy of a damaged file
    // that PDFium writes and reads back differently would be).
    let mut tools = PageTools::default();
    let token = tools.keep_for_tests(bytes, 5);
    e.set_page_tools_for_tests(doc, tools);
    let before = fingerprint(e, doc);
    assert_eq!(
        page_op(
            e,
            PagesRequest::Restore {
                doc,
                snapshot: token
            }
        )
        .unwrap_err(),
        ErrorCode::PageChangeFailed
    );
    assert_eq!(page_count(e, doc), 3);
    assert_eq!(fingerprint(e, doc), before);
    // The copy that does not read back is not kept, and the state that was not left is not either.
    assert_eq!(
        page_op(
            e,
            PagesRequest::Restore {
                doc,
                snapshot: token
            }
        )
        .unwrap_err(),
        ErrorCode::SnapshotGone
    );
    // The document still takes changes and saves.
    changed(page_op(
        e,
        PagesRequest::Rotate {
            doc,
            pages: vec![0],
            turns: 1,
        },
    ));
    assert!(save_all(e, doc).starts_with(b"%PDF-"));
}

#[test]
fn a_document_that_reads_back_with_no_pages_is_never_put_in_place_of_the_open_one() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(
        e,
        &kit::page_count_lost_pdf(),
        Some(kit::PAGE_COUNT_LOST_PASSWORD),
    )
    .expect("the file opens");
    assert_eq!(page_count(e, doc), 1);
    // A copy of the page, put in at the front. The document PDFium writes after that reads back
    // with no pages, so the insert is refused and the document is as it was.
    let (copy, _) = export(e, doc, vec![0]);
    let result = insert_document(e, doc, &copy, 0, 4096);
    assert_eq!(result.unwrap_err(), ErrorCode::PageChangeFailed);
    assert_eq!(page_count(e, doc), 1);
    // Taking steps back and forward never gives a document of fewer pages than it had.
    let bytes = save_all(e, doc);
    let again = open(e, &bytes, Some(kit::PAGE_COUNT_LOST_PASSWORD)).unwrap();
    assert!(page_count(e, again) >= 1);
    let (_, snapshot, _) = changed(page_op(
        e,
        PagesRequest::InsertBlank {
            doc,
            at: 1,
            width: 100.0,
            height: 100.0,
        },
    ));
    if snapshot != 0 {
        let r = page_op(e, PagesRequest::Restore { doc, snapshot });
        if let Ok(PagesResponse::Changed {
            pages,
            snapshot: redo,
            ..
        }) = r
        {
            assert!(!pages.is_empty());
            if redo != 0 {
                let again = page_op(
                    e,
                    PagesRequest::Restore {
                        doc,
                        snapshot: redo,
                    },
                );
                if let Ok(PagesResponse::Changed { pages, .. }) = again {
                    assert!(!pages.is_empty());
                }
            }
        }
    }
    assert!(page_count(e, doc) >= 1);
}

#[test]
fn a_document_over_half_the_limit_can_be_taken_back_and_made_again() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _f) = open_doc(e, &kit::marked_pdf(4));
    let size = save_all(e, doc).len();
    // Room for one and a half copies of the document: the copy of the state that is left must not
    // push out the copy that is wanted.
    e.set_page_tools_for_tests(doc, PageTools::with_limits(size * 3 / 2, 32));
    let (_, token, _) = changed(page_op(
        e,
        PagesRequest::Delete {
            doc,
            pages: vec![0],
        },
    ));
    assert_ne!(token, 0, "a copy was kept");
    let (sizes, redo, _) = changed(page_op(
        e,
        PagesRequest::Restore {
            doc,
            snapshot: token,
        },
    ));
    assert_eq!(sizes.len(), 4, "the delete was taken back");
    assert_ne!(redo, 0, "and can be made again");
    let (sizes, undo_again, _) = changed(page_op(
        e,
        PagesRequest::Restore {
            doc,
            snapshot: redo,
        },
    ));
    assert_eq!(sizes.len(), 3);
    assert_ne!(undo_again, 0);
    let (sizes, _, _) = changed(page_op(
        e,
        PagesRequest::Restore {
            doc,
            snapshot: undo_again,
        },
    ));
    assert_eq!(sizes.len(), 4);
}

// ---- what a delete leaves ----------------------------------------------------------------------

#[test]
fn a_deleted_page_takes_the_typed_text_the_look_of_it_and_the_notes_of_its_field_with_it() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _f) = open_doc(e, &kit::form_leftovers_pdf());
    let before = save_all(e, doc);
    for (what, needle) in kit::form_leftovers_gone()
        .into_iter()
        .chain(kit::form_leftovers_kept())
    {
        assert!(contains(&before, &needle), "the file holds the {what}");
    }
    changed(page_op(
        e,
        PagesRequest::Delete {
            doc,
            pages: vec![1],
        },
    ));
    let after = save_all(e, doc);
    for (what, needle) in kit::form_leftovers_gone() {
        assert!(
            !contains(&after, &needle),
            "the file still holds the {what} of the deleted page"
        );
    }
    // What PDFium gives no way to remove stays, and the README says so. If this fails because PDFium
    // now lets go of the text of a tag, say so in the README and take the line out.
    for (what, needle) in kit::form_leftovers_kept() {
        assert!(
            contains(&after, &needle),
            "the {what} of the deleted page is no longer in the file: update what the README says stays"
        );
    }
    assert!(contains(&after, &kit::hex_form("KEEP")));
}

// ---- what a copy of pages takes with it ----------------------------------------------------------

#[test]
fn the_data_of_a_signature_does_not_travel_with_a_copy_of_its_page() {
    let mut guard = engine();
    let e = &mut guard.0;
    for (name, bytes) in [
        ("signature field", kit::signed_pdf()),
        ("signature under a group", kit::signed_nested_pdf()),
    ] {
        let (doc, _f) = open_doc(e, &bytes);
        assert!(contains(&save_all(e, doc), "/ByteRange"), "{name}: source");
        let (copy, _) = export(e, doc, vec![0]);
        assert!(!contains(&copy, "ByteRange"), "{name}: copied out");
        assert!(!contains(&copy, "3030303030"), "{name}: copied out");
        // Inserted into another document and saved, the same.
        let (target, _f2) = open_doc(e, &kit::simple_pdf(2));
        insert_document(e, target, &copy, 1, 4096).unwrap();
        let saved = save_all(e, target);
        assert!(!contains(&saved, "ByteRange"), "{name}: inserted");
        // The document that was copied from is not changed.
        assert!(
            contains(&save_all(e, doc), "/ByteRange"),
            "{name}: source after"
        );
        // And the copy does not look signed when it is opened: no signature words in the bytes.
        let (again, _f3) = open_doc(e, &copy);
        let ResponseBody::DocInfo { maybe_signed, .. } =
            ask(e, RequestBody::DocInfo { doc: again })
        else {
            panic!("no DocInfo");
        };
        assert!(!maybe_signed, "{name}");
    }
}

#[test]
fn a_popup_in_a_copy_has_lost_its_note_and_that_is_a_limit_of_the_page_copy_in_pdfium() {
    // PDFium's page copy leaves the `/Parent` of a copied annotation as it was in the document it
    // came from. In the copy that number means something else. This pins that, so that a PDFium
    // that repairs it is noticed and the README can drop the line about it.
    fn popup_knows_its_note(e: &mut Engine, doc: u32, page: u32) -> bool {
        e.edit_for_tests(doc, |d| {
            let handle = partridge_engine::pages::raw_document(d).expect("a handle");
            let bindings = d.bindings();
            // SAFETY: the handle is the document's; the page and annotations are closed again.
            unsafe {
                let raw_page = bindings.FPDF_LoadPage(handle, page as i32);
                assert!(!raw_page.is_null());
                let mut found = false;
                for i in 0..bindings.FPDFPage_GetAnnotCount(raw_page) {
                    let annotation = bindings.FPDFPage_GetAnnot(raw_page, i);
                    if bindings.FPDFAnnot_GetSubtype(annotation) == 16 {
                        let note = bindings.FPDFAnnot_GetLinkedAnnot(annotation, "Parent");
                        if !note.is_null() {
                            found = bindings.FPDFAnnot_GetSubtype(note) == 1;
                            bindings.FPDFPage_CloseAnnot(note);
                        }
                    }
                    bindings.FPDFPage_CloseAnnot(annotation);
                }
                bindings.FPDF_ClosePage(raw_page);
                found
            }
        })
        .unwrap()
    }
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _f) = open_doc(e, &kit::popup_note_pdf());
    assert!(popup_knows_its_note(e, doc, 1), "in the document itself");
    let (copy, _) = export(e, doc, vec![1]);
    let (again, _f2) = open_doc(e, &copy);
    assert!(
        !popup_knows_its_note(e, again, 0),
        "PDFium repairs the parent of a copied pop-up now: update the README"
    );
}

// ---- permissions ---------------------------------------------------------------------------------

/// The permissions of a document that allows nothing but reading (bits 1, 2 clear; 7, 8 and the
/// high bits set as the standard has them).
const NOTHING: u32 = 0xFFFF_F0C0;
const MODIFY: u32 = 1 << 3;
const EXTRACT: u32 = 1 << 4;
const ASSEMBLE: u32 = 1 << 10;

/// Which of the tools a document with these permissions lets through: (delete, move, turn, blank
/// page, begin an insert, crop, set boxes, copy out).
fn tools_allowed(e: &mut Engine, bytes: &[u8]) -> [bool; 8] {
    let doc = open(e, bytes, Some("pw")).unwrap();
    let mut out = [false; 8];
    let mut check = |slot: usize, r: Result<PagesResponse, ErrorCode>, e_code: ErrorCode| {
        out[slot] = match r {
            Ok(_) => true,
            Err(code) => {
                assert_eq!(code, e_code, "slot {slot}");
                false
            }
        };
    };
    let denied = ErrorCode::PagesNotAllowed;
    check(
        0,
        page_op(
            e,
            PagesRequest::Delete {
                doc,
                pages: vec![0],
            },
        ),
        denied,
    );
    // A page to move needs two pages, after a delete there are two left: ask in terms of them.
    let now = page_count(e, doc);
    let order: Vec<u32> = (0..now).rev().collect();
    check(1, page_op(e, PagesRequest::Reorder { doc, order }), denied);
    check(
        2,
        page_op(
            e,
            PagesRequest::Rotate {
                doc,
                pages: vec![0],
                turns: 1,
            },
        ),
        denied,
    );
    check(
        3,
        page_op(
            e,
            PagesRequest::InsertBlank {
                doc,
                at: 0,
                width: 100.0,
                height: 100.0,
            },
        ),
        denied,
    );
    check(
        4,
        page_op(e, PagesRequest::ImportBegin { doc, size: 10 }),
        denied,
    );
    check(
        5,
        page_op(
            e,
            PagesRequest::Crop {
                doc,
                pages: vec![0],
                margins: [10.0, 10.0, 10.0, 10.0],
            },
        ),
        denied,
    );
    check(
        6,
        page_op(
            e,
            PagesRequest::SetBoxes {
                doc,
                pages: vec![0],
                boxes: vec![[10.0, 10.0, 200.0, 200.0]],
            },
        ),
        denied,
    );
    check(
        7,
        page_op(
            e,
            PagesRequest::ExportBegin {
                doc,
                pages: vec![0],
            },
        ),
        denied,
    );
    ask(e, RequestBody::SaveEnd { doc });
    out
}

#[test]
fn a_protected_document_allows_the_tools_its_permissions_allow_revision_2() {
    let mut guard = engine();
    let e = &mut guard.0;
    // Revision 2 has no bit for assembling: modifying the contents covers inserting, deleting,
    // moving and turning pages.
    let cases: [(u32, [bool; 8]); 4] = [
        (NOTHING, [false; 8]),
        (
            NOTHING | MODIFY,
            [true, true, true, true, true, true, true, false],
        ),
        (
            NOTHING | EXTRACT,
            [false, false, false, false, false, false, false, true],
        ),
        (NOTHING | ASSEMBLE, [false; 8]),
    ];
    for (p, want) in cases {
        let bytes = kit::encrypted_with_permissions_pdf("pw", 4, p);
        assert_eq!(tools_allowed(e, &bytes), want, "permissions {p:#x}");
    }
}

#[test]
fn a_protected_document_allows_the_tools_its_permissions_allow_revision_3() {
    let mut guard = engine();
    let e = &mut guard.0;
    // Revision 3 has a bit of its own for assembling; changing the contents is another.
    let cases: [(u32, [bool; 8]); 5] = [
        (NOTHING, [false; 8]),
        (
            NOTHING | ASSEMBLE,
            [true, true, true, true, true, false, false, false],
        ),
        (
            NOTHING | MODIFY,
            [false, false, false, false, false, true, true, false],
        ),
        (
            NOTHING | EXTRACT,
            [false, false, false, false, false, false, false, true],
        ),
        (
            NOTHING | ASSEMBLE | MODIFY | EXTRACT,
            [true, true, true, true, true, true, true, true],
        ),
    ];
    for (p, want) in cases {
        let bytes = kit::encrypted_r3_with_permissions_pdf("pw", 4, p);
        assert_eq!(tools_allowed(e, &bytes), want, "permissions {p:#x}");
    }
}

#[test]
fn a_document_that_forbids_changes_can_still_be_read_and_a_change_cannot_be_undone_into_it() {
    let mut guard = engine();
    let e = &mut guard.0;
    let bytes = kit::encrypted_r3_with_permissions_pdf("pw", 3, NOTHING);
    let doc = open(e, &bytes, Some("pw")).unwrap();
    assert_eq!(page_count(e, doc), 3);
    assert_eq!(
        page_op(e, PagesRequest::Restore { doc, snapshot: 1 }).unwrap_err(),
        ErrorCode::SnapshotGone
    );
    assert!(save_all(e, doc).starts_with(b"%PDF-"));
}

// ---- checks the page tools rely on, each put to the test -----------------------------------------

#[test]
fn a_file_that_changed_in_place_stops_every_tool_that_reads_the_document_again() {
    let mut guard = engine();
    let e = &mut guard.0;
    for tool in [
        "delete",
        "move",
        "blank page",
        "restore",
        "insert",
        "copy out",
    ] {
        // The engine watches the file of the document opened last: the other document of an insert
        // is opened and copied first.
        let bytes_to_insert = (tool == "insert").then(|| {
            let (other, _f) = open_doc(e, &kit::simple_pdf(1));
            export(e, other, vec![0]).0
        });
        let original = kit::simple_pdf(3);
        let file = kit::TempFile::new("inplace.pdf", &original);
        e.set_document(File::open(file.path()).unwrap());
        let ResponseBody::Opened { doc, .. } = ask(e, RequestBody::Open { password: None }) else {
            panic!("no document");
        };
        // Every part of the file the tools will use has been read.
        assert!(text(e, doc, 0).contains("Page 1"));
        let mut token = 0;
        match tool {
            "restore" => {
                token = changed(page_op(
                    e,
                    PagesRequest::Delete {
                        doc,
                        pages: vec![2],
                    },
                ))
                .1;
            }
            "insert" => {
                let bytes = bytes_to_insert.clone().unwrap();
                page_op(
                    e,
                    PagesRequest::ImportBegin {
                        doc,
                        size: bytes.len() as u32,
                    },
                )
                .unwrap();
                page_op(
                    e,
                    PagesRequest::ImportChunk {
                        doc,
                        offset: 0,
                        data: bytes,
                    },
                )
                .unwrap();
            }
            _ => {}
        }
        // Another program rewrites the file in place, with the same size.
        let mut theirs = original.clone();
        let at = theirs.windows(6).position(|w| w == b"Page 1").unwrap();
        theirs[at..at + 6].copy_from_slice(b"Page X");
        std::fs::write(file.path(), &theirs).unwrap();
        let op = match tool {
            "delete" => PagesRequest::Delete {
                doc,
                pages: vec![0],
            },
            "move" => PagesRequest::Reorder {
                doc,
                order: vec![2, 1, 0],
            },
            "blank page" => PagesRequest::InsertBlank {
                doc,
                at: 0,
                width: 100.0,
                height: 100.0,
            },
            "restore" => PagesRequest::Restore {
                doc,
                snapshot: token,
            },
            "insert" => PagesRequest::ImportEnd { doc, at: 1 },
            _ => PagesRequest::ExportBegin {
                doc,
                pages: vec![0],
            },
        };
        assert_eq!(
            page_op(e, op).unwrap_err(),
            ErrorCode::SourceChanged,
            "{tool}"
        );
        ask(e, RequestBody::Close { doc });
    }
}

#[test]
fn the_number_of_pages_is_checked_after_every_change_and_a_wrong_one_puts_the_document_back() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (other, _f) = open_doc(e, &kit::simple_pdf(2));
    let (copy, _) = export(e, other, vec![0, 1]);
    for tool in ["delete", "move", "blank page", "insert"] {
        let (doc, _f) = open_doc(e, &kit::marked_pdf(5));
        let start = fingerprint(e, doc);
        // PDFium counts right. The check is made to see one page too many.
        e.set_page_tools_for_tests(doc, PageTools::default().miscounting(1));
        let answer = match tool {
            "delete" => page_op(
                e,
                PagesRequest::Delete {
                    doc,
                    pages: vec![1, 2],
                },
            )
            .map(|_| ()),
            "move" => page_op(
                e,
                PagesRequest::Reorder {
                    doc,
                    order: vec![4, 3, 2, 1, 0],
                },
            )
            .map(|_| ()),
            "blank page" => page_op(
                e,
                PagesRequest::InsertBlank {
                    doc,
                    at: 2,
                    width: 100.0,
                    height: 100.0,
                },
            )
            .map(|_| ()),
            _ => insert_document(e, doc, &copy, 1, 700).map(|_| ()),
        };
        assert_eq!(answer.unwrap_err(), ErrorCode::PageChangeFailed, "{tool}");
        // The document is exactly what it was, in the file too.
        assert_eq!(page_count(e, doc), 5, "{tool}");
        assert_eq!(fingerprint(e, doc), start, "{tool}");
        assert_eq!(order_of(e, doc, 5), vec![0, 1, 2, 3, 4], "{tool}");
        let saved = save_all(e, doc);
        for page in 0..5 {
            assert_has(&saved, page);
        }
        ask(e, RequestBody::Close { doc });
    }
}

#[test]
fn every_tool_that_changes_or_goes_back_waits_for_a_save_or_a_copy_to_end() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _f) = open_doc(e, &kit::marked_pdf(3));
    let (_, token, _) = changed(page_op(
        e,
        PagesRequest::Delete {
            doc,
            pages: vec![0],
        },
    ));
    let ResponseBody::SaveReady { .. } = ask(e, RequestBody::SaveBegin { doc }) else {
        panic!("no SaveReady");
    };
    for (name, op) in [
        (
            "going back",
            PagesRequest::Restore {
                doc,
                snapshot: token,
            },
        ),
        (
            "crop",
            PagesRequest::Crop {
                doc,
                pages: vec![0],
                margins: [1.0, 1.0, 1.0, 1.0],
            },
        ),
        (
            "boxes",
            PagesRequest::SetBoxes {
                doc,
                pages: vec![0],
                boxes: vec![[1.0, 1.0, 100.0, 100.0]],
            },
        ),
        (
            "begin an insert",
            PagesRequest::ImportBegin { doc, size: 10 },
        ),
    ] {
        assert_eq!(page_op(e, op).unwrap_err(), ErrorCode::Busy, "{name}");
    }
    ask(e, RequestBody::SaveEnd { doc });
    // After the save the copy is still there to go back to.
    changed(page_op(
        e,
        PagesRequest::Restore {
            doc,
            snapshot: token,
        },
    ));
}

#[test]
fn a_move_that_leaves_the_pages_in_another_order_than_asked_is_taken_back() {
    let mut guard = engine();
    let e = &mut guard.0;
    // Pages of different sizes, so that the order can be told from their sizes.
    let (doc, _f) = open_doc(e, &kit::mixed_size_pdf());
    let start = fingerprint(e, doc);
    e.set_page_tools_for_tests(doc, PageTools::default().scrambling());
    assert_eq!(
        page_op(
            e,
            PagesRequest::Reorder {
                doc,
                order: vec![3, 2, 1, 0],
            },
        )
        .unwrap_err(),
        ErrorCode::PageChangeFailed
    );
    assert_eq!(fingerprint(e, doc), start);
    // Without the injected fault the same move works, and puts the sizes where they were asked.
    e.set_page_tools_for_tests(doc, PageTools::default());
    let (sizes, _, _) = changed(page_op(
        e,
        PagesRequest::Reorder {
            doc,
            order: vec![3, 2, 1, 0],
        },
    ));
    assert_eq!((sizes[0].width, sizes[0].height), (200.0, 1000.0));
    assert_eq!((sizes[3].width, sizes[3].height), (612.0, 792.0));
}

#[test]
fn a_page_far_from_the_origin_is_refused_for_a_crop_and_no_answer_is_made_that_the_app_cannot_read()
{
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _f) = open_doc(e, &kit::far_box_pdf());
    // The page is an ordinary size, but its boxes hold numbers above what the protocol carries; a
    // crop would report them, and the app would refuse the answer and end the engine.
    assert_eq!(
        page_op(
            e,
            PagesRequest::Crop {
                doc,
                pages: vec![0],
                margins: [10.0, 10.0, 10.0, 10.0],
            },
        )
        .unwrap_err(),
        ErrorCode::PageChangeFailed
    );
    assert_eq!(page_count(e, doc), 1);
    // The other tools go on working on it.
    changed(page_op(
        e,
        PagesRequest::Rotate {
            doc,
            pages: vec![0],
            turns: 1,
        },
    ));
}

#[test]
fn a_page_list_that_names_something_that_is_not_there_is_not_moved_but_everything_else_works() {
    let mut guard = engine();
    let e = &mut guard.0;
    for (name, bytes) in [
        ("a reference to nothing", kit::trailing_null_kid_pdf()),
        ("a name and an array", kit::junk_after_the_kids_pdf()),
    ] {
        let (doc, _f) = open_doc(e, &bytes);
        assert_eq!(page_count(e, doc), 3, "{name}");
        // PDFium's move stops the process on this tree; the tools look at the tree first, and refuse.
        assert_eq!(
            page_op(
                e,
                PagesRequest::Reorder {
                    doc,
                    order: vec![2, 1, 0],
                },
            )
            .unwrap_err(),
            ErrorCode::PageChangeFailed,
            "{name}"
        );
        assert_eq!(page_count(e, doc), 3, "{name}");
        // The document is whole and the other tools work on it, and it saves.
        changed(page_op(
            e,
            PagesRequest::Rotate {
                doc,
                pages: vec![0],
                turns: 1,
            },
        ));
        assert!(save_all(e, doc).starts_with(b"%PDF-"), "{name}");
    }
}

#[test]
fn dictionaries_written_in_place_and_nested_nodes_do_not_make_a_page_tree_look_damaged() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _f) = open_doc(e, &kit::inline_dictionaries_pdf());
    assert_eq!(page_count(e, doc), 3);
    let (sizes, _, _) = changed(page_op(
        e,
        PagesRequest::Reorder {
            doc,
            order: vec![2, 1, 0],
        },
    ));
    assert_eq!(sizes.len(), 3);
    changed(page_op(
        e,
        PagesRequest::Reorder {
            doc,
            order: vec![1, 2, 0],
        },
    ));
    assert!(save_all(e, doc).starts_with(b"%PDF-"));
}
