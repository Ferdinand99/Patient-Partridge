//! Tests of the signature messages: every cap at the cap and one past it, and text that is not
//! plain.

use crate::signatures::*;
use crate::{ProtocolError, Request, RequestBody, Response, ResponseBody};

fn info(index: u32) -> SignatureInfo {
    SignatureInfo {
        index,
        state: SignatureState::UnknownSigner,
        reason: SignatureReason::NoTrustList,
        certificate_time: CertificateTime::Valid,
        sub_filter: "adbe.pkcs7.detached".into(),
        byte_range: vec![0, 100, 300, 50],
        file_len: 350,
        doc_mdp: 2,
        certificates: 2,
        signer: Some("Åse Blåbær <b>not markup</b>".into()),
        organisation: Some("Example Ltd".into()),
        issuer: Some("Example CA".into()),
        serial: Some("01ab".into()),
        fingerprint: Some("ab".repeat(32)),
        valid_from: Some("2026-01-31 12:00 UTC".into()),
        valid_to: Some("2027-01-31 12:00 UTC".into()),
        key: Some("ECDSA P-256".into()),
        digest: Some("SHA-256".into()),
        claimed_time: Some("2026-02-01 08:00:00 +01:00".into()),
        stated_reason: Some("I approve".into()),
        stated_location: Some("Oslo".into()),
        trust_anchor: None,
    }
}

fn report(items: Vec<SignatureInfo>) -> SignatureReport {
    SignatureReport {
        total: items.len() as u32,
        unlisted: 1,
        trust_anchors: 0,
        items,
    }
}

fn response(report: SignatureReport) -> Vec<u8> {
    Response {
        id: 5,
        body: ResponseBody::Signatures { report },
    }
    .encode()
}

#[test]
fn a_request_and_a_report_survive_a_round_trip() {
    let request = Request {
        id: 3,
        body: RequestBody::Signatures {
            doc: 7,
            now: 1_780_000_000,
        },
    };
    assert_eq!(Request::decode(&request.encode()).unwrap(), request);
    let sent = Response {
        id: 9,
        body: ResponseBody::Signatures {
            report: report(vec![info(0), info(1)]),
        },
    };
    assert_eq!(Response::decode(sent.encode()).unwrap(), sent);
    // A report with nothing in it is a report.
    let empty = Response {
        id: 9,
        body: ResponseBody::Signatures {
            report: SignatureReport {
                total: 0,
                unlisted: 0,
                trust_anchors: 0,
                items: vec![],
            },
        },
    };
    assert_eq!(Response::decode(empty.encode()).unwrap(), empty);
}

#[test]
fn the_time_of_the_check_is_held_to_its_range() {
    for now in [0, 1, MAX_UNIX_TIME] {
        let bytes = Request {
            id: 1,
            body: RequestBody::Signatures { doc: 1, now },
        }
        .encode();
        assert!(Request::decode(&bytes).is_ok(), "{now}");
    }
    for now in [-1, MAX_UNIX_TIME + 1, i64::MAX, i64::MIN] {
        let bytes = Request {
            id: 1,
            body: RequestBody::Signatures { doc: 1, now },
        }
        .encode();
        assert!(Request::decode(&bytes).is_err(), "{now}");
    }
}

#[test]
fn every_reason_goes_with_one_state_and_none_is_lost() {
    for code in 0..=255u8 {
        if let Ok(reason) = SignatureReason::from_u8(code) {
            assert_eq!(reason as u8, code);
            assert_eq!(
                SignatureState::from_u8(reason.state() as u8).unwrap(),
                reason.state()
            );
            assert!(!reason.name().is_empty());
        }
    }
    // Only a trusted signature has the reason "verified", and every state has a reason.
    let mut seen = std::collections::HashSet::new();
    for code in 0..=255u8 {
        if let Ok(reason) = SignatureReason::from_u8(code) {
            seen.insert(reason.state() as u8);
            assert_eq!(
                reason == SignatureReason::Verified,
                reason.state() == SignatureState::Trusted
            );
        }
    }
    assert_eq!(seen.len(), 5);
}

#[test]
fn a_state_that_does_not_fit_its_reason_is_refused() {
    // A tick for a signature that is broken is what an engine must never be able to say.
    let mut bad = info(0);
    bad.state = SignatureState::Trusted;
    bad.reason = SignatureReason::SignatureInvalid;
    assert!(Response::decode(response(report(vec![bad]))).is_err());
    let mut bad = info(0);
    bad.state = SignatureState::Broken;
    bad.reason = SignatureReason::Verified;
    assert!(Response::decode(response(report(vec![bad]))).is_err());
}

