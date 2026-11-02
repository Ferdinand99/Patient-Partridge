//! The `Signatures` request, with the engine in this process and real PDFium: what PDFium lists,
//! what the engine makes of it, and the files an attacker would build.
//!
//! The sandbox and the process boundary are covered by `tests/signatures_process.rs`. PDFium wants
//! one user at a time, so the tests take turns on one engine.

use std::fs::File;
use std::sync::{Mutex, MutexGuard, OnceLock};

use partridge_engine::sigcheck::TrustList;
use partridge_engine::Engine;
use partridge_protocol::{
    CertificateTime, ErrorCode, Request, RequestBody, Response, ResponseBody, SignatureInfo,
    SignatureReason as R, SignatureReport, SignatureState as S, MAX_SIGNATURES,
};
use partridge_testkit as kit;
use partridge_testkit::signed::*;

mod common;

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

/// Opens `bytes`; the file lives as long as the returned guard.
fn open(engine: &mut Engine, bytes: &[u8]) -> (u32, kit::TempFile) {
    let file = kit::TempFile::new("sig.pdf", bytes);
    engine.set_document(File::open(file.path()).unwrap());
    match ask(engine, RequestBody::Open { password: None }) {
        ResponseBody::Opened { doc, .. } => (doc, file),
        other => panic!("{other:?}"),
    }
}

fn report_of(engine: &mut Engine, doc: u32, now: i64) -> SignatureReport {
    match ask(engine, RequestBody::Signatures { doc, now }) {
        ResponseBody::Signatures { report } => report,
        other => panic!("{other:?}"),
    }
}

/// Opens a file, asks for its signatures, and closes it again.
fn signatures(bytes: &[u8]) -> SignatureReport {
    let mut shared = engine();
    let engine = &mut shared.0;
    let (doc, _file) = open(engine, bytes);
    let report = report_of(engine, doc, NOW);
    ask(engine, RequestBody::Close { doc });
    report
}

#[track_caller]
fn expect(info: &SignatureInfo, state: S, reason: R) {
    assert_eq!(
        (info.state, info.reason),
        (state, reason),
        "got {:?}/{:?}",
        info.state,
        info.reason
    );
}

/// No signature of the report is intact: the reader makes no claim that anything is as signed.
#[track_caller]
fn none_intact(report: &SignatureReport) {
    for item in &report.items {
        assert!(
            !matches!(item.state, S::Trusted | S::UnknownSigner),
            "signature {} is called intact: {:?}/{:?}",
            item.index,
            item.state,
            item.reason
        );
    }
}

fn p256(name: &str) -> Identity {
    Identity::self_signed(KeyKind::P256, &CertSpec::new(name))
}

fn page_text(engine: &mut Engine, doc: u32) -> String {
    match ask(engine, RequestBody::PageText { doc, page: 0 }) {
        ResponseBody::PageText { text, .. } => text,
        other => panic!("{other:?}"),
    }
}

// ---- the plain cases ---------------------------------------------------------------------------

#[test]
fn a_document_without_signatures_has_none() {
    let report = signatures(&kit::simple_pdf(2));
    assert_eq!((report.total, report.unlisted), (0, 0));
    assert!(report.items.is_empty());
    assert_eq!(report.trust_anchors, 0);
}

