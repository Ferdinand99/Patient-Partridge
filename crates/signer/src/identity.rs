//! A certificate file read into a key and the certificates that go with it.
//!
//! The file is a PKCS #12 file: a password-protected bag that holds a private key and the
//! certificates (the holder's own and, usually, the ones that issued it). `p12-keystore` opens the
//! bag and checks the password; the rest is done here, on purpose, and not left to the library:
//!
//! - The key and its certificate are paired by their public keys, never by the identifier the file
//!   gives them (some programs leave it out, and a wrong one would pair a key with a certificate
//!   that is not its own).
//! - Only keys that readers check are taken: RSA of 2048 to 4096 bits (PKCS #1 v1.5) and ECDSA on
//!   P-256 and P-384. A key of another kind is refused with its name, not used in a way that
//!   other programs cannot read.
//! - The chain is the holder's certificate and the certificates of the file that lead from it to
//!   the one that signed itself, in order, at most [`MAX_CHAIN`] of them.

use p12_keystore::error::Error as P12Error;
use p12_keystore::Pkcs12Archive;
use partridge_signer_protocol::{
    usage, Certificate, ErrorCode, KeyKind, MAX_CHAIN, MAX_CMS_BYTES, MAX_P12_BYTES,
};
use ring::digest;
use ring::rand::SystemRandom;
use ring::signature::{self, EcdsaKeyPair, RsaKeyPair};
use x509_parser::prelude::*;

use crate::der;

/// Why the signer could not do what was asked, and the words for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignerError {
    pub code: ErrorCode,
    pub message: String,
}

