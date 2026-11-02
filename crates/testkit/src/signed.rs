//! Signed PDF files for the tests of the signature check.
//!
//! Every key is made when a test asks for it (nothing secret is stored in the repository), every
//! certificate and every signature is built here, byte by byte, from DER pieces, so that a test
//! can also build the wrong ones: a signature over other attributes, a certificate that is not the
//! one named, a byte range that leaves a gap. The files are small and valid enough for PDFium to
//! open them; their signatures are real, made with `ring`.
//!
//! The RSA keys are made with `num-bigint` (ring signs with RSA but does not make keys).

use std::sync::OnceLock;

use num_bigint::{BigInt, BigUint};
use ring::digest;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{self, EcdsaKeyPair, Ed25519KeyPair, KeyPair, RsaKeyPair};

// ---- DER ---------------------------------------------------------------------------------

/// A small writer for DER, enough for certificates and signatures.
pub mod der {
    pub fn length(n: usize) -> Vec<u8> {
        if n < 0x80 {
            return vec![n as u8];
        }
        let bytes = n.to_be_bytes();
        let skip = bytes.iter().take_while(|b| **b == 0).count();
        let mut out = vec![0x80 | (bytes.len() - skip) as u8];
        out.extend_from_slice(&bytes[skip..]);
        out
    }

    pub fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        out.extend(length(content.len()));
        out.extend_from_slice(content);
        out
    }

    pub fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
        parts.concat()
    }

    pub fn seq(parts: &[Vec<u8>]) -> Vec<u8> {
        tlv(0x30, &cat(parts))
    }

    /// A SET OF, in the order DER wants: by the bytes of the elements.
    pub fn set_of(parts: &[Vec<u8>]) -> Vec<u8> {
        let mut sorted = parts.to_vec();
        sorted.sort();
        tlv(0x31, &cat(&sorted))
    }

    pub fn oid(dotted: &str) -> Vec<u8> {
        let arcs: Vec<u64> = dotted.split('.').map(|a| a.parse().unwrap()).collect();
        let mut body = vec![(arcs[0] * 40 + arcs[1]) as u8];
        for arc in &arcs[2..] {
            let mut chunk = vec![(*arc & 0x7f) as u8];
            let mut rest = *arc >> 7;
            while rest > 0 {
                chunk.push(0x80 | (rest & 0x7f) as u8);
                rest >>= 7;
            }
            chunk.reverse();
            body.extend(chunk);
        }
        tlv(0x06, &body)
    }

    /// An INTEGER from the bytes of a non-negative number, most significant first.
    pub fn integer(be: &[u8]) -> Vec<u8> {
        let skip = be
            .iter()
            .take_while(|b| **b == 0)
            .count()
            .min(be.len().saturating_sub(1));
        let mut body = be[skip..].to_vec();
        if body.is_empty() {
            body.push(0);
        }
        if body[0] & 0x80 != 0 {
            body.insert(0, 0);
        }
        tlv(0x02, &body)
    }

    pub fn int(n: u64) -> Vec<u8> {
        integer(&n.to_be_bytes())
    }

    pub fn octets(bytes: &[u8]) -> Vec<u8> {
        tlv(0x04, bytes)
    }

    pub fn null() -> Vec<u8> {
        vec![0x05, 0x00]
    }

    pub fn boolean(v: bool) -> Vec<u8> {
        vec![0x01, 0x01, if v { 0xff } else { 0 }]
    }

    pub fn utf8(text: &str) -> Vec<u8> {
        tlv(0x0c, text.as_bytes())
    }

    pub fn bit_string(bytes: &[u8]) -> Vec<u8> {
        let mut body = vec![0u8];
        body.extend_from_slice(bytes);
        tlv(0x03, &body)
    }

    /// `[n] EXPLICIT`.
    pub fn explicit(n: u8, content: &[u8]) -> Vec<u8> {
        tlv(0xA0 | n, content)
    }

    /// The date for a count of days since 1970-01-01.
    pub fn civil(days: i64) -> (i64, u32, u32) {
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        (if m <= 2 { y + 1 } else { y }, m, d)
    }

    /// A time as UTCTime (1950 to 2049) or GeneralizedTime.
    pub fn time(unix: i64) -> Vec<u8> {
        let (y, m, d) = civil(unix.div_euclid(86_400));
        let secs = unix.rem_euclid(86_400);
        let (hh, mm, ss) = (secs / 3600, secs % 3600 / 60, secs % 60);
        if (1950..2050).contains(&y) {
            tlv(
                0x17,
                format!("{:02}{m:02}{d:02}{hh:02}{mm:02}{ss:02}Z", y % 100).as_bytes(),
            )
        } else {
            tlv(
                0x18,
                format!("{y:04}{m:02}{d:02}{hh:02}{mm:02}{ss:02}Z").as_bytes(),
            )
        }
    }
}

use der::{
    bit_string, boolean, cat, explicit, int, integer, null, octets, oid, seq, set_of, tlv, utf8,
};

// ---- keys --------------------------------------------------------------------------------

/// The hash of a signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hash {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    pub fn oid(self) -> &'static str {
        match self {
            Hash::Sha1 => "1.3.14.3.2.26",
            Hash::Sha256 => "2.16.840.1.101.3.4.2.1",
            Hash::Sha384 => "2.16.840.1.101.3.4.2.2",
            Hash::Sha512 => "2.16.840.1.101.3.4.2.3",
        }
    }

    pub fn algorithm(self) -> &'static digest::Algorithm {
        match self {
            Hash::Sha1 => &digest::SHA1_FOR_LEGACY_USE_ONLY,
            Hash::Sha256 => &digest::SHA256,
            Hash::Sha384 => &digest::SHA384,
            Hash::Sha512 => &digest::SHA512,
        }
    }

    pub fn of(self, data: &[u8]) -> Vec<u8> {
        digest::digest(self.algorithm(), data).as_ref().to_vec()
    }

    fn identifier(self) -> Vec<u8> {
        seq(&[oid(self.oid()), null()])
    }
}

/// What kind of key to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    P256,
    P384,
    Ed25519,
    Rsa2048,
}