#[test]
fn a_signed_document_is_described_from_the_file_and_pdfium() {
    let signer = p256("Test Signer");
    let options = SignOptions {
        doc_mdp: Some(2),
        ..SignOptions::default()
    };
    let file = signed_file(&signer, &options);
    let report = signatures(&file.bytes);
    assert_eq!(
        (report.total, report.unlisted, report.items.len()),
        (1, 0, 1)
    );
    let info = &report.items[0];
    expect(info, S::UnknownSigner, R::NoTrustList);
    assert_eq!(info.signer.as_deref(), Some("Test Signer"));
    assert_eq!(info.sub_filter, "adbe.pkcs7.detached");
    assert_eq!(
        info.stated_reason.as_deref(),
        Some("I approve this document")
    );
    assert_eq!(info.stated_location.as_deref(), Some("Oslo"));
    assert_eq!(
        info.claimed_time.as_deref(),
        Some("2026-06-15 12:00:00 +02:00")
    );
    assert_eq!(info.doc_mdp, 2);
    assert_eq!(
        info.byte_range,
        file.range.iter().map(|v| *v as u64).collect::<Vec<_>>()
    );
    assert_eq!(info.file_len, file.bytes.len() as u64);
    assert_eq!(info.certificate_time, CertificateTime::Valid);
    // Whatever the report says, it goes over the wire and comes back the same.
    let response = Response {
        id: 3,
        body: ResponseBody::Signatures {
            report: report.clone(),
        },
    };
    assert_eq!(Response::decode(response.encode()).unwrap(), response);
}

#[test]
fn a_signer_on_the_trust_list_is_trusted_and_only_then() {
    let root = Identity::self_signed(KeyKind::P256, &CertSpec::ca("Test Root"));
    let signer = Identity::issued_by(
        &root,
        KeyKind::P256,
        &CertSpec {
            serial: 9,
            ..CertSpec::new("Test Signer")
        },
    );
    let file = signed_file(&signer, &SignOptions::default());
    let mut shared = engine();
    let engine = &mut shared.0;
    let (doc, _file) = open(engine, &file.bytes);
    engine.set_trust_list_for_tests(TrustList::from_der(std::slice::from_ref(&root.cert)).unwrap());
    let report = report_of(engine, doc, NOW);
    engine.set_trust_list_for_tests(TrustList::empty());
    // The signer's certificate is not in the signature's chain, but it names the root as issuer
    // and was signed by it: the root is on the list.
    assert_eq!(report.trust_anchors, 1);
    expect(&report.items[0], S::Trusted, R::Verified);
    assert_eq!(report.items[0].trust_anchor.as_deref(), Some("Test Root"));
    // The same file with the list taken away again.
    let report = report_of(engine, doc, NOW);
    expect(&report.items[0], S::UnknownSigner, R::NoTrustList);
    ask(engine, RequestBody::Close { doc });
}

#[test]
fn the_trust_list_that_ships_is_empty_and_can_be_read() {
    let shipped = TrustList::shipped();
    assert!(shipped.is_empty(), "this version ships no trust anchors");
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("trust/anchors.pem"),
    )
    .unwrap();
    assert!(TrustList::from_pem(text.as_bytes()).is_ok());
    // No certificate in the shipped list, so no signature in a shipped engine is ever trusted.
    let signer = p256("Anyone");
    let report = signatures(&signed_file(&signer, &SignOptions::default()).bytes);
    assert_eq!(report.trust_anchors, 0);
    expect(&report.items[0], S::UnknownSigner, R::NoTrustList);
}

#[test]
fn the_raw_handle_of_a_document_can_be_read() {
    let mut shared = engine();
    let engine = &mut shared.0;
    let (doc, _file) = open(engine, &kit::simple_pdf(1));
    // The engine reads the handle from the text PDFium's wrapper gives for it. If that text ever
    // changes, signatures cannot be read and this test is the one that says so: the report below
    // would be an error.
    let report = report_of(engine, doc, NOW);
    assert_eq!(report.total, 0);
    let (doc2, _file2) = open(
        engine,
        &signed_file(&p256("S"), &SignOptions::default()).bytes,
    );
    assert_eq!(report_of(engine, doc2, NOW).total, 1);
    ask(engine, RequestBody::Close { doc });
    ask(engine, RequestBody::Close { doc: doc2 });
}

// ---- the attacks -----------------------------------------------------------------------------------

