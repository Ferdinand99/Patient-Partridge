//! The messages between the app and the signer process.
//!
//! The signer is a small program of its own. It is the only one that ever sees a certificate file,
//! its password and the private key in it. The app hands it the bytes of the file and the password
//! once (`Load`), the signer says what is in the certificate (`Loaded`), and later the app hands it
//! the digest of the part of a PDF that is to be signed and gets the signature back (`Sign`). The
//! signer never sees the document. It never has a file or a network to use.
//!
//! The messages are binary and length-prefixed, like those of the engine, and have caps that the
//! encoder and the decoder both hold: a peer that is damaged or hostile cannot make the other
//! allocate more than the caps or loop for ever.

use std::io::{self, Read, Write};

/// Version of these messages. The app and the signer must agree on it.
pub const PROTOCOL_VERSION: u32 = 1;

/// Largest frame on the wire: a certificate file at its cap and a little more.
pub const MAX_FRAME_LEN: usize = 2 * 1024 * 1024;

/// Largest certificate file the signer is given. A file with a key and a chain is a few kilobytes;
/// a file of this size is not a certificate file anyone made on purpose.
pub const MAX_P12_BYTES: usize = 1024 * 1024;

/// Longest password, in bytes of UTF-8.
pub const MAX_PASSWORD_BYTES: usize = 1024;

/// Longest text the signer reports (a name, an issuer), in bytes. Longer text is cut by the signer
/// and refused by the decoder.
pub const MAX_TEXT_BYTES: usize = 512;

/// Largest signature the signer returns. The app reserves room for it in the file, and gives up
/// when more is needed.
pub const MAX_CMS_BYTES: usize = 48 * 1024;

/// Most certificates the signer puts in a signature (the signer's own and the ones it leads to).
pub const MAX_CHAIN: u32 = 12;

/// How long the greeting text of the sandbox may be.
pub const MAX_SANDBOX_TEXT: usize = 256;

const TAG_HELLO: u8 = 1;
const TAG_LOAD: u8 = 2;
const TAG_SIGN: u8 = 3;
const TAG_CLOSE: u8 = 4;

const TAG_HELLO_OK: u8 = 129;
const TAG_LOADED: u8 = 130;
const TAG_SIGNED: u8 = 131;
const TAG_CLOSED: u8 = 132;
const TAG_ERROR: u8 = 255;

/// Why a message could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolError {
    /// The message ends before its fields do.
    Truncated,
    /// A length or a count is over its cap, or a value is not one the message may hold.
    InvalidLength,
    /// A tag or a code that is not known.
    UnknownValue(u8),
    /// A text that is not UTF-8, or that holds control characters.
    BadText,
    /// Bytes are left after the last field.
    Trailing,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::Truncated => write!(f, "the message is cut short"),
            ProtocolError::InvalidLength => write!(f, "a length or a value is not allowed"),
            ProtocolError::UnknownValue(v) => write!(f, "unknown value {v}"),
            ProtocolError::BadText => write!(f, "text that is not plain"),
            ProtocolError::Trailing => write!(f, "bytes after the end of the message"),
        }
    }
}

impl std::error::Error for ProtocolError {}

/// Why the signer refused a request. The numbers are part of the protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ErrorCode {
    /// The file is not a PKCS #12 file, or it is damaged.
    NotACertificateFile = 1,
    /// The password is wrong (or the file is damaged in a way that looks the same).
    WrongPassword = 2,
    /// The file holds no private key, or no certificate for it.
    NoKey = 3,
    /// The kind of key is not one the signer can sign with (Ed25519, RSA-PSS, a short RSA key,
    /// another curve).
    KeyUnsupported = 4,
    /// The key and the certificate of the signer are not a pair.
    KeyMismatch = 5,
    /// The file or the signature is bigger than the signer allows.
    TooLarge = 6,
    /// A signature was asked for before a certificate file was loaded, or after it was let go.
    NotLoaded = 7,
    /// The request is not one the signer takes (a digest of the wrong length, a second load).
    InvalidRequest = 8,
    /// The signer is not in a sandbox and the app insists on one.
    SandboxUnavailable = 9,
    /// Something went wrong that the user cannot do anything about.
    Internal = 10,
    /// The two sides do not speak the same version.
    ProtocolMismatch = 11,
}