#[test]
fn unknown_codes_are_refused() {
    let good = response(report(vec![info(0)]));
    // tag, id, total, unlisted, anchors, count, then the index: the state follows.
    let state = 1 + 4 + 4 + 4 + 4 + 4 + 4;
    for (offset, value) in [
        (state, 0u8),
        (state, 9),
        (state + 1, 99),
        (state + 2, 4),
        (state + 3, 4),
    ] {
        let mut bytes = good.clone();
        bytes[offset] = value;
        assert!(Response::decode(bytes).is_err(), "{offset} {value}");
    }
}

fn text_fields(item: &mut SignatureInfo, text: String) -> Vec<SignatureInfo> {
    let mut out = Vec::new();
    for field in 0..13 {
        let mut copy = item.clone();
        match field {
            0 => copy.sub_filter = text.clone(),
            1 => copy.signer = Some(text.clone()),
            2 => copy.organisation = Some(text.clone()),
            3 => copy.issuer = Some(text.clone()),
            4 => copy.serial = Some(text.clone()),
            5 => copy.fingerprint = Some(text.clone()),
            6 => copy.valid_from = Some(text.clone()),
            7 => copy.key = Some(text.clone()),
            8 => copy.claimed_time = Some(text.clone()),
            9 => copy.stated_reason = Some(text.clone()),
            10 => copy.stated_location = Some(text.clone()),
            11 => copy.trust_anchor = Some(text.clone()),
            _ => copy.digest = Some(text.clone()),
        }
        out.push(copy);
    }
    out
}

/// The cap of each text field, in the order of `text_fields`.
const CAPS: [usize; 13] = [
    MAX_SIG_SUBFILTER,
    MAX_SIG_NAME,
    MAX_SIG_NAME,
    MAX_SIG_NAME,
    MAX_SIG_SERIAL,
    64,
    MAX_SIG_SHORT,
    MAX_SIG_SHORT,
    MAX_SIG_SHORT,
    MAX_SIG_NOTE,
    MAX_SIG_NOTE,
    MAX_SIG_NAME,
    MAX_SIG_SHORT,
];

#[test]
fn every_text_is_held_to_its_cap() {
    let mut base = info(0);
    base.trust_anchor = Some("anchor".into());
    for (field, cap) in CAPS.iter().enumerate() {
        // At the cap: accepted, and it comes back as it was.
        let at = text_fields(&mut base, "a".repeat(*cap)).remove(field);
        let decoded = Response::decode(response(report(vec![at.clone()]))).unwrap();
        let ResponseBody::Signatures { report: got } = decoded.body else {
            panic!("not a report");
        };
        assert_eq!(got.items[0], at, "field {field} at its cap");
        // One over: the encoder cuts it, so the report still decodes (and is shorter) ...
        let over = text_fields(&mut base, "a".repeat(cap + 1)).remove(field);
        let ResponseBody::Signatures { report: cut } =
            Response::decode(response(report(vec![over.clone()])))
                .unwrap()
                .body
        else {
            panic!("not a report");
        };
        let shown = [
            &cut.items[0].sub_filter,
            cut.items[0].signer.as_ref().unwrap(),
            cut.items[0].organisation.as_ref().unwrap(),
            cut.items[0].issuer.as_ref().unwrap(),
            cut.items[0].serial.as_ref().unwrap(),
            cut.items[0].fingerprint.as_ref().unwrap(),
            cut.items[0].valid_from.as_ref().unwrap(),
            cut.items[0].key.as_ref().unwrap(),
            cut.items[0].claimed_time.as_ref().unwrap(),
            cut.items[0].stated_reason.as_ref().unwrap(),
            cut.items[0].stated_location.as_ref().unwrap(),
            cut.items[0].trust_anchor.as_ref().unwrap(),
            cut.items[0].digest.as_ref().unwrap(),
        ][field]
            .len();
        assert_eq!(shown, *cap, "field {field} cut to its cap");
    }
}