/// A save after signing (an incremental update): what the page shows is now different from what
/// was signed, and the panel says so.
#[test]
fn a_change_saved_after_signing_is_reported_and_the_page_shows_the_change() {
    let signer = p256("Test Signer");
    let file = signed_file(&signer, &SignOptions::default());
    let edited = change_text_revision(&file.bytes, "Pay 900 to Mallory");
    let mut shared = engine();
    let engine = &mut shared.0;
    let (doc, _f) = open(engine, &edited);
    // The page shows the new text: a reader that looked at the page would see it signed.
    assert!(page_text(engine, doc).contains("Pay 900 to Mallory"));
    let report = report_of(engine, doc, NOW);
    assert_eq!(report.items.len(), 1);
    expect(&report.items[0], S::Changed, R::DataAfterSignature);
    ask(engine, RequestBody::Close { doc });
}

#[test]
fn a_change_inside_the_signed_bytes_is_reported() {
    let signer = p256("Test Signer");
    let mut file = signed_file(&signer, &SignOptions::default());
    replace_in_place(&mut file.bytes, b"Pay 100 to", b"Pay 900 to");
    let report = signatures(&file.bytes);
    expect(&report.items[0], S::Changed, R::ContentChanged);
}

#[test]
fn a_signature_in_a_file_that_leaves_out_more_than_the_signature_is_not_intact() {
    let signer = p256("Test Signer");
    let file = signed_file(&signer, &SignOptions::default());
    let [a, b, c, d] = file.range.map(|v| v as u64);
    for (range, reason) in [
        ([a, b, c + 30, d - 30], R::RangeLeavesGap),
        ([a, b, c, d + 5], R::RangeInvalid),
        ([a, b + 3, c, d], R::RangeLeavesGap),
        ([5, b, c, d], R::RangeInvalid),
    ] {
        let mut bytes = file.bytes.clone();
        set_byte_range(&mut bytes, range);
        let report = signatures(&bytes);
        assert_eq!(report.items.len(), 1, "{range:?}");
        expect(&report.items[0], S::Broken, reason);
    }
}

/// The signature wrapping attack. The attacker builds a file in which the part the byte range
/// leaves out is bigger than the signature: it holds the signature and, behind it, a new
/// version of the signature dictionary with a byte range that covers the same bytes as before,
/// and a new page. A reader that only compares the digest of the covered bytes with the signature
/// sees a signature that holds.
#[test]
fn signature_wrapping_is_not_taken_for_a_good_signature() {
    let signer = p256("Test Signer");
    let file = signed_file(&signer, &SignOptions::default());
    let [_, b, c, d] = file.range;
    let hex = &file.bytes[file.contents_at..file.contents_end];
    let digits = 10;
    let template = |end: usize| {
        let mut evil = format!(
            "\n6 0 obj\n<< /Length 38 >>\nstream\nBT /F1 24 Tf 72 700 Td (Pay 900 to Mallory) Tj ET\nendstream\nendobj\n7 0 obj\n<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [{:0w$} {:0w$} {:0w$} {:0w$}] /Contents ",
            0, b, end, d, w = digits
        )
        .into_bytes();
        evil.extend_from_slice(hex);
        evil.extend_from_slice(b" >>\nendobj\n");
        evil
    };
    let evil_len = template(0).len();
    let evil = template(c + evil_len);
    let mut bytes = file.bytes[..c].to_vec();
    bytes.extend_from_slice(&evil);
    bytes.extend_from_slice(&file.bytes[c..]);
    // The second part, as the attacker's dictionary gives it, is the original second part: with
    // the byte range text in the first part unchanged, the digest is the one that was signed.
    assert_eq!(&bytes[..b], &file.bytes[..b]);
    assert_eq!(&bytes[c + evil_len..], &file.bytes[c..]);
    assert_eq!(d, file.bytes.len() - c);

    let mut shared = engine();
    let engine = &mut shared.0;
    let (doc, _f) = open(engine, &bytes);
    let report = report_of(engine, doc, NOW);
    ask(engine, RequestBody::Close { doc });
    none_intact(&report);
    // PDFium lists the dictionary the attacker wrote, and the check refuses it: the part left out
    // of the byte range is more than the signature. The first dictionary is still in the file
    // and is counted as a mark nobody owns.
    assert_eq!(report.items.len(), 1, "{report:?}");
    expect(&report.items[0], S::Broken, R::RangeLeavesGap);
    assert_eq!(report.unlisted, 1);
}

