#![no_main]
//! The check of one digital signature against the bytes of a file (`partridge_engine::signatures::
//! check`): the CMS parser, the signed attributes, the key arithmetic, the chain against a trust
//! list, the byte range and the hash of the file. This is the code that reads data a stranger wrote
//! and does arithmetic with keys they chose.
//!
//! The fuzzer's bytes are a script that starts from a signed file made by the test kit (a real
//! P-256 signature, made when the target starts) and spoils it: a bit flipped, bytes cut out or put
//! in, the signature data cut or filled, the byte range moved, the sub-filter changed, the file
//! given another size. Half of the runs keep the file and the signature data in step (the
//! hexadecimal string in the file is the signature data that is checked), so that the work goes
//! past the first comparison.
//!
//! Whatever the script does, the check must not panic, hang or use more memory than a file the
//! size of the input, and:
//!
//! - a signature is called intact (trusted, or intact with an unknown signer) only when the byte
//!   range, worked out here again by a separate and simpler routine, starts at the beginning of the
//!   file, stays inside it, leaves out exactly one hexadecimal string in `<` and `>`, and that
//!   string is the signature data; and a signature that is called intact reaches the end of the
//!   file, or else it is called changed;
//! - nothing is called trusted when the trust list is empty;
//! - the report is inside every cap of the protocol and survives encode and decode.

use std::sync::OnceLock;

use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;
use partridge_engine::sigcheck::TrustList;
use partridge_engine::signatures::{check, Raw, Settings, Work};
use partridge_fuzz::check_signature_report;
use partridge_protocol::{
    Response, ResponseBody, SignatureReason, SignatureReport, SignatureState,
};
use partridge_testkit::signed::{
    signed_file, CertSpec, Identity, KeyKind, SignOptions, SignedFile, NOW,
};

struct World {
    file: SignedFile,
    options: SignOptions,
    signer: Identity,
}

fn world() -> &'static World {
    static WORLD: OnceLock<World> = OnceLock::new();
    WORLD.get_or_init(|| {
        let signer = Identity::self_signed(KeyKind::P256, &CertSpec::new("Fuzz Signer"));
        let options = SignOptions {
            contents_size: 1024,
            ..SignOptions::default()
        };
        let file = signed_file(&signer, &options);
        World {
            file,
            options,
            signer,
        }
    })
}

