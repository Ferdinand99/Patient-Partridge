//! Describing and checking the digital signatures of a document.
//!
//! PDFium finds the signatures and hands over what it knows about each: the signature data
//! (`/Contents`), the byte range, the sub-filter, the reason and the time. This module does not
//! take PDFium's word for the rest. For each signature it reads the bytes of the file the document
//! came from and checks, in this order:
//!
//! 1. the sub-filter is one that is checked at all;
//! 2. the byte range is four numbers that start at the beginning of the file, stay inside it,
//!    do not overlap, and leave out exactly one stretch: a hexadecimal string in `<` and `>`;
//! 3. that string is the signature data PDFium reported (so what is checked is what the reader
//!    lists), and after the signature itself it holds only zeros;
//! 4. the signature is a DER encoded CMS signature of a kind that is supported, and it holds up
//!    (see `sigcheck`);
//! 5. the digest of the two stretches of the file that the byte range covers is the digest the
//!    signer signed;
//! 6. the signed part reaches the end of the file: anything after it was added later;
//! 7. only then, the certificate of the signer against the trust list.
//!
//! The first step that fails gives the state of the signature. A byte range that does not cover
//! the file as a signature must, a signature listed by PDFium that is not the one in the file, and
//! data hidden in the padding are all *broken*, not merely unverified. Everything the file says
//! (the name, the reason, the place, the time) reaches the app as plain text and nothing more.
//!
//! The engine writes nothing and opens nothing here: the file is read through the guard that
//! watches it, and the time is the one the app gave.

use std::io;
use std::os::raw::{c_char, c_int, c_ulong, c_void};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use partridge_protocol::{
    plain_text, CertificateTime, ErrorCode, SignatureInfo, SignatureReason, SignatureReport,
    SignatureState, MAX_BYTE_RANGE, MAX_SIGNATURES, MAX_SIG_NAME, MAX_SIG_NOTE, MAX_SIG_SERIAL,
    MAX_SIG_SHORT, MAX_SIG_SUBFILTER,
};
use pdfium_render::prelude::*;
use ring::digest;

use crate::sigcheck::{self, CertDetails, Trust, TrustList};
use crate::source::{lock, SourceGuard};
use crate::Failure;

/// Longest signature data the engine takes from PDFium or from the file, in bytes. A signature
/// with a chain and revocation data is some tens of kilobytes.
pub const MAX_CONTENTS: usize = 1024 * 1024;

/// Most bytes of the file the engine hashes for the signatures of one document. A document with
/// many signatures over a big file would otherwise take minutes.
pub const HASH_BUDGET: u64 = 2 * 1024 * 1024 * 1024;

/// The longest the check of the signatures of one document may take, in all: the scan of the file,
/// the signatures, and the look at the file afterwards that ties the report to it. The engine of a
/// document answers one request at a time, so drawing a page waits while this runs. What is not
/// done when the time is up is reported as not checked, or (for the scan of the file and the look
/// afterwards) as a check that could not be made; it is never reported as good. Two things are
/// not cut short by the clock: a single read of the file that does not return, and (when the
/// engine runs without the thread of `sigcheck::start_parser`) the decoding of a signature that
/// passed the limits on its shape.
pub const CHECK_TIME: Duration = Duration::from_secs(10);

/// Most numbers of a byte range read from PDFium.
const MAX_RAW_RANGE: usize = 64;

/// How much of the file around the signature is looked at for the place the signer wrote.
const LOCATION_WINDOW: u64 = 16 * 1024;

/// Reading size when hashing.
const HASH_CHUNK: usize = 64 * 1024;

/// What PDFium says about one signature. Untrusted: the checks below compare it with the file.
#[derive(Debug, Clone, Default)]
pub struct Raw {
    /// The signature data, as PDFium decoded it from `/Contents`.
    pub contents: Vec<u8>,
    /// The byte range, as PDFium read it (it may hold anything, including negative numbers).
    pub byte_range: Vec<i64>,
    pub sub_filter: String,
    pub reason: Option<String>,
    /// The time as the file writes it, such as `D:20260131120000+01'00'`.
    pub time: Option<String>,
    /// The change level the signer allows, 0 when none is named.
    pub doc_mdp: u8,
}