#[test]
fn a_signature_hidden_by_a_later_save_leaves_a_trace() {
    let signer = p256("Test Signer");
    let file = signed_file(&signer, &SignOptions::default());
    // The next revision takes the field out of the form and the page: a reader that only follows
    // the form sees no signature in the file.
    let hidden = append_revision(
        &file.bytes,
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
            (
                4,
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 6 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_vec(),
            ),
        ],
        10,
    );
    let report = signatures(&hidden);
    none_intact(&report);
    // PDFium lists nothing, and the file still holds the byte range of the signature.
    assert_eq!((report.total, report.unlisted), (0, 1), "{report:?}");
}

#[test]
fn a_signature_dictionary_replaced_by_a_later_save_is_not_intact_and_leaves_a_trace() {
    let signer = p256("Test Signer");
    let file = signed_file(&signer, &SignOptions::default());
    // A new version of the dictionary, with a byte range that suits the new file and made-up data.
    let forged = append_revision(
        &file.bytes,
        &[(
            7,
            format!(
                "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 0 0 0] /Contents <{}> /M (D:20260615120000Z) >>",
                "30".repeat(16)
            )
            .into_bytes(),
        )],
        10,
    );
    let report = signatures(&forged);
    none_intact(&report);
    expect(&report.items[0], S::Broken, R::RangeLeavesGap);
    // The old dictionary is still in the file: its byte range is a mark that no listed signature owns.
    assert_eq!(report.unlisted, 1);
}

#[test]
fn what_the_page_says_about_the_signature_is_not_believed() {
    // The page is a picture of a stamp. The signature behind it is made up. And the dictionary
    // claims a name that is not in the certificate.
    let signer = p256("Mallory");
    let options = SignOptions {
        text: "SIGNED AND VERIFIED - Trusted Bank AS - signature valid".into(),
        name: Some("Trusted Bank AS".into()),
        ..SignOptions::default()
    };
    let mut file = signed_file(&signer, &options);
    // Damage the signature data (a made-up signature).
    let at = file.contents_at + 1;
    file.bytes[at..at + 12].copy_from_slice(b"300301020300");
    let mut shared = engine();
    let engine = &mut shared.0;
    let (doc, _f) = open(engine, &file.bytes);
    assert!(page_text(engine, doc).contains("SIGNED AND VERIFIED"));
    let report = report_of(engine, doc, NOW);
    ask(engine, RequestBody::Close { doc });
    assert_eq!(report.items.len(), 1);
    none_intact(&report);
    assert_ne!(report.items[0].signer.as_deref(), Some("Trusted Bank AS"));
    drop(shared);
    // The real signature, same page, is named by the certificate and not by the claim.
    let file = signed_file(&signer, &options);
    let report = signatures(&file.bytes);
    assert_eq!(report.items[0].signer.as_deref(), Some("Mallory"));
}

#[test]
fn files_with_made_up_signatures_are_broken_and_files_with_signatures_pdfium_misses_leave_marks() {
    // A made-up signature with a byte range that cannot be real.
    let report = signatures(&kit::signed_pdf());
    assert_eq!(report.items.len(), 1);
    expect(&report.items[0], S::Broken, R::RangeLeavesGap);
    // Signatures PDFium does not list (nested in a group, or hanging off the catalog): the file
    // still has the words that belong to a signature, and the report counts them.
    for bytes in [kit::signed_nested_pdf(), kit::signed_without_field_pdf()] {
        let report = signatures(&bytes);
        assert_eq!(report.total, 0);
        assert_eq!(report.unlisted, 1);
    }
}

