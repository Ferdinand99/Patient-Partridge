//! Checking the signature of a PDF signature: the CMS structure, the signed attributes, the
//! certificate of the signer and, against a trust list, the chain behind it.
//!
//! This is the code that reads data a stranger wrote and does arithmetic with keys they chose, so
//! it runs only in the engine, in the sandbox, and it works on bytes it is given (it opens no file
//! and asks for no time: the time of the check is an argument). The structures are read with the
//! RustCrypto crates `cms`, `x509-cert` and `der`; the arithmetic of the signature is `ring`'s.
//! Nothing here goes to the network: a certificate is not checked for revocation, a time stamp is not
//! checked, and the chain is built only from the certificates in the signature and the trust list.
//!
//! What the module promises:
//! - [`parse`] reads exactly one DER encoded `SignedData` with one signer. Anything else is
//!   refused with a reason. A signature with another encoding (BER with indefinite lengths), one
//!   that the parser refuses for any other reason, and one that is bigger or more tangled than the
//!   limits below are *unsupported* ("not checked"), never guessed at and never called broken: the
//!   reader stopped reading, it did not find the signature false.
//! - Before the `der` crate sees a byte, [`check_shape`] walks the structure with limits on depth
//!   and on the number of elements in a set. The crate sorts every set it decodes with an insertion
//!   sort, so a set of some hundred thousand elements in a megabyte would take minutes. The walk
//!   does not trust the constructed bit of a context tag: the crate decodes the content of a field
//!   it expects (the certificates, the revocation data, the signed and unsigned attributes) and
//!   looks at that bit afterwards, so a primitive tag cannot be taken to hide nothing. A primitive
//!   element that is not of the universal class is refused above [`MAX_OPAQUE`] bytes.
//! - In the engine process the decoding itself runs on a thread of its own ([`start_parser`]) and
//!   is waited for only until the time of the check is up ([`parse_within`]), so that a shape
//!   nobody thought of cannot hold up the pages of the document.
//! - [`Cms::verify`] checks the signature over the signed attributes with the key of the signer's
//!   certificate, and the attributes themselves (content type, one message digest, the binding to
//!   the signer's certificate when the signer wrote it). It does not look at the file.
//! - [`Cms::digest_matches`] compares the digest the signer signed with a digest of the file.
//! - [`Cms::trust`] says whether the signer's certificate leads to a certificate on a trust list.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cms::content_info::ContentInfo;
use cms::signed_data::{SignedData, SignerIdentifier, SignerInfo};
use const_oid::ObjectIdentifier as Oid;
use der::asn1::OctetString;
use der::{Any, Decode, Encode, Tag, Tagged};
use partridge_protocol::{CertificateTime, SignatureReason};
use ring::{digest, signature};
use spki::SubjectPublicKeyInfoOwned;
use x509_cert::ext::pkix::{BasicConstraints, KeyUsage, KeyUsages};
use x509_cert::name::Name;
use x509_cert::Certificate;

const ID_DATA: Oid = Oid::new_unwrap("1.2.840.113549.1.7.1");
const ID_SIGNED_DATA: Oid = Oid::new_unwrap("1.2.840.113549.1.7.2");
const ID_CONTENT_TYPE: Oid = Oid::new_unwrap("1.2.840.113549.1.9.3");
const ID_MESSAGE_DIGEST: Oid = Oid::new_unwrap("1.2.840.113549.1.9.4");
const ID_SIGNING_CERT: Oid = Oid::new_unwrap("1.2.840.113549.1.9.16.2.12");
const ID_SIGNING_CERT_V2: Oid = Oid::new_unwrap("1.2.840.113549.1.9.16.2.47");

const ID_SHA256: Oid = Oid::new_unwrap("2.16.840.1.101.3.4.2.1");
const ID_SHA384: Oid = Oid::new_unwrap("2.16.840.1.101.3.4.2.2");
const ID_SHA512: Oid = Oid::new_unwrap("2.16.840.1.101.3.4.2.3");

const ID_RSA: Oid = Oid::new_unwrap("1.2.840.113549.1.1.1");
const ID_RSA_SHA256: Oid = Oid::new_unwrap("1.2.840.113549.1.1.11");
const ID_RSA_SHA384: Oid = Oid::new_unwrap("1.2.840.113549.1.1.12");
const ID_RSA_SHA512: Oid = Oid::new_unwrap("1.2.840.113549.1.1.13");
const ID_EC_PUBLIC_KEY: Oid = Oid::new_unwrap("1.2.840.10045.2.1");
const ID_ECDSA_SHA256: Oid = Oid::new_unwrap("1.2.840.10045.4.3.2");
const ID_ECDSA_SHA384: Oid = Oid::new_unwrap("1.2.840.10045.4.3.3");
const ID_ECDSA_SHA512: Oid = Oid::new_unwrap("1.2.840.10045.4.3.4");
const ID_P256: Oid = Oid::new_unwrap("1.2.840.10045.3.1.7");
const ID_P384: Oid = Oid::new_unwrap("1.3.132.0.34");
const ID_ED25519: Oid = Oid::new_unwrap("1.3.101.112");

const ID_COMMON_NAME: Oid = Oid::new_unwrap("2.5.4.3");
const ID_ORGANISATION: Oid = Oid::new_unwrap("2.5.4.10");
const ID_ORG_UNIT: Oid = Oid::new_unwrap("2.5.4.11");

/// Most certificates read out of one signature; more are left out.
const MAX_CERTIFICATES: usize = 64;

/// Longest chain built from signer to trust list, the signer's certificate included.
const MAX_CHAIN: usize = 8;

/// Deepest nesting of constructed elements in a signature. A signature with certificates and
/// revocation data is about a dozen deep.
const MAX_DEPTH: usize = 32;

