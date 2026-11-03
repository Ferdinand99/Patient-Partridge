//! What a save does to the streams of a password protected file, and what another program
//! (`qpdf`) makes of the file afterwards.
//!
//! PDFium encrypts every stream it writes for a protected document, except an XMP metadata
//! stream, which it writes as it holds it: in the clear (the parser has decrypted it) and, when it
//! was compressed, without its `/Filter` key. The engine does not hand over such a file (see
//! `partridge_engine::metadata`), so a protected document with XMP metadata is not saved. These
//! tests show, for each way of protecting a file that PDFium reads, that
//!
//! - the other streams (page content, character map, colour profile, embedded file) are written
//!   encrypted and read back by qpdf with the password, with and without an edit;
//! - a document with an XMP stream on the catalog or on a page is refused with its own error, and
//!   stays open;
//! - a file that says `/EncryptMetadata false` keeps the catalog's XMP stream in the clear and
//!   unchanged, and is refused when another stream would be damaged.
//!
//! The tools are looked for on the path. When qpdf is missing the tests say so and go on, unless
//! `PARTRIDGE_REQUIRE_READERS` is set (CI sets it), in which case a missing tool is a failure.

mod common;

use std::path::PathBuf;
use std::process::Command;

use common::*;
use partridge_protocol::{ErrorCode, RequestBody, ResponseBody};
use partridge_testkit as kit;
use pdfium_render::prelude::*;

const PASSWORD: &str = "secret";

fn required() -> bool {
    std::env::var_os("PARTRIDGE_REQUIRE_READERS").is_some()
}

fn have_qpdf() -> bool {
    let found = Command::new("qpdf")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !found {
        assert!(!required(), "qpdf is required and is not installed");
        eprintln!("qpdf is not installed: its checks are skipped");
    }
    found
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let dir =
            std::env::temp_dir().join(format!("partridge-protected-{name}-{}", std::process::id()));
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

/// Runs qpdf; 3 is a warning that still did the work.
fn qpdf(args: &[&str]) -> (bool, String) {
    let out = Command::new("qpdf")
        .args(args)
        .output()
        .expect("qpdf could not be started");
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (matches!(out.status.code(), Some(0) | Some(3)), text)
}

/// A way to protect a file: a name, and the arguments qpdf takes after `--encrypt` and the two
/// passwords (the user password is [`PASSWORD`]).
struct Way {
    name: &'static str,
    args: &'static [&'static str],
}

/// The standard security handler in each of its forms that PDFium reads: RC4 with a 40 and a 128
/// bit key, AES with a 128 bit key (revision 4) and AES with a 256 bit key (revision 6).
const WAYS: [Way; 4] = [
    Way {
        name: "RC4 40",
        args: &["--allow-weak-crypto", "40", "--"],
    },
    Way {
        name: "RC4 128",
        args: &["--allow-weak-crypto", "128", "--use-aes=n", "--"],
    },
    Way {
        name: "AES 128",
        args: &["128", "--use-aes=y", "--"],
    },
    Way {
        name: "AES 256",
        args: &["256", "--"],
    },
];

/// The ways that can leave the metadata in the clear (`/EncryptMetadata false`): revision 4 and up.
const CLEAR_WAYS: [Way; 3] = [
    Way {
        name: "RC4 128",
        args: &[
            "--allow-weak-crypto",
            "128",
            "--use-aes=n",
            "--cleartext-metadata",
            "--",
        ],
    },
    Way {
        name: "AES 128",
        args: &["128", "--use-aes=y", "--cleartext-metadata", "--"],
    },
    Way {
        name: "AES 256",
        args: &["256", "--cleartext-metadata", "--"],
    },
];