#[test]
fn two_signatures_are_both_described_and_the_first_is_changed_by_the_second() {
    let first = p256("First Signer");
    let second = p256("Second Signer");
    let one = signed_file(&first, &SignOptions::default());
    let two = add_signature(&one, &second, &SignOptions::default());
    let report = signatures(&two.bytes);
    assert_eq!(
        (report.total, report.items.len(), report.unlisted),
        (2, 2, 0)
    );
    let by_name = |name: &str| {
        report
            .items
            .iter()
            .find(|i| i.signer.as_deref() == Some(name))
            .unwrap()
    };
    expect(by_name("First Signer"), S::Changed, R::DataAfterSignature);
    expect(by_name("Second Signer"), S::UnknownSigner, R::NoTrustList);
}

#[test]
fn signatures_that_are_not_ours_to_check_are_unsupported() {
    let signer = p256("Test Signer");
    let options = SignOptions {
        sub_filter: "ETSI.RFC3161".into(),
        ..SignOptions::default()
    };
    let report = signatures(&signed_file(&signer, &options).bytes);
    expect(&report.items[0], S::Unsupported, R::DocumentTimestamp);
}

// ---- limits -------------------------------------------------------------------------------------------

#[test]
fn only_the_first_signatures_are_described_but_all_are_counted() {
    let count = MAX_SIGNATURES + 6;
    let report = signatures(&fake_signatures_pdf(count));
    assert_eq!(report.total as usize, count);
    assert_eq!(report.items.len(), MAX_SIGNATURES);
    assert!(report
        .items
        .iter()
        .enumerate()
        .all(|(i, item)| item.index as usize == i));
    // The report fits the protocol.
    let response = Response {
        id: 1,
        body: ResponseBody::Signatures { report },
    };
    assert!(Response::decode(response.encode()).is_ok());
}

#[test]
fn signature_data_that_is_too_big_is_unsupported() {
    let signer = p256("Test Signer");
    let options = SignOptions {
        contents_size: 1_100_000,
        ..SignOptions::default()
    };
    let report = signatures(&signed_file(&signer, &options).bytes);
    expect(&report.items[0], S::Unsupported, R::TooLarge);
}

#[test]
fn a_file_that_changes_while_it_is_open_is_not_checked() {
    let signer = p256("Test Signer");
    let file = signed_file(&signer, &SignOptions::default());
    let mut shared = engine();
    let engine = &mut shared.0;
    let (doc, temp) = open(engine, &file.bytes);
    // Another program changes the file under the open document.
    let mut changed = file.bytes.clone();
    changed[3] ^= 1;
    std::fs::write(temp.path(), &changed).unwrap();
    match ask(engine, RequestBody::Signatures { doc, now: NOW }) {
        ResponseBody::Error { code, .. } => {
            assert!(matches!(
                code,
                ErrorCode::SourceChanged | ErrorCode::SourceUnreadable
            ))
        }
        other => panic!("{other:?}"),
    }
    ask(engine, RequestBody::Close { doc });
}

#[test]
fn an_unknown_document_is_an_error() {
    let mut shared = engine();
    match ask(
        &mut shared.0,
        RequestBody::Signatures {
            doc: 99_999,
            now: NOW,
        },
    ) {
        ResponseBody::Error { code, .. } => assert_eq!(code, ErrorCode::UnknownDocument),
        other => panic!("{other:?}"),
    }
}

// ---- shadow attacks -----------------------------------------------------------------------------