/// Most elements in one set (a SET, or a context tag that may stand for one). The `der` crate sorts
/// them with an insertion sort. A real signature has a handful; a bundle of certificates some tens.
const MAX_SET_ELEMENTS: usize = 256;

/// Most work the sorts of all sets of one signature may add up to: the sum, over all sets, of the
/// number of elements added so far for every element read. About a million comparisons, which take
/// a few milliseconds.
const MAX_SET_WORK: u64 = 1 << 20;

/// Longest primitive element that is not of the universal class (a key identifier, a unique
/// identifier of a certificate). The `der` crate may decode the content of such an element as a
/// whole structure before it notices that the tag is wrong, so what is inside one must stay small.
const MAX_OPAQUE: usize = 1024;

/// The longest the thread that decodes a signature is waited for when no other time is given.
const PARSE_LIMIT: Duration = Duration::from_secs(10);

/// The digest algorithms that are checked. SHA-1 is known and refused: collisions are practical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DigestAlg {
    Sha256,
    Sha384,
    Sha512,
}

impl DigestAlg {
    fn from_oid(oid: &Oid) -> Option<DigestAlg> {
        if *oid == ID_SHA256 {
            Some(DigestAlg::Sha256)
        } else if *oid == ID_SHA384 {
            Some(DigestAlg::Sha384)
        } else if *oid == ID_SHA512 {
            Some(DigestAlg::Sha512)
        } else {
            None
        }
    }

    pub fn ring(self) -> &'static digest::Algorithm {
        match self {
            DigestAlg::Sha256 => &digest::SHA256,
            DigestAlg::Sha384 => &digest::SHA384,
            DigestAlg::Sha512 => &digest::SHA512,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            DigestAlg::Sha256 => "SHA-256",
            DigestAlg::Sha384 => "SHA-384",
            DigestAlg::Sha512 => "SHA-512",
        }
    }
}

/// What a certificate says about its signer and itself, ready to show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CertDetails {
    pub signer: Option<String>,
    pub organisation: Option<String>,
    pub issuer: Option<String>,
    pub serial: Option<String>,
    pub fingerprint: Option<String>,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub key: Option<String>,
}

/// The answer to "does this lead to a certificate we trust".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trust {
    /// It does, up to the certificate of this name on the list.
    Anchored(String),
    /// It does not (or there is no list).
    Not(SignatureReason),
}

/// The certificates the app trusts without asking: the trust list that ships with it.
#[derive(Debug, Clone, Default)]
pub struct TrustList {
    certs: Vec<Certificate>,
}

impl TrustList {
    /// A list with nothing on it: no signer is trusted.
    pub fn empty() -> TrustList {
        TrustList::default()
    }

    /// The list that ships with the app: the certificates in `trust/anchors.pem`, read when the
    /// engine is built. A list that cannot be read is an empty one (a test checks that the one in
    /// the repository can be read), which trusts nobody.
    pub fn shipped() -> TrustList {
        TrustList::from_pem(include_bytes!("../trust/anchors.pem")).unwrap_or_default()
    }

    /// Reads a list from PEM text: any number of `CERTIFICATE` blocks, with text between them.
    pub fn from_pem(text: &[u8]) -> Result<TrustList, String> {
        // Text outside the blocks is ignored: the list may carry comments.
        if !text.windows(11).any(|w| w == b"-----BEGIN ") {
            return Ok(TrustList::empty());
        }
        let certs =
            Certificate::load_pem_chain(text).map_err(|e| format!("the trust list: {e}"))?;
        Ok(TrustList { certs })
    }

    /// A list of certificates in DER.
    pub fn from_der(certs: &[Vec<u8>]) -> Result<TrustList, String> {
        let mut out = Vec::new();
        for der in certs {
            out.push(Certificate::from_der(der).map_err(|e| format!("a trust anchor: {e}"))?);
        }
        Ok(TrustList { certs: out })
    }

    pub fn len(&self) -> usize {
        self.certs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.certs.is_empty()
    }
}

/// Uses everything the check uses once, before the sandbox is applied, so that nothing is set up
/// for the first time (processor features, tables) when the engine can no longer ask the system
/// for anything. The result is of no interest.
pub fn warm_up() {
    let _ = digest::digest(&digest::SHA256, b"warm up");
    for algorithm in [
        &signature::ECDSA_P256_SHA256_ASN1 as &dyn signature::VerificationAlgorithm,
        &signature::ED25519,
        &signature::RSA_PKCS1_2048_8192_SHA256,
    ] {
        let _ =
            signature::UnparsedPublicKey::new(algorithm, [4u8; 65]).verify(b"warm up", &[0u8; 64]);
    }
}

/// A signature read out of its CMS wrapper.
pub struct Cms {
    der: Vec<u8>,
    certs: Vec<Certificate>,
    signer: Option<usize>,
    info: SignerInfo,
    digest: Option<DigestAlg>,
}

/// The length of the DER element at the start of `data`, header included. `None` when the length
/// is not a definite one (BER), is too long to be real, or the element does not fit.
pub fn der_element_len(data: &[u8]) -> Option<usize> {
    let (header, len) = header(data)?;
    let total = header.checked_add(len)?;
    (total <= data.len()).then_some(total)
}

/// The size of the header of a DER element and the length of its content.
fn header(data: &[u8]) -> Option<(usize, usize)> {
    let first = *data.get(1)?;
    if data.first().is_none_or(|tag| tag & 0x1f == 0x1f) {
        // A tag of more than one byte is not used by anything read here.
        return None;
    }
    if first < 0x80 {
        return Some((2, first as usize));
    }
    let count = (first & 0x7f) as usize;
    if count == 0 || count > 4 {
        return None;
    }
    let bytes = data.get(2..2 + count)?;
    // DER wants the shortest form.
    if bytes[0] == 0 {
        return None;
    }
    let len = bytes.iter().fold(0usize, |n, b| (n << 8) | *b as usize);
    if len < 0x80 {
        return None;
    }
    Some((2 + count, len))
}