/// Where the bytes of the file come from.
pub trait Source {
    /// Reads exactly `buf.len()` bytes at `pos`; an error when the file ends first.
    fn read_exact_at(&mut self, pos: u64, buf: &mut [u8]) -> io::Result<()>;
}

impl Source for &[u8] {
    fn read_exact_at(&mut self, pos: u64, buf: &mut [u8]) -> io::Result<()> {
        let start = usize::try_from(pos).map_err(|_| io::ErrorKind::UnexpectedEof)?;
        let end = start
            .checked_add(buf.len())
            .filter(|end| *end <= self.len())
            .ok_or(io::ErrorKind::UnexpectedEof)?;
        buf.copy_from_slice(&self[start..end]);
        Ok(())
    }
}

/// The file behind the guard. The guard is locked for one read at a time, never across a call
/// into PDFium.
struct Guarded<'a>(&'a Mutex<SourceGuard>);

impl Source for Guarded<'_> {
    fn read_exact_at(&mut self, pos: u64, buf: &mut [u8]) -> io::Result<()> {
        lock(self.0).read_exact_at(pos, buf)
    }
}

/// What a check needs besides the signature and the file.
pub struct Settings<'a> {
    pub trust: &'a TrustList,
    /// The time of the check, in seconds since 1970.
    pub now: i64,
}

/// Counts what is hashed and tells the caller now and then.
pub struct Work<'a> {
    /// Bytes of the file that may still be hashed.
    pub budget: u64,
    hashed: u64,
    ticks: u32,
    deadline: Option<Instant>,
    progress: &'a mut dyn FnMut(u32),
}

impl<'a> Work<'a> {
    pub fn new(budget: u64, progress: &'a mut dyn FnMut(u32)) -> Work<'a> {
        Work {
            budget,
            hashed: 0,
            ticks: 0,
            deadline: None,
            progress,
        }
    }

    /// The time after which no more of the file is hashed.
    pub fn with_deadline(mut self, deadline: Instant) -> Work<'a> {
        self.deadline = Some(deadline);
        self
    }

    /// The time after which no more is done, if one was set.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// True when the time of the check is up.
    pub fn out_of_time(&self) -> bool {
        self.deadline.is_some_and(|d| Instant::now() >= d)
    }

    fn add(&mut self, bytes: usize) {
        self.hashed += bytes as u64;
        while self.hashed >= (1 << 20) {
            self.hashed -= 1 << 20;
            self.ticks = self.ticks.saturating_add(1);
            (self.progress)(self.ticks);
        }
    }
}

/// The outcome of the steps: a reason, or (for a signature that holds) the name of the certificate
/// on the trust list that the signer leads to.
type Judgement = Result<String, SignatureReason>;

/// Checks one signature against the file. An error is a failure to read the file, not a verdict.
pub fn check(
    index: u32,
    raw: &Raw,
    file: &mut dyn Source,
    file_len: u64,
    settings: &Settings<'_>,
    work: &mut Work<'_>,
) -> io::Result<SignatureInfo> {
    let mut info = SignatureInfo {
        index,
        state: SignatureState::Unsupported,
        reason: SignatureReason::Unreadable,
        certificate_time: CertificateTime::Unknown,
        sub_filter: plain_text(&raw.sub_filter, MAX_SIG_SUBFILTER),
        byte_range: raw
            .byte_range
            .iter()
            .take(MAX_BYTE_RANGE)
            .map(|v| (*v).max(0) as u64)
            .collect(),
        file_len,
        doc_mdp: raw.doc_mdp.min(3),
        certificates: 0,
        signer: None,
        organisation: None,
        issuer: None,
        serial: None,
        fingerprint: None,
        valid_from: None,
        valid_to: None,
        key: None,
        digest: None,
        claimed_time: raw
            .time
            .as_deref()
            .map(claimed_time)
            .filter(|t| !t.is_empty()),
        stated_reason: raw
            .reason
            .as_deref()
            .map(|r| plain_text(r, MAX_SIG_NOTE))
            .filter(|r| !r.is_empty()),
        stated_location: None,
        trust_anchor: None,
    };
    match judge(raw, file, file_len, settings, work, &mut info)? {
        Ok(anchor) => {
            info.reason = SignatureReason::Verified;
            info.state = SignatureState::Trusted;
            info.trust_anchor = Some(plain_text(&anchor, MAX_SIG_NAME)).filter(|a| !a.is_empty());
        }
        Err(reason) => {
            info.reason = reason;
            info.state = reason.state();
        }
    }
    Ok(info)
}