impl ErrorCode {
    pub fn from_u8(v: u8) -> Result<ErrorCode, ProtocolError> {
        Ok(match v {
            1 => ErrorCode::NotACertificateFile,
            2 => ErrorCode::WrongPassword,
            3 => ErrorCode::NoKey,
            4 => ErrorCode::KeyUnsupported,
            5 => ErrorCode::KeyMismatch,
            6 => ErrorCode::TooLarge,
            7 => ErrorCode::NotLoaded,
            8 => ErrorCode::InvalidRequest,
            9 => ErrorCode::SandboxUnavailable,
            10 => ErrorCode::Internal,
            11 => ErrorCode::ProtocolMismatch,
            other => return Err(ProtocolError::UnknownValue(other)),
        })
    }

    /// A short stable name, for the window.
    pub fn name(self) -> &'static str {
        match self {
            ErrorCode::NotACertificateFile => "not_a_certificate_file",
            ErrorCode::WrongPassword => "wrong_password",
            ErrorCode::NoKey => "no_key",
            ErrorCode::KeyUnsupported => "key_unsupported",
            ErrorCode::KeyMismatch => "key_mismatch",
            ErrorCode::TooLarge => "too_large",
            ErrorCode::NotLoaded => "not_loaded",
            ErrorCode::InvalidRequest => "invalid_request",
            ErrorCode::SandboxUnavailable => "sandbox_unavailable",
            ErrorCode::Internal => "internal",
            ErrorCode::ProtocolMismatch => "protocol_mismatch",
        }
    }
}

/// The kind of key in the certificate file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    /// RSA with this many bits (2048 to 4096), signed with PKCS #1 v1.5.
    Rsa(u32),
    /// ECDSA on the curve P-256.
    EcdsaP256,
    /// ECDSA on the curve P-384.
    EcdsaP384,
}

impl KeyKind {
    /// The digest that goes with the key: SHA-384 for P-384, SHA-256 for the rest.
    pub fn hash(self) -> HashKind {
        match self {
            KeyKind::EcdsaP384 => HashKind::Sha384,
            _ => HashKind::Sha256,
        }
    }

    /// The kind as text, as the window shows it: `RSA 3072 bit`, `ECDSA P-256`.
    pub fn text(self) -> String {
        match self {
            KeyKind::Rsa(bits) => format!("RSA {bits} bit"),
            KeyKind::EcdsaP256 => "ECDSA P-256".to_string(),
            KeyKind::EcdsaP384 => "ECDSA P-384".to_string(),
        }
    }

    fn code(self) -> (u8, u32) {
        match self {
            KeyKind::Rsa(bits) => (1, bits),
            KeyKind::EcdsaP256 => (2, 0),
            KeyKind::EcdsaP384 => (3, 0),
        }
    }

    fn from_code(kind: u8, bits: u32) -> Result<KeyKind, ProtocolError> {
        match kind {
            1 if (2048..=4096).contains(&bits) => Ok(KeyKind::Rsa(bits)),
            1 => Err(ProtocolError::InvalidLength),
            2 => Ok(KeyKind::EcdsaP256),
            3 => Ok(KeyKind::EcdsaP384),
            other => Err(ProtocolError::UnknownValue(other)),
        }
    }
}

/// The digest of the part of the file that is signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashKind {
    Sha256,
    Sha384,
}

impl HashKind {
    /// How many bytes the digest has.
    pub fn len(self) -> usize {
        match self {
            HashKind::Sha256 => 32,
            HashKind::Sha384 => 48,
        }
    }

    /// A digest is never empty.
    pub fn is_empty(self) -> bool {
        false
    }
}

/// Bits of [`Certificate::usage`].
pub mod usage {
    /// The certificate has a key usage extension at all.
    pub const HAS_EXTENSION: u8 = 1;
    /// The extension allows digital signatures.
    pub const DIGITAL_SIGNATURE: u8 = 2;
    /// The extension allows non-repudiation (content commitment).
    pub const NON_REPUDIATION: u8 = 4;
}