/// One element: its tag, its content, and what follows it.
fn tlv(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (head, len) = header(data)?;
    let end = head.checked_add(len)?;
    if end > data.len() {
        return None;
    }
    Some((data[0], &data[head..end], &data[end..]))
}

/// Walks the structure of a DER element without decoding it, and refuses what would take the
/// decoder too long: nesting deeper than [`MAX_DEPTH`], a set with more than [`MAX_SET_ELEMENTS`]
/// elements, and sets that together would cost more than [`MAX_SET_WORK`]. The walk is a loop, not
/// a recursion, and takes time in proportion to the size of `der`.
///
/// The same strict reading of lengths as [`der_element_len`] is used all the way down: a length in
/// the indefinite form (BER) is [`SignatureReason::EncodingUnsupported`], anything else that is not
/// well formed is [`SignatureReason::Unreadable`], and a limit that is passed is
/// [`SignatureReason::TooLarge`]. All three are "not checked".
pub fn check_shape(der: &[u8]) -> Result<(), SignatureReason> {
    use SignatureReason::*;
    // One frame for each constructed element being walked: what is left of its content, how many
    // elements it has had so far, and whether it is a set.
    struct Frame<'a> {
        rest: &'a [u8],
        count: usize,
        set: bool,
    }
    let mut stack = vec![Frame {
        rest: der,
        count: 0,
        set: false,
    }];
    let mut work = 0u64;
    while let Some(frame) = stack.last_mut() {
        if frame.rest.is_empty() {
            stack.pop();
            continue;
        }
        let Some((tag, content, after)) = tlv(frame.rest) else {
            // An indefinite length is the way BER writes an element; say so.
            return Err(if frame.rest.get(1) == Some(&0x80) {
                EncodingUnsupported
            } else {
                Unreadable
            });
        };
        frame.rest = after;
        frame.count += 1;
        if frame.set {
            work += frame.count as u64;
            if frame.count > MAX_SET_ELEMENTS || work > MAX_SET_WORK {
                return Err(TooLarge);
            }
        }
        if tag & 0x20 == 0 {
            // A primitive element has nothing to walk, but `der` decodes the content of a field it
            // expects before it looks at the constructed bit of the tag, so a primitive element of
            // a context class may be read as the set it stands for. Only a small one is let through.
            if tag & 0xc0 != 0 && content.len() > MAX_OPAQUE {
                return Err(TooLarge);
            }
        } else {
            if stack.len() >= MAX_DEPTH {
                return Err(TooLarge);
            }
            // A SET, or a tag of the context: [0] and [1] stand for a set in a CMS (the
            // certificates, the signed attributes), and an explicit tag has one element.
            let set = tag == 0x31 || tag & 0xc0 == 0x80;
            stack.push(Frame {
                rest: content,
                count: 0,
                set,
            });
        }
    }
    Ok(())
}

/// Reads the CMS wrapper of a signature. `der` is exactly one element.
pub fn parse(der: &[u8]) -> Result<Cms, SignatureReason> {
    check_form(der)?;
    decode(der)
}

/// What is checked without decoding: one definite element, and a structure within the limits.
fn check_form(der: &[u8]) -> Result<(), SignatureReason> {
    if der_element_len(der) != Some(der.len()) {
        return Err(SignatureReason::EncodingUnsupported);
    }
    check_shape(der)
}

/// One request for the thread that decodes signatures, and where its answer goes.
struct Job {
    der: Vec<u8>,
    answer: mpsc::SyncSender<Result<Cms, SignatureReason>>,
}

/// The thread that decodes signatures, once it was started.
struct Parser {
    jobs: Mutex<mpsc::Sender<Job>>,
    /// True from the moment a job is handed over until the thread is done with it. A thread that
    /// is still busy with a job nobody waits for any more is not given another.
    busy: Arc<AtomicBool>,
}

static PARSER: OnceLock<Parser> = OnceLock::new();

/// Test builds only: how long the thread that decodes signatures waits before it starts on a job.
#[cfg(debug_assertions)]
static PARSE_DELAY_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Test builds only: makes the thread that decodes signatures slow, so that a test can show that
/// the caller does not wait for it.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn set_parse_delay_for_tests(millis: u64) {
    PARSE_DELAY_MS.store(millis, Ordering::Relaxed);
}

/// Starts the thread that decodes signatures for [`parse_within`]. It has to run before the
/// sandbox is applied: a process that is locked in cannot start a thread. Returns false when the
/// thread could not be started, and then signatures are decoded on the thread that asks.
pub fn start_parser() -> bool {
    if PARSER.get().is_some() {
        return true;
    }
    let (jobs, queue) = mpsc::channel::<Job>();
    let busy = Arc::new(AtomicBool::new(false));
    let (ready, started) = mpsc::channel();
    let flag = busy.clone();
    let spawned = std::thread::Builder::new()
        .name("signature-parser".into())
        // The decoder recurses with the depth of the structure, which is limited; this leaves room.
        .stack_size(4 * 1024 * 1024)
        .spawn(move || {
            let _ = ready.send(());
            for job in queue {
                #[cfg(debug_assertions)]
                std::thread::sleep(Duration::from_millis(
                    PARSE_DELAY_MS.load(Ordering::Relaxed),
                ));
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decode(&job.der)))
                        .unwrap_or(Err(SignatureReason::Unreadable));
                flag.store(false, Ordering::Release);
                // Nobody may be waiting any more; the answer is then dropped.
                let _ = job.answer.send(result);
            }
        });
    // The thread says when it has started: a thread that is still starting up when the system
    // call filter is put on every thread would be killed halfway.
    if spawned.is_err() || started.recv().is_err() {
        return false;
    }
    PARSER
        .set(Parser {
            jobs: Mutex::new(jobs),
            busy,
        })
        .is_ok()
}

