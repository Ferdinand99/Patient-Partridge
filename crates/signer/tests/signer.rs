//! The signer without a process: what it reads out of certificate files, what it refuses, and the
//! signatures it makes. The certificate files are in `tests/fixtures`; they are made for the tests
//! (see `scripts/make-signing-fixtures.py`) and hold nothing secret: the password of each is
//! `test`, or empty where the name says so.

use std::path::PathBuf;

use partridge_signer::{load, sign, Signer};
use partridge_signer_protocol::{
    usage, ErrorCode, HashKind, KeyKind, Request, Response, MAX_CMS_BYTES,
};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn loaded(name: &str) -> partridge_signer::Identity {
    load(&fixture(name), "test").unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn refused(name: &str, password: &str) -> ErrorCode {
    match load(&fixture(name), password) {
        Ok(_) => panic!("{name} was taken"),
        Err(e) => e.code,
    }
}

#[test]
fn each_kind_of_key_is_read_with_its_certificate_and_its_chain() {
    let cases = [
        ("rsa2048.p12", KeyKind::Rsa(2048), 3, "Test Signer RSA 2048"),
        ("rsa3072.p12", KeyKind::Rsa(3072), 2, "Test Signer RSA 3072"),
        ("rsa4096.p12", KeyKind::Rsa(4096), 2, "Test Signer RSA 4096"),
        ("p256.p12", KeyKind::EcdsaP256, 2, "Test Signer P-256"),
        ("p384.p12", KeyKind::EcdsaP384, 2, "Test Signer P-384"),
        // The older form that Windows and macOS still export (triple DES and a SHA-1 MAC).
        ("rsa2048-legacy.p12", KeyKind::Rsa(2048), 3, "Test Signer RSA 2048"),
        ("p256-legacy.p12", KeyKind::EcdsaP256, 2, "Test Signer P-256 legacy"),
    ];
    for (file, key, chain, name) in cases {
        let identity = loaded(file);
        let c = identity.certificate();
        assert_eq!(c.key, key, "{file}");
        assert_eq!(c.chain, chain, "{file}");
        assert_eq!(c.name, name, "{file}");
        assert!(!c.self_signed, "{file}");
        assert_eq!(c.fingerprint.len(), 64);
        assert!(c.may_sign());
        assert_eq!(
            c.usage,
            usage::HAS_EXTENSION | usage::DIGITAL_SIGNATURE | usage::NON_REPUDIATION
        );
        assert_eq!(
            c.hash(),
            if key == KeyKind::EcdsaP384 {
                HashKind::Sha384
            } else {
                HashKind::Sha256
            }
        );
    }
    // The issuer is the one that signed the holder's certificate: the intermediate where there is one.
    assert_eq!(
        loaded("rsa2048.p12").certificate().issuer,
        "Partridge Test Intermediate"
    );
    assert_eq!(
        loaded("rsa3072.p12").certificate().issuer,
        "Partridge Test Root"
    );
}

#[test]
fn a_file_without_a_chain_gives_a_chain_of_one_and_a_self_made_certificate_says_so() {
    let leaf = loaded("rsa2048-leaf-only.p12");
    assert_eq!(leaf.certificate().chain, 1);
    assert!(!leaf.certificate().self_signed);
    let own = loaded("rsa2048-self-signed.p12");
    assert_eq!(own.certificate().chain, 1);
    assert!(own.certificate().self_signed);
    assert_eq!(own.certificate().name, own.certificate().issuer);
}

#[test]
fn an_empty_password_opens_a_file_that_was_made_with_one() {
    let p12 = fixture("rsa2048-empty-password.p12");
    assert!(load(&p12, "").is_ok());
    assert_eq!(load(&p12, "test").err().unwrap().code, ErrorCode::WrongPassword);
}

#[test]
fn the_validity_and_the_key_usage_are_reported_as_the_certificate_has_them() {
    let expired = loaded("rsa2048-expired.p12");
    assert_eq!(expired.certificate().not_before, 1_420_070_400); // 2015-01-01
    assert_eq!(expired.certificate().not_after, 1_451_606_400); // 2016-01-01
    let not_yet = loaded("rsa2048-not-yet.p12");
    assert_eq!(not_yet.certificate().not_before, 3_786_912_000); // 2090-01-01

    let encrypt_only = loaded("rsa2048-encrypt-only.p12");
    assert_eq!(encrypt_only.certificate().usage, usage::HAS_EXTENSION);
    assert!(!encrypt_only.certificate().may_sign());
    let commitment = loaded("rsa2048-commitment.p12");
    assert_eq!(
        commitment.certificate().usage,
        usage::HAS_EXTENSION | usage::NON_REPUDIATION
    );
    assert!(commitment.certificate().may_sign());
    let none = loaded("rsa2048-no-usage.p12");
    assert_eq!(none.certificate().usage, 0);
    assert!(none.certificate().may_sign());
}

#[test]
fn names_with_letters_beyond_ascii_come_through_whole() {
    assert_eq!(
        loaded("rsa2048-nordic.p12").certificate().name,
        "\u{c5}se \u{d8}deg\u{e5}rd Test"
    );
    assert_eq!(
        loaded("rsa2048-cjk.p12").certificate().name,
        "\u{6d4b}\u{8bd5} Signer"
    );
}

#[test]
fn a_wrong_password_is_told_from_a_file_that_is_not_a_certificate_file() {
    for file in ["rsa2048.p12", "p256.p12", "rsa2048-legacy.p12"] {
        assert_eq!(refused(file, "wrong"), ErrorCode::WrongPassword, "{file}");
        assert_eq!(refused(file, ""), ErrorCode::WrongPassword, "{file}");
        assert_eq!(refused(file, "Test"), ErrorCode::WrongPassword, "{file}");
    }
    assert_eq!(
        load(b"", "test").err().unwrap().code,
        ErrorCode::NotACertificateFile
    );
    assert_eq!(
        load(b"This is not a certificate file at all.", "test")
            .err()
            .unwrap()
            .code,
        ErrorCode::NotACertificateFile
    );
    // A PDF is not one either.
    assert_eq!(
        load(b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\n", "test")
            .err()
            .unwrap()
            .code,
        ErrorCode::NotACertificateFile
    );
}

#[test]
fn keys_that_readers_cannot_check_are_refused_by_name() {
    for (file, name) in [
        ("ed25519.p12", "Ed25519"),
        ("p521.p12", "P-521"),
        ("rsa1024.p12", "2048"),
    ] {
        let e = load(&fixture(file), "test").err().unwrap();
        assert_eq!(e.code, ErrorCode::KeyUnsupported, "{file}");
        assert!(e.message.contains(name), "{file}: {}", e.message);
    }
}

#[test]
fn a_file_without_a_private_key_is_refused() {
    assert_eq!(refused("no-key.p12", "test"), ErrorCode::NoKey);
}

/// A file in which the key and the certificate are not a pair.
fn mismatched() -> Vec<u8> {
    use p12_keystore::{
        Certificate, KeyStore, KeyStoreEntry, Pkcs12Archive, PrivateKey, PrivateKeyChain,
    };
    let key_of = |file: &str| {
        let archive = Pkcs12Archive::from_pkcs12(&fixture(file), "test").unwrap();
        archive.keys[0].key.as_der().to_vec()
    };
    let cert_of = |file: &str| {
        let archive = Pkcs12Archive::from_pkcs12(&fixture(file), "test").unwrap();
        archive.certs[0].cert.as_der().to_vec()
    };
    let chain = PrivateKeyChain::new(
        "id".as_bytes().to_vec(),
        PrivateKey::from_der(&key_of("rsa2048.p12")).unwrap(),
        [Certificate::from_der(&cert_of("rsa3072.p12")).unwrap()],
    );
    let mut store = KeyStore::new();
    store.add_entry("pair", KeyStoreEntry::PrivateKeyChain(chain));
    store.writer("test").write().unwrap()
}

#[test]
fn a_key_with_the_certificate_of_another_key_is_refused() {
    let e = load(&mismatched(), "test").err().unwrap();
    assert_eq!(e.code, ErrorCode::KeyMismatch);
}

#[test]
fn a_key_is_found_by_its_public_key_and_not_by_the_identifier_the_file_gives_it() {
    use p12_keystore::{
        Certificate, KeyStore, KeyStoreEntry, Pkcs12Archive, PrivateKey, PrivateKeyChain,
    };
    // The certificate bags of the real file, in the other order and with an identifier that fits
    // nothing: the first certificate is not the holder's.
    let archive = Pkcs12Archive::from_pkcs12(&fixture("rsa2048.p12"), "test").unwrap();
    let key = PrivateKey::from_der(archive.keys[0].key.as_der()).unwrap();
    let mut certs: Vec<Certificate> = archive
        .certs
        .iter()
        .map(|c| Certificate::from_der(c.cert.as_der()).unwrap())
        .collect();
    certs.reverse();
    let chain = PrivateKeyChain::new("unrelated".as_bytes().to_vec(), key, certs);
    let mut store = KeyStore::new();
    store.add_entry("pair", KeyStoreEntry::PrivateKeyChain(chain));
    let file = store.writer("test").write().unwrap();
    let identity = load(&file, "test").unwrap();
    assert_eq!(identity.certificate().name, "Test Signer RSA 2048");
}

#[test]
fn a_file_over_the_cap_is_refused_before_it_is_read() {
    let big = vec![0u8; partridge_signer_protocol::MAX_P12_BYTES + 1];
    assert_eq!(load(&big, "test").err().unwrap().code, ErrorCode::TooLarge);
}

#[test]
fn a_signature_never_takes_more_room_than_the_signer_said_it_would() {
    for file in [
        "rsa2048.p12",
        "rsa3072.p12",
        "rsa4096.p12",
        "p256.p12",
        "p384.p12",
        "rsa2048-leaf-only.p12",
    ] {
        let identity = loaded(file);
        let said = identity.certificate().signature_bytes as usize;
        assert!(said > 0 && said <= MAX_CMS_BYTES);
        let len = identity.certificate().hash().len();
        let mut longest = 0;
        // ECDSA signatures differ in length from one to the next; try a good many digests.
        for round in 0..40u8 {
            let digest = vec![round.wrapping_mul(7).wrapping_add(1); len];
            let cms = sign(&identity, &digest).unwrap();
            assert!(cms.len() <= said, "{file}: {} over {said}", cms.len());
            longest = longest.max(cms.len());
        }
        if file.starts_with("rsa") {
            // An RSA signature has the length of the key every time: the size is exact.
            assert_eq!(longest, said, "{file}");
        }
    }
}

#[test]
fn only_a_digest_of_the_right_length_is_signed() {
    let rsa = loaded("rsa2048.p12");
    assert_eq!(
        sign(&rsa, &[0; 48]).err().unwrap().code,
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        sign(&rsa, &[0; 31]).err().unwrap().code,
        ErrorCode::InvalidRequest
    );
    assert!(sign(&rsa, &[0; 32]).is_ok());
    let p384 = loaded("p384.p12");
    assert_eq!(
        sign(&p384, &[0; 32]).err().unwrap().code,
        ErrorCode::InvalidRequest
    );
    assert!(sign(&p384, &[0; 48]).is_ok());
}

#[test]
fn a_signature_is_a_der_structure_with_the_certificates_of_the_chain() {
    let identity = loaded("rsa2048.p12");
    let cms = sign(&identity, &[9; 32]).unwrap();
    // ContentInfo, then the identifier of signed data.
    assert_eq!(cms[0], 0x30);
    let signed_data = [0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x02];
    assert!(cms.windows(signed_data.len()).any(|w| w == signed_data));
    // The three certificates are in it whole.
    let archive =
        p12_keystore::Pkcs12Archive::from_pkcs12(&fixture("rsa2048.p12"), "test").unwrap();
    for bag in &archive.certs {
        let der = bag.cert.as_der();
        assert!(cms.windows(der.len()).any(|w| w == der));
    }
    // The ESS signing-certificate-v2 attribute and the digest are in it.
    let ess = [0x06, 0x0B, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x09, 0x10, 0x02, 0x2F];
    assert!(cms.windows(ess.len()).any(|w| w == ess));
    assert!(cms.windows(32).any(|w| w == [9u8; 32]));
}

#[test]
fn the_same_digest_gives_the_same_signature_with_rsa_and_another_one_with_ecdsa() {
    // RSA with PKCS #1 v1.5 is deterministic; ECDSA is not. The tests of the file made with each
    // are in the engine, which checks the signatures with its own checker.
    let rsa = loaded("rsa2048.p12");
    assert_eq!(sign(&rsa, &[1; 32]).unwrap(), sign(&rsa, &[1; 32]).unwrap());
    let p256 = loaded("p256.p12");
    assert_ne!(sign(&p256, &[1; 32]).unwrap(), sign(&p256, &[1; 32]).unwrap());
}

// ---- the requests of the protocol -----------------------------------------------------------------

fn greeted() -> Signer {
    let mut signer = Signer::new("test");
    let Response::Hello { version, sandbox } = signer.handle(Request::Hello {
        version: partridge_signer_protocol::PROTOCOL_VERSION,
    }) else {
        panic!("no greeting");
    };
    assert_eq!(version, partridge_signer_protocol::PROTOCOL_VERSION);
    assert_eq!(sandbox, "test");
    signer
}

fn code(response: Response) -> ErrorCode {
    match response {
        Response::Error { code, .. } => code,
        other => panic!("not an error: {other:?}"),
    }
}

#[test]
fn nothing_is_done_before_the_greeting_and_a_wrong_version_is_refused() {
    let mut signer = Signer::new("test");
    assert_eq!(
        code(signer.handle(Request::Load {
            p12: fixture("rsa2048.p12"),
            password: b"test".to_vec()
        })),
        ErrorCode::InvalidRequest
    );
    assert_eq!(
        code(signer.handle(Request::Hello { version: 999 })),
        ErrorCode::ProtocolMismatch
    );
    assert_eq!(
        code(signer.handle(Request::Sign { digest: vec![0; 32] })),
        ErrorCode::InvalidRequest
    );
}

#[test]
fn a_signature_needs_a_certificate_and_one_certificate_is_held_at_a_time() {
    let mut signer = greeted();
    assert_eq!(
        code(signer.handle(Request::Sign { digest: vec![0; 32] })),
        ErrorCode::NotLoaded
    );
    // A wrong password leaves the signer ready for another try.
    assert_eq!(
        code(signer.handle(Request::Load {
            p12: fixture("rsa2048.p12"),
            password: b"nope".to_vec()
        })),
        ErrorCode::WrongPassword
    );
    let Response::Loaded(info) = signer.handle(Request::Load {
        p12: fixture("rsa2048.p12"),
        password: b"test".to_vec(),
    }) else {
        panic!("not loaded");
    };
    assert_eq!(info.name, "Test Signer RSA 2048");
    assert_eq!(
        code(signer.handle(Request::Load {
            p12: fixture("p256.p12"),
            password: b"test".to_vec()
        })),
        ErrorCode::InvalidRequest
    );
    let Response::Signed { cms } = signer.handle(Request::Sign { digest: vec![5; 32] }) else {
        panic!("not signed");
    };
    assert!(cms.len() <= info.signature_bytes as usize);
    // After Close the key is gone.
    assert_eq!(signer.handle(Request::Close), Response::Closed);
    assert_eq!(
        code(signer.handle(Request::Sign { digest: vec![5; 32] })),
        ErrorCode::NotLoaded
    );
}

#[test]
fn a_password_that_is_not_text_is_refused() {
    let mut signer = greeted();
    assert_eq!(
        code(signer.handle(Request::Load {
            p12: fixture("rsa2048.p12"),
            password: vec![0xff, 0xfe]
        })),
        ErrorCode::InvalidRequest
    );
}

#[test]
fn a_signer_that_refuses_says_why_to_everything() {
    let mut signer = Signer::new("none: x");
    signer.refuse("the sandbox could not be applied: x");
    assert!(signer.is_refusing());
    let Response::Error { code, message } = signer.handle(Request::Hello {
        version: partridge_signer_protocol::PROTOCOL_VERSION,
    }) else {
        panic!("no refusal");
    };
    assert_eq!(code, ErrorCode::SandboxUnavailable);
    assert!(message.contains("sandbox"));
}

#[test]
fn a_certificate_file_that_is_damaged_never_stops_the_signer() {
    // The same file with one byte changed, and cut short, at many places. Whatever the library
    // thinks of it, the signer answers and does not panic.
    let original = fixture("rsa2048.p12");
    let mut taken = 0;
    for round in 0..120usize {
        let mut file = original.clone();
        let at = (round * 41) % file.len();
        match round % 3 {
            0 => file[at] ^= 0x55,
            1 => file.truncate(at),
            _ => {
                file[at] = file[at].wrapping_add(1);
                file[(at * 7) % original.len()] ^= 0x01;
            }
        }
        if load(&file, "test").is_ok() {
            taken += 1;
        }
    }
    // Most damage is caught by the MAC; a change to a part that the MAC does not cover may still
    // open. Nothing is asked of the count except that the loop ran to the end.
    assert!(taken < 120);
}