enum Inner {
    Ec(EcdsaKeyPair, KeyKind),
    Ed(Ed25519KeyPair),
    Rsa(RsaKeyPair),
}

/// A key pair made for a test.
pub struct Key {
    kind: KeyKind,
    inner: Inner,
}

fn rng() -> SystemRandom {
    SystemRandom::new()
}

impl Key {
    pub fn generate(kind: KeyKind) -> Key {
        let rng = rng();
        let inner = match kind {
            KeyKind::P256 | KeyKind::P384 => {
                let alg = if kind == KeyKind::P256 {
                    &signature::ECDSA_P256_SHA256_ASN1_SIGNING
                } else {
                    &signature::ECDSA_P384_SHA384_ASN1_SIGNING
                };
                let pkcs8 = EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
                Inner::Ec(
                    EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap(),
                    kind,
                )
            }
            KeyKind::Ed25519 => {
                let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
                Inner::Ed(Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap())
            }
            KeyKind::Rsa2048 => Inner::Rsa(RsaKeyPair::from_der(&rsa_private_key(2048)).unwrap()),
        };
        Key { kind, inner }
    }

    pub fn kind(&self) -> KeyKind {
        self.kind
    }

    /// The hash that goes with this key in a signature made here: ECDSA keys are used with the
    /// hash of their curve, others with SHA-256.
    pub fn natural_hash(&self) -> Hash {
        if self.kind == KeyKind::P384 {
            Hash::Sha384
        } else {
            Hash::Sha256
        }
    }

    /// SubjectPublicKeyInfo.
    pub fn spki(&self) -> Vec<u8> {
        match &self.inner {
            Inner::Ec(pair, kind) => {
                let curve = if *kind == KeyKind::P256 {
                    "1.2.840.10045.3.1.7"
                } else {
                    "1.3.132.0.34"
                };
                seq(&[
                    seq(&[oid("1.2.840.10045.2.1"), oid(curve)]),
                    bit_string(pair.public_key().as_ref()),
                ])
            }
            Inner::Ed(pair) => seq(&[
                seq(&[oid("1.3.101.112")]),
                bit_string(pair.public_key().as_ref()),
            ]),
            Inner::Rsa(pair) => seq(&[
                seq(&[oid("1.2.840.113549.1.1.1"), null()]),
                bit_string(pair.public().as_ref()),
            ]),
        }
    }

    /// The algorithm identifier a signature with this key carries, for `hash`. `combined` names the
    /// hash in the algorithm (sha256WithRSAEncryption); the other form leaves it to the digest
    /// algorithm of the signer (rsaEncryption).
    pub fn signature_algorithm(&self, hash: Hash, combined: bool) -> Vec<u8> {
        match &self.inner {
            Inner::Ec(..) => {
                let oid_text = match hash {
                    Hash::Sha1 => "1.2.840.10045.4.1",
                    Hash::Sha256 => "1.2.840.10045.4.3.2",
                    Hash::Sha384 => "1.2.840.10045.4.3.3",
                    Hash::Sha512 => "1.2.840.10045.4.3.4",
                };
                if combined {
                    seq(&[oid(oid_text)])
                } else {
                    seq(&[oid("1.2.840.10045.2.1")])
                }
            }
            Inner::Ed(_) => seq(&[oid("1.3.101.112")]),
            Inner::Rsa(_) => {
                let oid_text = match (combined, hash) {
                    (false, _) => "1.2.840.113549.1.1.1",
                    (true, Hash::Sha1) => "1.2.840.113549.1.1.5",
                    (true, Hash::Sha256) => "1.2.840.113549.1.1.11",
                    (true, Hash::Sha384) => "1.2.840.113549.1.1.12",
                    (true, Hash::Sha512) => "1.2.840.113549.1.1.13",
                };
                seq(&[oid(oid_text), null()])
            }
        }
    }

    /// Signs `message`; the hash is the key's own for ECDSA and `hash` for RSA.
    pub fn sign(&self, hash: Hash, message: &[u8]) -> Vec<u8> {
        let rng = rng();
        match &self.inner {
            Inner::Ec(pair, _) => pair.sign(&rng, message).unwrap().as_ref().to_vec(),
            Inner::Ed(pair) => pair.sign(message).as_ref().to_vec(),
            Inner::Rsa(pair) => {
                let padding: &'static dyn signature::RsaEncoding = match hash {
                    Hash::Sha256 => &signature::RSA_PKCS1_SHA256,
                    Hash::Sha384 => &signature::RSA_PKCS1_SHA384,
                    Hash::Sha512 => &signature::RSA_PKCS1_SHA512,
                    Hash::Sha1 => panic!("no SHA-1 signatures are made"),
                };
                let mut out = vec![0u8; pair.public().modulus_len()];
                pair.sign(padding, &rng, message, &mut out).unwrap();
                out
            }
        }
    }
}

// ---- RSA key generation ----------------------------------------------------------------------

fn random_bytes(n: usize) -> Vec<u8> {
    let mut out = vec![0u8; n];
    rng().fill(&mut out).unwrap();
    out
}

const SMALL_PRIMES: [u32; 25] = [
    3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71, 73, 79, 83, 89, 97,
    101,
];

fn is_probable_prime(n: &BigUint) -> bool {
    let zero = BigUint::from(0u32);
    let one = BigUint::from(1u32);
    for p in SMALL_PRIMES {
        if n % p == zero {
            return *n == BigUint::from(p);
        }
    }
    let n_minus_1 = n - &one;
    let mut d = n_minus_1.clone();
    let mut s = 0;
    while &d % 2u32 == zero {
        d >>= 1;
        s += 1;
    }
    'round: for _ in 0..10 {
        let a = BigUint::from_bytes_be(&random_bytes(n.to_bytes_be().len())) % (n - 3u32) + 2u32;
        let mut x = a.modpow(&d, n);
        if x == one || x == n_minus_1 {
            continue;
        }
        for _ in 1..s {
            x = x.modpow(&BigUint::from(2u32), n);
            if x == n_minus_1 {
                continue 'round;
            }
        }
        return false;
    }
    true
}

fn random_prime(bits: usize) -> BigUint {
    loop {
        let mut bytes = random_bytes(bits / 8);
        bytes[0] |= 0xc0;
        *bytes.last_mut().unwrap() |= 1;
        let candidate = BigUint::from_bytes_be(&bytes);
        if is_probable_prime(&candidate) {
            return candidate;
        }
    }
}

