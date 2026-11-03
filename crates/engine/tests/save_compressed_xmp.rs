//! A document with a Flate compressed XMP metadata stream, saved.
//!
//! PDFium writes the stream of such a document as its compressed bytes with no `/Filter`, which
//! no reader can read (see `partridge_engine::metadata::repair`). The engine mends the bytes it
//! hands over. These tests save generated files, with and without an edit and more than once, and
//! ask other programs (qpdf, and pdfinfo from Poppler) to read the result: the packet has to be
//! read back as it was in the input, and every other stream has to be as it was.
//!
//! The tools are looked for on the path. When one is missing the tests say so and go on, unless
//! `PARTRIDGE_REQUIRE_READERS` is set (CI sets it), in which case a missing tool is a failure.

mod common;

use std::path::PathBuf;
use std::process::Command;

use common::*;
use partridge_protocol::{ErrorCode, RequestBody, ResponseBody};
use partridge_testkit as kit;
use pdfium_render::prelude::*;

fn required() -> bool {
    std::env::var_os("PARTRIDGE_REQUIRE_READERS").is_some()
}

fn have(tool: &str, version_flag: &str) -> bool {
    let found = Command::new(tool)
        .arg(version_flag)
        .output()
        .map(|o| o.status.success() || !o.stderr.is_empty() || !o.stdout.is_empty())
        .unwrap_or(false);
    if !found {
        assert!(!required(), "{tool} is required and is not installed");
        eprintln!("{tool} is not installed: its checks are skipped");
    }
    found
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let dir =
            std::env::temp_dir().join(format!("partridge-xmpflate-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs a program; 3 from qpdf is a warning that still did the work.
fn run(program: &str, args: &[&str]) -> (bool, String) {
    let out = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("{program} could not be started: {e}"));
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (matches!(out.status.code(), Some(0) | Some(3)), text)
}

/// qpdf checks the structure of the file and every stream; a warning is a failure.
fn assert_clean(dir: &Dir, file: &[u8], what: &str) {
    let input = dir.write("checked.pdf", file);
    let (ok, text) = run("qpdf", &["--check", input.to_str().unwrap()]);
    assert!(ok, "{what}: qpdf --check: {text}");
    assert!(
        !text.contains("WARNING") && !text.contains("ERROR"),
        "{what}: qpdf --check: {text}"
    );
}

/// The file with every stream uncompressed, as qpdf reads it.
fn qdf(dir: &Dir, file: &[u8], what: &str) -> Vec<u8> {
    let input = dir.write("qdf-in.pdf", file);
    let output = dir.0.join("qdf-out.pdf");
    let (ok, text) = run(
        "qpdf",
        &[
            "--qdf",
            "--object-streams=disable",
            input.to_str().unwrap(),
            output.to_str().unwrap(),
        ],
    );
    assert!(ok, "{what}: qpdf could not read the saved file: {text}");
    std::fs::read(&output).unwrap()
}

/// How many times `needle` is in `haystack`.
fn count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}

/// The packets that qpdf reads from the saved file: each of `titles` has to be there once, whole,
/// exactly as the input holds it.
fn assert_packets(dir: &Dir, file: &[u8], titles: &[&str], what: &str) {
    let read = qdf(dir, file, what);
    for title in titles {
        let packet = kit::xmp_packet(title);
        assert_eq!(
            count(&read, packet.as_bytes()),
            1,
            "{what}: qpdf does not read the packet of {title} back as it was"
        );
    }
}

/// Poppler prints the XMP packet of the catalog for `-meta`.
fn assert_pdfinfo_meta(dir: &Dir, file: &[u8], title: &str, what: &str) {
    let input = dir.write("info.pdf", file);
    let (ok, text) = run("pdfinfo", &["-meta", input.to_str().unwrap()]);
    assert!(ok, "{what}: pdfinfo: {text}");
    assert!(
        text.contains(&format!("{title}</rdf:li>")) && text.contains("<x:xmpmeta"),
        "{what}: pdfinfo -meta does not print the packet of {title}: {text}"
    );
}

fn try_save(
    engine: &mut partridge_engine::Engine,
    doc: u32,
) -> Result<Vec<u8>, (ErrorCode, String)> {
    match ask(engine, RequestBody::SaveBegin { doc }) {
        ResponseBody::SaveReady { .. } => Ok(save_all(engine, doc)),
        ResponseBody::Error { code, message } => Err((code, message)),
        other => panic!("{other:?}"),
    }
}

fn save(engine: &mut partridge_engine::Engine, doc: u32) -> Vec<u8> {
    try_save(engine, doc).unwrap_or_else(|e| panic!("the save failed: {e:?}"))
}

/// Puts a line of text on the first page, the way an editing feature will.
fn add_text(engine: &mut partridge_engine::Engine, doc: u32) {
    engine
        .edit_for_tests(doc, |document| {
            let font = document.fonts_mut().helvetica();
            let mut page = document.pages().get(0).unwrap();
            page.objects_mut()
                .create_text_object(
                    PdfPoints::new(100.0),
                    PdfPoints::new(300.0),
                    "AN-EDIT",
                    font,
                    PdfPoints::new(18.0),
                )
                .unwrap();
            page.regenerate_content().unwrap();
        })
        .expect("the document is open");
}

fn titles(catalog: bool, page: bool) -> Vec<&'static str> {
    let mut titles = Vec::new();
    if catalog {
        titles.push(kit::XMP_CATALOG_TITLE);
    }
    if page {
        titles.push(kit::XMP_PAGE_TITLE);
    }
    titles
}

