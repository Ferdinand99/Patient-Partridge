//! Digital signatures on the wire: the states, the reasons, the limits and the report.
//!
//! Everything a signature carries that came from a PDF (the name in the certificate, the reason
//! and the place the signer wrote, the times, the byte ranges) is untrusted. The caps below are held
//! by the encoder and by the decoder, on both sides of the connection. Text is plain text: the
//! engine removes control characters and the characters that turn text around, and the decoder
//! refuses text that still has them. The state of a signature and the reason for it are codes, not
//! words: the window has its own wording for each, so nothing in a file can change what the panel
//! says about it.

use super::{put_str_max, put_u32, ProtocolError, Reader};

/// Most signatures the engine reports for one document. Later ones are counted, not described.
pub const MAX_SIGNATURES: usize = 64;

/// Longest name (of the signer, the organisation or the issuer), in bytes.
pub const MAX_SIG_NAME: usize = 256;

/// Longest reason or place the signer wrote, in bytes.
pub const MAX_SIG_NOTE: usize = 512;

/// Longest sub-filter name, in bytes.
pub const MAX_SIG_SUBFILTER: usize = 64;

/// Longest short text (a date, a key description, an algorithm name), in bytes.
pub const MAX_SIG_SHORT: usize = 96;

/// Longest serial number, in characters of hexadecimal text.
pub const MAX_SIG_SERIAL: usize = 128;

/// Most numbers in a byte range the engine reports. A byte range is four numbers; a file with
/// more is reported with these first ones and marked as not supported.
pub const MAX_BYTE_RANGE: usize = 8;

/// Latest time the app may give the engine: the end of the year 9999, in seconds since 1970.
pub const MAX_UNIX_TIME: i64 = 253_402_300_799;

/// The fewest bytes one described signature takes on the wire.
/// An index, five one byte values, the sub-filter length, the byte range count, the file size and
/// the presence byte of each of the thirteen optional texts.
pub const SIGNATURE_INFO_MIN: usize = 4 + 5 + 4 + 1 + 8 + 13;

/// What the reader can say about one signature. These are the only states there are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SignatureState {
    /// The signature is intact and the signer's certificate leads to a certificate on the trust
    /// list that ships with the app.
    Trusted = 1,
    /// The signature is intact, but nothing says who the signer is: the certificate does not lead
    /// to the trust list, or there is no trust list. The only claim is that the signed part of the
    /// file is as it was when the holder of the key signed it.
    UnknownSigner = 2,
    /// The signature is real, but the file is not what was signed: something follows the signed
    /// part, or the signed part differs from what the signature covers.
    Changed = 3,
    /// The signature does not hold, or it does not say what part of the file it covers.
    Broken = 4,
    /// The reader cannot check this signature (an algorithm or a form it does not support). This
    /// says nothing for or against it.
    Unsupported = 5,
}

impl SignatureState {
    pub fn from_u8(v: u8) -> Result<SignatureState, ProtocolError> {
        Ok(match v {
            1 => SignatureState::Trusted,
            2 => SignatureState::UnknownSigner,
            3 => SignatureState::Changed,
            4 => SignatureState::Broken,
            5 => SignatureState::Unsupported,
            other => return Err(ProtocolError::UnknownValue(other)),
        })
    }

    /// A short stable name.
    pub fn name(self) -> &'static str {
        match self {
            SignatureState::Trusted => "trusted",
            SignatureState::UnknownSigner => "unknown_signer",
            SignatureState::Changed => "changed",
            SignatureState::Broken => "broken",
            SignatureState::Unsupported => "unsupported",
        }
    }
}