#[test]
fn a_text_longer_than_its_cap_is_refused_by_the_decoder() {
    // Built by hand: the sub-filter is the first text of an item.
    let good = response(report(vec![info(0)]));
    let at = 1 + 4 + 4 + 4 + 4 + 4 + 4 + 5;
    let old = u32::from_le_bytes(good[at..at + 4].try_into().unwrap()) as usize;
    let mut bytes = good[..at].to_vec();
    bytes.extend((MAX_SIG_SUBFILTER as u32 + 1).to_le_bytes());
    bytes.extend(vec![b'x'; MAX_SIG_SUBFILTER + 1]);
    bytes.extend(&good[at + 4 + old..]);
    assert!(matches!(
        Response::decode(bytes),
        Err(ProtocolError::StringTooLong(_))
    ));
}

#[test]
fn text_that_is_not_plain_is_refused() {
    for bad in [
        "line\nbreak",
        "tab\there",
        "nul\0here",
        "\u{202E}gnp.exe",
        "\u{2066}isolate",
        "zero\u{200B}width",
        "bom\u{FEFF}",
        "bell\u{7}",
        "next\u{85}line",
        "sep\u{2028}arator",
    ] {
        let mut item = info(0);
        item.signer = Some(bad.to_string());
        assert!(
            Response::decode(response(report(vec![item]))).is_err(),
            "{bad:?}"
        );
        let mut item = info(0);
        item.sub_filter = bad.to_string();
        assert!(
            Response::decode(response(report(vec![item]))).is_err(),
            "{bad:?}"
        );
    }
}

#[test]
fn plain_text_drops_what_misleads_and_cuts_on_a_character() {
    assert_eq!(plain_text("a\u{202E}b\u{0}c", 100), "abc");
    assert_eq!(plain_text("  one \n\t two  ", 100), "one two");
    assert_eq!(plain_text("\u{200B}\u{FEFF}", 100), "");
    // The cut text ends in an ellipsis and stays within the cap, also over a wide character.
    let cut = plain_text(&"å".repeat(100), 10);
    assert!(cut.len() <= 10 && cut.ends_with('\u{2026}'), "{cut:?}");
    assert!(is_plain_text(&cut));
    let cut = plain_text(&"\u{1F600}".repeat(50), 9);
    assert!(cut.len() <= 9 && cut.ends_with('\u{2026}'), "{cut:?}");
    assert_eq!(plain_text("short", 5), "short");
    assert_eq!(plain_text("shorty", 5), "sh\u{2026}");
    assert!(is_plain_text("Åse \u{1F600} 日本語 שלום"));
}