/// Everything a good saved file has to show: the structure is clean, each packet is read back as
/// it was, the metadata is there for Poppler, and the other streams hold their phrases.
fn assert_good(dir: &Dir, saved: &[u8], catalog: bool, page: bool, edit: bool, what: &str) {
    assert_clean(dir, saved, what);
    assert_packets(dir, saved, &titles(catalog, page), what);
    if catalog {
        assert_pdfinfo_meta(dir, saved, kit::XMP_CATALOG_TITLE, what);
    }
    let read = qdf(dir, saved, what);
    for phrase in kit::XMP_OTHER_PHRASES {
        assert!(
            contains(&read, phrase.as_bytes()),
            "{what}: qpdf does not read {phrase}"
        );
    }
    assert_eq!(contains(&read, b"AN-EDIT"), edit, "{what}");
}

#[test]
fn a_compressed_xmp_stream_is_saved_so_that_other_programs_read_it() {
    if !have("qpdf", "--version") || !have("pdfinfo", "-v") {
        return;
    }
    let dir = Dir::new("saved");
    for (catalog, page) in [(true, false), (false, true), (true, true)] {
        for edit in [false, true] {
            let what = format!("catalog {catalog}, page {page}, edit {edit}");
            let bytes = kit::xmp_pdf_with(catalog, page, true);
            let mut guard = engine();
            let e = &mut guard.0;
            let doc = open(e, &bytes, None).unwrap();
            if edit {
                add_text(e, doc);
            }
            let saved = save(e, doc);
            assert_good(&dir, &saved, catalog, page, edit, &what);
            // The mended stream is the compressed one with its filter back.
            assert!(contains(&saved, b"/Filter/FlateDecode "), "{what}");

            // The same document saved again gives a good file as well.
            let again = save(e, doc);
            assert_good(
                &dir,
                &again,
                catalog,
                page,
                edit,
                &format!("{what}, saved twice"),
            );

            // The saved file, opened and saved once more, and once more after an edit.
            let reopened = open(e, &saved, None).unwrap();
            let third = save(e, reopened);
            assert_good(
                &dir,
                &third,
                catalog,
                page,
                edit,
                &format!("{what}, reopened"),
            );
            add_text(e, reopened);
            let fourth = save(e, reopened);
            assert_good(
                &dir,
                &fourth,
                catalog,
                page,
                true,
                &format!("{what}, reopened and edited"),
            );
            ask(e, RequestBody::Close { doc: reopened });
            ask(e, RequestBody::Close { doc });
        }
    }
}