impl SignerError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> SignerError {
        SignerError {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SignerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for SignerError {}

/// The private key, ready to sign.
pub(crate) enum Key {
    Rsa(RsaKeyPair),
    P256(EcdsaKeyPair),
    P384(EcdsaKeyPair),
}

/// A key and the certificates it signs for.
pub struct Identity {
    pub(crate) key: Key,
    /// The certificates in DER, the holder's first, then the ones that issued it.
    pub(crate) chain: Vec<Vec<u8>>,
    /// The issuer of the holder's certificate, as DER (a `Name`).
    pub(crate) issuer: Vec<u8>,
    /// The serial number of the holder's certificate, the content bytes of the INTEGER.
    pub(crate) serial: Vec<u8>,
    /// What is said about the certificate.
    pub(crate) info: Certificate,
}

impl Identity {
    /// What the signer tells the app about the certificate.
    pub fn certificate(&self) -> &Certificate {
        &self.info
    }

    /// The most bytes an ECDSA signature takes in DER: two integers of the size of the curve, each
    /// with a byte in front that keeps it positive, in a sequence.
    pub(crate) fn longest_signature(&self) -> usize {
        match &self.key {
            Key::Rsa(pair) => pair.public().modulus_len(),
            Key::P256(_) => 72,
            Key::P384(_) => 104,
        }
    }
}

/// Reads a certificate file with its password.
pub fn load(p12: &[u8], password: &str) -> Result<Identity, SignerError> {
    if p12.len() > MAX_P12_BYTES {
        return Err(SignerError::new(
            ErrorCode::TooLarge,
            "the certificate file is too big",
        ));
    }
    let archive = Pkcs12Archive::from_pkcs12(p12, password).map_err(open_error)?;
    if archive.keys.is_empty() {
        return Err(SignerError::new(
            ErrorCode::NoKey,
            "the file holds no private key",
        ));
    }
    if archive.certs.len() > 256 {
        return Err(SignerError::new(
            ErrorCode::TooLarge,
            "the file holds too many certificates",
        ));
    }
    let certs: Vec<&[u8]> = archive.certs.iter().map(|c| c.cert.as_der()).collect();
    if certs.is_empty() {
        return Err(SignerError::new(
            ErrorCode::NoKey,
            "the file holds no certificate for the key",
        ));
    }

    // The first key that can be used and has a certificate of its own. What stopped the others
    // is kept, to say why nothing could be used.
    let mut unsupported: Option<SignerError> = None;
    let mut unpaired = false;
    for bag in &archive.keys {
        match pair(bag.key.as_der(), &certs) {
            Ok((key, at)) => return finish(key, at, &certs),
            Err(Paired::Unsupported(why)) => {
                unsupported.get_or_insert(why);
            }
            Err(Paired::NoCertificate) => unpaired = true,
        }
    }
    if unpaired {
        return Err(SignerError::new(
            ErrorCode::KeyMismatch,
            "the private key does not belong to any certificate in the file",
        ));
    }
    Err(unsupported.unwrap_or_else(|| {
        SignerError::new(ErrorCode::KeyUnsupported, "the key cannot be used")
    }))
}

/// What the library's errors mean for the person who typed the password.
fn open_error(error: P12Error) -> SignerError {
    match error {
        // The check of the password failed: the password is wrong, or the file is damaged in a way
        // that looks the same.
        P12Error::MacError(_) | P12Error::UnpadError | P12Error::Pkcs5Error(_) => {
            SignerError::new(ErrorCode::WrongPassword, "the password is wrong")
        }
        P12Error::UnsupportedEncryptionScheme | P12Error::UnsupportedMacAlgorithm => {
            SignerError::new(
                ErrorCode::NotACertificateFile,
                "the file is protected in a way this program cannot open",
            )
        }
        _ => SignerError::new(
            ErrorCode::NotACertificateFile,
            "the file is not a certificate file, or it is damaged",
        ),
    }
}

enum Paired {
    Unsupported(SignerError),
    /// The key is fine, and no certificate of the file is its own.
    NoCertificate,
}

/// The kinds of private key that are told apart before a key is handed to `ring`.
enum Kind {
    Rsa,
    Ec(&'static signature::EcdsaSigningAlgorithm, usize, KeyKind),
    Other(String),
}

const OID_RSA: &str = "1.2.840.113549.1.1.1";
const OID_EC: &str = "1.2.840.10045.2.1";
const OID_P256: &str = "1.2.840.10045.3.1.7";
const OID_P384: &str = "1.3.132.0.34";
const OID_P521: &str = "1.3.132.0.35";

fn oid_content(dotted: &str) -> Vec<u8> {
    der::oid(dotted)[2..].to_vec()
}

/// The kind of a PKCS #8 key and the content of its private key field.
fn classify(pkcs8: &[u8]) -> Option<(Kind, &[u8])> {
    let outer = der::read(pkcs8)?;
    if outer.tag != 0x30 {
        return None;
    }
    let version = der::read(outer.content)?;
    let algorithm = der::read(version.rest)?;
    let private = der::read(algorithm.rest)?;
    if algorithm.tag != 0x30 || private.tag != 0x04 {
        return None;
    }
    let name = der::read(algorithm.content)?;
    if name.tag != 0x06 {
        return None;
    }
    let is = |dotted: &str| name.content == oid_content(dotted).as_slice();
    let kind = if is(OID_RSA) {
        Kind::Rsa
    } else if is(OID_EC) {
        let curve = der::read(name.rest)?;
        let curve_is = |dotted: &str| curve.tag == 0x06 && curve.content == oid_content(dotted);
        if curve_is(OID_P256) {
            Kind::Ec(&signature::ECDSA_P256_SHA256_ASN1_SIGNING, 32, KeyKind::EcdsaP256)
        } else if curve_is(OID_P384) {
            Kind::Ec(&signature::ECDSA_P384_SHA384_ASN1_SIGNING, 48, KeyKind::EcdsaP384)
        } else if curve_is(OID_P521) {
            Kind::Other("ECDSA P-521".to_string())
        } else {
            Kind::Other("ECDSA on another curve".to_string())
        }
    } else if is("1.3.101.112") {
        Kind::Other("Ed25519".to_string())
    } else if is("1.3.101.113") {
        Kind::Other("Ed448".to_string())
    } else if is("1.2.840.113549.1.1.10") {
        Kind::Other("RSA-PSS".to_string())
    } else {
        Kind::Other("an unknown kind of key".to_string())
    };
    Some((kind, private.content))
}

fn unsupported(what: &str) -> Paired {
    Paired::Unsupported(SignerError::new(
        ErrorCode::KeyUnsupported,
        format!("this kind of key is not supported ({what})"),
    ))
}

/// The key, and the place of its certificate among `certs`.
fn pair(pkcs8: &[u8], certs: &[&[u8]]) -> Result<(Key, usize), Paired> {
    let Some((kind, private)) = classify(pkcs8) else {
        return Err(unsupported("the key cannot be read"));
    };
    match kind {
        Kind::Other(name) => Err(unsupported(&name)),
        Kind::Rsa => {
            let pair = RsaKeyPair::from_pkcs8(pkcs8)
                .map_err(|_| unsupported("RSA keys of 2048 to 4096 bits are supported"))?;
            let public = pair.public().as_ref();
            let (n, e) = rsa_numbers(public).ok_or_else(|| unsupported("RSA key"))?;
            if der::bit_length(&n) > 4096 {
                return Err(unsupported("RSA keys of 2048 to 4096 bits are supported"));
            }
            let at = certs
                .iter()
                .position(|der| rsa_of_certificate(der).is_some_and(|c| c == (n.clone(), e.clone())))
                .ok_or(Paired::NoCertificate)?;
            Ok((Key::Rsa(pair), at))
        }
        Kind::Ec(algorithm, size, key_kind) => {
            // The private number of the key. The public part that the file may carry is not used:
            // the certificate that goes with the key is the one whose public key makes a pair.
            let scalar = ec_scalar(private, size).ok_or_else(|| unsupported("EC key"))?;
            let rng = SystemRandom::new();
            for (at, der) in certs.iter().enumerate() {
                let Some(public) = ec_of_certificate(der, key_kind) else {
                    continue;
                };
                if let Ok(pair) =
                    EcdsaKeyPair::from_private_key_and_public_key(algorithm, &scalar, &public, &rng)
                {
                    let key = if key_kind == KeyKind::EcdsaP256 {
                        Key::P256(pair)
                    } else {
                        Key::P384(pair)
                    };
                    return Ok((key, at));
                }
            }
            Err(Paired::NoCertificate)
        }
    }
}

/// The modulus and the exponent of an `RSAPublicKey`, without leading zeros.
fn rsa_numbers(public: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let outer = der::read(public)?;
    let n = der::read(outer.content)?;
    let e = der::read(n.rest)?;
    (outer.tag == 0x30 && n.tag == 0x02 && e.tag == 0x02)
        .then(|| (der::unsigned(n.content).to_vec(), der::unsigned(e.content).to_vec()))
}

fn rsa_of_certificate(der: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let (_, cert) = X509Certificate::from_der(der).ok()?;
    let spki = cert.public_key();
    if spki.algorithm.algorithm.to_id_string() != OID_RSA {
        return None;
    }
    rsa_numbers(&spki.subject_public_key.data)
}

/// The point of an EC public key of the curve, if the certificate holds one.
fn ec_of_certificate(der: &[u8], kind: KeyKind) -> Option<Vec<u8>> {
    let (_, cert) = X509Certificate::from_der(der).ok()?;
    let spki = cert.public_key();
    if spki.algorithm.algorithm.to_id_string() != OID_EC {
        return None;
    }
    let curve = spki
        .algorithm
        .parameters
        .as_ref()?
        .clone()
        .as_oid()
        .ok()?
        .to_id_string();
    let wanted = if kind == KeyKind::EcdsaP256 {
        OID_P256
    } else {
        OID_P384
    };
    (curve == wanted).then(|| spki.subject_public_key.data.to_vec())
}

/// The private number out of an `ECPrivateKey`, as many bytes as the curve has.
fn ec_scalar(private: &[u8], size: usize) -> Option<Vec<u8>> {
    let outer = der::read(private)?;
    let version = der::read(outer.content)?;
    let scalar = der::read(version.rest)?;
    if outer.tag != 0x30 || version.tag != 0x02 || scalar.tag != 0x04 || scalar.content.len() > size
    {
        return None;
    }
    let mut out = vec![0u8; size - scalar.content.len()];
    out.extend_from_slice(scalar.content);
    Some(out)
}

/// The name as people read it: the common name, else the organisation, else the unit.
fn display_name(name: &X509Name<'_>) -> String {
    let text = |value: Option<&AttributeTypeAndValue<'_>>| {
        value.and_then(|a| a.as_str().ok()).map(clean)
    };
    text(name.iter_common_name().next())
        .or_else(|| text(name.iter_organization().next()))
        .or_else(|| text(name.iter_organizational_unit().next()))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "(no name)".to_string())
}

/// Text from a certificate, for the window: no control characters, no characters that change the
/// direction of text or hide it.
fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(*c,
                    '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}'
                    | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The chain from the holder's certificate: each next one is the one that issued the one before,
/// found by name among the certificates of the file. It ends at a certificate that signed itself
/// or at the last one the file has.
fn chain_of(leaf: usize, certs: &[&[u8]]) -> Vec<Vec<u8>> {
    let mut chain = vec![leaf];
    loop {
        if chain.len() as u32 >= MAX_CHAIN {
            break;
        }
        let Some(&last) = chain.last() else { break };
        let Ok((_, current)) = X509Certificate::from_der(certs[last]) else {
            break;
        };
        if current.subject().as_raw() == current.issuer().as_raw() {
            break;
        }
        let issuer = certs.iter().enumerate().find(|(at, der)| {
            !chain.contains(at)
                && X509Certificate::from_der(der)
                    .is_ok_and(|(_, c)| c.subject().as_raw() == current.issuer().as_raw())
        });
        match issuer {
            Some((at, _)) => chain.push(at),
            None => break,
        }
    }
    chain.into_iter().map(|at| certs[at].to_vec()).collect()
}

fn finish(key: Key, leaf: usize, certs: &[&[u8]]) -> Result<Identity, SignerError> {
    let chain = chain_of(leaf, certs);
    let (_, cert) = X509Certificate::from_der(&chain[0]).map_err(|_| {
        SignerError::new(ErrorCode::NotACertificateFile, "the certificate cannot be read")
    })?;
    let key_kind = match &key {
        Key::Rsa(pair) => {
            let (n, _) = rsa_numbers(pair.public().as_ref()).ok_or_else(|| {
                SignerError::new(ErrorCode::KeyUnsupported, "the key cannot be read")
            })?;
            KeyKind::Rsa(der::bit_length(&n))
        }
        Key::P256(_) => KeyKind::EcdsaP256,
        Key::P384(_) => KeyKind::EcdsaP384,
    };
    let mut usage_bits = 0u8;
    if let Ok(Some(extension)) = cert.key_usage() {
        usage_bits |= usage::HAS_EXTENSION;
        if extension.value.digital_signature() {
            usage_bits |= usage::DIGITAL_SIGNATURE;
        }
        if extension.value.non_repudiation() {
            usage_bits |= usage::NON_REPUDIATION;
        }
    }
    let serial_raw = cert.raw_serial();
    let serial_shown = match serial_raw {
        [0, rest @ ..] if !rest.is_empty() => rest,
        other => other,
    };
    let info = Certificate {
        name: display_name(cert.subject()),
        issuer: display_name(cert.issuer()),
        self_signed: cert.subject().as_raw() == cert.issuer().as_raw(),
        serial: hex(serial_shown),
        fingerprint: hex(digest::digest(&digest::SHA256, &chain[0]).as_ref()),
        not_before: cert.validity().not_before.timestamp(),
        not_after: cert.validity().not_after.timestamp(),
        key: key_kind,
        usage: usage_bits,
        chain: chain.len() as u32,
        // Filled in below, when the size of a signature is known.
        signature_bytes: 1,
    };
    let mut identity = Identity {
        key,
        issuer: cert.issuer().as_raw().to_vec(),
        serial: serial_raw.to_vec(),
        chain,
        info,
    };
    let size = crate::cms::signature_size(&identity);
    if size > MAX_CMS_BYTES {
        return Err(SignerError::new(
            ErrorCode::TooLarge,
            "the certificates in the file make a signature that is too big",
        ));
    }
    identity.info.signature_bytes = size as u32;
    Ok(identity)
}