/// Why a signature is in the state it is in. Which reasons go with which state is fixed (see
/// [`SignatureReason::state`]); the decoder refuses a report that mixes them up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SignatureReason {
    /// Nothing to add: the signature is intact and the signer is trusted.
    Verified = 0,

    // Changed
    /// The file goes on after the signed part. A later signature or a form that was filled in does
    /// this too, as does any other change.
    DataAfterSignature = 1,
    /// The signed part of the file does not have the digest the signature holds.
    ContentChanged = 2,

    // Broken
    /// The signature does not match the signed attributes and the certificate's key.
    SignatureInvalid = 10,
    /// The byte range does not describe the file: outside it, overlapping, or not from the start.
    RangeInvalid = 11,
    /// Something other than the signature itself is left out of the byte range.
    RangeLeavesGap = 12,
    /// The signature the reader lists is not the one in the space the byte range leaves out.
    ContentsMismatch = 13,
    /// Data other than zeros follows the signature inside the space made for it.
    ContentsPadding = 14,
    /// The signature data cannot be read as a signature.
    Unreadable = 15,
    /// The signature does not hold or point to the certificate of the signer.
    NoSignerCertificate = 16,
    /// The certificate in the signature is not the one the signer named.
    CertificateMismatch = 17,
    /// The signed attributes are missing a required one, or have it wrong.
    AttributesInvalid = 18,

    // Unsupported
    /// The form of the signature (its sub-filter) is not one the reader checks.
    SubFilterUnsupported = 30,
    /// The entry is a document time stamp, not a signature.
    DocumentTimestamp = 31,
    /// The digest algorithm is not supported or is no longer safe (such as SHA-1).
    DigestUnsupported = 32,
    /// The signature algorithm or the kind of key is not supported.
    AlgorithmUnsupported = 33,
    /// The key is too short to be checked safely.
    KeyTooSmall = 34,
    /// The signature has no signed attributes.
    NoSignedAttributes = 35,
    /// The signature is not encoded the way the reader requires (DER).
    EncodingUnsupported = 36,
    /// The signature or the file is too big for the reader's limits.
    TooLarge = 37,

    // Unknown signer
    /// The reader has no trust list, so no signer can be trusted.
    NoTrustList = 50,
    /// The certificate does not lead to a certificate on the trust list.
    NotInTrustList = 51,
    /// The chain reaches the trust list, but a certificate in it has expired.
    CertificateExpired = 52,
    /// The chain reaches the trust list, but a certificate in it is not valid yet.
    CertificateNotYetValid = 53,
    /// The chain reaches the trust list, but the certificate of the signer may not be used to sign.
    KeyUsage = 54,
}

impl SignatureReason {
    pub fn from_u8(v: u8) -> Result<SignatureReason, ProtocolError> {
        use SignatureReason::*;
        Ok(match v {
            0 => Verified,
            1 => DataAfterSignature,
            2 => ContentChanged,
            10 => SignatureInvalid,
            11 => RangeInvalid,
            12 => RangeLeavesGap,
            13 => ContentsMismatch,
            14 => ContentsPadding,
            15 => Unreadable,
            16 => NoSignerCertificate,
            17 => CertificateMismatch,
            18 => AttributesInvalid,
            30 => SubFilterUnsupported,
            31 => DocumentTimestamp,
            32 => DigestUnsupported,
            33 => AlgorithmUnsupported,
            34 => KeyTooSmall,
            35 => NoSignedAttributes,
            36 => EncodingUnsupported,
            37 => TooLarge,
            50 => NoTrustList,
            51 => NotInTrustList,
            52 => CertificateExpired,
            53 => CertificateNotYetValid,
            54 => KeyUsage,
            other => return Err(ProtocolError::UnknownValue(other)),
        })
    }

    /// The one state this reason goes with.
    pub fn state(self) -> SignatureState {
        use SignatureReason::*;
        match self {
            Verified => SignatureState::Trusted,
            DataAfterSignature | ContentChanged => SignatureState::Changed,
            SignatureInvalid | RangeInvalid | RangeLeavesGap | ContentsMismatch
            | ContentsPadding | Unreadable | NoSignerCertificate | CertificateMismatch
            | AttributesInvalid => SignatureState::Broken,
            SubFilterUnsupported | DocumentTimestamp | DigestUnsupported | AlgorithmUnsupported
            | KeyTooSmall | NoSignedAttributes | EncodingUnsupported | TooLarge => {
                SignatureState::Unsupported
            }
            NoTrustList
            | NotInTrustList
            | CertificateExpired
            | CertificateNotYetValid
            | KeyUsage => SignatureState::UnknownSigner,
        }
    }

