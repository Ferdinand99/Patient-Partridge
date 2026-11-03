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
use partridge_protocol::{
    field_flag, ErrorCode, FormFieldKind, FormValue, RequestBody, ResponseBody,
};
use partridge_testkit as kit;
use pdfium_render::prelude::*;

const PASSWORD: &str = "secret";
const OWNER_PASSWORD: &str = "owner-secret";

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
    protect_for(dir, plain, way, PASSWORD)
}

/// As `protect`, with another user password (the owner password is always `owner-secret`).
fn protect_for(dir: &Dir, plain: &[u8], way: &Way, user: &str) -> Vec<u8> {
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
    args.extend(["--encrypt", user, OWNER_PASSWORD]);
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
    read_back_with(dir, file, what, PASSWORD)
}

fn read_back_with(dir: &Dir, file: &[u8], what: &str, password: &str) -> Vec<u8> {
    let input = dir.write("saved.pdf", file);
    let output = dir.0.join("read-back.pdf");
    let (ok, text) = qpdf(&[
        &format!("--password={password}"),
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
    assert_clean_with(dir, file, what, PASSWORD)
}

fn assert_clean_with(dir: &Dir, file: &[u8], what: &str, password: &str) {
    let input = dir.write("checked.pdf", file);
    let (ok, text) = qpdf(&[
        &format!("--password={password}"),
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

#[test]
fn a_protected_form_with_xmp_is_filled_in_and_is_not_saved() {
    if !have_qpdf() {
        return;
    }
    // A form that is protected with an owner password only (the user password is empty, so it
    // opens without one) and carries XMP metadata, as most forms do. The bytes that the form code
    // looks at never leave the engine, so the check must not run on them: the fields stay
    // editable. The save is what is refused.
    let dir = Dir::new("form");
    let way = &WAYS[3];
    let mut baseline: Option<Vec<(String, u16)>> = None;
    for xmp in [false, true] {
        let plain = kit::forms_with(&kit::FormOptions {
            xmp,
            ..kit::FormOptions::default()
        });
        let bytes = protect_for(&dir, &plain, way, "");
        let mut guard = engine();
        let e = &mut guard.0;
        let doc = open(e, &bytes, None).unwrap();
        let ResponseBody::FormFields { fields } =
            ask(e, RequestBody::ListFormFields { doc, page: 0 })
        else {
            panic!("no fields");
        };
        assert!(fields.len() > 5, "xmp {xmp}: {} fields", fields.len());
        // No field is blocked as a whole (the form is not taken for one with actions), and the
        // flags are the ones of the same form without the metadata (one field is read only in
        // the form itself).
        let blocked = field_flag::FORM_BLOCKED | field_flag::ACTION_BLOCKED;
        let flags: Vec<(String, u16)> = fields.iter().map(|f| (f.name.clone(), f.flags)).collect();
        for (name, flag) in &flags {
            assert_eq!(
                flag & blocked,
                0,
                "xmp {xmp}: {name} is blocked ({flag:#x})"
            );
        }
        match &baseline {
            None => baseline = Some(flags),
            Some(before) => assert_eq!(&flags, before, "the flags changed with the metadata"),
        }
        let text = fields
            .iter()
            .find(|f| f.kind == FormFieldKind::Text)
            .expect("a text field");
        let answer = ask(
            e,
            RequestBody::SetFormField {
                doc,
                page: 0,
                index: text.index,
                name: text.name.clone(),
                value: FormValue::Text("filled in".into()),
            },
        );
        assert!(
            matches!(answer, ResponseBody::FormFields { .. }),
            "xmp {xmp}: {answer:?}"
        );
        match try_save(e, doc) {
            Ok(saved) => {
                assert!(!xmp, "a form with XMP was saved");
                assert_clean_with(&dir, &saved, "form", "");
            }
            Err((code, _)) => {
                assert!(xmp, "a form without XMP was refused");
                assert_eq!(code, ErrorCode::SaveMetadataNotEncrypted);
            }
        }
        ask(e, RequestBody::Close { doc });
    }
}

#[test]
fn a_file_protected_with_an_owner_password_only_is_refused_with_metadata_too() {
    if !have_qpdf() {
        return;
    }
    // The most common real case: no user password, so nobody types one, and XMP on the catalog.
    let dir = Dir::new("owner-only");
    for way in &WAYS {
        let bytes = protect_for(&dir, &kit::xmp_pdf(true, false), way, "");
        let mut guard = engine();
        let e = &mut guard.0;
        let doc = open(e, &bytes, None).unwrap();
        let (code, _) = refused(try_save(e, doc), way.name);
        assert_eq!(code, ErrorCode::SaveMetadataNotEncrypted, "{}", way.name);
        ask(e, RequestBody::Close { doc });
    }
}

#[test]
fn a_document_opened_with_the_owner_password_is_written_again_with_that_password() {
    if !have_qpdf() {
        return;
    }
    // A save reads the written bytes in again as a second document, with the password the
    // document was opened with: here the owner password, for a revision 3 (RC4 128) file and a
    // revision 6 (AES 256) one.
    let dir = Dir::new("owner-open");
    for way in [&WAYS[1], &WAYS[3]] {
        let bytes = protect(&dir, &kit::xmp_pdf(false, false), way);
        let mut guard = engine();
        let e = &mut guard.0;
        let doc = open(e, &bytes, Some(OWNER_PASSWORD)).unwrap();
        add_text(e, doc);
        let saved = try_save(e, doc).unwrap_or_else(|err| panic!("{}: {err:?}", way.name));
        assert_clean_with(&dir, &saved, way.name, OWNER_PASSWORD);
        let back = read_back_with(&dir, &saved, way.name, OWNER_PASSWORD);
        for phrase in kit::XMP_OTHER_PHRASES {
            assert!(plain_in(&back, phrase), "{}: {phrase}", way.name);
        }
        let again = open(e, &saved, Some(OWNER_PASSWORD)).unwrap();
        assert!(page_text(e, again, 0).0.contains("AN-EDIT"), "{}", way.name);
        ask(e, RequestBody::Close { doc: again });
    }
}

#[test]
fn a_file_of_an_older_revision_that_says_it_leaves_the_metadata_clear_is_kept_so() {
    // The generator's own protection (revision 2, RC4 40) with `/EncryptMetadata false`. The key
    // is read by PDFium for every revision, though a reader that follows the standard to the
    // letter reads it only for revision 4 and up; the check goes by what PDFium writes. The
    // stream is clear and is written back as it was; without the key it is refused.
    let mut guard = engine();
    let e = &mut guard.0;
    let packet = kit::xmp_packet(kit::XMP_CATALOG_TITLE);
    let clear = open(e, &kit::encrypted_xmp_pdf(PASSWORD, true), Some(PASSWORD)).unwrap();
    let saved = try_save(e, clear).expect("a clear stream is kept");
    assert!(plain_in(&saved, &packet));
    let again = open(e, &saved, Some(PASSWORD)).unwrap();
    assert!(try_save(e, again).is_ok());
    let locked = open(e, &kit::encrypted_xmp_pdf(PASSWORD, false), Some(PASSWORD)).unwrap();
    let (code, _) = refused(
        try_save(e, locked),
        "a stream that is meant to be encrypted",
    );
    assert_eq!(code, ErrorCode::SaveMetadataNotEncrypted);
}

#[test]
fn a_stream_whose_type_is_a_reference_is_encrypted_like_any_other() {
    if !have_qpdf() {
        return;
    }
    // PDFium does not follow the reference, so the stream is not a metadata stream to it and is
    // written encrypted; the check agrees and lets the save through. This pins the agreement: a
    // PDFium that followed the reference would make the saved file hold the packet in the clear.
    let dir = Dir::new("indirect");
    let packet = kit::xmp_packet(kit::XMP_CATALOG_TITLE);
    for way in &WAYS {
        let bytes = protect(&dir, &kit::xmp_indirect_type_pdf(), way);
        let mut guard = engine();
        let e = &mut guard.0;
        let doc = open(e, &bytes, Some(PASSWORD)).unwrap();
        let saved = try_save(e, doc).unwrap_or_else(|err| panic!("{}: {err:?}", way.name));
        assert!(!plain_in(&saved, &packet), "{}: in the clear", way.name);
        let back = read_back(&dir, &saved, way.name);
        assert!(
            plain_in(&back, &packet),
            "{}: qpdf does not read it",
            way.name
        );
        ask(e, RequestBody::Close { doc });
    }
}