fn judge(
    raw: &Raw,
    file: &mut dyn Source,
    file_len: u64,
    settings: &Settings<'_>,
    work: &mut Work<'_>,
    info: &mut SignatureInfo,
) -> io::Result<Judgement> {
    use SignatureReason::*;
    macro_rules! bail {
        ($reason:expr) => {
            return Ok(Err($reason))
        };
    }

    // 1. The kind of signature.
    match raw.sub_filter.as_str() {
        "adbe.pkcs7.detached" | "ETSI.CAdES.detached" => {}
        "ETSI.RFC3161" => bail!(DocumentTimestamp),
        _ => bail!(SubFilterUnsupported),
    }

    // 2. The byte range.
    let range = &raw.byte_range;
    if range.len() != 4 {
        if range.len() > 4 && range.len().is_multiple_of(2) {
            bail!(SubFilterUnsupported);
        }
        bail!(RangeInvalid);
    }
    if range.iter().any(|v| *v < 0) {
        bail!(RangeInvalid);
    }
    let [first_start, first_len, second_start, second_len] = [
        range[0] as u64,
        range[1] as u64,
        range[2] as u64,
        range[3] as u64,
    ];
    if first_start != 0 {
        bail!(RangeInvalid);
    }
    let Some(gap_start) = first_start.checked_add(first_len) else {
        bail!(RangeInvalid);
    };
    let Some(second_end) = second_start.checked_add(second_len) else {
        bail!(RangeInvalid);
    };
    if gap_start > second_start || second_end > file_len {
        bail!(RangeInvalid);
    }
    let gap = second_start - gap_start;
    if gap > 2 * MAX_CONTENTS as u64 + 2 {
        bail!(TooLarge);
    }
    if gap < 2 {
        bail!(RangeLeavesGap);
    }
    // The time may be up already (an earlier signature took it): what is left is not looked at.
    if work.out_of_time() {
        bail!(TooLarge);
    }
    let mut gap_bytes = vec![0u8; gap as usize];
    file.read_exact_at(gap_start, &mut gap_bytes)?;
    info.stated_location = location(file, file_len, gap_start, &gap_bytes, second_start)
        .map(|l| plain_text(&l, MAX_SIG_NOTE))
        .filter(|l| !l.is_empty());

    // The one stretch that is left out is a hexadecimal string, and nothing else.
    let Some(from_file) = hex_string(&gap_bytes) else {
        bail!(RangeLeavesGap);
    };

    // 3. It is the signature PDFium listed.
    if from_file != raw.contents {
        bail!(ContentsMismatch);
    }
    let Some(der_len) = sigcheck::der_element_len(&from_file) else {
        bail!(EncodingUnsupported);
    };
    if from_file[der_len..].iter().any(|b| *b != 0) {
        bail!(ContentsPadding);
    }

    // 4. The signature itself.
    let cms = match sigcheck::parse_within(&from_file[..der_len], work.deadline()) {
        Ok(cms) => cms,
        Err(reason) => bail!(reason),
    };
    let details = cms.details();
    fill(info, &details);
    info.certificates = cms.certificate_count();
    info.certificate_time = cms.certificate_time(settings.now);
    info.digest = cms.digest_alg().map(|d| d.name().to_string());
    if let Err(reason) = cms.verify() {
        bail!(reason);
    }
    let alg = cms.digest_alg().ok_or(io::ErrorKind::InvalidData)?;

    // 5. The digest of the file.
    let covered = first_len + second_len;
    if covered > work.budget {
        bail!(TooLarge);
    }
    work.budget -= covered;
    let mut context = digest::Context::new(alg.ring());
    if !hash_range(file, &mut context, 0, first_len, work)?
        || !hash_range(file, &mut context, second_start, second_len, work)?
    {
        // The time is up: the signature may be fine, it was not looked at to the end.
        bail!(TooLarge);
    }
    if !cms.digest_matches(context.finish().as_ref()) {
        bail!(ContentChanged);
    }

    // 6. The end of the file.
    if second_end < file_len {
        bail!(DataAfterSignature);
    }

    // 7. The signer.
    match cms.trust(settings.trust, settings.now) {
        Trust::Anchored(name) => Ok(Ok(name)),
        Trust::Not(reason) => Ok(Err(reason)),
    }
}