    /// A short stable name.
    pub fn name(self) -> &'static str {
        use SignatureReason::*;
        match self {
            Verified => "verified",
            DataAfterSignature => "data_after_signature",
            ContentChanged => "content_changed",
            SignatureInvalid => "signature_invalid",
            RangeInvalid => "range_invalid",
            RangeLeavesGap => "range_leaves_gap",
            ContentsMismatch => "contents_mismatch",
            ContentsPadding => "contents_padding",
            Unreadable => "unreadable",
            NoSignerCertificate => "no_signer_certificate",
            CertificateMismatch => "certificate_mismatch",
            AttributesInvalid => "attributes_invalid",
            SubFilterUnsupported => "sub_filter_unsupported",
            DocumentTimestamp => "document_timestamp",
            DigestUnsupported => "digest_unsupported",
            AlgorithmUnsupported => "algorithm_unsupported",
            KeyTooSmall => "key_too_small",
            NoSignedAttributes => "no_signed_attributes",
            EncodingUnsupported => "encoding_unsupported",
            TooLarge => "too_large",
            NoTrustList => "no_trust_list",
            NotInTrustList => "not_in_trust_list",
            CertificateExpired => "certificate_expired",
            CertificateNotYetValid => "certificate_not_yet_valid",
            KeyUsage => "key_usage",
        }
    }
}

/// Whether the signer's certificate was valid at the time of the check. This is about the clock of
/// the computer, not about the time the signature was made (no time stamp is checked).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CertificateTime {
    /// The certificate could not be read, so there is nothing to say.
    Unknown = 0,
    Valid = 1,
    Expired = 2,
    NotYetValid = 3,
}

impl CertificateTime {
    pub fn from_u8(v: u8) -> Result<CertificateTime, ProtocolError> {
        Ok(match v {
            0 => CertificateTime::Unknown,
            1 => CertificateTime::Valid,
            2 => CertificateTime::Expired,
            3 => CertificateTime::NotYetValid,
            other => return Err(ProtocolError::UnknownValue(other)),
        })
    }
}

/// One signature of the document, as far as the reader could check it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureInfo {
    /// The place of the signature in the list the reader found, from 0.
    pub index: u32,
    pub state: SignatureState,
    pub reason: SignatureReason,
    /// How the certificate stands at the time of the check.
    pub certificate_time: CertificateTime,
    /// The form of the signature as the file names it, for example `adbe.pkcs7.detached`.
    pub sub_filter: String,
    /// The byte range as the file gives it (four numbers for an ordinary signature), at most
    /// [`MAX_BYTE_RANGE`] of them.
    pub byte_range: Vec<u64>,
    /// The size of the file the signature was checked against.
    pub file_len: u64,
    /// The level of change the signer allows after signing (1 to 3), or 0 when none is named.
    pub doc_mdp: u8,
    /// How many certificates the signature carries.
    pub certificates: u8,
    // The text below comes from the file or from its certificate: shown as it is, as text.
    /// The name of the signer in the certificate.
    pub signer: Option<String>,
    pub organisation: Option<String>,
    /// The name of the one who issued the certificate.
    pub issuer: Option<String>,
    /// The serial number of the certificate, in hexadecimal.
    pub serial: Option<String>,
    /// SHA-256 of the certificate, in hexadecimal.
    pub fingerprint: Option<String>,
    /// First and last day the certificate is valid, as `2026-01-31 12:00 UTC`.
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    /// The kind of key, for example `RSA 2048 bit` or `ECDSA P-256`.
    pub key: Option<String>,
    /// The digest algorithm, for example `SHA-256`.
    pub digest: Option<String>,
    /// The time the signer's computer said it was, as the file gives it. Not checked.
    pub claimed_time: Option<String>,
    /// The reason the signer wrote. Not checked.
    pub stated_reason: Option<String>,
    /// The place the signer wrote. Not checked.
    pub stated_location: Option<String>,
    /// The name of the certificate on the trust list that the chain leads to, when it does.
    pub trust_anchor: Option<String>,
}