fn mod_inverse(a: &BigUint, m: &BigUint) -> BigUint {
    let zero = BigInt::from(0);
    let (mut old_r, mut r) = (BigInt::from(a.clone()), BigInt::from(m.clone()));
    let (mut old_s, mut s) = (BigInt::from(1), BigInt::from(0));
    while r != zero {
        let q = &old_r / &r;
        let next_r = &old_r - &q * &r;
        old_r = std::mem::replace(&mut r, next_r);
        let next_s = &old_s - &q * &s;
        old_s = std::mem::replace(&mut s, next_s);
    }
    let m = BigInt::from(m.clone());
    (((old_s % &m) + &m) % &m).to_biguint().unwrap()
}

/// The parts of a new RSA key: `(n, e, d, p, q)`.
fn rsa_parts(bits: usize) -> (BigUint, BigUint, BigUint, BigUint, BigUint) {
    let e = BigUint::from(65_537u32);
    loop {
        let (mut p, mut q) = (random_prime(bits / 2), random_prime(bits / 2));
        if p == q {
            continue;
        }
        if p < q {
            std::mem::swap(&mut p, &mut q);
        }
        let phi = (&p - 1u32) * (&q - 1u32);
        // e must have an inverse: it shares no factor with phi.
        if (&phi % &e) == BigUint::from(0u32) {
            continue;
        }
        let d = mod_inverse(&e, &phi);
        return (&p * &q, e, d, p, q);
    }
}

/// An RSA private key in PKCS #1 form.
fn rsa_private_key(bits: usize) -> Vec<u8> {
    let (n, e, d, p, q) = rsa_parts(bits);
    let dp = &d % (&p - 1u32);
    let dq = &d % (&q - 1u32);
    let qinv = mod_inverse(&q, &p);
    let be = |v: &BigUint| integer(&v.to_bytes_be());
    seq(&[
        int(0),
        be(&n),
        be(&e),
        be(&d),
        be(&p),
        be(&q),
        be(&dp),
        be(&dq),
        be(&qinv),
    ])
}

/// The public key of a made-up RSA key of `bits` bits, as the SubjectPublicKeyInfo of a
/// certificate: for tests of keys that are too short (ring cannot sign with them).
pub fn rsa_public_spki(bits: usize) -> Vec<u8> {
    let (n, e, ..) = rsa_parts(bits);
    let public = seq(&[integer(&n.to_bytes_be()), integer(&e.to_bytes_be())]);
    seq(&[
        seq(&[oid("1.2.840.113549.1.1.1"), null()]),
        bit_string(&public),
    ])
}

// ---- certificates --------------------------------------------------------------------------

/// What goes into a certificate.
#[derive(Debug, Clone)]
pub struct CertSpec {
    pub common_name: String,
    pub organisation: Option<String>,
    pub serial: u64,
    pub not_before: i64,
    pub not_after: i64,
    /// `basicConstraints` with `cA` set.
    pub ca: bool,
    /// Whether to write a `basicConstraints` extension at all.
    pub basic_constraints: bool,
    /// The bits of `keyUsage` (bit 0 digitalSignature, 1 nonRepudiation, 5 keyCertSign), or none.
    pub key_usage: Option<u16>,
}

/// A time that is well inside the validity of every certificate made with [`CertSpec::new`]:
/// 2026-06-15 12:00:00 UTC.
pub const NOW: i64 = 1_781_524_800;

impl CertSpec {
    pub fn new(common_name: &str) -> CertSpec {
        CertSpec {
            common_name: common_name.into(),
            organisation: Some("Test Organisation".into()),
            serial: 0x1234_5678,
            // 2025-01-01 to 2035-01-01.
            not_before: 1_735_689_600,
            not_after: 2_051_222_400,
            ca: false,
            basic_constraints: false,
            key_usage: Some(0x01),
        }
    }

    pub fn ca(common_name: &str) -> CertSpec {
        CertSpec {
            ca: true,
            basic_constraints: true,
            key_usage: Some(1 << 5),
            ..CertSpec::new(common_name)
        }
    }
}

/// A key and its certificate.
pub struct Identity {
    pub key: Key,
    /// The certificate, DER.
    pub cert: Vec<u8>,
    /// The subject name, DER (as it is in the certificate).
    pub name: Vec<u8>,
    /// The issuer name, DER (the subject name for a certificate that signed itself).
    pub issuer: Vec<u8>,
    pub serial: u64,
}

fn name(common_name: &str, organisation: Option<&str>) -> Vec<u8> {
    let mut rdns = Vec::new();
    if let Some(org) = organisation {
        rdns.push(set_of(&[seq(&[oid("2.5.4.10"), utf8(org)])]));
    }
    rdns.push(set_of(&[seq(&[oid("2.5.4.3"), utf8(common_name)])]));
    seq(&rdns)
}

impl Identity {
    /// A key and a certificate it signed itself.
    pub fn self_signed(kind: KeyKind, spec: &CertSpec) -> Identity {
        let key = Key::generate(kind);
        let subject = name(&spec.common_name, spec.organisation.as_deref());
        let spki = key.spki();
        let cert = certificate(spec, &subject, &subject, &spki, &key);
        Identity {
            key,
            cert,
            issuer: subject.clone(),
            name: subject,
            serial: spec.serial,
        }
    }

    /// A key and a certificate that `issuer` signed.
    pub fn issued_by(issuer: &Identity, kind: KeyKind, spec: &CertSpec) -> Identity {
        let key = Key::generate(kind);
        let subject = name(&spec.common_name, spec.organisation.as_deref());
        let spki = key.spki();
        let cert = certificate(spec, &issuer.name, &subject, &spki, &issuer.key);
        Identity {
            key,
            cert,
            issuer: issuer.name.clone(),
            name: subject,
            serial: spec.serial,
        }
    }

    /// The same key with a certificate for `subject_spki` of another key: used to make a
    /// certificate that names the key of someone else.
    pub fn certificate_for(&self, spec: &CertSpec, subject_spki: &[u8]) -> Vec<u8> {
        let subject = name(&spec.common_name, spec.organisation.as_deref());
        certificate(spec, &self.name, &subject, subject_spki, &self.key)
    }