/// As [`parse`], but the decoding is waited for only until `deadline` (at most [`PARSE_LIMIT`] when
/// there is none), when the thread of [`start_parser`] runs. A signature that takes longer is
/// refused as too large, and the thread is left to finish by itself: the caller does not wait for
/// it, so no shape of a signature can hold up the pages of the document. While the thread is busy
/// with such a signature the next ones are refused at once.
pub fn parse_within(der: &[u8], deadline: Option<Instant>) -> Result<Cms, SignatureReason> {
    use SignatureReason::*;
    check_form(der)?;
    let Some(parser) = PARSER.get() else {
        return decode(der);
    };
    if parser.busy.swap(true, Ordering::AcqRel) {
        return Err(TooLarge);
    }
    let (answer, reply) = mpsc::sync_channel(1);
    let sent = parser
        .jobs
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .send(Job {
            der: der.to_vec(),
            answer,
        });
    if sent.is_err() {
        parser.busy.store(false, Ordering::Release);
        return Err(Unreadable);
    }
    let wait = deadline
        .map(|d| d.saturating_duration_since(Instant::now()))
        .unwrap_or(PARSE_LIMIT)
        .min(PARSE_LIMIT);
    match reply.recv_timeout(wait) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(TooLarge),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(Unreadable),
    }
}

/// Decodes a signature that passed [`check_form`].
fn decode(der: &[u8]) -> Result<Cms, SignatureReason> {
    use SignatureReason::*;
    let info = ContentInfo::from_der(der).map_err(|_| Unreadable)?;
    if info.content_type != ID_SIGNED_DATA {
        return Err(Unreadable);
    }
    let data: SignedData = info.content.decode_as().map_err(|_| Unreadable)?;
    // The signature of a PDF is detached: the content is the file.
    if data.encap_content_info.econtent.is_some() {
        return Err(AttributesInvalid);
    }
    let mut signers = data.signer_infos.0.iter();
    let Some(signer_info) = signers.next() else {
        return Err(Unreadable);
    };
    if signers.next().is_some() {
        return Err(SubFilterUnsupported);
    }
    let mut certs = Vec::new();
    if let Some(set) = &data.certificates {
        for choice in set.0.iter() {
            if let cms::cert::CertificateChoices::Certificate(cert) = choice {
                if certs.len() < MAX_CERTIFICATES {
                    certs.push(cert.clone());
                }
            }
        }
    }
    let signer = certs
        .iter()
        .position(|cert| matches_signer(cert, &signer_info.sid));
    Ok(Cms {
        der: der.to_vec(),
        digest: DigestAlg::from_oid(&signer_info.digest_alg.oid),
        certs,
        signer,
        info: signer_info.clone(),
    })
}

fn matches_signer(cert: &Certificate, sid: &SignerIdentifier) -> bool {
    match sid {
        SignerIdentifier::IssuerAndSerialNumber(wanted) => {
            cert.tbs_certificate.serial_number == wanted.serial_number
                && same_name(&cert.tbs_certificate.issuer, &wanted.issuer)
        }
        SignerIdentifier::SubjectKeyIdentifier(wanted) => {
            match cert
                .tbs_certificate
                .get::<x509_cert::ext::pkix::SubjectKeyIdentifier>()
            {
                Ok(Some((_, id))) => id.0.as_bytes() == wanted.0.as_bytes(),
                _ => false,
            }
        }
    }
}