#[test]
fn an_xmp_stream_that_is_not_compressed_is_saved_as_before() {
    if !have("qpdf", "--version") || !have("pdfinfo", "-v") {
        return;
    }
    let dir = Dir::new("plain");
    for (catalog, page) in [(true, false), (true, true)] {
        for edit in [false, true] {
            let what = format!("catalog {catalog}, page {page}, edit {edit}");
            let bytes = kit::xmp_pdf_with(catalog, page, false);
            let mut guard = engine();
            let e = &mut guard.0;
            let doc = open(e, &bytes, None).unwrap();
            if edit {
                add_text(e, doc);
            }
            let saved = save(e, doc);
            assert_good(&dir, &saved, catalog, page, edit, &what);
            // Nothing was added: one table, one end of file, the packets as they are in the file.
            assert_eq!(count(&saved, b"startxref"), 1, "{what}");
            assert_eq!(count(&saved, b"%%EOF"), 1, "{what}");
            for title in titles(catalog, page) {
                assert_eq!(
                    count(&saved, kit::xmp_packet(title).as_bytes()),
                    1,
                    "{what}: {title}"
                );
            }
            ask(e, RequestBody::Close { doc });
        }
    }
}

#[test]
fn a_document_without_xmp_gets_no_update() {
    let bytes = kit::xmp_pdf(false, false);
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &bytes, None).unwrap();
    let saved = save(e, doc);
    assert_eq!(count(&saved, b"startxref"), 1);
    ask(e, RequestBody::Close { doc });
}

#[test]
fn a_file_that_an_earlier_save_damaged_is_mended() {
    if !have("qpdf", "--version") || !have("pdfinfo", "-v") {
        return;
    }
    // What PDFium made of a compressed stream before: its compressed bytes and no filter.
    let packed = kit::flate(kit::xmp_packet(kit::XMP_CATALOG_TITLE).as_bytes());
    let damaged = kit::xmp_pdf_raw("", &packed);
    let dir = Dir::new("damaged");
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &damaged, None).unwrap();
    let saved = save(e, doc);
    assert_good(&dir, &saved, true, false, false, "a damaged input");
    ask(e, RequestBody::Close { doc });
}

#[test]
fn xmp_in_text_filters_is_mended_too() {
    if !have("qpdf", "--version") {
        return;
    }
    let dir = Dir::new("text");
    let packet = kit::xmp_packet(kit::XMP_CATALOG_TITLE);
    let flated = kit::flate(packet.as_bytes());
    for (dict, data) in [
        ("/Filter /ASCIIHexDecode", kit::ascii_hex(packet.as_bytes())),
        ("/Filter /ASCII85Decode", kit::ascii_85(packet.as_bytes())),
        (
            "/Filter [/ASCIIHexDecode /FlateDecode]",
            kit::ascii_hex(&flated),
        ),
        (
            "/Filter [/ASCII85Decode /FlateDecode]",
            kit::ascii_85(&flated),
        ),
    ] {
        let bytes = kit::xmp_pdf_raw(dict, &data);
        let mut guard = engine();
        let e = &mut guard.0;
        let doc = open(e, &bytes, None).unwrap();
        let saved = save(e, doc);
        assert_clean(&dir, &saved, dict);
        assert_packets(&dir, &saved, &[kit::XMP_CATALOG_TITLE], dict);
        ask(e, RequestBody::Close { doc });
    }
}

#[test]
fn xmp_that_cannot_be_told_what_it_was_does_not_stop_a_save() {
    // A filter the engine cannot check (LZW), data that is not any filter's, and Flate data that
    // is not markup: PDFium drops the filter and nothing is known about the original, so the
    // save goes ahead as PDFium writes it and nothing is added to the file.
    for (dict, data) in [
        (
            "/Filter /LZWDecode",
            &[0x80, 0x0b, 0x60, 0x50, 0x22, 0x0c, 0x0c, 0x85, 0x01][..],
        ),
        ("", &b"just some words"[..]),
        (
            "/Filter /FlateDecode",
            &kit::flate(b"not markup at all")[..],
        ),
    ] {
        let bytes = kit::xmp_pdf_raw(dict, data);
        let mut guard = engine();
        let e = &mut guard.0;
        let doc = open(e, &bytes, None).unwrap();
        let saved = save(e, doc);
        assert_eq!(count(&saved, b"startxref"), 1, "{dict}");
        ask(e, RequestBody::Close { doc });
    }
}