    /// A key identifier for the certificate: the SHA-256 of the public key, cut to 20 bytes.
    pub fn key_identifier(&self) -> Vec<u8> {
        Hash::Sha256.of(&self.key.spki())[..20].to_vec()
    }
}

/// A DER certificate signed with `signer`.
pub fn certificate(
    spec: &CertSpec,
    issuer: &[u8],
    subject: &[u8],
    spki: &[u8],
    signer: &Key,
) -> Vec<u8> {
    let hash = signer.natural_hash();
    let algorithm = signer.signature_algorithm(hash, true);
    let mut extensions = Vec::new();
    // The subject key identifier: the first 20 bytes of the SHA-256 of the public key info.
    extensions.push(seq(&[
        oid("2.5.29.14"),
        octets(&octets(&Hash::Sha256.of(spki)[..20])),
    ]));
    if spec.basic_constraints {
        let value = if spec.ca {
            seq(&[boolean(true)])
        } else {
            seq(&[])
        };
        extensions.push(seq(&[oid("2.5.29.19"), boolean(true), octets(&value)]));
    }
    if let Some(bits) = spec.key_usage {
        // A BIT STRING of up to nine bits, bit 0 first.
        let mut first = 0u8;
        let mut second = 0u8;
        for bit in 0..9 {
            if bits & (1 << bit) != 0 {
                if bit < 8 {
                    first |= 0x80 >> bit;
                } else {
                    second |= 0x80;
                }
            }
        }
        let used = if second != 0 {
            7
        } else {
            first.trailing_zeros().min(7) as u8
        };
        let mut body = vec![used, first];
        if second != 0 {
            body.push(second);
        }
        extensions.push(seq(&[
            oid("2.5.29.15"),
            boolean(true),
            octets(&tlv(0x03, &body)),
        ]));
    }
    let mut fields = vec![
        explicit(0, &int(2)),
        int(spec.serial),
        algorithm.clone(),
        issuer.to_vec(),
        seq(&[der::time(spec.not_before), der::time(spec.not_after)]),
        subject.to_vec(),
        spki.to_vec(),
    ];
    if !extensions.is_empty() {
        fields.push(explicit(3, &seq(&extensions)));
    }
    let tbs = seq(&fields);
    let signature = signer.sign(hash, &tbs);
    seq(&[tbs, algorithm, bit_string(&signature)])
}

// ---- CMS ---------------------------------------------------------------------------------------

/// How to build the signature (CMS) of a PDF signature. The default is an ordinary, correct one.
#[derive(Debug, Clone)]
pub struct CmsOptions {
    pub hash: Hash,
    /// Name the signer by key identifier instead of issuer and serial number.
    pub by_key_identifier: bool,
    /// Add a signing-certificate-v2 attribute (CAdES).
    pub signing_certificate: bool,
    /// Put the hash of this certificate in that attribute instead of the signer's.
    pub signing_certificate_of: Option<Vec<u8>>,
    /// The form of the signature algorithm identifier (see [`Key::signature_algorithm`]).
    pub combined_algorithm: bool,
    /// No signed attributes: the signature is over nothing but a made-up value.
    pub without_attributes: bool,
    /// Leave out the content type attribute.
    pub without_content_type: bool,
    /// Write the message digest attribute twice, the same both times (a set may not hold the same
    /// element twice, so a parser may refuse this before anything is checked).
    pub twice_digest: bool,
    /// Write the message digest attribute twice with two different values: one that is not the
    /// digest of the file, and, sorting after it, the right one.
    pub digest_conflict: bool,
    /// Use this object identifier (such as `1.2.840.113549.1.7.2`) as the value of the content type
    /// attribute instead of `id-data`.
    pub content_type_value: Option<String>,
    /// Give the signing-certificate attribute two values: the right one and, sorting after it, a
    /// wrong one. Implies `signing_certificate`.
    pub binding_two_values: bool,
    /// Write the signed attributes in the signature in another order than the DER one the
    /// signature is made over (the order in the file is the reverse).
    pub attributes_out_of_order: bool,
    /// Sign this digest instead of the real one.
    pub digest_override: Option<Vec<u8>>,
    /// Embed this content in the CMS (a signature that is not detached).
    pub embed_content: bool,
    /// Add these certificates to the ones in the signature, after the signer's.
    pub extra_certs: Vec<Vec<u8>>,
    /// Leave the signer's certificate out of the signature.
    pub without_signer_cert: bool,
    /// Write the length of the outer element in the indefinite form (BER).
    pub indefinite: bool,
    /// Write the length of the `[0]` below the outer element in the indefinite form, and leave the
    /// outer element definite.
    pub indefinite_inner: bool,
    /// Flip one bit of the signature value.
    pub damage_signature: bool,
    /// Two signers.
    pub two_signers: bool,
}

impl Default for CmsOptions {
    fn default() -> Self {
        CmsOptions {
            hash: Hash::Sha256,
            by_key_identifier: false,
            signing_certificate: false,
            signing_certificate_of: None,
            combined_algorithm: false,
            without_attributes: false,
            without_content_type: false,
            twice_digest: false,
            digest_conflict: false,
            content_type_value: None,
            binding_two_values: false,
            attributes_out_of_order: false,
            digest_override: None,
            embed_content: false,
            extra_certs: Vec::new(),
            without_signer_cert: false,
            indefinite: false,
            indefinite_inner: false,
            damage_signature: false,
            two_signers: false,
        }
    }
}

fn attribute(oid_text: &str, value: Vec<u8>) -> Vec<u8> {
    seq(&[oid(oid_text), set_of(&[value])])
}