/// `plain` protected by qpdf in `way`, with [`PASSWORD`] as the user password.
fn protect(dir: &Dir, plain: &[u8], way: &Way) -> Vec<u8> {
    let input = dir.write("plain.pdf", plain);
    let output = dir.0.join("protected.pdf");
    // An option for all of qpdf (the weak ciphers) comes before the encryption options.
    let (weak, tail) = match way.args.split_first() {
        Some((&"--allow-weak-crypto", tail)) => (true, tail),
        _ => (false, way.args),
    };
    let mut args: Vec<&str> = Vec::new();
    if weak {
        args.push("--allow-weak-crypto");
    }
    args.extend(["--encrypt", PASSWORD, "owner-secret"]);
    args.extend_from_slice(tail);
    let input_str = input.to_str().unwrap();
    let output_str = output.to_str().unwrap();
    args.push(input_str);
    args.push(output_str);
    let (ok, text) = qpdf(&args);
    assert!(ok, "qpdf could not protect the file ({}): {text}", way.name);
    std::fs::read(&output).unwrap()
}

/// What qpdf reads from `file` with the password: the file without its encryption, with the
/// streams uncompressed.
fn read_back(dir: &Dir, file: &[u8], what: &str) -> Vec<u8> {
    let input = dir.write("saved.pdf", file);
    let output = dir.0.join("read-back.pdf");
    let (ok, text) = qpdf(&[
        &format!("--password={PASSWORD}"),
        "--decrypt",
        "--stream-data=uncompress",
        input.to_str().unwrap(),
        output.to_str().unwrap(),
    ]);
    assert!(ok, "{what}: qpdf could not read the saved file: {text}");
    std::fs::read(&output).unwrap()
}

/// qpdf checks the structure of the file and the decryption of every stream.
fn assert_clean(dir: &Dir, file: &[u8], what: &str) {
    let input = dir.write("checked.pdf", file);
    let (ok, text) = qpdf(&[
        &format!("--password={PASSWORD}"),
        "--check",
        input.to_str().unwrap(),
    ]);
    assert!(ok, "{what}: qpdf --check: {text}");
    assert!(
        !text.contains("WARNING") && !text.contains("ERROR"),
        "{what}: qpdf --check: {text}"
    );
}