/// What the signer says about the certificate it loaded. The texts come from the file: show them
/// as text, nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certificate {
    /// The name of the holder: the common name, else the organisation.
    pub name: String,
    /// The name of the issuer, in the same way.
    pub issuer: String,
    /// The certificate was issued by its own holder (the subject and the issuer are the same name).
    pub self_signed: bool,
    /// The serial number in hexadecimal.
    pub serial: String,
    /// SHA-256 of the certificate in lowercase hexadecimal.
    pub fingerprint: String,
    /// First and last second of the validity, in seconds since 1970.
    pub not_before: i64,
    pub not_after: i64,
    pub key: KeyKind,
    /// A sum of the bits of [`usage`].
    pub usage: u8,
    /// How many certificates go into a signature, the holder's own included.
    pub chain: u32,
    /// How many bytes a signature takes, to the byte for an RSA key and at most this for ECDSA
    /// (the length of an ECDSA signature changes by a byte or two). The app reserves room for it.
    pub signature_bytes: u32,
}

impl Certificate {
    /// The digest that goes with the key.
    pub fn hash(&self) -> HashKind {
        self.key.hash()
    }

    /// True when the key usage allows signing documents: there is no extension, or it allows
    /// digital signatures or non-repudiation.
    pub fn may_sign(&self) -> bool {
        self.usage & usage::HAS_EXTENSION == 0
            || self.usage & (usage::DIGITAL_SIGNATURE | usage::NON_REPUDIATION) != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// The first message: the version of the messages. The answer carries the sandbox report.
    Hello { version: u32 },
    /// A certificate file (PKCS #12) and its password.
    Load { p12: Vec<u8>, password: Vec<u8> },
    /// The digest of the signed part of the file. It must be as long as the digest of the key.
    Sign { digest: Vec<u8> },
    /// Let the key go. The process ends after it.
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    Hello {
        version: u32,
        sandbox: String,
    },
    Loaded(Certificate),
    /// The signature, as DER (a CMS `ContentInfo`).
    Signed {
        cms: Vec<u8>,
    },
    Closed,
    Error {
        code: ErrorCode,
        message: String,
    },
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_i64(out: &mut Vec<u8>, v: i64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8], max: usize) {
    let b = &b[..b.len().min(max)];
    put_u32(out, b.len() as u32);
    out.extend_from_slice(b);
}

/// Text is cut at a character boundary below the cap.
fn put_text(out: &mut Vec<u8>, s: &str, max: usize) {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    put_bytes(out, &s.as_bytes()[..end], max);
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader { data, at: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ProtocolError> {
        let end = self.at.checked_add(n).ok_or(ProtocolError::Truncated)?;
        let slice = self
            .data
            .get(self.at..end)
            .ok_or(ProtocolError::Truncated)?;
        self.at = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, ProtocolError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn i64(&mut self) -> Result<i64, ProtocolError> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(i64::from_le_bytes(a))
    }

    fn bytes(&mut self, max: usize) -> Result<Vec<u8>, ProtocolError> {
        let n = self.u32()? as usize;
        if n > max {
            return Err(ProtocolError::InvalidLength);
        }
        Ok(self.take(n)?.to_vec())
    }

    fn text(&mut self, max: usize) -> Result<String, ProtocolError> {
        let b = self.bytes(max)?;
        let s = String::from_utf8(b).map_err(|_| ProtocolError::BadText)?;
        if s.chars().any(char::is_control) {
            return Err(ProtocolError::BadText);
        }
        Ok(s)
    }

    fn done(&self) -> Result<(), ProtocolError> {
        if self.at == self.data.len() {
            Ok(())
        } else {
            Err(ProtocolError::Trailing)
        }
    }
}

impl Request {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Request::Hello { version } => {
                out.push(TAG_HELLO);
                put_u32(&mut out, *version);
            }
            Request::Load { p12, password } => {
                out.push(TAG_LOAD);
                put_bytes(&mut out, p12, MAX_P12_BYTES);
                put_bytes(&mut out, password, MAX_PASSWORD_BYTES);
            }
            Request::Sign { digest } => {
                out.push(TAG_SIGN);
                put_bytes(&mut out, digest, 48);
            }
            Request::Close => out.push(TAG_CLOSE),
        }
        out
    }

    pub fn decode(data: &[u8]) -> Result<Request, ProtocolError> {
        let mut r = Reader::new(data);
        let request = match r.u8()? {
            TAG_HELLO => Request::Hello { version: r.u32()? },
            TAG_LOAD => Request::Load {
                p12: r.bytes(MAX_P12_BYTES)?,
                password: r.bytes(MAX_PASSWORD_BYTES)?,
            },
            TAG_SIGN => Request::Sign {
                digest: r.bytes(48)?,
            },
            TAG_CLOSE => Request::Close,
            other => return Err(ProtocolError::UnknownValue(other)),
        };
        r.done()?;
        Ok(request)
    }
}