/// The CMS signature of `data_digest` (the digest of the signed part of a file) by `signer`.
pub fn cms(signer: &Identity, options: &CmsOptions, data_digest: &[u8]) -> Vec<u8> {
    let hash = options.hash;
    let digest_value = options
        .digest_override
        .clone()
        .unwrap_or_else(|| data_digest.to_vec());
    let mut attrs = Vec::new();
    if !options.without_content_type {
        let value = options
            .content_type_value
            .as_deref()
            .unwrap_or("1.2.840.113549.1.7.1");
        attrs.push(attribute("1.2.840.113549.1.9.3", oid(value)));
    }
    attrs.push(attribute("1.2.840.113549.1.9.4", octets(&digest_value)));
    if options.twice_digest {
        attrs.push(attribute("1.2.840.113549.1.9.4", octets(&digest_value)));
    }
    if options.digest_conflict {
        // A value that sorts before the right one, so the right one is the last in the set.
        let mut other = digest_value.clone();
        let at = other
            .iter()
            .position(|b| *b > 0)
            .expect("a digest is not all zeros");
        other[at] -= 1;
        attrs.push(attribute("1.2.840.113549.1.9.4", octets(&other)));
    }
    if options.signing_certificate || options.binding_two_values {
        let named = options
            .signing_certificate_of
            .as_ref()
            .unwrap_or(&signer.cert);
        let hash = Hash::Sha256.of(named);
        let id_of = |hash: &[u8]| seq(&[seq(&[octets(hash)])]);
        let value = seq(&[id_of(&hash)]);
        let mut values = vec![value];
        if options.binding_two_values {
            // A hash that sorts after the right one: the first byte that can go up does.
            let mut other = hash.clone();
            let at = other.iter().position(|b| *b < 0xff).expect("not all ones");
            other[at] += 1;
            values.push(seq(&[id_of(&other)]));
        }
        attrs.push(seq(&[oid("1.2.840.113549.1.9.16.2.47"), set_of(&values)]));
    }
    let attrs_set = set_of(&attrs);
    let mut signature = if options.without_attributes {
        signer.key.sign(signer.key.natural_hash(), b"no attributes")
    } else {
        // The hash of the key (ECDSA) is the key's own; RSA uses the digest algorithm.
        let sign_hash = if matches!(signer.key.kind(), KeyKind::P256 | KeyKind::P384) {
            signer.key.natural_hash()
        } else {
            hash
        };
        signer.key.sign(sign_hash, &attrs_set)
    };
    if options.damage_signature {
        let last = signature.len() - 1;
        signature[last] ^= 1;
    }
    let sid = if options.by_key_identifier {
        tlv(0x80, &signer.key_identifier())
    } else {
        seq(&[signer.issuer.clone(), integer(&signer.serial.to_be_bytes())])
    };
    let mut info_fields = vec![
        int(if options.by_key_identifier { 3 } else { 1 }),
        sid,
        hash.identifier(),
    ];
    if !options.without_attributes {
        // [0] IMPLICIT: the same bytes as the SET, with the tag of the context.
        let mut written = attrs_set.clone();
        written[0] = 0xA0;
        if options.attributes_out_of_order {
            let mut sorted = attrs.clone();
            sorted.sort();
            sorted.reverse();
            written = tlv(0xA0, &cat(&sorted));
        }
        info_fields.push(written);
    }
    info_fields.push(
        signer
            .key
            .signature_algorithm(hash, options.combined_algorithm),
    );
    info_fields.push(octets(&signature));
    let info = seq(&info_fields);
    let mut infos = vec![info.clone()];
    if options.two_signers {
        let mut second = info_fields.clone();
        second[1] = seq(&[
            signer.issuer.clone(),
            integer(&(signer.serial + 1).to_be_bytes()),
        ]);
        infos.push(seq(&second));
    }

    let mut certs = Vec::new();
    if !options.without_signer_cert {
        certs.push(signer.cert.clone());
    }
    certs.extend(options.extra_certs.iter().cloned());
    certs.sort();
    let mut data_fields = vec![int(1), set_of(&[hash.identifier()]), {
        let mut content = vec![oid("1.2.840.113549.1.7.1")];
        if options.embed_content {
            content.push(explicit(0, &octets(b"embedded")));
        }
        seq(&content)
    }];
    if !certs.is_empty() {
        data_fields.push(tlv(0xA0, &cat(&certs)));
    }
    data_fields.push(tlv(
        0x31,
        &cat(&{
            let mut sorted = infos;
            sorted.sort();
            sorted
        }),
    ));
    let data = seq(&data_fields);
    let wrapped = if options.indefinite_inner {
        let mut again = vec![0xA0, 0x80];
        again.extend_from_slice(&data);
        again.extend_from_slice(&[0, 0]);
        again
    } else {
        explicit(0, &data)
    };
    let outer = seq(&[oid("1.2.840.113549.1.7.2"), wrapped]);
    if options.indefinite {
        // The outer SEQUENCE with an indefinite length, closed by two zero bytes.
        let header = 1 + der::length(content_len(&outer)).len();
        let mut again = vec![0x30, 0x80];
        again.extend_from_slice(&outer[header..]);
        again.extend_from_slice(&[0, 0]);
        return again;
    }
    outer
}

fn content_len(element: &[u8]) -> usize {
    let first = element[1];
    if first < 0x80 {
        first as usize
    } else {
        let count = (first & 0x7f) as usize;
        element[2..2 + count]
            .iter()
            .fold(0, |n, b| (n << 8) | *b as usize)
    }
}

// ---- PDF files -------------------------------------------------------------------------------

/// Digits in the placeholder of a byte range number.
const NUMBER_WIDTH: usize = 10;

fn placeholder_range() -> String {
    let zeros = "0".repeat(NUMBER_WIDTH);
    format!("[{zeros} {zeros} {zeros} {zeros}]")
}

/// Turns text into a PDF string: a literal string when it is ASCII, UTF-16 in hexadecimal when not.
pub fn pdf_string(text: &str) -> String {
    if text.is_ascii() {
        let mut out = String::from("(");
        for c in text.chars() {
            if matches!(c, '(' | ')' | '\\') {
                out.push('\\');
            }
            out.push(c);
        }
        out.push(')');
        out
    } else {
        let mut out = String::from("<FEFF");
        for unit in text.encode_utf16() {
            out.push_str(&format!("{unit:04X}"));
        }
        out.push('>');
        out
    }
}