/// A shadow attack: the signed file holds a second content stream that no page shows. After
/// signing, a revision makes the page show it. The signature still holds for every byte it
/// covers, and the page shows something that was never on it when it was signed. Whether the
/// signer allowed the change is not the reader's to say: the signature is reported as changed,
/// also when the signer asked for no changes at all (`DocMDP` level 1).
#[test]
fn a_shadow_attack_that_shows_hidden_content_after_signing_is_a_change() {
    let signer = p256("Test Signer");
    let options = SignOptions {
        hidden_text: Some("Pay 900 to Mallory".into()),
        doc_mdp: Some(1),
        ..SignOptions::default()
    };
    let file = signed_file(&signer, &options);
    // As signed, the page shows the honest text, and the signature is intact.
    {
        let mut shared = engine();
        let engine = &mut shared.0;
        let (doc, _f) = open(engine, &file.bytes);
        let text = page_text(engine, doc);
        assert!(
            text.contains("Pay 100 to Alice") && !text.contains("Mallory"),
            "{text}"
        );
        let report = report_of(engine, doc, NOW);
        expect(&report.items[0], S::UnknownSigner, R::NoTrustList);
        assert_eq!(report.items[0].doc_mdp, 1);
        ask(engine, RequestBody::Close { doc });
    }
    // After the attack, the page shows the hidden text, and the signature says it was changed.
    let shown = show_hidden_content_revision(&file.bytes);
    let mut shared = engine();
    let engine = &mut shared.0;
    let (doc, _f) = open(engine, &shown);
    let text = page_text(engine, doc);
    assert!(
        text.contains("Pay 900 to Mallory") && !text.contains("Alice"),
        "{text}"
    );
    let report = report_of(engine, doc, NOW);
    none_intact(&report);
    expect(&report.items[0], S::Changed, R::DataAfterSignature);
    ask(engine, RequestBody::Close { doc });
}

// ---- a read that fails, and a draw that fails, after the verdict ---------------------------------

/// A content stream with `text` in it, as an object body.
fn stream_object(text: &str) -> Vec<u8> {
    format!("<< /Length {} >>\nstream\n{text}\nendstream", text.len()).into_bytes()
}

/// A signed page of two content streams, far apart in the file: the text, and a stamp over it that
/// the signer saw and signed.
fn stamped_page() -> SignedFile {
    let options = SignOptions::default();
    let mut objects: Vec<(u32, Vec<u8>)> = Vec::new();
    for (n, body) in base_objects(&options) {
        match n {
            4 => objects.push((
                4,
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents [6 0 R 9 0 R] /Resources << /Font << /F1 3 0 R >> >> /Annots [5 0 R] >>".to_vec(),
            )),
            7 => {
                // Enough in between that the stamp is in another block of the file than the text.
                objects.push((8, stream_object(&"% filler\n".repeat(8000))));
                objects.push((9, stream_object("BT /F1 24 Tf 72 600 Td (VOID DO NOT PAY) Tj ET")));
                objects.push((7, body));
            }
            _ => objects.push((n, body)),
        }
    }
    sign_placeholder(write_file(&objects), &p256("Test Signer"), &options)
}

/// Opens `bytes` from a file the test controls.
fn open_controlled(engine: &mut Engine, bytes: &[u8]) -> (u32, common::Controls) {
    let (file, controls) = common::ControlledFile::new(bytes);
    engine.set_document_source(Box::new(file));
    match ask(engine, RequestBody::Open { password: None }) {
        ResponseBody::Opened { doc, .. } => (doc, controls),
        other => panic!("{other:?}"),
    }
}

fn error_code(body: &ResponseBody) -> Option<ErrorCode> {
    match body {
        ResponseBody::Error { code, .. } => Some(*code),
        _ => None,
    }
}

fn render(engine: &mut Engine, doc: u32, page: u32) -> ResponseBody {
    ask(
        engine,
        RequestBody::Render {
            doc,
            page,
            width: 300,
            height: 390,
        },
    )
}