fn fill(info: &mut SignatureInfo, details: &CertDetails) {
    let name = |text: &Option<String>, max| {
        text.as_deref()
            .map(|t| plain_text(t, max))
            .filter(|t| !t.is_empty())
    };
    info.signer = name(&details.signer, MAX_SIG_NAME);
    info.organisation = name(&details.organisation, MAX_SIG_NAME);
    info.issuer = name(&details.issuer, MAX_SIG_NAME);
    info.serial = name(&details.serial, MAX_SIG_SERIAL);
    info.fingerprint = name(&details.fingerprint, 64);
    info.valid_from = name(&details.valid_from, MAX_SIG_SHORT);
    info.valid_to = name(&details.valid_to, MAX_SIG_SHORT);
    info.key = name(&details.key, MAX_SIG_SHORT);
}

fn hash_range(
    file: &mut dyn Source,
    context: &mut digest::Context,
    start: u64,
    len: u64,
    work: &mut Work<'_>,
) -> io::Result<bool> {
    let mut buf = vec![0u8; HASH_CHUNK];
    let mut done = 0u64;
    while done < len {
        if work.out_of_time() {
            return Ok(false);
        }
        let n = (len - done).min(HASH_CHUNK as u64) as usize;
        file.read_exact_at(start + done, &mut buf[..n])?;
        context.update(&buf[..n]);
        work.add(n);
        done += n as u64;
    }
    Ok(true)
}

/// The bytes of a hexadecimal string in angle brackets, which is all of `gap` or nothing: no
/// white space, no other character, an even number of digits.
fn hex_string(gap: &[u8]) -> Option<Vec<u8>> {
    let inner = gap.strip_prefix(b"<")?.strip_suffix(b">")?;
    if inner.len() % 2 != 0 {
        return None;
    }
    let digit = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    };
    inner
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| Some(digit(pair[0])? << 4 | digit(pair[1])?))
        .collect()
}

/// The time of the signature as the file claims it, as text: `2026-01-31 12:00:05 +01:00` when
/// the file wrote a date in PDF's form, and the file's own text when it did not.
pub fn claimed_time(text: &str) -> String {
    format_pdf_date(text).unwrap_or_else(|| plain_text(text, MAX_SIG_SHORT))
}