/// The error of a save that was refused; a save that was not is a failure of the test, and the
/// file is not printed.
fn refused(result: Result<Vec<u8>, (ErrorCode, String)>, what: &str) -> (ErrorCode, String) {
    match result {
        Ok(file) => panic!(
            "{what}: the save was not refused, it gave {} bytes",
            file.len()
        ),
        Err(error) => error,
    }
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

fn plain_in(file: &[u8], text: &str) -> bool {
    contains(file, text.as_bytes())
}

#[test]
fn the_streams_of_a_protected_file_are_written_encrypted_with_and_without_an_edit() {
    if !have_qpdf() {
        return;
    }
    let dir = Dir::new("streams");
    for way in WAYS.iter().chain(CLEAR_WAYS.iter()) {
        // No XMP stream: the page content, the character map of the font, the colour profile and
        // the embedded file are the streams that have to come out encrypted.
        let bytes = protect(&dir, &kit::xmp_pdf(false, false), way);
        for edit in [false, true] {
            let what = format!("{}, edit {edit}", way.name);
            let mut guard = engine();
            let e = &mut guard.0;
            let doc = open(e, &bytes, Some(PASSWORD)).unwrap();
            if edit {
                add_text(e, doc);
            }
            let saved = try_save(e, doc).unwrap_or_else(|err| panic!("{what}: {err:?}"));
            for phrase in kit::XMP_OTHER_PHRASES.iter().chain(&["AN-EDIT"]) {
                assert!(
                    !plain_in(&saved, phrase),
                    "{what}: {phrase} is in the clear"
                );
            }
            assert_eq!(
                open(e, &saved, None).unwrap_err(),
                ErrorCode::PasswordRequired,
                "{what}"
            );
            assert_clean(&dir, &saved, &what);
            let back = read_back(&dir, &saved, &what);
            for phrase in kit::XMP_OTHER_PHRASES {
                assert!(
                    plain_in(&back, phrase),
                    "{what}: qpdf does not read {phrase}"
                );
            }
            let again = open(e, &saved, Some(PASSWORD)).unwrap();
            assert_eq!(page_text(e, again, 0).0.contains("AN-EDIT"), edit, "{what}");
            ask(e, RequestBody::Close { doc: again });
            ask(e, RequestBody::Close { doc });
        }
    }
}

#[test]
fn a_protected_document_with_xmp_metadata_is_not_saved_in_the_clear() {
    if !have_qpdf() {
        return;
    }
    let dir = Dir::new("refused");
    // An XMP stream on the catalog, on the catalog and a page, written uncompressed and
    // compressed (a compressed stream is saved without its filter).
    for (catalog, page) in [(true, false), (true, true), (false, true)] {
        for compressed in [false, true] {
            let plain = kit::xmp_pdf_with(catalog, page, compressed);
            for way in &WAYS {
                let bytes = protect(&dir, &plain, way);
                for edit in [false, true] {
                    let what = format!(
                        "{}, catalog {catalog}, page {page}, compressed {compressed}, edit {edit}",
                        way.name
                    );
                    let mut guard = engine();
                    let e = &mut guard.0;
                    let doc = open(e, &bytes, Some(PASSWORD)).unwrap();
                    if edit {
                        add_text(e, doc);
                    }
                    let (code, message) = refused(try_save(e, doc), &what);
                    assert_eq!(code, ErrorCode::SaveMetadataNotEncrypted, "{what}");
                    assert!(message.contains("XMP metadata"), "{what}: {message}");
                    assert_eq!(code.name(), "save_metadata_not_encrypted");
                    // The document is as it was: the same answer again, and it can be read.
                    let (again, _) = refused(try_save(e, doc), &what);
                    assert_eq!(again, ErrorCode::SaveMetadataNotEncrypted, "{what}");
                    assert!(matches!(
                        ask(e, RequestBody::DocInfo { doc }),
                        ResponseBody::DocInfo { pages: 1, .. }
                    ));
                    ask(e, RequestBody::Close { doc });
                }
            }
        }
    }
}

#[test]
fn a_file_that_leaves_the_metadata_in_the_clear_keeps_it_unchanged() {
    if !have_qpdf() {
        return;
    }
    let dir = Dir::new("clear");
    let packet = kit::xmp_packet(kit::XMP_CATALOG_TITLE);
    for way in &CLEAR_WAYS {
        let bytes = protect(&dir, &kit::xmp_pdf(true, false), way);
        // The input says what it should: the XMP stream is readable without the password.
        assert!(
            plain_in(&bytes, &packet),
            "{}: the input is not clear",
            way.name
        );
        for edit in [false, true] {
            let what = format!("{}, edit {edit}", way.name);
            let mut guard = engine();
            let e = &mut guard.0;
            let doc = open(e, &bytes, Some(PASSWORD)).unwrap();
            if edit {
                add_text(e, doc);
            }
            let saved = try_save(e, doc).unwrap_or_else(|err| panic!("{what}: {err:?}"));
            // The stream is the one of the original, byte for byte, and in the clear as the
            // dictionary says; everything else is encrypted.
            assert!(plain_in(&saved, &packet), "{what}: the metadata changed");
            for phrase in kit::XMP_OTHER_PHRASES.iter().chain(&["AN-EDIT"]) {
                assert!(
                    !plain_in(&saved, phrase),
                    "{what}: {phrase} is in the clear"
                );
            }
            assert_clean(&dir, &saved, &what);
            // qpdf, which decrypts the other streams and leaves this one alone, reads the same.
            let back = read_back(&dir, &saved, &what);
            assert!(
                plain_in(&back, &packet),
                "{what}: qpdf reads other metadata"
            );
            for phrase in kit::XMP_OTHER_PHRASES {
                assert!(
                    plain_in(&back, phrase),
                    "{what}: qpdf does not read {phrase}"
                );
            }
            // And the saved file opens again, and saves again, with the same metadata.
            let again = open(e, &saved, Some(PASSWORD)).unwrap();
            let twice = try_save(e, again).unwrap_or_else(|err| panic!("{what}: {err:?}"));
            assert!(
                plain_in(&twice, &packet),
                "{what}: the second save changed it"
            );
            ask(e, RequestBody::Close { doc });
            ask(e, RequestBody::Close { doc: again });
        }
    }
}

#[test]
fn clear_metadata_that_a_save_would_damage_is_refused() {
    if !have_qpdf() {
        return;
    }
    let dir = Dir::new("clear-damage");
    let packet = kit::xmp_packet(kit::XMP_CATALOG_TITLE);
    for way in &CLEAR_WAYS {
        // A second XMP stream (on a page) is in the clear in this file too (qpdf leaves every
        // metadata stream alone), but PDFium takes it for an encrypted one and decrypts it.
        let what = format!("{}, a stream on a page", way.name);
        let bytes = protect(&dir, &kit::xmp_pdf(true, true), way);
        assert!(
            plain_in(&bytes, &kit::xmp_packet(kit::XMP_PAGE_TITLE)),
            "{what}"
        );
        let mut guard = engine();
        let e = &mut guard.0;
        let doc = open(e, &bytes, Some(PASSWORD)).unwrap();
        let (code, _) = refused(try_save(e, doc), &what);
        assert_eq!(code, ErrorCode::SaveMetadataNotEncrypted, "{what}");
        ask(e, RequestBody::Close { doc });

        // A stream in the clear that is not XML: PDFium writes a compressed stream as its
        // compressed bytes and takes the filter off, and this is what a reader then finds. (The
        // start of the text is turned into the start of a zlib stream, byte for byte.)
        let what = format!("{}, a stream that is not XML", way.name);
        let mut bytes = protect(&dir, &kit::xmp_pdf(true, false), way);
        let at = bytes
            .windows(packet.len())
            .position(|w| w == packet.as_bytes())
            .expect("the input is in the clear");
        bytes[at..at + 2].copy_from_slice(&[0x78, 0x9c]);
        let doc = open(e, &bytes, Some(PASSWORD)).unwrap();
        let (code, _) = refused(try_save(e, doc), &what);
        assert_eq!(code, ErrorCode::SaveMetadataNotEncrypted, "{what}");
        ask(e, RequestBody::Close { doc });
    }
}

#[test]
fn a_file_that_holds_plain_metadata_but_says_it_is_encrypted_is_not_saved_again() {
    if !have_qpdf() {
        return;
    }
    // What earlier versions wrote: an AES 256 file that says `/EncryptMetadata true` (the key
    // does not depend on it), with the metadata in the clear. Saving it would decrypt the clear
    // text into garbage and write that.
    let dir = Dir::new("damaged");
    let way = &CLEAR_WAYS[2];
    let mut bytes = protect(&dir, &kit::xmp_pdf(true, false), way);
    let at = bytes
        .windows(b"/EncryptMetadata false".len())
        .position(|w| w == b"/EncryptMetadata false")
        .expect("the file says it leaves the metadata in the clear");
    let value = at + b"/EncryptMetadata ".len();
    bytes[value..value + 5].copy_from_slice(b"true ");
    assert!(plain_in(&bytes, &kit::xmp_packet(kit::XMP_CATALOG_TITLE)));
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &bytes, Some(PASSWORD)).unwrap();
    let (code, _) = refused(try_save(e, doc), "the metadata would be garbled");
    assert_eq!(code, ErrorCode::SaveMetadataNotEncrypted);
}

#[test]
fn an_unprotected_document_keeps_its_metadata_unchanged() {
    // The refusal is for protected documents only.
    let plain = kit::xmp_pdf(true, true);
    let mut guard = engine();
    let e = &mut guard.0;
    for edit in [false, true] {
        let doc = open(e, &plain, None).unwrap();
        if edit {
            add_text(e, doc);
        }
        let saved = try_save(e, doc).unwrap_or_else(|err| panic!("edit {edit}: {err:?}"));
        for title in [kit::XMP_CATALOG_TITLE, kit::XMP_PAGE_TITLE] {
            assert!(
                plain_in(&saved, &kit::xmp_packet(title)),
                "edit {edit}: {title} changed"
            );
        }
        ask(e, RequestBody::Close { doc });
    }
}

#[test]
fn a_protected_document_without_metadata_is_saved_as_before() {
    // The document of the older test, with a text that is in no stream but the content.
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::encrypted_pdf("secret", 2), Some(PASSWORD)).unwrap();
    let saved = try_save(e, doc).unwrap();
    assert_eq!(
        open(e, &saved, None).unwrap_err(),
        ErrorCode::PasswordRequired
    );
}