#[test]
fn the_number_of_signatures_is_held_to_its_cap() {
    // At the cap.
    let full: Vec<SignatureInfo> = (0..MAX_SIGNATURES as u32).map(info).collect();
    let mut cap_report = report(full);
    cap_report.total = 500;
    let ResponseBody::Signatures { report: got } =
        Response::decode(response(cap_report)).unwrap().body
    else {
        panic!("not a report");
    };
    assert_eq!(got.items.len(), MAX_SIGNATURES);
    assert_eq!(got.total, 500);
    // The largest report stays below the bound the app uses to refuse an answer.
    let mut biggest = info(0);
    biggest.byte_range = vec![u64::MAX; MAX_BYTE_RANGE];
    biggest.sub_filter = "s".repeat(MAX_SIG_SUBFILTER);
    biggest.signer = Some("n".repeat(MAX_SIG_NAME));
    biggest.organisation = Some("n".repeat(MAX_SIG_NAME));
    biggest.issuer = Some("n".repeat(MAX_SIG_NAME));
    biggest.trust_anchor = Some("n".repeat(MAX_SIG_NAME));
    biggest.serial = Some("0".repeat(MAX_SIG_SERIAL));
    biggest.fingerprint = Some("0".repeat(64));
    biggest.valid_from = Some("d".repeat(MAX_SIG_SHORT));
    biggest.valid_to = Some("d".repeat(MAX_SIG_SHORT));
    biggest.key = Some("k".repeat(MAX_SIG_SHORT));
    biggest.digest = Some("k".repeat(MAX_SIG_SHORT));
    biggest.claimed_time = Some("d".repeat(MAX_SIG_SHORT));
    biggest.stated_reason = Some("r".repeat(MAX_SIG_NOTE));
    biggest.stated_location = Some("r".repeat(MAX_SIG_NOTE));
    let worst = response(report(vec![biggest; MAX_SIGNATURES]));
    assert!(worst.len() <= MAX_SIGNATURE_REPORT, "{}", worst.len());
    assert!(Response::decode(worst).is_ok());

    // One more than the cap, claimed in the count: refused before anything is read.
    let good = response(report(vec![info(0)]));
    let count_at = 1 + 4 + 4 + 4 + 4;
    let mut bytes = good.clone();
    bytes[count_at..count_at + 4].copy_from_slice(&(MAX_SIGNATURES as u32 + 1).to_le_bytes());
    assert!(matches!(
        Response::decode(bytes),
        Err(ProtocolError::InvalidLength)
    ));
    // A count that does not fit in the frame, or exceeds the total.
    let mut bytes = good.clone();
    bytes[count_at..count_at + 4].copy_from_slice(&(MAX_SIGNATURES as u32).to_le_bytes());
    assert!(Response::decode(bytes).is_err());
    let mut bytes = good.clone();
    bytes[count_at..count_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(Response::decode(bytes).is_err());
    let mut bytes = good;
    // total 0, one item
    bytes[5..9].copy_from_slice(&0u32.to_le_bytes());
    assert!(Response::decode(bytes).is_err());
}

#[test]
fn the_byte_range_is_held_to_its_cap() {
    let mut item = info(0);
    item.byte_range = vec![7; MAX_BYTE_RANGE];
    assert!(Response::decode(response(report(vec![item.clone()]))).is_ok());
    // The encoder keeps the first numbers only.
    item.byte_range = vec![7; MAX_BYTE_RANGE + 5];
    let ResponseBody::Signatures { report: got } =
        Response::decode(response(report(vec![item]))).unwrap().body
    else {
        panic!("not a report");
    };
    assert_eq!(got.items[0].byte_range.len(), MAX_BYTE_RANGE);
    // A count of nine, written by hand, is refused.
    let good = response(report(vec![info(0)]));
    let at = 1 + 4 + 4 + 4 + 4 + 4 + 4 + 5 + 4 + "adbe.pkcs7.detached".len();
    let mut bytes = good;
    bytes[at] = MAX_BYTE_RANGE as u8 + 1;
    assert!(Response::decode(bytes).is_err());
    // The document protection level is 0 to 3.
    let mut item = info(0);
    item.doc_mdp = 4;
    assert!(Response::decode(response(report(vec![item]))).is_err());
}

#[test]
fn a_report_cut_short_anywhere_is_refused_and_extra_bytes_too() {
    let good = response(report(vec![info(0), info(1)]));
    for cut in 0..good.len() {
        assert!(Response::decode(good[..cut].to_vec()).is_err(), "{cut}");
    }
    let mut more = good;
    more.push(0);
    assert!(matches!(
        Response::decode(more),
        Err(ProtocolError::TrailingBytes(1))
    ));
}

#[test]
fn a_signature_the_reader_could_not_read_is_not_called_broken() {
    // "Broken" is for a signature that was read and found false (or a byte range that does not
    // hold). A structure the parser refuses says nothing for or against the signature.
    assert_eq!(
        SignatureReason::Unreadable.state(),
        SignatureState::Unsupported
    );
    let mut unread = info(0);
    unread.state = SignatureState::Unsupported;
    unread.reason = SignatureReason::Unreadable;
    assert!(Response::decode(response(report(vec![unread]))).is_ok());
    let mut broken = info(0);
    broken.state = SignatureState::Broken;
    broken.reason = SignatureReason::Unreadable;
    assert!(Response::decode(response(report(vec![broken]))).is_err());
}

#[test]
fn marks_that_show_nothing_are_not_plain_text() {
    for hidden in [
        '\u{00AD}',
        '\u{034F}',
        '\u{115F}',
        '\u{1160}',
        '\u{17B4}',
        '\u{17B5}',
        '\u{180B}',
        '\u{180F}',
        '\u{3164}',
        '\u{FE00}',
        '\u{FE0F}',
        '\u{E0001}',
        '\u{E0020}',
        '\u{E0FFF}',
    ] {
        let text = format!("Bank{hidden}Name");
        assert!(!is_plain_text(&text), "U+{:04X}", hidden as u32);
        assert_eq!(plain_text(&text, 64), "BankName", "U+{:04X}", hidden as u32);
    }
    // Ordinary letters, also from other scripts, stay.
    assert!(is_plain_text("Åse Müller 山田 العربية"));
}