impl Response {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Response::Hello { version, sandbox } => {
                out.push(TAG_HELLO_OK);
                put_u32(&mut out, *version);
                put_text(&mut out, sandbox, MAX_SANDBOX_TEXT);
            }
            Response::Loaded(c) => {
                out.push(TAG_LOADED);
                put_text(&mut out, &c.name, MAX_TEXT_BYTES);
                put_text(&mut out, &c.issuer, MAX_TEXT_BYTES);
                out.push(u8::from(c.self_signed));
                put_text(&mut out, &c.serial, MAX_TEXT_BYTES);
                put_text(&mut out, &c.fingerprint, 64);
                put_i64(&mut out, c.not_before);
                put_i64(&mut out, c.not_after);
                let (kind, bits) = c.key.code();
                out.push(kind);
                put_u32(&mut out, bits);
                out.push(c.usage);
                put_u32(&mut out, c.chain);
                put_u32(&mut out, c.signature_bytes);
            }
            Response::Signed { cms } => {
                out.push(TAG_SIGNED);
                put_bytes(&mut out, cms, MAX_CMS_BYTES);
            }
            Response::Closed => out.push(TAG_CLOSED),
            Response::Error { code, message } => {
                out.push(TAG_ERROR);
                out.push(*code as u8);
                put_text(&mut out, message, MAX_TEXT_BYTES);
            }
        }
        out
    }

    pub fn decode(data: &[u8]) -> Result<Response, ProtocolError> {
        let mut r = Reader::new(data);
        let response = match r.u8()? {
            TAG_HELLO_OK => Response::Hello {
                version: r.u32()?,
                sandbox: r.text(MAX_SANDBOX_TEXT)?,
            },
            TAG_LOADED => {
                let name = r.text(MAX_TEXT_BYTES)?;
                let issuer = r.text(MAX_TEXT_BYTES)?;
                let self_signed = match r.u8()? {
                    0 => false,
                    1 => true,
                    other => return Err(ProtocolError::UnknownValue(other)),
                };
                let serial = r.text(MAX_TEXT_BYTES)?;
                let fingerprint = r.text(64)?;
                if fingerprint.len() != 64
                    || !fingerprint
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(ProtocolError::InvalidLength);
                }
                let not_before = r.i64()?;
                let not_after = r.i64()?;
                let kind = r.u8()?;
                let bits = r.u32()?;
                let key = KeyKind::from_code(kind, bits)?;
                let usage = r.u8()?;
                if usage & !7 != 0 {
                    return Err(ProtocolError::UnknownValue(usage));
                }
                let chain = r.u32()?;
                if !(1..=MAX_CHAIN).contains(&chain) {
                    return Err(ProtocolError::InvalidLength);
                }
                let signature_bytes = r.u32()?;
                if signature_bytes == 0 || signature_bytes as usize > MAX_CMS_BYTES {
                    return Err(ProtocolError::InvalidLength);
                }
                Response::Loaded(Certificate {
                    name,
                    issuer,
                    self_signed,
                    serial,
                    fingerprint,
                    not_before,
                    not_after,
                    key,
                    usage,
                    chain,
                    signature_bytes,
                })
            }
            TAG_SIGNED => Response::Signed {
                cms: r.bytes(MAX_CMS_BYTES)?,
            },
            TAG_CLOSED => Response::Closed,
            TAG_ERROR => Response::Error {
                code: ErrorCode::from_u8(r.u8()?)?,
                message: r.text(MAX_TEXT_BYTES)?,
            },
            other => return Err(ProtocolError::UnknownValue(other)),
        };
        r.done()?;
        Ok(response)
    }
}

/// Writes one frame: four bytes of length, then the message.
pub fn write_frame(out: &mut impl Write, message: &[u8]) -> io::Result<()> {
    if message.len() > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the frame is too long",
        ));
    }
    out.write_all(&(message.len() as u32).to_le_bytes())?;
    out.write_all(message)?;
    out.flush()
}