fn same_name(a: &Name, b: &Name) -> bool {
    match (a.to_der(), b.to_der()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

impl Cms {
    pub fn digest_alg(&self) -> Option<DigestAlg> {
        self.digest
    }

    pub fn certificate_count(&self) -> u8 {
        self.certs.len().min(u8::MAX as usize) as u8
    }

    fn signer_cert(&self) -> Option<&Certificate> {
        self.signer.map(|i| &self.certs[i])
    }

    /// The digest the signer signed: the `messageDigest` attribute. `None` when it is missing,
    /// present twice, or not a byte string.
    fn message_digest(&self) -> Option<Vec<u8>> {
        let attrs = self.info.signed_attrs.as_ref()?;
        let mut found = None;
        for attr in attrs.iter().filter(|a| a.oid == ID_MESSAGE_DIGEST) {
            let mut values = attr.values.iter();
            let (Some(value), None) = (values.next(), values.next()) else {
                return None;
            };
            if found.is_some() {
                return None;
            }
            found = Some(value.decode_as::<OctetString>().ok()?.as_bytes().to_vec());
        }
        found
    }

    /// True when `computed` is the digest the signer signed.
    pub fn digest_matches(&self, computed: &[u8]) -> bool {
        self.message_digest().is_some_and(|d| d == computed)
    }

    /// What the certificate of the signer says, as far as it can be read.
    pub fn details(&self) -> CertDetails {
        self.signer_cert().map(details_of).unwrap_or_default()
    }

    /// The signer's certificate against the clock.
    pub fn certificate_time(&self, now: i64) -> CertificateTime {
        match self.signer_cert() {
            Some(cert) => time_status(cert, now),
            None => CertificateTime::Unknown,
        }
    }

    /// Checks the signature and the attributes it signs. `Ok` means: the holder of the key of
    /// the certificate in the signature signed these attributes. The digest of the file is a
    /// separate question ([`Cms::digest_matches`]).
    pub fn verify(&self) -> Result<(), SignatureReason> {
        use SignatureReason::*;
        let digest = self.digest.ok_or(DigestUnsupported)?;
        let cert = self.signer_cert().ok_or(NoSignerCertificate)?;
        let attrs = self.info.signed_attrs.as_ref().ok_or(NoSignedAttributes)?;

        // The attributes the signature relies on: the type of the content (once, and data) and
        // the digest (once).
        let mut types = attrs.iter().filter(|a| a.oid == ID_CONTENT_TYPE);
        let content_type = types.next().ok_or(AttributesInvalid)?;
        if types.next().is_some() {
            return Err(AttributesInvalid);
        }
        let mut values = content_type.values.iter();
        match (values.next(), values.next()) {
            (Some(value), None) if value.decode_as::<Oid>().ok() == Some(ID_DATA) => {}
            _ => return Err(AttributesInvalid),
        }
        let wanted = self.message_digest().ok_or(AttributesInvalid)?;
        if wanted.len() != digest.ring().output_len() {
            return Err(AttributesInvalid);
        }

        // The key, the algorithm, and whether this reader can check them.
        let spki = &cert.tbs_certificate.subject_public_key_info;
        let scheme = scheme(spki, &self.info.signature_algorithm.oid, Some(digest))?;

        // The signer's certificate is the one the signer meant, when the signer said which.
        if !self.signing_certificate_matches(cert)? {
            return Err(CertificateMismatch);
        }

        // The signature is over the attributes in their DER form (a SET), which is not the form
        // they are written in (a [0] tag). The form that was written must be the one the signer
        // signed: the bytes are put back and looked for, so a signature over some other
        // arrangement of the same attributes is not accepted.
        let signed = attrs.to_der().map_err(|_| EncodingUnsupported)?;
        let mut written = signed.clone();
        written[0] = 0xA0;
        if !self.der.windows(written.len()).any(|w| w == written) {
            return Err(EncodingUnsupported);
        }
        signature::UnparsedPublicKey::new(scheme.algorithm, scheme.key)
            .verify(&signed, self.info.signature.as_bytes())
            .map_err(|_| SignatureInvalid)
    }

    /// The binding of the signature to one certificate (RFC 5035): when the signed attributes name
    /// a certificate by its hash, it is the signer's. `Ok(true)` also when nothing is named.
    fn signing_certificate_matches(&self, cert: &Certificate) -> Result<bool, SignatureReason> {
        let attrs = self
            .info
            .signed_attrs
            .as_ref()
            .ok_or(SignatureReason::Unreadable)?;
        let der = cert.to_der().map_err(|_| SignatureReason::Unreadable)?;
        for attr in attrs.iter() {
            let v2 = attr.oid == ID_SIGNING_CERT_V2;
            if !v2 && attr.oid != ID_SIGNING_CERT {
                continue;
            }
            let mut values = attr.values.iter();
            let (Some(value), None) = (values.next(), values.next()) else {
                return Ok(false);
            };
            let value = value.to_der().map_err(|_| SignatureReason::Unreadable)?;
            let Some(named) = first_cert_id(&value, v2) else {
                return Ok(false);
            };
            let computed = digest::digest(named.0, &der);
            return Ok(computed.as_ref() == named.1);
        }
        Ok(true)
    }

    /// Whether the signer's certificate leads to a certificate on `list`, at the time `now`.
    ///
    /// Only what is in the signature and on the list is used: no certificate is fetched. A
    /// certificate on the list is trusted as it stands, so a leaf that is itself on the list is
    /// trusted. Names are compared byte for byte, a certificate between the signer and the list
    /// must say it is a CA, and the signer's certificate must be allowed to sign (when it says what
    /// it is for). Revocation, policies, name constraints and the extended key usage are not
    /// looked at.
    pub fn trust(&self, list: &TrustList, now: i64) -> Trust {
        use SignatureReason::*;
        if list.is_empty() {
            return Trust::Not(NoTrustList);
        }
        let Some(signer) = self.signer_cert() else {
            return Trust::Not(NotInTrustList);
        };
        let mut path: Vec<&Certificate> = vec![signer];
        let anchor = loop {
            let current = *path.last().expect("the path has the signer");
            let der = current.to_der().ok();
            // The certificate itself is on the list.
            if let Some(anchor) = list
                .certs
                .iter()
                .find(|a| a.to_der().ok().is_some() && a.to_der().ok() == der)
            {
                break anchor;
            }
            // Its issuer is on the list.
            if let Some(anchor) = list.certs.iter().find(|a| {
                same_name(&a.tbs_certificate.subject, &current.tbs_certificate.issuer)
                    && signed_by(current, &a.tbs_certificate.subject_public_key_info)
            }) {
                break anchor;
            }
            if path.len() >= MAX_CHAIN {
                return Trust::Not(NotInTrustList);
            }
            // Or its issuer is in the signature, and is a CA that signed it.
            let next = self.certs.iter().find(|c| {
                !path.iter().any(|p| std::ptr::eq(*p, *c))
                    && same_name(&c.tbs_certificate.subject, &current.tbs_certificate.issuer)
                    && is_ca(c)
                    && signed_by(current, &c.tbs_certificate.subject_public_key_info)
            });
            match next {
                Some(next) => path.push(next),
                None => return Trust::Not(NotInTrustList),
            }
        };
        // The chain is complete; now the clock and what the signer's certificate is for.
        for cert in path.iter().copied().chain(std::iter::once(anchor)) {
            match time_status(cert, now) {
                CertificateTime::Expired => return Trust::Not(CertificateExpired),
                CertificateTime::NotYetValid => return Trust::Not(CertificateNotYetValid),
                _ => {}
            }
        }
        if !may_sign(signer) {
            return Trust::Not(KeyUsage);
        }
        Trust::Anchored(name_of(&anchor.tbs_certificate.subject).unwrap_or_default())
    }
}

/// The hash algorithm and the hash of the first certificate named by a signing-certificate
/// attribute. `value` is the DER of the attribute value. `v2` selects the newer form, which may name
/// its own hash algorithm (SHA-256 when it does not); the older form is always SHA-1.
fn first_cert_id(value: &[u8], v2: bool) -> Option<(&'static digest::Algorithm, Vec<u8>)> {
    // SigningCertificate ::= SEQUENCE { certs SEQUENCE OF ESSCertID, policies OPTIONAL }
    let (tag, outer, _) = tlv(value)?;
    if tag != 0x30 {
        return None;
    }
    let (tag, ids, _) = tlv(outer)?;
    if tag != 0x30 {
        return None;
    }
    let (tag, id, _) = tlv(ids)?;
    if tag != 0x30 {
        return None;
    }
    let mut alg = if v2 {
        &digest::SHA256
    } else {
        &digest::SHA1_FOR_LEGACY_USE_ONLY
    };
    let mut rest = id;
    if v2 {
        // An algorithm identifier comes first when it is not the default.
        if rest.first() == Some(&0x30) {
            let (_, ident, after) = tlv(rest)?;
            let (tag, oid, _) = tlv(ident)?;
            if tag != 0x06 {
                return None;
            }
            let oid = Oid::from_bytes(oid).ok()?;
            alg = DigestAlg::from_oid(&oid)?.ring();
            rest = after;
        }
    }
    let (tag, hash, _) = tlv(rest)?;
    (tag == 0x04).then(|| (alg, hash.to_vec()))
}

/// The key and the algorithm to check a signature with.
struct Scheme<'a> {
    algorithm: &'static dyn signature::VerificationAlgorithm,
    key: &'a [u8],
}

/// Works out how to check a signature made with the key `spki` and the algorithm `sig_oid`.
/// `digest` is the digest of the signed data when the algorithm identifier does not name one.
fn scheme<'a>(
    spki: &'a SubjectPublicKeyInfoOwned,
    sig_oid: &Oid,
    digest: Option<DigestAlg>,
) -> Result<Scheme<'a>, SignatureReason> {
    use SignatureReason::*;
    let key = spki.subject_public_key.raw_bytes();
    let hash_of = |named: DigestAlg| -> Result<DigestAlg, SignatureReason> {
        match digest {
            Some(d) if d != named => Err(AttributesInvalid),
            _ => Ok(named),
        }
    };
    if spki.algorithm.oid == ID_RSA {
        match rsa_bits(key) {
            Some(bits) if bits < 2048 => return Err(KeyTooSmall),
            Some(bits) if bits > 8192 => return Err(AlgorithmUnsupported),
            None => return Err(Unreadable),
            _ => {}
        }
        let hash = if *sig_oid == ID_RSA {
            digest.ok_or(AlgorithmUnsupported)?
        } else if *sig_oid == ID_RSA_SHA256 {
            hash_of(DigestAlg::Sha256)?
        } else if *sig_oid == ID_RSA_SHA384 {
            hash_of(DigestAlg::Sha384)?
        } else if *sig_oid == ID_RSA_SHA512 {
            hash_of(DigestAlg::Sha512)?
        } else {
            return Err(AlgorithmUnsupported);
        };
        let algorithm: &'static dyn signature::VerificationAlgorithm = match hash {
            DigestAlg::Sha256 => &signature::RSA_PKCS1_2048_8192_SHA256,
            DigestAlg::Sha384 => &signature::RSA_PKCS1_2048_8192_SHA384,
            DigestAlg::Sha512 => &signature::RSA_PKCS1_2048_8192_SHA512,
        };
        return Ok(Scheme { algorithm, key });
    }
    if spki.algorithm.oid == ID_EC_PUBLIC_KEY {
        let hash = if *sig_oid == ID_EC_PUBLIC_KEY {
            digest.ok_or(AlgorithmUnsupported)?
        } else if *sig_oid == ID_ECDSA_SHA256 {
            hash_of(DigestAlg::Sha256)?
        } else if *sig_oid == ID_ECDSA_SHA384 {
            hash_of(DigestAlg::Sha384)?
        } else if *sig_oid == ID_ECDSA_SHA512 {
            hash_of(DigestAlg::Sha512)?
        } else {
            return Err(AlgorithmUnsupported);
        };
        let curve = spki
            .algorithm
            .parameters
            .as_ref()
            .and_then(|p| p.decode_as::<Oid>().ok())
            .ok_or(AlgorithmUnsupported)?;
        let algorithm: &'static dyn signature::VerificationAlgorithm = match (curve, hash) {
            (c, DigestAlg::Sha256) if c == ID_P256 => &signature::ECDSA_P256_SHA256_ASN1,
            (c, DigestAlg::Sha384) if c == ID_P256 => &signature::ECDSA_P256_SHA384_ASN1,
            (c, DigestAlg::Sha256) if c == ID_P384 => &signature::ECDSA_P384_SHA256_ASN1,
            (c, DigestAlg::Sha384) if c == ID_P384 => &signature::ECDSA_P384_SHA384_ASN1,
            _ => return Err(AlgorithmUnsupported),
        };
        return Ok(Scheme { algorithm, key });
    }
    if spki.algorithm.oid == ID_ED25519 && *sig_oid == ID_ED25519 {
        return Ok(Scheme {
            algorithm: &signature::ED25519,
            key,
        });
    }
    Err(AlgorithmUnsupported)
}

