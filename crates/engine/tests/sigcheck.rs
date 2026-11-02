//! The check of one signature against the bytes of a file, without PDFium: every state the panel
//! can show, every reason, and the attacks the check exists for. Keys, certificates and signatures
//! are made in the tests (see `partridge_testkit::signed`).

use partridge_engine::sigcheck::TrustList;
use partridge_engine::signatures::{check, Raw, Settings, Work, HASH_BUDGET};
use partridge_protocol::{
    CertificateTime, SignatureInfo, SignatureReason as R, SignatureState as S,
};
use partridge_testkit::signed::*;

fn unhex(text: &[u8]) -> Vec<u8> {
    text.chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

/// What PDFium would report for a file made by the testkit, with the range it was signed with.
fn raw_of(file: &SignedFile, options: &SignOptions) -> Raw {
    Raw {
        contents: unhex(&file.bytes[file.contents_at + 1..file.contents_end - 1]),
        byte_range: file.range.iter().map(|v| *v as i64).collect(),
        sub_filter: options.sub_filter.clone(),
        reason: options.reason.clone(),
        time: options.time.clone(),
        doc_mdp: options.doc_mdp.unwrap_or(0),
    }
}

fn run_at(bytes: &[u8], raw: &Raw, trust: &TrustList, now: i64) -> SignatureInfo {
    let mut progress = |_: u32| {};
    let mut work = Work::new(HASH_BUDGET, &mut progress);
    let mut source: &[u8] = bytes;
    check(
        0,
        raw,
        &mut source,
        bytes.len() as u64,
        &Settings { trust, now },
        &mut work,
    )
    .unwrap()
}

fn run(file: &SignedFile, options: &SignOptions) -> SignatureInfo {
    run_at(
        &file.bytes,
        &raw_of(file, options),
        &TrustList::empty(),
        NOW,
    )
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

fn p256(name: &str) -> Identity {
    Identity::self_signed(KeyKind::P256, &CertSpec::new(name))
}

fn sign(signer: &Identity, options: &SignOptions) -> SignedFile {
    signed_file(signer, options)
}

fn trust_of(identities: &[&Identity]) -> TrustList {
    let certs: Vec<Vec<u8>> = identities.iter().map(|i| i.cert.clone()).collect();
    TrustList::from_der(&certs).unwrap()
}

// ---- a signature that holds ----------------------------------------------------------------------

#[test]
fn a_signature_that_holds_is_intact_but_the_signer_is_not_known_without_a_trust_list() {
    let signer = p256("Test Signer");
    let options = SignOptions::default();
    let info = run(&sign(&signer, &options), &options);
    expect(&info, S::UnknownSigner, R::NoTrustList);
    assert_eq!(info.certificate_time, CertificateTime::Valid);
    assert!(info.trust_anchor.is_none());
}

#[test]
fn what_the_certificate_and_the_file_say_is_reported_as_text() {
    let signer = p256("Test Signer");
    let options = SignOptions {
        reason: Some("I approve this document".into()),
        location: Some("Oslo (Norway)".into()),
        time: Some("D:20260615120000+02'00'".into()),
        doc_mdp: Some(2),
        ..SignOptions::default()
    };
    let file = sign(&signer, &options);
    let info = run(&file, &options);
    assert_eq!(info.signer.as_deref(), Some("Test Signer"));
    assert_eq!(info.organisation.as_deref(), Some("Test Organisation"));
    assert_eq!(info.issuer.as_deref(), Some("Test Signer"));
    assert_eq!(info.serial.as_deref(), Some("12345678"));
    assert_eq!(info.valid_from.as_deref(), Some("2025-01-01 00:00 UTC"));
    assert_eq!(info.valid_to.as_deref(), Some("2035-01-01 00:00 UTC"));
    assert_eq!(info.key.as_deref(), Some("ECDSA P-256"));
    assert_eq!(info.digest.as_deref(), Some("SHA-256"));
    assert_eq!(
        info.claimed_time.as_deref(),
        Some("2026-06-15 12:00:00 +02:00")
    );
    assert_eq!(
        info.stated_reason.as_deref(),
        Some("I approve this document")
    );
    assert_eq!(info.stated_location.as_deref(), Some("Oslo (Norway)"));
    assert_eq!(info.sub_filter, "adbe.pkcs7.detached");
    assert_eq!(info.doc_mdp, 2);
    assert_eq!(info.certificates, 1);
    assert_eq!(
        info.byte_range,
        file.range.iter().map(|v| *v as u64).collect::<Vec<_>>()
    );
    assert_eq!(info.file_len, file.bytes.len() as u64);
    // The fingerprint is the SHA-256 of the certificate.
    let expected: String = Hash::Sha256
        .of(&signer.cert)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(info.fingerprint.as_deref(), Some(expected.as_str()));
}

#[test]
fn every_supported_kind_of_key_and_digest_is_checked() {
    let rsa = shared_rsa();
    let p384 = Identity::self_signed(KeyKind::P384, &CertSpec::new("P384 Signer"));
    let ed = Identity::self_signed(KeyKind::Ed25519, &CertSpec::new("Ed Signer"));
    let cases: Vec<(&Identity, Hash, bool, &str)> = vec![
        (rsa, Hash::Sha256, false, "RSA 2048 bit"),
        (rsa, Hash::Sha256, true, "RSA 2048 bit"),
        (rsa, Hash::Sha384, false, "RSA 2048 bit"),
        (rsa, Hash::Sha512, true, "RSA 2048 bit"),
        (&p384, Hash::Sha384, true, "ECDSA P-384"),
        (&p384, Hash::Sha384, false, "ECDSA P-384"),
        (&ed, Hash::Sha512, false, "Ed25519"),
        (&ed, Hash::Sha256, false, "Ed25519"),
    ];
    for (signer, hash, combined, key) in cases {
        let options = SignOptions {
            cms: CmsOptions {
                hash,
                combined_algorithm: combined,
                ..CmsOptions::default()
            },
            ..SignOptions::default()
        };
        let info = run(&sign(signer, &options), &options);
        expect(&info, S::UnknownSigner, R::NoTrustList);
        assert_eq!(info.key.as_deref(), Some(key));
    }
}

#[test]
fn the_cades_form_and_the_binding_to_the_certificate_are_checked() {
    let signer = p256("Cades Signer");
    let other = p256("Someone Else");
    // The attribute that names the signer's certificate.
    for sub_filter in ["ETSI.CAdES.detached", "adbe.pkcs7.detached"] {
        let options = SignOptions {
            sub_filter: sub_filter.into(),
            cms: CmsOptions {
                signing_certificate: true,
                ..CmsOptions::default()
            },
            ..SignOptions::default()
        };
        expect(
            &run(&sign(&signer, &options), &options),
            S::UnknownSigner,
            R::NoTrustList,
        );
    }
    // ... naming another certificate: the signature was made for a different signer.
    let options = SignOptions {
        cms: CmsOptions {
            signing_certificate: true,
            signing_certificate_of: Some(other.cert.clone()),
            ..CmsOptions::default()
        },
        ..SignOptions::default()
    };
    expect(
        &run(&sign(&signer, &options), &options),
        S::Broken,
        R::CertificateMismatch,
    );
}

#[test]
fn a_signer_named_by_key_identifier_is_found() {
    let signer = Identity::self_signed(KeyKind::P256, &CertSpec::new("Ski Signer"));
    let options = SignOptions {
        cms: CmsOptions {
            by_key_identifier: true,
            ..CmsOptions::default()
        },
        ..SignOptions::default()
    };
    expect(
        &run(&sign(&signer, &options), &options),
        S::UnknownSigner,
        R::NoTrustList,
    );
}

// ---- the file ----------------------------------------------------------------------------------

#[test]
fn bytes_after_the_signed_part_are_a_change_after_signing() {
    let signer = p256("Test Signer");
    let options = SignOptions {
        leave_out: 6,
        ..SignOptions::default()
    };
    let info = run(&sign(&signer, &options), &options);
    expect(&info, S::Changed, R::DataAfterSignature);
    // The signer is still told, as text: it is the file that changed.
    assert_eq!(info.signer.as_deref(), Some("Test Signer"));
}

#[test]
fn an_incremental_save_after_signing_is_a_change() {
    let signer = p256("Test Signer");
    let options = SignOptions::default();
    let file = sign(&signer, &options);
    let raw = raw_of(&file, &options);
    let edited = change_text_revision(&file.bytes, "Pay 900 to Mallory");
    let info = run_at(&edited, &raw, &TrustList::empty(), NOW);
    expect(&info, S::Changed, R::DataAfterSignature);
}

#[test]
fn a_change_inside_the_signed_part_is_a_change() {
    let signer = p256("Test Signer");
    let options = SignOptions::default();
    let mut file = sign(&signer, &options);
    replace_in_place(&mut file.bytes, b"Pay 100 to", b"Pay 900 to");
    expect(&run(&file, &options), S::Changed, R::ContentChanged);
}

#[test]
fn a_change_after_signing_is_not_hidden_by_a_trusted_signer() {
    // Even when the signer is on the list, a changed file is not shown as trusted.
    let signer = p256("Test Signer");
    let options = SignOptions::default();
    let mut file = sign(&signer, &options);
    replace_in_place(&mut file.bytes, b"Pay 100 to", b"Pay 900 to");
    let info = run_at(
        &file.bytes,
        &raw_of(&file, &options),
        &trust_of(&[&signer]),
        NOW,
    );
    expect(&info, S::Changed, R::ContentChanged);
    let tail = SignOptions {
        leave_out: 6,
        ..SignOptions::default()
    };
    let file = sign(&signer, &tail);
    let info = run_at(
        &file.bytes,
        &raw_of(&file, &tail),
        &trust_of(&[&signer]),
        NOW,
    );
    expect(&info, S::Changed, R::DataAfterSignature);
}

#[test]
fn a_signature_cut_or_damaged_does_not_hold() {
    let signer = p256("Test Signer");
    let options = SignOptions {
        cms: CmsOptions {
            damage_signature: true,
            ..CmsOptions::default()
        },
        ..SignOptions::default()
    };
    expect(
        &run(&sign(&signer, &options), &options),
        S::Broken,
        R::SignatureInvalid,
    );
}

#[test]
fn a_signature_by_another_key_than_the_certificate_does_not_hold() {
    // The attacker's certificate has the name and the serial number of the real one, and
    // another key: the signature was made with that key.
    let real = p256("Bank Signer");
    let fake = p256("Bank Signer");
    let options = SignOptions {
        cms: CmsOptions {
            without_signer_cert: true,
            extra_certs: vec![real.cert.clone()],
            ..CmsOptions::default()
        },
        ..SignOptions::default()
    };
    // Signed with the fake key, certificate of the real one.
    let file = sign(&fake, &options);
    expect(&run(&file, &options), S::Broken, R::SignatureInvalid);
}

#[test]
fn a_digest_of_other_data_is_a_change_not_a_pass() {
    let signer = p256("Test Signer");
    let options = SignOptions {
        cms: CmsOptions {
            digest_override: Some(Hash::Sha256.of(b"something else")),
            ..CmsOptions::default()
        },
        ..SignOptions::default()
    };
    expect(
        &run(&sign(&signer, &options), &options),
        S::Changed,
        R::ContentChanged,
    );
}

#[test]
fn malformed_signed_attributes_do_not_hold() {
    let signer = p256("Test Signer");
    for (cms, reason) in [
        (
            CmsOptions {
                without_content_type: true,
                ..CmsOptions::default()
            },
            R::AttributesInvalid,
        ),
        // Two digests that disagree, the right one last in the set: a reader that takes the last
        // one would call this intact.
        (
            CmsOptions {
                digest_conflict: true,
                ..CmsOptions::default()
            },
            R::AttributesInvalid,
        ),
        // A content type that is not id-data.
        (
            CmsOptions {
                content_type_value: Some("1.2.840.113549.1.7.2".into()),
                ..CmsOptions::default()
            },
            R::AttributesInvalid,
        ),
        // The binding to the certificate with two values, the right one first.
        (
            CmsOptions {
                binding_two_values: true,
                ..CmsOptions::default()
            },
            R::CertificateMismatch,
        ),
        (
            CmsOptions {
                embed_content: true,
                ..CmsOptions::default()
            },
            R::AttributesInvalid,
        ),
        (
            CmsOptions {
                without_signer_cert: true,
                ..CmsOptions::default()
            },
            R::NoSignerCertificate,
        ),
    ] {
        let options = SignOptions {
            cms,
            ..SignOptions::default()
        };
        expect(&run(&sign(&signer, &options), &options), S::Broken, reason);
    }
}

#[test]
fn data_hidden_in_the_padding_of_the_signature_is_refused() {
    let signer = p256("Test Signer");
    let options = SignOptions::default();
    let mut file = sign(&signer, &options);
    // One bit in the last byte of the space made for the signature. It is outside the signed
    // bytes, so a check of the signature alone would not see it.
    let last = file.contents_end - 2;
    file.bytes[last] = b'1';
    let raw = raw_of(&file, &options);
    expect(
        &run_at(&file.bytes, &raw, &TrustList::empty(), NOW),
        S::Broken,
        R::ContentsPadding,
    );
}

// ---- the byte range -------------------------------------------------------------------------------

fn ranged(file: &SignedFile, options: &SignOptions, range: [u64; 4]) -> SignatureInfo {
    let mut bytes = file.bytes.clone();
    set_byte_range(&mut bytes, range);
    let mut raw = raw_of(file, options);
    raw.byte_range = range.iter().map(|v| *v as i64).collect();
    run_at(&bytes, &raw, &TrustList::empty(), NOW)
}

#[test]
fn a_byte_range_that_does_not_describe_the_file_is_broken() {
    let signer = p256("Test Signer");
    let options = SignOptions::default();
    let file = sign(&signer, &options);
    let [a, b, c, d] = file.range.map(|v| v as u64);
    let len = file.bytes.len() as u64;
    // Not from the start of the file.
    expect(
        &ranged(&file, &options, [10, b, c, d]),
        S::Broken,
        R::RangeInvalid,
    );
    // Past the end of the file.
    expect(
        &ranged(&file, &options, [a, b, c, d + 1]),
        S::Broken,
        R::RangeInvalid,
    );
    expect(
        &ranged(&file, &options, [a, b, len, 1]),
        S::Broken,
        R::RangeInvalid,
    );
    // Overlapping.
    expect(
        &ranged(&file, &options, [a, b + 5, c, d]),
        S::Broken,
        R::RangeLeavesGap,
    );
    expect(
        &ranged(&file, &options, [a, c + 1, c, d]),
        S::Broken,
        R::RangeInvalid,
    );
    // A part that is not a number the file can have.
    expect(
        &ranged(&file, &options, [a, b, 9_999_999_999, d]),
        S::Broken,
        R::RangeInvalid,
    );
    // Not four numbers.
    let mut raw = raw_of(&file, &options);
    raw.byte_range = vec![0, b as i64, c as i64];
    expect(
        &run_at(&file.bytes, &raw, &TrustList::empty(), NOW),
        S::Broken,
        R::RangeInvalid,
    );
    raw.byte_range = vec![];
    expect(
        &run_at(&file.bytes, &raw, &TrustList::empty(), NOW),
        S::Broken,
        R::RangeInvalid,
    );
    raw.byte_range = vec![0, -5, c as i64, d as i64];
    expect(
        &run_at(&file.bytes, &raw, &TrustList::empty(), NOW),
        S::Broken,
        R::RangeInvalid,
    );
}

#[test]
fn a_byte_range_that_leaves_out_more_than_the_signature_is_broken() {
    let signer = p256("Test Signer");
    let options = SignOptions::default();
    let file = sign(&signer, &options);
    let [a, b, c, d] = file.range.map(|v| v as u64);
    // A hole after the signature: the bytes between are in neither part.
    expect(
        &ranged(&file, &options, [a, b, c + 20, d - 20]),
        S::Broken,
        R::RangeLeavesGap,
    );
    // A hole before it.
    expect(
        &ranged(&file, &options, [a, b - 20, c, d]),
        S::Broken,
        R::RangeLeavesGap,
    );
    // The range leaves out nothing: no space for the signature at all.
    expect(
        &ranged(&file, &options, [a, c, c, d]),
        S::Broken,
        R::RangeLeavesGap,
    );
    // The left out part is the signature but is not a string: it starts one byte late.
    expect(
        &ranged(&file, &options, [a, b + 1, c, d]),
        S::Broken,
        R::RangeLeavesGap,
    );
}

/// The signature wrapping attack: the space left out of the byte range is made bigger, and the
/// attacker's bytes are put in it. The two signed parts are the same bytes as before, so the
/// signature still holds for them; only the check that the left out part is the signature and
/// nothing else sees what was done.
#[test]
fn signature_wrapping_through_a_bigger_gap_is_refused() {
    let signer = p256("Test Signer");
    let options = SignOptions::default();
    let file = sign(&signer, &options);
    let [a, b, c, d] = file.range;
    let evil: &[u8] = b"\n6 0 obj\n<< /Length 33 >>\nstream\nBT /F1 24 Tf 72 700 Td (Pay 900 to Mallory) Tj ET\nendstream\nendobj\n";
    // The signed parts, unchanged, with the attacker's object between the signature and the rest.
    let mut bytes = file.bytes[..c].to_vec();
    bytes.extend_from_slice(evil);
    bytes.extend_from_slice(&file.bytes[c..]);
    let new_c = c + evil.len();
    // The attack works on a naive check: the two parts, as the attacker's range gives them, are
    // the same bytes as the signer signed, so the digest and the signature are right. (A real
    // attack puts a second dictionary with this range in the left out part; the reader then
    // reports that range.)
    let with_range = bytes.clone();
    let mut raw = raw_of(&file, &options);
    raw.byte_range = vec![a as i64, b as i64, new_c as i64, d as i64];
    let info = run_at(&with_range, &raw, &TrustList::empty(), NOW);
    expect(&info, S::Broken, R::RangeLeavesGap);
    // ... also when the signature is in the file's own trust list.
    let info = run_at(&with_range, &raw, &trust_of(&[&signer]), NOW);
    expect(&info, S::Broken, R::RangeLeavesGap);
}

#[test]
fn a_signature_that_is_not_the_one_in_the_file_is_refused() {
    // The reader lists one signature; the bytes the range leaves out are another. What is checked
    // must be what is listed.
    let signer = p256("Test Signer");
    let options = SignOptions::default();
    let file = sign(&signer, &options);
    let mut raw = raw_of(&file, &options);
    raw.contents[10] ^= 1;
    expect(
        &run_at(&file.bytes, &raw, &TrustList::empty(), NOW),
        S::Broken,
        R::ContentsMismatch,
    );
    raw.contents.truncate(100);
    expect(
        &run_at(&file.bytes, &raw, &TrustList::empty(), NOW),
        S::Broken,
        R::ContentsMismatch,
    );
    raw.contents.clear();
    expect(
        &run_at(&file.bytes, &raw, &TrustList::empty(), NOW),
        S::Broken,
        R::ContentsMismatch,
    );
}

// ---- forms that are not supported -----------------------------------------------------------------

#[test]
fn forms_the_reader_does_not_check_are_unsupported_not_broken() {
    let signer = p256("Test Signer");
    for (sub_filter, reason) in [
        ("adbe.pkcs7.sha1", R::SubFilterUnsupported),
        ("adbe.x509.rsa_sha1", R::SubFilterUnsupported),
        ("Something.else", R::SubFilterUnsupported),
        ("", R::SubFilterUnsupported),
        ("ETSI.RFC3161", R::DocumentTimestamp),
    ] {
        let options = SignOptions {
            sub_filter: sub_filter.into(),
            ..SignOptions::default()
        };
        let info = run(&sign(&signer, &options), &options);
        expect(&info, S::Unsupported, reason);
    }
}

#[test]
fn algorithms_and_encodings_the_reader_does_not_check_are_unsupported() {
    let signer = p256("Test Signer");
    let cases: Vec<(CmsOptions, R)> = vec![
        // SHA-1 is not safe.
        (
            CmsOptions {
                hash: Hash::Sha1,
                ..CmsOptions::default()
            },
            R::DigestUnsupported,
        ),
        (
            CmsOptions {
                without_attributes: true,
                ..CmsOptions::default()
            },
            R::NoSignedAttributes,
        ),
        (
            CmsOptions {
                indefinite: true,
                ..CmsOptions::default()
            },
            R::EncodingUnsupported,
        ),
        (
            CmsOptions {
                two_signers: true,
                ..CmsOptions::default()
            },
            R::SubFilterUnsupported,
        ),
    ];
    for (cms, reason) in cases {
        let options = SignOptions {
            cms,
            ..SignOptions::default()
        };
        let info = run(&sign(&signer, &options), &options);
        expect(&info, S::Unsupported, reason);
    }
    // A curve with a digest the reader has no check for.
    let p384 = Identity::self_signed(KeyKind::P384, &CertSpec::new("P384 Signer"));
    let options = SignOptions {
        cms: CmsOptions {
            hash: Hash::Sha512,
            combined_algorithm: true,
            ..CmsOptions::default()
        },
        ..SignOptions::default()
    };
    expect(
        &run(&sign(&p384, &options), &options),
        S::Unsupported,
        R::AlgorithmUnsupported,
    );
}

#[test]
fn a_key_that_is_too_short_is_unsupported() {
    // A certificate with a 1024 bit RSA key. The signature is by another key: the reader refuses
    // to go on at the key.
    let issuer = p256("Short Key Signer");
    let spec = CertSpec::new("Short Key Signer");
    let cert = certificate(
        &spec,
        &issuer.name,
        &issuer.name,
        &rsa_public_spki(1024),
        &issuer.key,
    );
    let signer = Identity {
        key: Key::generate(KeyKind::P256),
        cert,
        name: issuer.name.clone(),
        issuer: issuer.name.clone(),
        serial: spec.serial,
    };
    let options = SignOptions::default();
    let info = run(&sign(&signer, &options), &options);
    expect(&info, S::Unsupported, R::KeyTooSmall);
    assert_eq!(info.key.as_deref(), Some("RSA 1024 bit"));
}

#[test]
fn data_that_is_not_a_signature_is_broken_or_unsupported_and_never_a_panic() {
    let signer = p256("Test Signer");
    let options = SignOptions::default();
    let file = sign(&signer, &options);
    let good = raw_of(&file, &options);
    let length = partridge_engine::sigcheck::der_element_len(&good.contents).unwrap();
    // Cuts, and bytes changed one at a time across the whole signature: nothing may panic and
    // nothing may come out as a signature that holds.
    let mut states = std::collections::HashMap::new();
    for cut in (0..length).step_by(7) {
        let mut bytes = file.bytes.clone();
        let mut raw = good.clone();
        raw.contents.iter_mut().skip(cut).for_each(|b| *b = 0);
        let hex: String = raw.contents.iter().map(|b| format!("{b:02x}")).collect();
        bytes[file.contents_at + 1..file.contents_end - 1].copy_from_slice(hex.as_bytes());
        let info = run_at(&bytes, &raw, &TrustList::empty(), NOW);
        assert!(
            !matches!(info.state, S::Trusted | S::UnknownSigner),
            "cut {cut}"
        );
        *states.entry(info.state as u8).or_insert(0) += 1;
    }
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..300 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let place = (seed as usize) % length;
        let mut bytes = file.bytes.clone();
        let mut raw = good.clone();
        raw.contents[place] ^= 1 << (seed >> 40 & 7);
        let hex: String = raw.contents.iter().map(|b| format!("{b:02x}")).collect();
        bytes[file.contents_at + 1..file.contents_end - 1].copy_from_slice(hex.as_bytes());
        let info = run_at(&bytes, &raw, &TrustList::empty(), NOW);
        // A changed byte may be in a part that is not signed (the certificate's validity says the
        // same either way, say, or the order of a set); all that matters is that nothing panics
        // and that a signature which does hold was not made worse in its claims.
        if matches!(info.state, S::UnknownSigner) {
            assert_eq!(info.reason, R::NoTrustList);
        }
    }
    // Contents that are not DER at all.
    for junk in [
        vec![0u8; 100],
        vec![0x30, 0x80, 0, 0],
        vec![0xff; 50],
        vec![0x30, 0x03, 1, 2, 3],
        vec![0x30, 0x00],
    ] {
        let mut raw = good.clone();
        raw.contents = junk.clone();
        let mut bytes = file.bytes.clone();
        // Keep the file's own gap the same size.
        let hex: String = junk.iter().map(|b| format!("{b:02x}")).collect();
        let at = file.contents_at + 1;
        bytes[at..at + hex.len()].copy_from_slice(hex.as_bytes());
        for b in &mut bytes[at + hex.len()..file.contents_end - 1] {
            *b = b'0';
        }
        raw.contents.resize((file.contents_end - 1 - at) / 2, 0);
        let info = run_at(&bytes, &raw, &TrustList::empty(), NOW);
        assert!(
            matches!(info.state, S::Broken | S::Unsupported),
            "{:?} {:?}",
            info.state,
            info.reason
        );
    }
}

#[test]
fn limits_of_size_and_work_are_kept() {
    let signer = p256("Test Signer");
    let options = SignOptions::default();
    let file = sign(&signer, &options);
    let raw = raw_of(&file, &options);
    // Less budget than the signed part is long.
    let mut progress = |_: u32| {};
    let mut work = Work::new(100, &mut progress);
    let mut source: &[u8] = &file.bytes;
    let info = check(
        0,
        &raw,
        &mut source,
        file.bytes.len() as u64,
        &Settings {
            trust: &TrustList::empty(),
            now: NOW,
        },
        &mut work,
    )
    .unwrap();
    expect(&info, S::Unsupported, R::TooLarge);
    // A file that ends before the range says: the read fails, which is an error, not a verdict.
    let mut source: &[u8] = &file.bytes[..file.bytes.len() - 10];
    let mut progress = |_: u32| {};
    let mut work = Work::new(HASH_BUDGET, &mut progress);
    let short = check(
        0,
        &raw,
        &mut source,
        file.bytes.len() as u64,
        &Settings {
            trust: &TrustList::empty(),
            now: NOW,
        },
        &mut work,
    );
    assert!(short.is_err());
    // A big file reports progress while it is hashed.
    let big = SignOptions {
        text: "x".repeat(5 * 1024 * 1024),
        ..SignOptions::default()
    };
    let file = sign(&signer, &big);
    let raw = raw_of(&file, &big);
    let mut ticks = Vec::new();
    let mut progress = |n: u32| ticks.push(n);
    let mut work = Work::new(HASH_BUDGET, &mut progress);
    let mut source: &[u8] = &file.bytes;
    let info = check(
        0,
        &raw,
        &mut source,
        file.bytes.len() as u64,
        &Settings {
            trust: &TrustList::empty(),
            now: NOW,
        },
        &mut work,
    )
    .unwrap();
    expect(&info, S::UnknownSigner, R::NoTrustList);
    assert!(ticks.len() >= 4, "{ticks:?}");
    assert!(ticks.windows(2).all(|w| w[0] < w[1]));
}

// ---- the signer ---------------------------------------------------------------------------------

#[test]
fn a_signer_on_the_trust_list_is_trusted() {
    let signer = p256("Listed Signer");
    let options = SignOptions::default();
    let file = sign(&signer, &options);
    let info = run_at(
        &file.bytes,
        &raw_of(&file, &options),
        &trust_of(&[&signer]),
        NOW,
    );
    expect(&info, S::Trusted, R::Verified);
    assert_eq!(info.trust_anchor.as_deref(), Some("Listed Signer"));
}

#[test]
fn a_chain_to_a_root_on_the_list_is_followed() {
    let root = Identity::self_signed(KeyKind::P256, &CertSpec::ca("Root CA"));
    let inter = Identity::issued_by(
        &root,
        KeyKind::P256,
        &CertSpec {
            serial: 2,
            ..CertSpec::ca("Intermediate CA")
        },
    );
    let leaf = Identity::issued_by(
        &inter,
        KeyKind::P256,
        &CertSpec {
            serial: 3,
            ..CertSpec::new("Leaf Signer")
        },
    );
    let with_chain = SignOptions {
        cms: CmsOptions {
            extra_certs: vec![inter.cert.clone()],
            ..CmsOptions::default()
        },
        ..SignOptions::default()
    };
    let file = sign(&leaf, &with_chain);
    let raw = raw_of(&file, &with_chain);
    let trust = trust_of(&[&root]);
    let info = run_at(&file.bytes, &raw, &trust, NOW);
    expect(&info, S::Trusted, R::Verified);
    assert_eq!(info.trust_anchor.as_deref(), Some("Root CA"));
    assert_eq!(info.signer.as_deref(), Some("Leaf Signer"));
    assert_eq!(info.issuer.as_deref(), Some("Intermediate CA"));
    assert_eq!(info.certificates, 2);

    // The intermediate certificate on the list instead of the root also works.
    expect(
        &run_at(&file.bytes, &raw, &trust_of(&[&inter]), NOW),
        S::Trusted,
        R::Verified,
    );

    // Without the intermediate certificate in the signature the chain is broken, and nothing is
    // fetched to mend it.
    let without = SignOptions::default();
    let file = sign(&leaf, &without);
    let raw = raw_of(&file, &without);
    expect(
        &run_at(&file.bytes, &raw, &trust, NOW),
        S::UnknownSigner,
        R::NotInTrustList,
    );
}

#[test]
fn a_look_alike_of_a_listed_root_is_not_trusted() {
    // Same name, other key: the name is not what is trusted.
    let root = Identity::self_signed(KeyKind::P256, &CertSpec::ca("Root CA"));
    let fake_root = Identity::self_signed(KeyKind::P256, &CertSpec::ca("Root CA"));
    let leaf = Identity::issued_by(
        &fake_root,
        KeyKind::P256,
        &CertSpec {
            serial: 3,
            ..CertSpec::new("Leaf Signer")
        },
    );
    let options = SignOptions::default();
    let file = sign(&leaf, &options);
    let raw = raw_of(&file, &options);
    expect(
        &run_at(&file.bytes, &raw, &trust_of(&[&root]), NOW),
        S::UnknownSigner,
        R::NotInTrustList,
    );
    // A signer with the name and serial number of a listed one, and another key.
    let imposter = p256("Listed Signer");
    let listed = p256("Listed Signer");
    let file = sign(&imposter, &options);
    let raw = raw_of(&file, &options);
    expect(
        &run_at(&file.bytes, &raw, &trust_of(&[&listed]), NOW),
        S::UnknownSigner,
        R::NotInTrustList,
    );
}

#[test]
fn a_certificate_that_is_no_authority_does_not_lead_anywhere() {
    let root = Identity::self_signed(KeyKind::P256, &CertSpec::ca("Root CA"));
    // An intermediate certificate that does not say it is a CA.
    let inter = Identity::issued_by(
        &root,
        KeyKind::P256,
        &CertSpec {
            serial: 2,
            ..CertSpec::new("Not A CA")
        },
    );
    let leaf = Identity::issued_by(
        &inter,
        KeyKind::P256,
        &CertSpec {
            serial: 3,
            ..CertSpec::new("Leaf Signer")
        },
    );
    let options = SignOptions {
        cms: CmsOptions {
            extra_certs: vec![inter.cert.clone()],
            ..CmsOptions::default()
        },
        ..SignOptions::default()
    };
    let file = sign(&leaf, &options);
    let raw = raw_of(&file, &options);
    expect(
        &run_at(&file.bytes, &raw, &trust_of(&[&root]), NOW),
        S::UnknownSigner,
        R::NotInTrustList,
    );
}

#[test]
fn the_clock_and_the_use_of_the_certificate_count() {
    let root = Identity::self_signed(KeyKind::P256, &CertSpec::ca("Root CA"));
    let leaf = Identity::issued_by(
        &root,
        KeyKind::P256,
        &CertSpec {
            serial: 3,
            ..CertSpec::new("Leaf Signer")
        },
    );
    let options = SignOptions::default();
    let file = sign(&leaf, &options);
    let raw = raw_of(&file, &options);
    let trust = trust_of(&[&root]);
    expect(
        &run_at(&file.bytes, &raw, &trust, NOW),
        S::Trusted,
        R::Verified,
    );
    // After the end of the validity of the certificates, and before its start.
    let late = run_at(&file.bytes, &raw, &trust, 2_100_000_000);
    expect(&late, S::UnknownSigner, R::CertificateExpired);
    assert_eq!(late.certificate_time, CertificateTime::Expired);
    let early = run_at(&file.bytes, &raw, &trust, 1_600_000_000);
    expect(&early, S::UnknownSigner, R::CertificateNotYetValid);
    assert_eq!(early.certificate_time, CertificateTime::NotYetValid);

    // A certificate that may only sign other certificates does not make a trusted signer.
    let key_cert_only = Identity::issued_by(
        &root,
        KeyKind::P256,
        &CertSpec {
            serial: 4,
            key_usage: Some(1 << 5),
            ..CertSpec::new("Cert Signing Only")
        },
    );
    let file = sign(&key_cert_only, &options);
    let raw = raw_of(&file, &options);
    expect(
        &run_at(&file.bytes, &raw, &trust, NOW),
        S::UnknownSigner,
        R::KeyUsage,
    );
    // A certificate that names no use may be used for anything.
    let no_usage = Identity::issued_by(
        &root,
        KeyKind::P256,
        &CertSpec {
            serial: 5,
            key_usage: None,
            ..CertSpec::new("No Usage")
        },
    );
    let file = sign(&no_usage, &options);
    let raw = raw_of(&file, &options);
    expect(
        &run_at(&file.bytes, &raw, &trust, NOW),
        S::Trusted,
        R::Verified,
    );
}

#[test]
fn rsa_and_ed25519_chains_are_followed_too() {
    let root = Identity::self_signed(KeyKind::Ed25519, &CertSpec::ca("Ed Root"));
    let leaf = Identity::issued_by(
        &root,
        KeyKind::P256,
        &CertSpec {
            serial: 3,
            ..CertSpec::new("Leaf")
        },
    );
    let options = SignOptions::default();
    let file = sign(&leaf, &options);
    expect(
        &run_at(
            &file.bytes,
            &raw_of(&file, &options),
            &trust_of(&[&root]),
            NOW,
        ),
        S::Trusted,
        R::Verified,
    );
    let rsa_root = Identity::self_signed(KeyKind::Rsa2048, &CertSpec::ca("RSA Root"));
    let leaf = Identity::issued_by(
        &rsa_root,
        KeyKind::P384,
        &CertSpec {
            serial: 3,
            ..CertSpec::new("Leaf")
        },
    );
    let p384 = SignOptions {
        cms: CmsOptions {
            hash: Hash::Sha384,
            combined_algorithm: true,
            ..CmsOptions::default()
        },
        ..SignOptions::default()
    };
    let file = sign(&leaf, &p384);
    expect(
        &run_at(
            &file.bytes,
            &raw_of(&file, &p384),
            &trust_of(&[&rsa_root]),
            NOW,
        ),
        S::Trusted,
        R::Verified,
    );
}

// ---- more than one signature -------------------------------------------------------------------------

#[test]
fn an_earlier_signature_is_changed_by_a_later_one_and_the_later_one_holds() {
    let first = p256("First Signer");
    let second = p256("Second Signer");
    let options = SignOptions::default();
    let one = sign(&first, &options);
    let two = add_signature(&one, &second, &options);
    let raw_one = raw_of(&one, &options);
    let raw_two = raw_of(&two, &options);
    expect(
        &run_at(&two.bytes, &raw_one, &TrustList::empty(), NOW),
        S::Changed,
        R::DataAfterSignature,
    );
    let held = run_at(&two.bytes, &raw_two, &TrustList::empty(), NOW);
    expect(&held, S::UnknownSigner, R::NoTrustList);
    assert_eq!(held.signer.as_deref(), Some("Second Signer"));
}

// ---- hostile text ---------------------------------------------------------------------------------

#[test]
fn text_from_the_file_is_plain_and_cut() {
    let signer = Identity::self_signed(
        KeyKind::P256,
        &CertSpec {
            organisation: Some("Org\u{202E}gnp.exe".into()),
            ..CertSpec::new(&format!("Name\u{0}\u{202E}{}", "x".repeat(400)))
        },
    );
    let options = SignOptions {
        reason: Some(format!("line\none\u{202E} {}", "r".repeat(2000))),
        location: Some("Tøyen \u{1F600}\u{2066}".into()),
        time: Some("tomorrow\u{202E}".into()),
        ..SignOptions::default()
    };
    let info = run(&sign(&signer, &options), &options);
    expect(&info, S::UnknownSigner, R::NoTrustList);
    for text in [
        &info.signer,
        &info.organisation,
        &info.stated_reason,
        &info.stated_location,
        &info.claimed_time,
    ] {
        let text = text.as_deref().unwrap();
        assert!(partridge_protocol::is_plain_text(text), "{text:?}");
    }
    assert!(info.signer.as_deref().unwrap().len() <= partridge_protocol::MAX_SIG_NAME);
    assert!(info.stated_reason.as_deref().unwrap().len() <= partridge_protocol::MAX_SIG_NOTE);
    assert_eq!(info.organisation.as_deref(), Some("Orggnp.exe"));
    assert_eq!(info.stated_location.as_deref(), Some("Tøyen \u{1F600}"));
    assert_eq!(info.claimed_time.as_deref(), Some("tomorrow"));
    // The /Name entry of the dictionary is a claim nobody reads: only the certificate names the signer.
    let spoof = SignOptions {
        name: Some("Trusted Bank AS".into()),
        ..SignOptions::default()
    };
    let signer = p256("Mallory");
    let info = run(&sign(&signer, &spoof), &spoof);
    assert_eq!(info.signer.as_deref(), Some("Mallory"));
}

#[test]
fn attributes_written_in_another_order_than_the_one_signed_are_not_checked() {
    // The signature is over the attributes in the order DER wants; the file has them in the
    // reverse order. Both are the same set, and the signature holds for the first. A reader that
    // does not look at what was written would call this intact; this one does not check it.
    let signer = p256("Test Signer");
    let options = SignOptions {
        cms: CmsOptions {
            attributes_out_of_order: true,
            ..CmsOptions::default()
        },
        ..SignOptions::default()
    };
    expect(
        &run(&sign(&signer, &options), &options),
        S::Unsupported,
        R::EncodingUnsupported,
    );
}

#[test]
fn a_structure_the_parser_refuses_is_not_checked_and_not_called_broken() {
    // None of these is a signature that was shown to be false: the reader could not read it.
    let signer = p256("Test Signer");
    for (what, cms, reason) in [
        // The same attribute twice, equal: a set may not hold the same element twice.
        (
            "the same attribute twice",
            CmsOptions {
                twice_digest: true,
                ..CmsOptions::default()
            },
            R::Unreadable,
        ),
        // The signer's certificate twice in the set of certificates, as some signers write it.
        (
            "a certificate twice",
            CmsOptions {
                extra_certs: vec![signer.cert.clone()],
                ..CmsOptions::default()
            },
            R::Unreadable,
        ),
        // A length in the indefinite form below a definite outer element (BER).
        (
            "BER below the outer element",
            CmsOptions {
                indefinite_inner: true,
                ..CmsOptions::default()
            },
            R::EncodingUnsupported,
        ),
        (
            "BER outer element",
            CmsOptions {
                indefinite: true,
                ..CmsOptions::default()
            },
            R::EncodingUnsupported,
        ),
    ] {
        let options = SignOptions {
            cms,
            ..SignOptions::default()
        };
        let info = run(&sign(&signer, &options), &options);
        assert_eq!(
            (info.state, info.reason),
            (S::Unsupported, reason),
            "{what}: {:?}/{:?}",
            info.state,
            info.reason
        );
    }
}

#[test]
fn what_the_reader_proved_false_stays_broken() {
    // The other side of the test above: a signature that was read and found false, a byte range
    // that does not hold and a digest that does not match are not softened.
    let signer = p256("Test Signer");
    let damaged = SignOptions {
        cms: CmsOptions {
            damage_signature: true,
            ..CmsOptions::default()
        },
        ..SignOptions::default()
    };
    expect(
        &run(&sign(&signer, &damaged), &damaged),
        S::Broken,
        R::SignatureInvalid,
    );
    let options = SignOptions::default();
    let file = sign(&signer, &options);
    assert_eq!(ranged(&file, &options, [0, 10, 20, 30]).state, S::Broken);
    let changed = SignOptions {
        cms: CmsOptions {
            digest_override: Some(Hash::Sha256.of(b"other")),
            ..CmsOptions::default()
        },
        ..SignOptions::default()
    };
    expect(
        &run(&sign(&signer, &changed), &changed),
        S::Changed,
        R::ContentChanged,
    );
}

// ---- hostile structures --------------------------------------------------------------------------

/// A file whose signature is a hostile set of `count` elements.
fn hostile_file(set: HostileSet, count: usize) -> (SignedFile, SignOptions) {
    let signer = p256("Hostile");
    let options = SignOptions {
        contents_size: 1024 * 1024 - 64,
        ..SignOptions::default()
    };
    let mut file = sign(&signer, &options);
    replace_signature(&mut file, &hostile_set_signature(set, count));
    (file, options)
}

#[test]
fn a_set_of_very_many_elements_is_refused_at_once() {
    // The `der` crate sorts a set with an insertion sort: some tens of thousands of elements in
    // reverse order took over a minute, and 200,000 in a megabyte took minutes. The reader looks
    // at the shape first and does not decode such a signature.
    for set in [HostileSet::Values, HostileSet::Attributes] {
        for count in [257usize, 30_000] {
            let (file, options) = hostile_file(set, count);
            let started = std::time::Instant::now();
            let info = run(&file, &options);
            let took = started.elapsed();
            assert_eq!(
                (info.state, info.reason),
                (S::Unsupported, R::TooLarge),
                "{set:?} {count}"
            );
            assert!(
                took < std::time::Duration::from_secs(5),
                "{set:?} {count}: {took:?}"
            );
        }
    }
}

#[test]
fn a_set_within_the_limit_is_still_read() {
    // 256 elements are allowed; the structure is then read and judged like any other, here as a
    // signature that does not hold together.
    let (file, options) = hostile_file(HostileSet::Values, 256);
    let info = run(&file, &options);
    assert_ne!(info.reason, R::TooLarge);
    assert!(!matches!(info.state, S::Trusted | S::UnknownSigner));
}

#[test]
fn the_time_for_the_check_is_kept() {
    // When the time is up before the file is hashed, the signature is not checked to the end, and
    // that is told as such: not as intact, and not as broken.
    let signer = p256("Test Signer");
    let options = SignOptions::default();
    let file = sign(&signer, &options);
    let raw = raw_of(&file, &options);
    let trust = TrustList::empty();
    let mut progress = |_: u32| {};
    let mut work = Work::new(HASH_BUDGET, &mut progress).with_deadline(std::time::Instant::now());
    let mut source: &[u8] = &file.bytes;
    let info = check(
        0,
        &raw,
        &mut source,
        file.bytes.len() as u64,
        &Settings {
            trust: &trust,
            now: NOW,
        },
        &mut work,
    )
    .unwrap();
    expect(&info, S::Unsupported, R::TooLarge);
}

// ---- the chain -----------------------------------------------------------------------------------

/// Root, intermediate (issued by the root) and a leaf (issued by the intermediate).
fn chain_with(root_spec: CertSpec, inter_spec: CertSpec) -> (Identity, Identity, Identity) {
    let root = Identity::self_signed(KeyKind::P256, &root_spec);
    let inter = Identity::issued_by(&root, KeyKind::P256, &inter_spec);
    let leaf = Identity::issued_by(
        &inter,
        KeyKind::P256,
        &CertSpec {
            serial: 3,
            ..CertSpec::new("Leaf Signer")
        },
    );
    (root, inter, leaf)
}

fn run_chain(leaf: &Identity, carried: &[&Identity], trust: &TrustList) -> SignatureInfo {
    let options = SignOptions {
        cms: CmsOptions {
            extra_certs: carried.iter().map(|i| i.cert.clone()).collect(),
            ..CmsOptions::default()
        },
        ..SignOptions::default()
    };
    let file = sign(leaf, &options);
    run_at(&file.bytes, &raw_of(&file, &options), trust, NOW)
}

#[test]
fn an_intermediate_that_did_not_sign_the_leaf_leads_nowhere() {
    // The leaf names the intermediate as its issuer, with the right name, but it was signed by
    // another key. Taking the name for the proof would make a stranger a signer of the list.
    let root = Identity::self_signed(KeyKind::P256, &CertSpec::ca("Root CA"));
    let inter = Identity::issued_by(
        &root,
        KeyKind::P256,
        &CertSpec {
            serial: 2,
            ..CertSpec::ca("Intermediate CA")
        },
    );
    let rogue = Identity::self_signed(KeyKind::P256, &CertSpec::ca("Intermediate CA"));
    let leaf = Identity::issued_by(
        &rogue,
        KeyKind::P256,
        &CertSpec {
            serial: 3,
            ..CertSpec::new("Leaf Signer")
        },
    );
    let info = run_chain(&leaf, &[&inter], &trust_of(&[&root]));
    expect(&info, S::UnknownSigner, R::NotInTrustList);
    // The same chain with the real intermediate does lead to the root.
    let (root, inter, leaf) = chain_with(
        CertSpec::ca("Root CA"),
        CertSpec {
            serial: 2,
            ..CertSpec::ca("Intermediate CA")
        },
    );
    expect(
        &run_chain(&leaf, &[&inter], &trust_of(&[&root])),
        S::Trusted,
        R::Verified,
    );
}

#[test]
fn every_certificate_of_the_chain_is_judged_by_the_clock() {
    // 2020-01-01, 2023-11-14 (before NOW) and 2033-05-18 (after NOW).
    let (long_ago, expired, far_off) = (1_577_836_800, 1_700_000_000, 2_000_000_000);
    // An intermediate that has expired.
    let (root, inter, leaf) = chain_with(
        CertSpec::ca("Root CA"),
        CertSpec {
            serial: 2,
            not_before: long_ago,
            not_after: expired,
            ..CertSpec::ca("Intermediate CA")
        },
    );
    let info = run_chain(&leaf, &[&inter], &trust_of(&[&root]));
    expect(&info, S::UnknownSigner, R::CertificateExpired);
    // The signer's own certificate is fine, and says so.
    assert_eq!(info.certificate_time, CertificateTime::Valid);

    // An intermediate that is not valid yet.
    let (root, inter, leaf) = chain_with(
        CertSpec::ca("Root CA"),
        CertSpec {
            serial: 2,
            not_before: far_off,
            ..CertSpec::ca("Intermediate CA")
        },
    );
    expect(
        &run_chain(&leaf, &[&inter], &trust_of(&[&root])),
        S::UnknownSigner,
        R::CertificateNotYetValid,
    );

    // A root on the list that has expired.
    let (root, inter, leaf) = chain_with(
        CertSpec {
            not_before: long_ago,
            not_after: expired,
            ..CertSpec::ca("Root CA")
        },
        CertSpec {
            serial: 2,
            ..CertSpec::ca("Intermediate CA")
        },
    );
    expect(
        &run_chain(&leaf, &[&inter], &trust_of(&[&root])),
        S::UnknownSigner,
        R::CertificateExpired,
    );
}