#[test]
fn a_compressed_stream_that_swells_is_refused_and_the_document_stays_open() {
    // About 4 MiB of white space inside a comment: a few kilobytes packed.
    let mut text = String::from("<x><!--");
    text.push_str(&" ".repeat(5 * 1024 * 1024));
    text.push_str("--></x>");
    let bytes = kit::xmp_pdf_raw("/Filter /FlateDecode", &kit::flate(text.as_bytes()));
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &bytes, None).unwrap();
    for _ in 0..2 {
        let (code, message) = try_save(e, doc).unwrap_err();
        assert_eq!(code, ErrorCode::SaveFailed);
        assert!(message.contains("XMP metadata"), "{message}");
    }
    assert!(matches!(
        ask(e, RequestBody::DocInfo { doc }),
        ResponseBody::DocInfo { pages: 1, .. }
    ));
    ask(e, RequestBody::Close { doc });
}

#[test]
fn the_other_compressed_streams_keep_their_filter() {
    if !have("qpdf", "--version") {
        return;
    }
    let dir = Dir::new("others");
    let plain = kit::flate_streams_pdf();
    // The same file with its objects packed into object streams, as most writers make it.
    let input = dir.write("flate.pdf", &plain);
    let packed_path = dir.0.join("packed.pdf");
    let (ok, text) = run(
        "qpdf",
        &[
            "--object-streams=generate",
            input.to_str().unwrap(),
            packed_path.to_str().unwrap(),
        ],
    );
    assert!(ok, "qpdf could not pack the file: {text}");
    let packed = std::fs::read(&packed_path).unwrap();
    assert!(contains(&packed, b"/ObjStm"));
    for (name, bytes) in [("plain", plain), ("packed", packed)] {
        for edit in [false, true] {
            let what = format!("{name}, edit {edit}");
            let mut guard = engine();
            let e = &mut guard.0;
            let doc = open(e, &bytes, None).unwrap();
            if edit {
                add_text(e, doc);
            }
            let saved = save(e, doc);
            assert_clean(&dir, &saved, &what);
            assert_packets(&dir, &saved, &[kit::XMP_CATALOG_TITLE], &what);
            // Each stream is still a Flate stream that inflates to what it held.
            let inflated = kit::inflated_streams(&saved);
            for payload in kit::FLATE_STREAM_PAYLOADS {
                assert!(
                    inflated.iter().any(|s| contains(s, payload.as_bytes())),
                    "{what}: {payload} is not in a stream that inflates"
                );
            }
            ask(e, RequestBody::Close { doc });
        }
    }
}

#[test]
fn pages_taken_out_of_a_document_with_compressed_xmp_are_mended() {
    if !have("qpdf", "--version") {
        return;
    }
    // The page tools write a new document for the pages that are taken out. Its bytes go to the
    // app like a save, so a compressed stream that came with a page is mended as well.
    let dir = Dir::new("export");
    let bytes = kit::xmp_pdf_with(false, true, true);
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &bytes, None).unwrap();
    let body = RequestBody::Pages(partridge_protocol::PagesRequest::ExportBegin {
        doc,
        pages: vec![0],
    });
    let ResponseBody::Pages(partridge_protocol::PagesResponse::Exported { size, .. }) =
        ask(e, body)
    else {
        panic!("the pages were not taken out");
    };
    let saved = read_exported(e, doc, size);
    assert_clean(&dir, &saved, "the extracted page");
    let read = qdf(&dir, &saved, "the extracted page");
    // The page's metadata comes along with the page, whole.
    let packet = kit::xmp_packet(kit::XMP_PAGE_TITLE);
    assert_eq!(count(&read, packet.as_bytes()), 1);
    ask(e, RequestBody::Close { doc });
}

/// The bytes of an export, read in pieces like those of a save.
fn read_exported(engine: &mut partridge_engine::Engine, doc: u32, size: u32) -> Vec<u8> {
    let mut out = Vec::new();
    while (out.len() as u32) < size {
        let len = (1 << 20).min(size - out.len() as u32);
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