/// The size of the modulus of an RSA public key (`RSAPublicKey`, PKCS #1), in bits.
fn rsa_bits(key: &[u8]) -> Option<usize> {
    let (tag, body, _) = tlv(key)?;
    if tag != 0x30 {
        return None;
    }
    let (tag, modulus, _) = tlv(body)?;
    if tag != 0x02 {
        return None;
    }
    let digits = match modulus.first() {
        Some(0) => &modulus[1..],
        _ => modulus,
    };
    let first = *digits.first()?;
    Some((digits.len() - 1) * 8 + (8 - first.leading_zeros() as usize))
}

/// True when `cert` was signed by the key `issuer_key`. The digest is the one the certificate's
/// own signature algorithm names.
fn signed_by(cert: &Certificate, issuer_key: &SubjectPublicKeyInfoOwned) -> bool {
    let Ok(scheme) = scheme(issuer_key, &cert.signature_algorithm.oid, None) else {
        return false;
    };
    let Ok(tbs) = cert.tbs_certificate.to_der() else {
        return false;
    };
    signature::UnparsedPublicKey::new(scheme.algorithm, scheme.key)
        .verify(&tbs, cert.signature.raw_bytes())
        .is_ok()
}

/// True when the certificate says it is a certificate authority.
fn is_ca(cert: &Certificate) -> bool {
    matches!(
        cert.tbs_certificate.get::<BasicConstraints>(),
        Ok(Some((_, constraints))) if constraints.ca
    )
}

