//! The signature that goes into a PDF: a CMS `SignedData` for a detached signature, in the form
//! that the PDF standard calls `ETSI.CAdES.detached`.
//!
//! What is in it:
//!
//! - one signer, named by the issuer and the serial number of its certificate;
//! - the signed attributes: the content type (`id-data`), the digest of the signed part of the
//!   file, and the ESS `signing-certificate-v2` attribute that binds the signature to the
//!   certificate of the signer (a SHA-256 hash of it, and its issuer and serial number). There is
//!   no signing time in the signature: the time is the `/M` entry of the signature dictionary,
//!   and it is the clock of this computer, which nobody else has confirmed;
//! - the certificates of the chain, the signer's first;
//! - no content: the signature is detached, the digest is over the byte range of the file.
//!
//! The signature is made over the DER of the signed attributes, as the standard says, and the
//! attributes are written with the tag `[0]` in the structure.
//!
//! The `cms` crate of RustCrypto is not used to build this: its signing goes through the `rsa`
//! crate, which has a known timing weakness that has no fix. `ring` signs, and this module only
//! assembles bytes.

use partridge_signer_protocol::{ErrorCode, HashKind};
use ring::digest;
use ring::rand::SystemRandom;
use ring::signature;

use crate::der::{cat, explicit, int, integer_raw, null, octets, oid, seq, set_of, tlv};
use crate::identity::{Identity, Key, SignerError};

const OID_DATA: &str = "1.2.840.113549.1.7.1";
const OID_SIGNED_DATA: &str = "1.2.840.113549.1.7.2";
const OID_CONTENT_TYPE: &str = "1.2.840.113549.1.9.3";
const OID_MESSAGE_DIGEST: &str = "1.2.840.113549.1.9.4";
const OID_SIGNING_CERTIFICATE_V2: &str = "1.2.840.113549.1.9.16.2.47";
const OID_SHA256: &str = "2.16.840.1.101.3.4.2.1";
const OID_SHA384: &str = "2.16.840.1.101.3.4.2.2";
const OID_RSA_SHA256: &str = "1.2.840.113549.1.1.11";
const OID_ECDSA_SHA256: &str = "1.2.840.10045.4.3.2";
const OID_ECDSA_SHA384: &str = "1.2.840.10045.4.3.3";

fn hash_of(identity: &Identity) -> HashKind {
    identity.info.key.hash()
}

fn digest_algorithm(hash: HashKind) -> Vec<u8> {
    seq(&[oid(match hash {
        HashKind::Sha256 => OID_SHA256,
        HashKind::Sha384 => OID_SHA384,
    })])
}

fn signature_algorithm(key: &Key) -> Vec<u8> {
    match key {
        Key::Rsa(_) => seq(&[oid(OID_RSA_SHA256), null()]),
        Key::P256(_) => seq(&[oid(OID_ECDSA_SHA256)]),
        Key::P384(_) => seq(&[oid(OID_ECDSA_SHA384)]),
    }
}

fn attribute(kind: &str, value: Vec<u8>) -> Vec<u8> {
    seq(&[oid(kind), set_of(&[value])])
}

/// The signed attributes as the DER `SET` that is signed.
fn signed_attributes(identity: &Identity, document_digest: &[u8]) -> Vec<u8> {
    // The hash of the certificate is SHA-256, the default of `ESSCertIDv2`, so it is left out.
    let certificate_hash = digest::digest(&digest::SHA256, &identity.chain[0]);
    let issuer_and_serial = seq(&[
        seq(&[explicit(4, &identity.issuer)]),
        integer_raw(&identity.serial),
    ]);
    let signing_certificate = seq(&[seq(&[seq(&[
        octets(certificate_hash.as_ref()),
        issuer_and_serial,
    ])])]);
    set_of(&[
        attribute(OID_CONTENT_TYPE, oid(OID_DATA)),
        attribute(OID_MESSAGE_DIGEST, octets(document_digest)),
        attribute(OID_SIGNING_CERTIFICATE_V2, signing_certificate),
    ])
}

/// The structure around the signature value. `value` is the signature over `attributes`.
fn assemble(identity: &Identity, attributes: &[u8], value: &[u8]) -> Vec<u8> {
    let digest_algorithm = digest_algorithm(hash_of(identity));
    // The attributes are signed as a SET (tag 0x31) and written with the implicit tag [0].
    let mut written = attributes.to_vec();
    written[0] = 0xA0;
    let info = seq(&[
        int(1),
        seq(&[identity.issuer.clone(), integer_raw(&identity.serial)]),
        digest_algorithm.clone(),
        written,
        signature_algorithm(&identity.key),
        octets(value),
    ]);
    // A SET OF certificates is sorted by the bytes of its elements, like every DER set.
    let mut certificates = identity.chain.clone();
    certificates.sort();
    let signed_data = seq(&[
        int(1),
        set_of(&[digest_algorithm]),
        seq(&[oid(OID_DATA)]),
        tlv(0xA0, &cat(&certificates)),
        set_of_one(info),
    ]);
    seq(&[oid(OID_SIGNED_DATA), explicit(0, &signed_data)])
}

/// A SET with one element (the list of signer infos).
fn set_of_one(element: Vec<u8>) -> Vec<u8> {
    tlv(0x31, &element)
}

/// How many bytes a signature takes, for a signature value of the longest size the key makes. The
/// app reserves this much room in the file.
pub(crate) fn signature_size(identity: &Identity) -> usize {
    let digest = vec![0u8; hash_of(identity).len()];
    let attributes = signed_attributes(identity, &digest);
    assemble(
        identity,
        &attributes,
        &vec![0x30; identity.longest_signature()],
    )
    .len()
}

/// Signs the digest of the signed part of the file.
pub fn sign(identity: &Identity, document_digest: &[u8]) -> Result<Vec<u8>, SignerError> {
    if document_digest.len() != hash_of(identity).len() {
        return Err(SignerError::new(
            ErrorCode::InvalidRequest,
            "the digest does not have the length that the key needs",
        ));
    }
    let attributes = signed_attributes(identity, document_digest);
    let rng = SystemRandom::new();
    let failed = || SignerError::new(ErrorCode::Internal, "the key could not sign");
    let value = match &identity.key {
        Key::Rsa(pair) => {
            let mut value = vec![0u8; pair.public().modulus_len()];
            pair.sign(&signature::RSA_PKCS1_SHA256, &rng, &attributes, &mut value)
                .map_err(|_| failed())?;
            value
        }
        Key::P256(pair) | Key::P384(pair) => pair
            .sign(&rng, &attributes)
            .map_err(|_| failed())?
            .as_ref()
            .to_vec(),
    };
    Ok(assemble(identity, &attributes, &value))
}