fn unhex(text: &[u8]) -> Vec<u8> {
    text.chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

/// A few kinds of damage to a byte string.
fn spoil(u: &mut Unstructured, bytes: &mut Vec<u8>) {
    for _ in 0..u.int_in_range(0..=4u8).unwrap_or(0) {
        if bytes.is_empty() {
            return;
        }
        let at = u.int_in_range(0..=bytes.len() - 1).unwrap_or(0);
        match u.int_in_range(0..=5u8).unwrap_or(0) {
            0 => bytes[at] ^= 1 << u.int_in_range(0..=7u8).unwrap_or(0),
            1 => bytes[at] = u.arbitrary().unwrap_or(0),
            2 => bytes.truncate(at),
            3 => {
                let end = (at + u.int_in_range(1..=16usize).unwrap_or(1)).min(bytes.len());
                bytes.drain(at..end);
            }
            4 => {
                let extra: Vec<u8> = (0..u.int_in_range(1..=16usize).unwrap_or(1))
                    .map(|_| u.arbitrary().unwrap_or(0))
                    .collect();
                bytes.splice(at..at, extra);
            }
            _ => {
                let end = (at + u.int_in_range(1..=32usize).unwrap_or(1)).min(bytes.len());
                for b in &mut bytes[at..end] {
                    *b = 0;
                }
            }
        }
    }
}

/// A number near `n`, or far from it.
fn near(u: &mut Unstructured, n: i64) -> i64 {
    match u.int_in_range(0..=5u8).unwrap_or(0) {
        0 | 1 | 2 => n,
        3 => n + i64::from(u.int_in_range(-40..=40i32).unwrap_or(0)),
        4 => i64::from(u.arbitrary::<i32>().unwrap_or(0)),
        _ => i64::from(u.arbitrary::<u16>().unwrap_or(0)),
    }
}

/// The byte range worked out again, simply: is the file, as these numbers cut it, an
/// ordinary signed file whose left out part is exactly the signature data? Returns the length of
/// the signed part when it is.
fn plain_structure(range: &[i64], bytes: &[u8], contents: &[u8]) -> Option<usize> {
    let [a, b, c, d] = <[i64; 4]>::try_from(range).ok()?;
    if a != 0 || b < 0 || c < b || d < 0 {
        return None;
    }
    let (b, c, d) = (
        usize::try_from(b).ok()?,
        usize::try_from(c).ok()?,
        usize::try_from(d).ok()?,
    );
    let end = c.checked_add(d)?;
    if end > bytes.len() {
        return None;
    }
    let gap = &bytes[b..c];
    if gap.len() < 2 || gap[0] != b'<' || gap[gap.len() - 1] != b'>' {
        return None;
    }
    let digits = &gap[1..gap.len() - 1];
    if digits.len() % 2 != 0 || !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    if unhex(digits) != contents {
        return None;
    }
    Some(end)
}

fuzz_target!(|data: &[u8]| {
    let world = world();
    let mut u = Unstructured::new(data);
    let mut bytes = world.file.bytes.clone();
    let mut contents = unhex(&bytes[world.file.contents_at + 1..world.file.contents_end - 1]);
    let mut range: Vec<i64> = world.file.range.iter().map(|v| *v as i64).collect();
    let mut sub_filter = world.options.sub_filter.clone();
    let keep_in_step = u.arbitrary::<bool>().unwrap_or(true);

    // The signature data: as signed, spoiled, or made up.
    match u.int_in_range(0..=3u8).unwrap_or(0) {
        0 => {}
        1 | 2 => spoil(&mut u, &mut contents),
        _ => {
            contents = (0..u.int_in_range(0..=300usize).unwrap_or(0))
                .map(|_| u.arbitrary().unwrap_or(0))
                .collect();
        }
    }
    if keep_in_step {
        // The file holds exactly this signature data, in a space of the same size as before.
        let room = (world.file.contents_end - world.file.contents_at - 2) / 2;
        let mut padded = contents.clone();
        padded.resize(room.max(contents.len()), 0);
        let hex: String = padded.iter().map(|b| format!("{b:02x}")).collect();
        let mut rebuilt = bytes[..world.file.contents_at + 1].to_vec();
        rebuilt.extend_from_slice(hex.as_bytes());
        rebuilt.extend_from_slice(&bytes[world.file.contents_end - 1..]);
        let delta =
            (hex.len() as i64) - (world.file.contents_end - world.file.contents_at - 2) as i64;
        bytes = rebuilt;
        range[2] += delta;
        contents = padded;
    }
    // The rest of the file, and the numbers of the range.
    if u.arbitrary::<bool>().unwrap_or(false) {
        spoil(&mut u, &mut bytes);
    }
    if u.arbitrary::<bool>().unwrap_or(false) {
        for number in &mut range {
            *number = near(&mut u, *number);
        }
    }
    if u.int_in_range(0..=9u8).unwrap_or(0) == 0 {
        sub_filter = match u.int_in_range(0..=3u8).unwrap_or(0) {
            0 => "ETSI.CAdES.detached".into(),
            1 => "ETSI.RFC3161".into(),
            2 => "adbe.pkcs7.sha1".into(),
            _ => String::from_utf8_lossy(&data[..data.len().min(12)]).into_owned(),
        };
    }
    if u.int_in_range(0..=9u8).unwrap_or(0) == 0 {
        range.truncate(u.int_in_range(0..=5usize).unwrap_or(4));
    }

    let trust = if u.arbitrary::<bool>().unwrap_or(false) {
        TrustList::from_der(&[world.signer.cert.clone()]).expect("the signer's certificate")
    } else {
        TrustList::empty()
    };
    let trust_len = trust.len();
    let raw = Raw {
        contents: contents.clone(),
        byte_range: range.clone(),
        sub_filter: sub_filter.clone(),
        reason: Some("fuzz\u{202E}".into()),
        time: Some("D:20260615120000Z".into()),
        doc_mdp: 0,
    };
    let mut progress = |_: u32| {};
    let mut work = Work::new(1 << 30, &mut progress);
    let mut source: &[u8] = &bytes;
    let now = if u.arbitrary::<bool>().unwrap_or(true) {
        NOW
    } else {
        2_100_000_000
    };
    let info = match check(
        0,
        &raw,
        &mut source,
        bytes.len() as u64,
        &Settings { trust: &trust, now },
        &mut work,
    ) {
        Ok(info) => info,
        // The range reaches past the end of the bytes that were given: a read error, not a verdict.
        Err(_) => return,
    };

    assert_eq!(info.state, info.reason.state());
    let intact = matches!(
        info.state,
        SignatureState::Trusted | SignatureState::UnknownSigner
    );
    if info.state == SignatureState::Trusted {
        assert!(trust_len > 0, "trusted without a trust list");
        assert_eq!(info.reason, SignatureReason::Verified);
    }
    if intact || info.state == SignatureState::Changed {
        // A signature that was checked as far as its digest had a byte range of the usual kind.
        assert!(
            sub_filter == "adbe.pkcs7.detached" || sub_filter == "ETSI.CAdES.detached",
            "{sub_filter}"
        );
        let end = plain_structure(&range, &bytes, &contents)
            .expect("a signature is checked further than its structure allows");
        if intact {
            assert_eq!(
                end,
                bytes.len(),
                "intact, but the file goes on after the signed part"
            );
        }
    }
    if info.reason == SignatureReason::DataAfterSignature {
        assert!(plain_structure(&range, &bytes, &contents).is_some_and(|end| end < bytes.len()));
    }

    // The report is inside the caps and goes over the wire.
    let report = SignatureReport {
        total: 1,
        unlisted: 0,
        trust_anchors: trust_len as u32,
        items: vec![info],
    };
    check_signature_report(&report);
    let response = Response {
        id: 1,
        body: ResponseBody::Signatures { report },
    };
    let again = Response::decode(response.encode()).expect("decode own encoding");
    assert_eq!(again, response);
});