/// True when the certificate may be used to sign data: it names no use at all, or names
/// signing or non-repudiation.
fn may_sign(cert: &Certificate) -> bool {
    match cert.tbs_certificate.get::<KeyUsage>() {
        Ok(Some((_, usage))) => {
            usage.0.contains(KeyUsages::DigitalSignature)
                || usage.0.contains(KeyUsages::NonRepudiation)
        }
        Ok(None) => true,
        Err(_) => false,
    }
}

fn time_status(cert: &Certificate, now: i64) -> CertificateTime {
    let validity = &cert.tbs_certificate.validity;
    let from = validity.not_before.to_unix_duration().as_secs() as i64;
    let to = validity.not_after.to_unix_duration().as_secs() as i64;
    if now < from {
        CertificateTime::NotYetValid
    } else if now > to {
        CertificateTime::Expired
    } else {
        CertificateTime::Valid
    }
}

/// The value of a name attribute as text. The string types of X.509 are byte strings of some
/// character set; what cannot be read is left out.
fn attribute_text(value: &Any) -> Option<String> {
    let bytes = value.value();
    match value.tag() {
        Tag::Utf8String => String::from_utf8(bytes.to_vec()).ok(),
        Tag::BmpString => {
            if !bytes.len().is_multiple_of(2) {
                return None;
            }
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes(*c))
                .collect();
            String::from_utf16(&units).ok()
        }
        Tag::PrintableString | Tag::Ia5String | Tag::VisibleString | Tag::TeletexString => {
            // ASCII, or (for Teletex) Latin-1 as in practice.
            Some(bytes.iter().map(|b| *b as char).collect())
        }
        _ => None,
    }
}

/// A name part by its type, such as the common name.
fn name_part(name: &Name, oid: &Oid) -> Option<String> {
    for rdn in &name.0 {
        for attr in rdn.0.iter() {
            if attr.oid == *oid {
                if let Some(text) = attribute_text(&attr.value) {
                    return Some(text);
                }
            }
        }
    }
    None
}