/// What goes into a signed file.
#[derive(Clone)]
pub struct SignOptions {
    pub sub_filter: String,
    pub reason: Option<String>,
    pub location: Option<String>,
    /// The `/Name` entry: what the file claims about the signer. Nothing reads it.
    pub name: Option<String>,
    /// The time as the file writes it.
    pub time: Option<String>,
    /// Size of the space for the signature, in bytes.
    pub contents_size: usize,
    /// How many bytes at the end of the file the byte range leaves out.
    pub leave_out: usize,
    pub cms: CmsOptions,
    /// The text of the page.
    pub text: String,
    /// The `/DocMDP` level, with a reference in the signature, or none.
    pub doc_mdp: Option<u8>,
    /// A content stream no page uses (object 8), signed with the rest: what a shadow attack hides
    /// and shows later.
    pub hidden_text: Option<String>,
}

impl Default for SignOptions {
    fn default() -> Self {
        SignOptions {
            sub_filter: "adbe.pkcs7.detached".into(),
            reason: Some("I approve this document".into()),
            location: Some("Oslo".into()),
            name: None,
            time: Some("D:20260615120000+02'00'".into()),
            contents_size: 8192,
            leave_out: 0,
            cms: CmsOptions::default(),
            text: "Pay 100 to Alice".into(),
            doc_mdp: None,
            hidden_text: None,
        }
    }
}

/// A signed file, and where the pieces of the signature are in it.
#[derive(Clone)]
pub struct SignedFile {
    pub bytes: Vec<u8>,
    /// The byte range the file was signed with.
    pub range: [usize; 4],
    /// Offset of the `<` that opens the signature data.
    pub contents_at: usize,
    /// Offset after the `>` that closes it.
    pub contents_end: usize,
}

fn object(n: u32, body: &[u8]) -> Vec<u8> {
    let mut out = format!("{n} 0 obj\n").into_bytes();
    out.extend_from_slice(body);
    out.extend_from_slice(b"\nendobj\n");
    out
}