fn format_pdf_date(text: &str) -> Option<String> {
    let text = text.trim();
    let text = text.strip_prefix("D:").unwrap_or(text);
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    if ![4, 6, 8, 10, 12, 14].contains(&digits) {
        return None;
    }
    // Two digits at `from`, or the default when the date stops before them.
    let part = |from: usize, default: u32| -> Option<u32> {
        if digits >= from + 2 {
            text[from..from + 2].parse().ok()
        } else {
            Some(default)
        }
    };
    let year: u32 = text[0..4].parse().ok()?;
    let (month, day) = (part(4, 1)?, part(6, 1)?);
    let (hour, minute, second) = (part(8, 0)?, part(10, 0)?, part(12, 0)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let rest = &text[digits..];
    let zone = match rest.as_bytes().first() {
        None | Some(b'Z') => "UTC".to_string(),
        Some(sign @ (b'+' | b'-')) => {
            let zone: String = rest[1..].chars().filter(char::is_ascii_digit).collect();
            let hours: u32 = zone.get(0..2)?.parse().ok()?;
            let minutes: u32 = zone.get(2..4).map_or(Some(0), |m| m.parse().ok())?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            format!("{}{hours:02}:{minutes:02}", *sign as char)
        }
        Some(_) => return None,
    };
    Some(format!(
        "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} {zone}"
    ))
}

/// The place the signer wrote (`/Location`). PDFium has no call for it, so it is read from the
/// dictionary that holds the signature: the object that the signature data is in, looked at from
/// the start of the object to its end, at the top level of the dictionary only. What is found is
/// text for the person to read and is never used for anything else.
fn location(
    file: &mut dyn Source,
    file_len: u64,
    gap_start: u64,
    gap: &[u8],
    gap_end: u64,
) -> Option<String> {
    let before = gap_start.min(LOCATION_WINDOW);
    let after = file_len.saturating_sub(gap_end).min(LOCATION_WINDOW);
    let mut region = vec![0u8; before as usize];
    file.read_exact_at(gap_start - before, &mut region).ok()?;
    // The start of the object: the last `obj` that is not the end of `endobj`.
    let start = (3..=region.len()).rev().find(|&end| {
        &region[end - 3..end] == b"obj" && (end < 6 || &region[end - 6..end - 3] != b"end")
    })?;
    let mut region = region[start..].to_vec();
    region.extend_from_slice(gap);
    let mut tail = vec![0u8; after as usize];
    file.read_exact_at(gap_end, &mut tail).ok()?;
    region.extend_from_slice(&tail);
    let value = dictionary_string(&region, b"Location")?;
    Some(pdf_text(&value))
}

/// The string value of a top level key of the first dictionary in `region`, which starts just
/// after the `obj` keyword. A string is `(...)` or `<...>`.
fn dictionary_string(region: &[u8], key: &[u8]) -> Option<Vec<u8>> {
    let mut depth = 0i32;
    let mut started = false;
    let mut i = 0;
    while i < region.len() {
        match region[i] {
            b'%' => {
                while i < region.len() && region[i] != b'\n' && region[i] != b'\r' {
                    i += 1;
                }
            }
            b'(' => {
                let (end, _) = literal_string(region, i)?;
                i = end;
            }
            b'<' if region.get(i + 1) == Some(&b'<') => {
                depth += 1;
                started = true;
                i += 2;
            }
            b'<' => {
                let close = region[i..].iter().position(|b| *b == b'>')?;
                i += close + 1;
            }
            b'>' if region.get(i + 1) == Some(&b'>') => {
                depth -= 1;
                i += 2;
                if started && depth <= 0 {
                    return None;
                }
            }
            b'/' => {
                let begin = i + 1;
                i = begin;
                while i < region.len() && !is_delimiter(region[i]) {
                    i += 1;
                }
                if depth == 1 && &region[begin..i] == key {
                    while i < region.len() && region[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    return match region.get(i)? {
                        b'(' => literal_string(region, i).map(|(_, bytes)| bytes),
                        b'<' if region.get(i + 1) != Some(&b'<') => {
                            let close = region[i..].iter().position(|b| *b == b'>')?;
                            hex_string(&region[i..=i + close])
                        }
                        _ => None,
                    };
                }
            }
            _ => i += 1,
        }
    }
    None
}

fn is_delimiter(b: u8) -> bool {
    b.is_ascii_whitespace() || b"()<>[]{}/%".contains(&b)
}

/// A literal string starting at the `(` at `start`: where it ends, and its bytes (cut at 4 KiB).
fn literal_string(data: &[u8], start: usize) -> Option<(usize, Vec<u8>)> {
    let mut out = Vec::new();
    let mut depth = 0u32;
    let mut i = start;
    while i < data.len() {
        let b = data[i];
        i += 1;
        match b {
            b'(' => {
                depth += 1;
                if depth > 1 {
                    out.push(b);
                }
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((i, out));
                }
                out.push(b);
            }
            b'\\' => {
                let next = *data.get(i)?;
                i += 1;
                match next {
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'\r' => {
                        if data.get(i) == Some(&b'\n') {
                            i += 1;
                        }
                    }
                    b'\n' => {}
                    b'0'..=b'7' => {
                        let mut value = u32::from(next - b'0');
                        for _ in 0..2 {
                            match data.get(i) {
                                Some(d @ b'0'..=b'7') => {
                                    value = value * 8 + u32::from(d - b'0');
                                    i += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push(value as u8);
                    }
                    other => out.push(other),
                }
            }
            _ => out.push(b),
        }
        if out.len() >= 4096 {
            out.truncate(4096);
        }
    }
    None
}

/// A PDF text string as text: UTF-16 with a byte order mark, UTF-8 with one, or else PDFDocEncoding
/// (read as Latin-1 here, which is what it is for ordinary text).
fn pdf_text(bytes: &[u8]) -> String {
    if let Some(body) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = body
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes(*c))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(body) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = body
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(body) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(body).into_owned();
    }
    bytes.iter().map(|b| *b as char).collect()
}

/// The raw handle PDFium has for a document. `pdfium-render` keeps it to itself; the text it gives
/// for `Debug` names it, and a test (`the_raw_handle_of_a_document_can_be_read`) fails if that
/// ever stops being so.
pub fn raw_document(document: &PdfDocument<'_>) -> Option<FPDF_DOCUMENT> {
    let text = format!("{document:?}");
    let start = text.find("0x")? + 2;
    let hex: String = text[start..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    let value = usize::from_str_radix(&hex, 16).ok()?;
    (value != 0).then_some(value as FPDF_DOCUMENT)
}

/// The text of PDFium's UTF-16LE strings (ended by a zero).
fn utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|u| *u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// Asks PDFium for what it knows about one signature.
///
/// # Safety
/// `handle` is a signature handle that PDFium gave out for a document that is still open.
unsafe fn read_raw(bindings: &dyn PdfiumLibraryBindings, handle: FPDF_SIGNATURE) -> Raw {
    // The signature data.
    let size = bindings.FPDFSignatureObj_GetContents(handle, std::ptr::null_mut(), 0) as usize;
    let contents = if size == 0 || size > MAX_CONTENTS {
        // Too long to be taken: the check then sees data of the wrong length and says so.
        vec![0u8; usize::from(size > MAX_CONTENTS)]
    } else {
        let mut buf = vec![0u8; size];
        let got = bindings.FPDFSignatureObj_GetContents(
            handle,
            buf.as_mut_ptr().cast::<c_void>(),
            size as c_ulong,
        ) as usize;
        buf.truncate(got.min(size));
        buf
    };

    // The byte range (a count of numbers).
    let count = bindings.FPDFSignatureObj_GetByteRange(handle, std::ptr::null_mut(), 0) as usize;
    let mut byte_range: Vec<i64> = Vec::new();
    if count > 0 && count <= MAX_RAW_RANGE {
        let mut buf = vec![0 as c_int; count];
        let got = bindings.FPDFSignatureObj_GetByteRange(handle, buf.as_mut_ptr(), count as c_ulong)
            as usize;
        byte_range = buf[..got.min(count)]
            .iter()
            .map(|v| i64::from(*v))
            .collect();
    } else if count > MAX_RAW_RANGE {
        // More numbers than any real signature has: an odd-sized list that is not four numbers.
        byte_range = vec![-1; 5];
    }

    let sub_filter = ascii(bindings, handle, |b, h, p, l| unsafe {
        b.FPDFSignatureObj_GetSubFilter(h, p, l)
    })
    .unwrap_or_default();
    let time = ascii(bindings, handle, |b, h, p, l| unsafe {
        b.FPDFSignatureObj_GetTime(h, p, l)
    });

    let reason_size = bindings.FPDFSignatureObj_GetReason(handle, std::ptr::null_mut(), 0) as usize;
    let reason = if reason_size == 0 || reason_size > 64 * 1024 {
        None
    } else {
        let mut buf = vec![0u8; reason_size];
        let got = bindings.FPDFSignatureObj_GetReason(
            handle,
            buf.as_mut_ptr().cast::<c_void>(),
            reason_size as c_ulong,
        ) as usize;
        Some(utf16le(&buf[..got.min(reason_size)])).filter(|r| !r.is_empty())
    };

    let doc_mdp = match bindings.FPDFSignatureObj_GetDocMDPPermission(handle) {
        level @ 1..=3 => level as u8,
        _ => 0,
    };

    Raw {
        contents,
        byte_range,
        sub_filter,
        reason,
        time,
        doc_mdp,
    }
}

/// A short ASCII string of PDFium (ended by a zero), at most 4 KiB.
///
/// # Safety
/// As for [`read_raw`]; `get` calls one of PDFium's functions of that shape.
unsafe fn ascii(
    bindings: &dyn PdfiumLibraryBindings,
    handle: FPDF_SIGNATURE,
    get: impl Fn(&dyn PdfiumLibraryBindings, FPDF_SIGNATURE, *mut c_char, c_ulong) -> c_ulong,
) -> Option<String> {
    let size = get(bindings, handle, std::ptr::null_mut(), 0) as usize;
    if size == 0 || size > 4096 {
        return None;
    }
    let mut buf = vec![0u8; size];
    let got = get(
        bindings,
        handle,
        buf.as_mut_ptr().cast::<c_char>(),
        size as c_ulong,
    ) as usize;
    let bytes = &buf[..got.min(size)];
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    Some(String::from_utf8_lossy(&bytes[..end]).into_owned()).filter(|s| !s.is_empty())
}

fn unreadable(index: u32, file_len: u64) -> SignatureInfo {
    SignatureInfo {
        index,
        // PDFium lists the signature and gives no handle for it: the reader could not look at it.
        state: SignatureState::Unsupported,
        reason: SignatureReason::Unreadable,
        certificate_time: CertificateTime::Unknown,
        sub_filter: String::new(),
        byte_range: Vec::new(),
        file_len,
        doc_mdp: 0,
        certificates: 0,
        signer: None,
        organisation: None,
        issuer: None,
        serial: None,
        fingerprint: None,
        valid_from: None,
        valid_to: None,
        key: None,
        digest: None,
        claimed_time: None,
        stated_reason: None,
        stated_location: None,
        trust_anchor: None,
    }
}

/// What a read of the file that failed means for a request.
fn read_failure(error: &io::Error) -> Failure {
    (
        ErrorCode::SourceUnreadable,
        format!("a part of the file could not be read for the signatures ({error})"),
    )
}

/// Describes and checks the signatures of `document`, which was opened from the file behind
/// `source`. `now` is the time of the check in seconds since 1970. `deadline` is when the whole
/// check must be done (see [`CHECK_TIME`]); the caller sets it and uses the same one for what it
/// does after the report.
pub fn report(
    pdfium: &Pdfium,
    document: &PdfDocument<'_>,
    source: &Mutex<SourceGuard>,
    trust: &TrustList,
    now: i64,
    deadline: Instant,
    progress: &mut dyn FnMut(u32),
) -> Result<SignatureReport, Failure> {
    let Some(handle) = raw_document(document) else {
        return Err((
            ErrorCode::Internal,
            "the signatures could not be looked at".to_string(),
        ));
    };
    let bindings = pdfium.bindings();
    // SAFETY: `handle` is the handle of an open document.
    let total = unsafe { bindings.FPDF_GetSignatureCount(handle) }.max(0) as u32;
    let file_len = lock(source).file_len();
    // How many places look like the start of a signature, whatever PDFium lists.
    let marks = lock(source)
        .count_occurrences(b"/ByteRange", Some(deadline), progress)
        .map_err(|e| {
            if e.kind() == io::ErrorKind::TimedOut {
                (
                    ErrorCode::Internal,
                    "the check of the signatures took longer than its time limit".to_string(),
                )
            } else {
                read_failure(&e)
            }
        })?;
    let settings = Settings { trust, now };
    let mut forward = |done: u32| progress(done);
    let mut work = Work::new(HASH_BUDGET, &mut forward).with_deadline(deadline);
    let mut items = Vec::new();
    for index in 0..total.min(MAX_SIGNATURES as u32) {
        // SAFETY: the index is below the count PDFium gave.
        let handle = unsafe { bindings.FPDF_GetSignatureObject(handle, index as c_int) };
        if handle.is_null() {
            items.push(unreadable(index, file_len));
            continue;
        }
        // SAFETY: the handle is one PDFium gave out just now for the open document.
        let raw = unsafe { read_raw(bindings, handle) };
        let info = check(
            index,
            &raw,
            &mut Guarded(source),
            file_len,
            &settings,
            &mut work,
        )
        .map_err(|e| read_failure(&e))?;
        items.push(info);
    }
    Ok(SignatureReport {
        total,
        unlisted: marks.saturating_sub(total),
        trust_anchors: trust.len() as u32,
        items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hexadecimal_string_is_all_of_the_gap_or_nothing() {
        assert_eq!(hex_string(b"<00ff>"), Some(vec![0, 255]));
        assert_eq!(hex_string(b"<>"), Some(vec![]));
        assert_eq!(hex_string(b"<AaBb>"), Some(vec![0xaa, 0xbb]));
        for bad in [
            &b"<00 ff>"[..],
            b"<0>",
            b"<0g>",
            b"00ff",
            b"<00ff",
            b"00ff>",
            b"<00ff> ",
            b" <00ff>",
            b"<00ff>x",
            b"<00>\n<ff>",
            b"",
        ] {
            assert_eq!(hex_string(bad), None, "{:?}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn dates_in_the_form_of_pdf_are_read_and_the_rest_is_kept_as_text() {
        assert_eq!(
            claimed_time("D:20260131120005+01'00'"),
            "2026-01-31 12:00:05 +01:00"
        );
        assert_eq!(claimed_time("D:20260131120005Z"), "2026-01-31 12:00:05 UTC");
        assert_eq!(claimed_time("D:20260131"), "2026-01-31 00:00:00 UTC");
        assert_eq!(claimed_time("D:2026"), "2026-01-01 00:00:00 UTC");
        assert_eq!(claimed_time("D:20261301"), "D:20261301");
        assert_eq!(claimed_time("yesterday\u{202E}"), "yesterday");
        assert_eq!(
            claimed_time("D:20260131120005-05'30"),
            "2026-01-31 12:00:05 -05:30"
        );
        assert_eq!(claimed_time(""), "");
    }

    #[test]
    fn the_place_is_read_from_the_top_of_the_dictionary_only() {
        let object = b" << /Type /Sig /Prop << /Location (nested) >> /Contents <0000> /Reason (r) /Location (Oslo \\(Norway\\)) >> endobj";
        assert_eq!(
            dictionary_string(object, b"Location"),
            Some(b"Oslo (Norway)".to_vec())
        );
        let hex = b" << /Location <FEFF00480069> >> endobj";
        assert_eq!(
            pdf_text(&dictionary_string(hex, b"Location").unwrap()),
            "Hi"
        );
        // A key outside the dictionary, or not there, is nothing.
        assert_eq!(
            dictionary_string(b" << /A 1 >> /Location (x)", b"Location"),
            None
        );
        assert_eq!(dictionary_string(b" << /Location 12 >>", b"Location"), None);
        assert_eq!(dictionary_string(b" << /Location (open", b"Location"), None);
        // Text that looks like the key inside a string is not the key.
        assert_eq!(
            dictionary_string(b" << /R (a /Location (x) b) >>", b"Location"),
            None
        );
    }

    #[test]
    fn strings_unescape_and_stop_at_the_cap() {
        assert_eq!(
            literal_string(b"(a\\nb\\101\\\n c(d))", 0),
            Some((17, b"a\nbA c(d)".to_vec()))
        );
        assert_eq!(literal_string(b"(open", 0), None);
        let long = [b"(".as_slice(), &[b'x'; 10_000], b")"].concat();
        assert_eq!(literal_string(&long, 0).unwrap().1.len(), 4096);
    }
}