/// What the engine found out about the signatures of a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureReport {
    /// How many signatures the PDF library lists in all (the report describes the first
    /// [`MAX_SIGNATURES`]).
    pub total: u32,
    /// How many places in the file look like the start of a signature (a `/ByteRange`) beyond the
    /// ones listed. A signature that was hidden from the list, or removed from it by a later change,
    /// leaves such a trace.
    pub unlisted: u32,
    /// How many certificates are on the trust list that ships with the app.
    pub trust_anchors: u32,
    pub items: Vec<SignatureInfo>,
}

/// True when `text` is plain text: no control characters and none of the characters that change
/// the direction of text or hide it.
pub fn is_plain_text(text: &str) -> bool {
    text.chars().all(|c| !is_unsafe_char(c))
}

fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}'
                | '\u{200B}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{2069}'
                | '\u{FEFF}'
                | '\u{FFF9}'..='\u{FFFB}'
        )
}

/// Makes plain text of `text` and cuts it to at most `max` bytes on a character boundary (a cut
/// text ends in an ellipsis). Control characters and direction controls are dropped; runs of
/// white space become one space.
pub fn plain_text(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if is_unsafe_char(c) {
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    if out.len() <= max {
        return out;
    }
    let mut end = max.saturating_sub('\u{2026}'.len_utf8());
    while !out.is_char_boundary(end) {
        end -= 1;
    }
    out.truncate(end);
    out.push('\u{2026}');
    out
}

fn put_text(out: &mut Vec<u8>, text: &str, max: usize) {
    put_str_max(out, text, max);
}

fn put_opt_text(out: &mut Vec<u8>, text: &Option<String>, max: usize) {
    match text {
        Some(text) => {
            out.push(1);
            put_text(out, text, max);
        }
        None => out.push(0),
    }
}

fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

impl<'a> Reader<'a> {
    fn plain(&mut self, max: usize) -> Result<String, ProtocolError> {
        let text = self.string_max(max)?;
        if !is_plain_text(&text) {
            return Err(ProtocolError::InvalidLength);
        }
        Ok(text)
    }

    fn opt_plain(&mut self, max: usize) -> Result<Option<String>, ProtocolError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.plain(max)?)),
            other => Err(ProtocolError::UnknownValue(other)),
        }
    }

    fn u64(&mut self) -> Result<u64, ProtocolError> {
        let low = u64::from(self.u32()?);
        let high = u64::from(self.u32()?);
        Ok(low | (high << 32))
    }
}

/// The largest a report can be on the wire; the app refuses a longer answer without reading it.
pub const MAX_SIGNATURE_REPORT: usize = 32
    + MAX_SIGNATURES
        * (SIGNATURE_INFO_MIN
            + 13 * 4
            + MAX_SIG_SUBFILTER
            + MAX_BYTE_RANGE * 8
            + 4 * MAX_SIG_NAME
            + MAX_SIG_SERIAL
            + 64
            + 5 * MAX_SIG_SHORT
            + 2 * MAX_SIG_NOTE);