/// A file from numbered objects, with a table of offsets.
pub fn write_file(objects: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = std::collections::BTreeMap::new();
    for (n, body) in objects {
        offsets.insert(*n, out.len());
        out.extend(object(*n, body));
    }
    let size = offsets.keys().max().copied().unwrap_or(0) + 1;
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for n in 1..size {
        match offsets.get(&n) {
            Some(off) => out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes()),
            None => out.extend_from_slice(b"0000000000 65535 f \n"),
        }
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// Adds a revision to `previous`: the objects are written again after it, with their own table,
/// and the table points back at the old one. This is how a PDF is saved without rewriting it.
pub fn append_revision(previous: &[u8], objects: &[(u32, Vec<u8>)], size: u32) -> Vec<u8> {
    let text = String::from_utf8_lossy(previous);
    let at = text.rfind("startxref").expect("a file with a table");
    let prev: usize = text[at + 9..]
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let mut out = previous.to_vec();
    let mut offsets = Vec::new();
    for (n, body) in objects {
        offsets.push((*n, out.len()));
        out.extend(object(*n, body));
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n");
    for (n, off) in &offsets {
        out.extend_from_slice(format!("{n} 1\n{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R /Prev {prev} >>\nstartxref\n{xref}\n%%EOF\n")
            .as_bytes(),
    );
    out
}

fn signature_dictionary(options: &SignOptions, contents_size: usize) -> Vec<u8> {
    let mut dict = format!(
        "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /{} /ByteRange {} /Contents <{}>",
        options.sub_filter,
        placeholder_range(),
        "0".repeat(contents_size * 2)
    );
    if let Some(time) = &options.time {
        dict.push_str(&format!(" /M {}", pdf_string(time)));
    }
    if let Some(reason) = &options.reason {
        dict.push_str(&format!(" /Reason {}", pdf_string(reason)));
    }
    if let Some(location) = &options.location {
        dict.push_str(&format!(" /Location {}", pdf_string(location)));
    }
    if let Some(name) = &options.name {
        dict.push_str(&format!(" /Name {}", pdf_string(name)));
    }
    if let Some(level) = options.doc_mdp {
        dict.push_str(&format!(
            " /Reference [<< /Type /SigRef /TransformMethod /DocMDP /TransformParams << /Type /TransformParams /P {level} /V /1.2 >> >>]"
        ));
    }
    dict.push_str(" >>");
    dict.into_bytes()
}

fn page_content(text: &str) -> Vec<u8> {
    let stream = format!("BT /F1 24 Tf 72 700 Td ({text}) Tj ET");
    format!(
        "<< /Length {} >>\nstream\n{stream}\nendstream",
        stream.len()
    )
    .into_bytes()
}

/// The objects of the unsigned base: 1 catalog, 2 page tree, 3 font, 4 page, 5 signature field,
/// 6 page content, 7 signature dictionary.
fn base_objects(options: &SignOptions) -> Vec<(u32, Vec<u8>)> {
    let mut objects = vec![
        (
            1,
            b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /SigFlags 3 >> >>".to_vec(),
        ),
        (2, b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_vec()),
        (3, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec()),
        (
            4,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 6 0 R /Resources << /Font << /F1 3 0 R >> >> /Annots [5 0 R] >>".to_vec(),
        ),
        (
            5,
            b"<< /Type /Annot /Subtype /Widget /FT /Sig /T (Signature1) /Rect [72 72 272 122] /F 132 /P 4 0 R /V 7 0 R >>".to_vec(),
        ),
        (6, page_content(&options.text)),
        (7, signature_dictionary(options, options.contents_size)),
    ];
    if let Some(hidden) = &options.hidden_text {
        objects.push((8, page_content(hidden)));
    }
    objects
}

/// Signs the last signature dictionary of `file` (the one with the placeholders) and returns the
/// signed file. The byte range runs from the start of the file to the signature data, and from
/// the end of the signature data to the end of the file less `options.leave_out`.
pub fn sign_placeholder(mut file: Vec<u8>, signer: &Identity, options: &SignOptions) -> SignedFile {
    let placeholder = placeholder_range();
    let range_at = find_last(&file, placeholder.as_bytes()).expect("a placeholder");
    let contents_at = range_at
        + placeholder.len()
        + file[range_at + placeholder.len()..]
            .windows(11)
            .position(|w| w == b" /Contents ")
            .expect("contents")
        + 11;
    assert_eq!(file[contents_at], b'<');
    let close = contents_at + file[contents_at..].iter().position(|b| *b == b'>').unwrap();
    let contents_end = close + 1;
    let first_len = contents_at;
    let second_start = contents_end;
    let second_len = file.len() - options.leave_out - contents_end;
    let numbers = format!(
        "[{:0w$} {first_len:0w$} {second_start:0w$} {second_len:0w$}]",
        0,
        w = NUMBER_WIDTH
    );
    file[range_at..range_at + placeholder.len()].copy_from_slice(numbers.as_bytes());
    let digest = options.cms.hash.of(&[
        &file[..first_len],
        &file[second_start..second_start + second_len],
    ]
    .concat());
    let mut der = cms(signer, &options.cms, &digest);
    let room = (close - contents_at - 1) / 2;
    assert!(
        der.len() <= room,
        "the signature ({} bytes) does not fit ({room})",
        der.len()
    );
    der.resize(room, 0);
    let hex: String = der.iter().map(|b| format!("{b:02x}")).collect();
    file[contents_at + 1..close].copy_from_slice(hex.as_bytes());
    SignedFile {
        bytes: file,
        range: [0, first_len, second_start, second_len],
        contents_at,
        contents_end,
    }
}

fn find_last(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

/// A one page file with one signature by `signer`.
pub fn signed_file(signer: &Identity, options: &SignOptions) -> SignedFile {
    sign_placeholder(write_file(&base_objects(options)), signer, options)
}

/// Adds a second signature to a signed file, as a new revision: a new field and a new dictionary,
/// the catalog and the page rewritten to name them. The second signature covers the whole file,
/// the first signature included.
pub fn add_signature(
    previous: &SignedFile,
    signer: &Identity,
    options: &SignOptions,
) -> SignedFile {
    let objects = vec![
        (
            1,
            b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 8 0 R] /SigFlags 3 >> >>".to_vec(),
        ),
        (
            4,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 6 0 R /Resources << /Font << /F1 3 0 R >> >> /Annots [5 0 R 8 0 R] >>".to_vec(),
        ),
        (
            8,
            b"<< /Type /Annot /Subtype /Widget /FT /Sig /T (Signature2) /Rect [300 72 500 122] /F 132 /P 4 0 R /V 9 0 R >>".to_vec(),
        ),
        (9, signature_dictionary(options, options.contents_size)),
    ];
    let file = append_revision(&previous.bytes, &objects, 10);
    sign_placeholder(file, signer, options)
}

/// A revision on top of a file that rewrites the content of the page: what an editor that saves
/// incrementally does, and what an attacker does to a signed file.
pub fn change_text_revision(previous: &[u8], text: &str) -> Vec<u8> {
    append_revision(previous, &[(6, page_content(text))], 10)
}

/// A revision that makes the page show the hidden content stream (object 8) instead of its own: the
/// second half of a shadow attack, on a file signed with `SignOptions::hidden_text`.
pub fn show_hidden_content_revision(previous: &[u8]) -> Vec<u8> {
    append_revision(
        previous,
        &[(
            4,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 8 0 R /Resources << /Font << /F1 3 0 R >> >> /Annots [5 0 R] >>".to_vec(),
        )],
        10,
    )
}

/// The sequence of the file's bytes with `from` replaced by `to` (same length) at the first place
/// it occurs.
pub fn replace_in_place(bytes: &mut [u8], from: &[u8], to: &[u8]) {
    assert_eq!(from.len(), to.len());
    let at = bytes
        .windows(from.len())
        .position(|w| w == from)
        .expect("the text to replace");
    bytes[at..at + from.len()].copy_from_slice(to);
}

/// Writes a different byte range into the dictionary of a signed file (the numbers keep their
/// width, so nothing moves).
pub fn set_byte_range(bytes: &mut [u8], range: [u64; 4]) {
    let placeholder_len = placeholder_range().len();
    let at = bytes
        .windows(11)
        .rposition(|w| w == b"/ByteRange ")
        .expect("a byte range")
        + 11;
    let text = format!(
        "[{:0w$} {:0w$} {:0w$} {:0w$}]",
        range[0],
        range[1],
        range[2],
        range[3],
        w = NUMBER_WIDTH
    );
    assert_eq!(text.len(), placeholder_len, "numbers too wide");
    bytes[at..at + placeholder_len].copy_from_slice(text.as_bytes());
}

/// A key and certificate of a given kind, made once per test run (RSA keys take a moment).
pub fn shared_rsa() -> &'static Identity {
    static KEY: OnceLock<Identity> = OnceLock::new();
    KEY.get_or_init(|| Identity::self_signed(KeyKind::Rsa2048, &CertSpec::new("RSA Signer")))
}

/// A one page file with `count` signature fields whose signatures are not real (a made-up byte
/// range and data): for the limit on how many signatures are described.
pub fn fake_signatures_pdf(count: usize) -> Vec<u8> {
    let mut objects: Vec<(u32, Vec<u8>)> = Vec::new();
    let widgets: Vec<u32> = (0..count as u32).map(|i| 5 + i).collect();
    let refs = |list: &[u32]| {
        list.iter()
            .map(|n| format!("{n} 0 R"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    objects.push((
        1,
        format!(
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [{}] /SigFlags 3 >> >>",
            refs(&widgets)
        )
        .into_bytes(),
    ));
    objects.push((2, b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_vec()));
    objects.push((
        3,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ));
    objects.push((
        4,
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Annots [{}] >>",
            refs(&widgets)
        )
        .into_bytes(),
    ));
    for i in 0..count as u32 {
        objects.push((
            5 + i,
            format!(
                "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Sig{i}) /Rect [10 10 20 20] /F 132 /P 4 0 R /V {} 0 R >>",
                5 + count as u32 + i
            )
            .into_bytes(),
        ));
    }
    for i in 0..count as u32 {
        objects.push((
            5 + count as u32 + i,
            format!(
                "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 100 200 100] /Contents <{}> /M (D:20260101000000Z) >>",
                "30".repeat(40)
            )
            .into_bytes(),
        ));
    }
    write_file(&objects)
}

/// Where a hostile signature puts its very large set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostileSet {
    /// One signed attribute with that many values (a SET OF).
    Values,
    /// That many signed attributes (the `[0]` that stands for a SET OF).
    Attributes,
}

/// A `SignedData` for one signer whose set has `count` different elements, written in descending
/// order, which is the worst case for a decoder that sorts a set with an insertion sort. The
/// signature in it is not real: this is for the limits of the reader, not for its verdicts.
pub fn hostile_set_signature(set: HostileSet, count: usize) -> Vec<u8> {
    let element = |i: usize| {
        // Different, and ordered by their bytes when written big endian after a length.
        let bytes = (i as u32).to_be_bytes();
        match set {
            HostileSet::Values => octets(&bytes),
            HostileSet::Attributes => {
                seq(&[oid("1.2.840.113549.1.9.5"), tlv(0x31, &octets(&bytes))])
            }
        }
    };
    let mut elements: Vec<Vec<u8>> = (0..count).map(element).collect();
    elements.reverse();
    let body = cat(&elements);
    let attrs = match set {
        HostileSet::Values => tlv(0xA0, &seq(&[oid("1.2.840.113549.1.9.4"), tlv(0x31, &body)])),
        HostileSet::Attributes => tlv(0xA0, &body),
    };
    let sha256 = seq(&[oid("2.16.840.1.101.3.4.2.1")]);
    let sid = seq(&[seq(&[]), int(1)]);
    let info = seq(&[
        int(1),
        sid,
        sha256.clone(),
        attrs,
        sha256.clone(),
        octets(&[0u8; 64]),
    ]);
    let data = seq(&[
        int(1),
        tlv(0x31, &sha256),
        seq(&[oid("1.2.840.113549.1.7.1")]),
        tlv(0x31, &info),
    ]);
    seq(&[oid("1.2.840.113549.1.7.2"), tlv(0xA0, &data)])
}

/// The four places of a `SignedData` where a set is written with an implicit tag, and so where
/// the decoder reads the content of an element before it looks at whether the tag is the right one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImplicitField {
    /// The signed attributes of the signer, `[0]`.
    SignedAttributes,
    /// The unsigned attributes of the signer, `[1]`.
    UnsignedAttributes,
    /// The certificates, `[0]`.
    Certificates,
    /// The revocation data, `[1]`.
    Revocations,
}

impl ImplicitField {
    /// The tag number of the field: 0 or 1.
    pub fn number(self) -> u8 {
        match self {
            ImplicitField::SignedAttributes | ImplicitField::Certificates => 0,
            ImplicitField::UnsignedAttributes | ImplicitField::Revocations => 1,
        }
    }

    /// The tag the field has when it is written right: constructed, of the context class.
    pub fn constructed_tag(self) -> u8 {
        0xA0 | self.number()
    }

    /// The same tag with the constructed bit off.
    pub fn primitive_tag(self) -> u8 {
        0x80 | self.number()
    }
}

/// A `SignedData` for one signer in which `field` is written with `tag` and holds an element with a
/// name of `count` different parts, written in descending order: the worst case for a decoder that
/// sorts a set with an insertion sort. With the right tag (see [`ImplicitField::constructed_tag`])
/// the set is in plain sight; with the primitive one it is hidden inside an element that a walk
/// that follows only constructed elements does not enter. The signature in it is not real.
pub fn hidden_set_signature(field: ImplicitField, tag: u8, count: usize) -> Vec<u8> {
    let mut parts: Vec<Vec<u8>> = (0..count)
        .map(|i| seq(&[oid("2.5.4.3"), octets(&(i as u32).to_be_bytes())]))
        .collect();
    parts.reverse();
    let name = seq(&[tlv(0x31, &cat(&parts))]);
    let algorithm = seq(&[oid("1.2.840.113549.1.1.11")]);
    // What the field holds: a certificate, a revocation list or an attribute, each of which has
    // the name (or a value set) as one of its first parts, so that it is decoded in full before
    // anything after it can be found wrong.
    let content = match field {
        ImplicitField::Certificates => seq(&[seq(&[int(1), algorithm, name])]),
        ImplicitField::Revocations => seq(&[seq(&[algorithm, name])]),
        ImplicitField::SignedAttributes | ImplicitField::UnsignedAttributes => {
            let values: Vec<Vec<u8>> = parts.iter().map(|p| octets(p)).collect();
            seq(&[oid("1.2.840.113549.1.9.4"), tlv(0x31, &cat(&values))])
        }
    };
    let hidden = tlv(tag, &content);
    let small = tlv(
        0xA0,
        &seq(&[oid("1.2.840.113549.1.9.4"), tlv(0x31, &octets(&[0u8; 32]))]),
    );
    let sha256 = seq(&[oid("2.16.840.1.101.3.4.2.1")]);
    let sid = seq(&[seq(&[]), int(1)]);
    let mut info_parts = vec![int(1), sid, sha256.clone()];
    let mut data_parts = vec![
        int(1),
        tlv(0x31, &sha256),
        seq(&[oid("1.2.840.113549.1.7.1")]),
    ];
    match field {
        ImplicitField::SignedAttributes => info_parts.push(hidden.clone()),
        _ => info_parts.push(small),
    }
    info_parts.push(sha256);
    info_parts.push(octets(&[0u8; 64]));
    match field {
        ImplicitField::UnsignedAttributes => info_parts.push(hidden),
        ImplicitField::Certificates | ImplicitField::Revocations => data_parts.push(hidden),
        ImplicitField::SignedAttributes => {}
    }
    data_parts.push(tlv(0x31, &seq(&info_parts)));
    seq(&[oid("1.2.840.113549.1.7.2"), tlv(0xA0, &seq(&data_parts))])
}

/// Puts `der`, padded with zeros, in the place of the signature of `file`.
pub fn replace_signature(file: &mut SignedFile, der: &[u8]) {
    let room = (file.contents_end - file.contents_at - 2) / 2;
    assert!(
        der.len() <= room,
        "the signature ({}) does not fit ({room})",
        der.len()
    );
    let mut padded = der.to_vec();
    padded.resize(room, 0);
    let hex: String = padded.iter().map(|b| format!("{b:02x}")).collect();
    file.bytes[file.contents_at + 1..file.contents_end - 1].copy_from_slice(hex.as_bytes());
}