/// Reads one frame. `None` when the other side closed the connection before a frame began. A
/// length over the cap is an error before anything is allocated.
pub fn read_frame(input: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut head = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match input.read(&mut head[got..])? {
            0 if got == 0 => return Ok(None),
            0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            n => got += n,
        }
    }
    let len = u32::from_le_bytes(head) as usize;
    if len > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the frame is too long",
        ));
    }
    let mut data = Vec::new();
    data.try_reserve_exact(len)
        .map_err(|_| io::Error::new(io::ErrorKind::OutOfMemory, "no memory for the frame"))?;
    data.resize(len, 0);
    input.read_exact(&mut data)?;
    Ok(Some(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn certificate() -> Certificate {
        Certificate {
            name: "Anna Alvorlig".into(),
            issuer: "Test Root".into(),
            self_signed: false,
            serial: "01ab".into(),
            fingerprint: "ab".repeat(32),
            not_before: 1_700_000_000,
            not_after: 1_900_000_000,
            key: KeyKind::Rsa(3072),
            usage: usage::HAS_EXTENSION | usage::DIGITAL_SIGNATURE,
            chain: 2,
            signature_bytes: 2_900,
        }
    }

    #[test]
    fn every_message_survives_a_round_trip() {
        let requests = [
            Request::Hello { version: 1 },
            Request::Load {
                p12: vec![1, 2, 3],
                password: b"secret".to_vec(),
            },
            Request::Sign {
                digest: vec![7; 32],
            },
            Request::Close,
        ];
        for r in requests {
            assert_eq!(Request::decode(&r.encode()), Ok(r));
        }
        let responses = [
            Response::Hello {
                version: 1,
                sandbox: "seccomp+landlock".into(),
            },
            Response::Loaded(certificate()),
            Response::Signed { cms: vec![9; 100] },
            Response::Closed,
            Response::Error {
                code: ErrorCode::WrongPassword,
                message: "the password is wrong".into(),
            },
        ];
        for r in responses {
            assert_eq!(Response::decode(&r.encode()), Ok(r));
        }
    }

    #[test]
    fn a_message_cut_at_any_byte_is_refused() {
        let load = Request::Load {
            p12: vec![1; 40],
            password: b"pw".to_vec(),
        }
        .encode();
        for cut in 0..load.len() {
            assert!(Request::decode(&load[..cut]).is_err(), "cut at {cut}");
        }
        for response in [
            Response::Loaded(certificate()),
            Response::Signed { cms: vec![9; 50] },
            Response::Hello {
                version: 1,
                sandbox: "x".into(),
            },
            Response::Error {
                code: ErrorCode::Internal,
                message: "m".into(),
            },
        ] {
            let bytes = response.encode();
            for cut in 0..bytes.len() {
                assert!(Response::decode(&bytes[..cut]).is_err(), "cut at {cut}");
            }
        }
    }

    #[test]
    fn bytes_after_the_end_are_refused() {
        let mut bytes = Request::Close.encode();
        bytes.push(0);
        assert_eq!(Request::decode(&bytes), Err(ProtocolError::Trailing));
        let mut bytes = Response::Closed.encode();
        bytes.push(0);
        assert_eq!(Response::decode(&bytes), Err(ProtocolError::Trailing));
    }

    #[test]
    fn the_caps_hold_on_both_sides() {
        // A length over the cap is refused before any bytes are read for it.
        let mut over = vec![TAG_LOAD];
        put_u32(&mut over, MAX_P12_BYTES as u32 + 1);
        assert_eq!(Request::decode(&over), Err(ProtocolError::InvalidLength));
        let mut over = vec![TAG_LOAD];
        put_u32(&mut over, 0);
        put_u32(&mut over, MAX_PASSWORD_BYTES as u32 + 1);
        assert_eq!(Request::decode(&over), Err(ProtocolError::InvalidLength));
        let mut over = vec![TAG_SIGN];
        put_u32(&mut over, 49);
        assert_eq!(Request::decode(&over), Err(ProtocolError::InvalidLength));
        let mut over = vec![TAG_SIGNED];
        put_u32(&mut over, MAX_CMS_BYTES as u32 + 1);
        assert_eq!(Response::decode(&over), Err(ProtocolError::InvalidLength));
        // The encoder cuts what is over the cap, so a decoder never sees it.
        let big = Request::Load {
            p12: vec![0; MAX_P12_BYTES + 10],
            password: vec![b'a'; MAX_PASSWORD_BYTES + 10],
        };
        let Request::Load { p12, password } = Request::decode(&big.encode()).unwrap() else {
            panic!("not a load");
        };
        assert_eq!(p12.len(), MAX_P12_BYTES);
        assert_eq!(password.len(), MAX_PASSWORD_BYTES);
    }

    #[test]
    fn a_certificate_that_cannot_be_true_is_refused() {
        let refused = |change: fn(&mut Certificate)| {
            let mut c = certificate();
            change(&mut c);
            Response::decode(&Response::Loaded(c).encode()).is_err()
        };
        assert!(refused(|c| c.fingerprint = "zz".repeat(32)));
        assert!(refused(|c| c.fingerprint = "AB".repeat(32)));
        assert!(refused(|c| c.fingerprint = "ab".repeat(31)));
        assert!(refused(|c| c.chain = 0));
        assert!(refused(|c| c.chain = MAX_CHAIN + 1));
        assert!(refused(|c| c.signature_bytes = 0));
        assert!(refused(|c| c.signature_bytes = MAX_CMS_BYTES as u32 + 1));
        assert!(refused(|c| c.key = KeyKind::Rsa(1024)));
        assert!(refused(|c| c.key = KeyKind::Rsa(8192)));
        assert!(refused(|c| c.usage = 0x80));
        // The same certificate with sound values is taken.
        assert!(!refused(|_| {}));
    }

    #[test]
    fn text_with_control_characters_is_refused() {
        let mut c = certificate();
        c.name = "Anna\u{7}".into();
        assert_eq!(
            Response::decode(&Response::Loaded(c).encode()),
            Err(ProtocolError::BadText)
        );
        let r = Response::Error {
            code: ErrorCode::Internal,
            message: "a\nb".into(),
        };
        assert_eq!(Response::decode(&r.encode()), Err(ProtocolError::BadText));
    }

    #[test]
    fn unknown_tags_and_codes_are_refused() {
        assert_eq!(Request::decode(&[99]), Err(ProtocolError::UnknownValue(99)));
        assert_eq!(
            Response::decode(&[99]),
            Err(ProtocolError::UnknownValue(99))
        );
        assert_eq!(
            Response::decode(&[TAG_ERROR, 0, 0, 0, 0, 0]),
            Err(ProtocolError::UnknownValue(0))
        );
        assert_eq!(Request::decode(&[]), Err(ProtocolError::Truncated));
    }

    #[test]
    fn long_text_is_cut_by_the_encoder_at_a_character_boundary() {
        let mut c = certificate();
        c.name = "ø".repeat(MAX_TEXT_BYTES);
        let Response::Loaded(back) = Response::decode(&Response::Loaded(c).encode()).unwrap()
        else {
            panic!("not loaded");
        };
        assert!(back.name.len() <= MAX_TEXT_BYTES);
        assert!(back.name.chars().all(|c| c == 'ø'));
    }

    #[test]
    fn frames_carry_a_message_and_refuse_a_length_over_the_cap() {
        let mut wire = Vec::new();
        write_frame(&mut wire, &Request::Close.encode()).unwrap();
        let mut input = &wire[..];
        assert_eq!(
            read_frame(&mut input).unwrap(),
            Some(Request::Close.encode())
        );
        assert_eq!(read_frame(&mut input).unwrap(), None);
        let long = (MAX_FRAME_LEN as u32 + 1).to_le_bytes();
        assert!(read_frame(&mut &long[..]).is_err());
        // A frame that ends in the middle is an error, not a clean end.
        assert!(read_frame(&mut &wire[..3]).is_err());
        assert!(read_frame(&mut &wire[..wire.len() - 1]).is_err());
    }

    #[test]
    fn the_key_decides_the_digest() {
        assert_eq!(KeyKind::Rsa(2048).hash(), HashKind::Sha256);
        assert_eq!(KeyKind::EcdsaP256.hash(), HashKind::Sha256);
        assert_eq!(KeyKind::EcdsaP384.hash(), HashKind::Sha384);
        assert_eq!(HashKind::Sha384.len(), 48);
        assert_eq!(KeyKind::Rsa(3072).text(), "RSA 3072 bit");
    }

    #[test]
    fn the_key_usage_decides_whether_signing_is_allowed() {
        let mut c = certificate();
        assert!(c.may_sign());
        c.usage = usage::HAS_EXTENSION;
        assert!(!c.may_sign());
        c.usage = usage::HAS_EXTENSION | usage::NON_REPUDIATION;
        assert!(c.may_sign());
        c.usage = 0;
        assert!(c.may_sign());
    }
}