#[test]
fn a_stamp_does_not_vanish_from_a_page_under_an_intact_signature() {
    let file = stamped_page();
    let stamp_at = file.bytes.windows(4).position(|w| w == b"VOID").unwrap() as u64;
    let stamp_block = stamp_at / partridge_engine::source::BLOCK;
    let text_block = file.bytes.windows(4).position(|w| w == b"Pay ").unwrap() as u64
        / partridge_engine::source::BLOCK;
    assert_ne!(stamp_block, text_block, "the stamp has a block of its own");

    let mut shared = engine();
    let engine = &mut shared.0;

    // As signed, the page shows the stamp and the signature is intact.
    let (doc, _controls) = open_controlled(engine, &file.bytes);
    expect(
        &report_of(engine, doc, NOW).items[0],
        S::UnknownSigner,
        R::NoTrustList,
    );
    let signed_text = page_text(engine, doc);
    assert!(signed_text.contains("VOID DO NOT PAY"), "{signed_text}");
    ask(engine, RequestBody::Close { doc });

    // Before any verdict there is nothing to protect: a page that lost a part to a failed read is
    // drawn as it always was.
    let (doc, controls) = open_controlled(engine, &file.bytes);
    controls.fail_reads_of_block(stamp_block);
    let text = page_text(engine, doc);
    assert!(
        text.contains("Pay 100 to Alice") && !text.contains("VOID"),
        "{text}"
    );
    ask(engine, RequestBody::Close { doc });

    // After the verdict, the read of the block with the stamp fails. The text and the drawing of the
    // page are refused: a page without the stamp must not stand under the "intact" badge.
    let (doc, controls) = open_controlled(engine, &file.bytes);
    expect(
        &report_of(engine, doc, NOW).items[0],
        S::UnknownSigner,
        R::NoTrustList,
    );
    controls.fail_reads_of_block(stamp_block);
    let refused = |engine: &mut Engine, doc: u32| {
        [
            ask(engine, RequestBody::PageText { doc, page: 0 }),
            render(engine, doc, 0),
            ask(engine, RequestBody::PageLinks { doc, page: 0 }),
            ask(
                engine,
                RequestBody::Search {
                    doc,
                    page: 0,
                    query: "Alice".into(),
                },
            ),
            ask(engine, RequestBody::Outline { doc }),
        ]
        .iter()
        .map(error_code)
        .collect::<Vec<_>>()
    };
    let all_refused = vec![Some(ErrorCode::SourceUnreadable); 5];
    assert_eq!(refused(engine, doc), all_refused);
    // And it stays so when the share is back: what was parsed may hold the hole.
    controls.reads_work_again();
    assert_eq!(refused(engine, doc), all_refused);
    // A new question is refused too, as before.
    assert_eq!(
        error_code(&ask(engine, RequestBody::Signatures { doc, now: NOW })),
        Some(ErrorCode::SourceUnreadable)
    );
    ask(engine, RequestBody::Close { doc });
}

#[test]
fn a_draw_that_fails_after_the_file_changed_tells_the_window_the_file_changed() {
    // A document of two pages whose second page cannot be loaded: a draw of it fails as `Corrupt`.
    let options = SignOptions::default();
    let objects: Vec<(u32, Vec<u8>)> = base_objects(&options)
        .into_iter()
        .map(|(n, body)| match n {
            2 => (
                2,
                b"<< /Type /Pages /Kids [4 0 R 99 0 R] /Count 2 >>".to_vec(),
            ),
            _ => (n, body),
        })
        .collect();
    let file = sign_placeholder(write_file(&objects), &p256("Test Signer"), &options);

    let mut shared = engine();
    let engine = &mut shared.0;
    let (doc, controls) = open_controlled(engine, &file.bytes);
    expect(
        &report_of(engine, doc, NOW).items[0],
        S::UnknownSigner,
        R::NoTrustList,
    );
    // The page that works draws, and the one that does not fails the way it always did.
    assert!(matches!(
        render(engine, doc, 0),
        ResponseBody::Rendered { .. }
    ));
    let plain = |engine: &mut Engine| {
        [
            error_code(&render(engine, doc, 1)),
            error_code(&ask(engine, RequestBody::PageText { doc, page: 1 })),
            error_code(&ask(engine, RequestBody::PageLinks { doc, page: 1 })),
        ]
    };
    for code in plain(engine) {
        assert!(
            matches!(code, Some(ErrorCode::Corrupt | ErrorCode::RenderFailed)),
            "{code:?}"
        );
    }
    // The file grows after the verdict: the same failures are now what they are, a file that is not
    // the one that was checked.
    controls.data.lock().unwrap().push(b'\n');
    for code in plain(engine) {
        assert_eq!(code, Some(ErrorCode::SourceChanged));
    }
    ask(engine, RequestBody::Close { doc });
}