/// The name to show for a certificate subject or issuer: the common name, or else the
/// organisation, or else the organisational unit. `None` for a name with none of them.
fn name_of(name: &Name) -> Option<String> {
    name_part(name, &ID_COMMON_NAME)
        .or_else(|| name_part(name, &ID_ORGANISATION))
        .or_else(|| name_part(name, &ID_ORG_UNIT))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn key_text(spki: &SubjectPublicKeyInfoOwned) -> Option<String> {
    let key = spki.subject_public_key.raw_bytes();
    if spki.algorithm.oid == ID_RSA {
        return rsa_bits(key).map(|bits| format!("RSA {bits} bit"));
    }
    if spki.algorithm.oid == ID_EC_PUBLIC_KEY {
        let curve = spki
            .algorithm
            .parameters
            .as_ref()
            .and_then(|p| p.decode_as::<Oid>().ok())?;
        return Some(if curve == ID_P256 {
            "ECDSA P-256".to_string()
        } else if curve == ID_P384 {
            "ECDSA P-384".to_string()
        } else {
            format!("ECDSA curve {curve}")
        });
    }
    if spki.algorithm.oid == ID_ED25519 {
        return Some("Ed25519".to_string());
    }
    Some(format!("key type {}", spki.algorithm.oid))
}

fn details_of(cert: &Certificate) -> CertDetails {
    let tbs = &cert.tbs_certificate;
    let serial = tbs.serial_number.as_bytes();
    let serial = match serial {
        [0, rest @ ..] if !rest.is_empty() => rest,
        other => other,
    };
    let validity = &tbs.validity;
    CertDetails {
        signer: name_of(&tbs.subject),
        organisation: name_part(&tbs.subject, &ID_ORGANISATION),
        issuer: name_of(&tbs.issuer),
        serial: Some(hex(serial)),
        fingerprint: cert
            .to_der()
            .ok()
            .map(|der| hex(digest::digest(&digest::SHA256, &der).as_ref())),
        valid_from: Some(format_time(
            validity.not_before.to_unix_duration().as_secs() as i64,
        )),
        valid_to: Some(format_time(
            validity.not_after.to_unix_duration().as_secs() as i64
        )),
        key: key_text(&tbs.subject_public_key_info),
    }
}

/// A time as `2026-01-31 12:00 UTC`.
pub fn format_time(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let (y, m, d) = civil(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        secs / 3600,
        secs % 3600 / 60
    )
}

/// The date for a count of days since 1970-01-01 (proleptic Gregorian calendar).
pub(crate) fn civil(days: i64) -> (i64, u32, u32) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_formatted_right_around_the_edges() {
        assert_eq!(format_time(0), "1970-01-01 00:00 UTC");
        assert_eq!(format_time(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(format_time(1_709_251_199), "2024-02-29 23:59 UTC");
        assert_eq!(format_time(1_709_251_200), "2024-03-01 00:00 UTC");
        assert_eq!(format_time(253_402_300_799), "9999-12-31 23:59 UTC");
        assert_eq!(format_time(-1), "1969-12-31 23:59 UTC");
    }

    #[test]
    fn der_lengths_are_read_strictly() {
        assert_eq!(der_element_len(&[0x30, 0x00]), Some(2));
        assert_eq!(der_element_len(&[0x30, 0x03, 1, 2, 3]), Some(5));
        // Shortest form only, and definite only.
        assert_eq!(der_element_len(&[0x30, 0x81, 0x05, 1, 2, 3, 4, 5]), None);
        assert_eq!(der_element_len(&[0x30, 0x80, 0, 0]), None);
        assert_eq!(der_element_len(&[0x30, 0x82, 0x00, 0x05]), None);
        // Longer than the data.
        assert_eq!(der_element_len(&[0x30, 0x04, 1, 2, 3]), None);
        assert_eq!(der_element_len(&[0x30, 0x84, 0xff, 0xff, 0xff, 0xff]), None);
        assert_eq!(der_element_len(&[0x30]), None);
        assert_eq!(der_element_len(&[]), None);
        let mut long = vec![0x30, 0x81, 0x80];
        long.extend([0u8; 0x80]);
        assert_eq!(der_element_len(&long), Some(long.len()));
    }

    #[test]
    fn the_size_of_an_rsa_key_is_read_from_its_modulus() {
        // SEQUENCE { INTEGER 00 80 (a 8 bit modulus), INTEGER 3 }
        assert_eq!(
            rsa_bits(&[0x30, 0x07, 0x02, 0x02, 0x00, 0x80, 0x02, 0x01, 0x03]),
            Some(8)
        );
        assert_eq!(
            rsa_bits(&[0x30, 0x06, 0x02, 0x01, 0x7f, 0x02, 0x01, 0x03]),
            Some(7)
        );
        assert_eq!(rsa_bits(&[0x30, 0x00]), None);
        assert_eq!(rsa_bits(&[]), None);
    }

    /// An element: tag, definite length, content.
    fn element(tag: u8, content: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        if content.len() < 0x80 {
            out.push(content.len() as u8);
        } else {
            out.extend([0x82, (content.len() >> 8) as u8, content.len() as u8]);
        }
        out.extend_from_slice(content);
        out
    }

    #[test]
    fn the_shape_of_a_signature_is_walked_with_limits() {
        use SignatureReason::*;
        let integer = |n: u8| element(0x02, &[n]);
        // A set of 256 elements is allowed, 257 is not; the same for the context tags that stand
        // for a set; a sequence has no such limit (it is not sorted).
        let many = |tag: u8, count: usize| -> Vec<u8> {
            let body: Vec<u8> = (0..count).flat_map(|i| integer(i as u8)).collect();
            element(tag, &body)
        };
        assert_eq!(check_shape(&many(0x31, MAX_SET_ELEMENTS)), Ok(()));
        assert_eq!(
            check_shape(&many(0x31, MAX_SET_ELEMENTS + 1)),
            Err(TooLarge)
        );
        assert_eq!(
            check_shape(&many(0xA0, MAX_SET_ELEMENTS + 1)),
            Err(TooLarge)
        );
        assert_eq!(check_shape(&many(0x30, 5_000)), Ok(()));
        // Many sets that are each within the limit but together cost too much.
        let sets: Vec<u8> = (0..40).flat_map(|_| many(0x31, MAX_SET_ELEMENTS)).collect();
        assert_eq!(check_shape(&element(0x30, &sets)), Err(TooLarge));
        // Nesting: 31 deep is allowed, 33 is not. A loop, so no stack is used up either way.
        let nested = |depth: usize| {
            let mut inner = integer(1);
            for _ in 0..depth {
                inner = element(0x30, &inner);
            }
            inner
        };
        assert_eq!(check_shape(&nested(MAX_DEPTH - 1)), Ok(()));
        assert_eq!(check_shape(&nested(MAX_DEPTH + 1)), Err(TooLarge));
        // A length in the indefinite form is BER; other damage is unreadable.
        assert_eq!(
            check_shape(&[0x30, 0x80, 0x02, 0x01, 0x01, 0, 0]),
            Err(EncodingUnsupported)
        );
        assert_eq!(
            check_shape(&[0x30, 0x05, 0x02, 0x01, 0x01]),
            Err(Unreadable)
        );
        assert_eq!(check_shape(&[0x30, 0x81, 0x01, 0x00]), Err(Unreadable));
        assert_eq!(check_shape(&[0x1f, 0x01, 0x00]), Err(Unreadable));
        assert_eq!(check_shape(&[]), Ok(()));
    }

    #[test]
    fn a_trust_list_without_certificates_is_empty() {
        assert!(TrustList::from_pem(b"# nothing here\n").unwrap().is_empty());
        assert!(TrustList::from_pem(b"").unwrap().is_empty());
        assert!(TrustList::from_pem(
            b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n"
        )
        .is_err());
    }
}