impl SignatureInfo {
    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        put_u32(out, self.index);
        out.push(self.state as u8);
        out.push(self.reason as u8);
        out.push(self.certificate_time as u8);
        out.push(self.doc_mdp);
        out.push(self.certificates);
        put_text(out, &self.sub_filter, MAX_SIG_SUBFILTER);
        let count = self.byte_range.len().min(MAX_BYTE_RANGE);
        out.push(count as u8);
        for v in &self.byte_range[..count] {
            put_u64(out, *v);
        }
        put_u64(out, self.file_len);
        put_opt_text(out, &self.signer, MAX_SIG_NAME);
        put_opt_text(out, &self.organisation, MAX_SIG_NAME);
        put_opt_text(out, &self.issuer, MAX_SIG_NAME);
        put_opt_text(out, &self.serial, MAX_SIG_SERIAL);
        put_opt_text(out, &self.fingerprint, 64);
        put_opt_text(out, &self.valid_from, MAX_SIG_SHORT);
        put_opt_text(out, &self.valid_to, MAX_SIG_SHORT);
        put_opt_text(out, &self.key, MAX_SIG_SHORT);
        put_opt_text(out, &self.digest, MAX_SIG_SHORT);
        put_opt_text(out, &self.claimed_time, MAX_SIG_SHORT);
        put_opt_text(out, &self.stated_reason, MAX_SIG_NOTE);
        put_opt_text(out, &self.stated_location, MAX_SIG_NOTE);
        put_opt_text(out, &self.trust_anchor, MAX_SIG_NAME);
    }

    pub(crate) fn decode(r: &mut Reader<'_>) -> Result<SignatureInfo, ProtocolError> {
        let index = r.u32()?;
        let state = SignatureState::from_u8(r.u8()?)?;
        let reason = SignatureReason::from_u8(r.u8()?)?;
        if reason.state() != state {
            return Err(ProtocolError::InvalidLength);
        }
        let certificate_time = CertificateTime::from_u8(r.u8()?)?;
        let doc_mdp = r.u8()?;
        if doc_mdp > 3 {
            return Err(ProtocolError::UnknownValue(doc_mdp));
        }
        let certificates = r.u8()?;
        let sub_filter = r.plain(MAX_SIG_SUBFILTER)?;
        let count = r.u8()? as usize;
        if count > MAX_BYTE_RANGE {
            return Err(ProtocolError::InvalidLength);
        }
        let mut byte_range = Vec::with_capacity(count);
        for _ in 0..count {
            byte_range.push(r.u64()?);
        }
        let file_len = r.u64()?;
        Ok(SignatureInfo {
            index,
            state,
            reason,
            certificate_time,
            sub_filter,
            byte_range,
            file_len,
            doc_mdp,
            certificates,
            signer: r.opt_plain(MAX_SIG_NAME)?,
            organisation: r.opt_plain(MAX_SIG_NAME)?,
            issuer: r.opt_plain(MAX_SIG_NAME)?,
            serial: r.opt_plain(MAX_SIG_SERIAL)?,
            fingerprint: r.opt_plain(64)?,
            valid_from: r.opt_plain(MAX_SIG_SHORT)?,
            valid_to: r.opt_plain(MAX_SIG_SHORT)?,
            key: r.opt_plain(MAX_SIG_SHORT)?,
            digest: r.opt_plain(MAX_SIG_SHORT)?,
            claimed_time: r.opt_plain(MAX_SIG_SHORT)?,
            stated_reason: r.opt_plain(MAX_SIG_NOTE)?,
            stated_location: r.opt_plain(MAX_SIG_NOTE)?,
            trust_anchor: r.opt_plain(MAX_SIG_NAME)?,
        })
    }
}

impl SignatureReport {
    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        put_u32(out, self.total);
        put_u32(out, self.unlisted);
        put_u32(out, self.trust_anchors);
        let count = self.items.len().min(MAX_SIGNATURES);
        put_u32(out, count as u32);
        for item in &self.items[..count] {
            item.encode(out);
        }
    }

    pub(crate) fn decode(r: &mut Reader<'_>) -> Result<SignatureReport, ProtocolError> {
        let total = r.u32()?;
        let unlisted = r.u32()?;
        let trust_anchors = r.u32()?;
        let count = r.u32()? as usize;
        // Every item takes at least SIGNATURE_INFO_MIN bytes: a count that cannot fit is refused
        // before anything is allocated.
        if count > MAX_SIGNATURES
            || count > total as usize
            || count.saturating_mul(SIGNATURE_INFO_MIN) > r.remaining()
        {
            return Err(ProtocolError::InvalidLength);
        }
        let mut items = Vec::with_capacity(count);
        for _ in 0..count {
            items.push(SignatureInfo::decode(r)?);
        }
        Ok(SignatureReport {
            total,
            unlisted,
            trust_anchors,
            items,
        })
    }
}
