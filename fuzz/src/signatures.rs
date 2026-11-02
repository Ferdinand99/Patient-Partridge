//! Generators and checks for the signature messages, and the world that the signature check
//! target runs in.

use arbitrary::{Result, Unstructured};
use partridge_protocol::{
    plain_text, CertificateTime, SignatureInfo, SignatureReason, SignatureReport, MAX_BYTE_RANGE,
    MAX_SIGNATURES, MAX_SIG_NAME, MAX_SIG_NOTE, MAX_SIG_SERIAL, MAX_SIG_SHORT, MAX_SIG_SUBFILTER,
};

use crate::{chance, string_up_to};

/// Every reason the engine can give for a signature, in the order of their codes.
pub const SIGNATURE_REASONS: [u8; 25] = [
    0, 1, 2, 10, 11, 12, 13, 14, 15, 16, 17, 18, 30, 31, 32, 33, 34, 35, 36, 37, 50, 51, 52, 53, 54,
];

/// Plain text of at most `max` bytes, as the engine sends it: no control characters, nothing that
/// turns text around. The decoder refuses anything else.
fn plain_up_to(u: &mut Unstructured, max: usize) -> Result<String> {
    Ok(plain_text(&string_up_to(u, max)?, max))
}

fn opt_plain(u: &mut Unstructured, max: usize) -> Result<Option<String>> {
    Ok(if u.arbitrary()? {
        Some(plain_up_to(u, max)?)
    } else {
        None
    })
}

/// A report of signatures that is inside every cap, with a state that goes with every reason.
pub fn arb_signature_report(u: &mut Unstructured, count: usize) -> Result<SignatureReport> {
    let mut items = Vec::new();
    for index in 0..count.min(MAX_SIGNATURES) {
        let reason = SignatureReason::from_u8(SIGNATURE_REASONS[u.int_in_range(0..=24usize)?])
            .expect("the list holds codes");
        let ranges = u.int_in_range(0..=MAX_BYTE_RANGE)?;
        let mut byte_range = Vec::new();
        for _ in 0..ranges {
            byte_range.push(u.arbitrary::<u64>()?);
        }
        items.push(SignatureInfo {
            index: index as u32,
            state: reason.state(),
            reason,
            certificate_time: CertificateTime::from_u8(u.int_in_range(0..=3u8)?)
                .expect("0 to 3 are times"),
            sub_filter: plain_up_to(u, MAX_SIG_SUBFILTER)?,
            byte_range,
            file_len: u.arbitrary()?,
            doc_mdp: u.int_in_range(0..=3u8)?,
            certificates: u.arbitrary()?,
            signer: opt_plain(u, MAX_SIG_NAME)?,
            organisation: opt_plain(u, MAX_SIG_NAME)?,
            issuer: opt_plain(u, MAX_SIG_NAME)?,
            serial: opt_plain(u, MAX_SIG_SERIAL)?,
            fingerprint: opt_plain(u, 64)?,
            valid_from: opt_plain(u, MAX_SIG_SHORT)?,
            valid_to: opt_plain(u, MAX_SIG_SHORT)?,
            key: opt_plain(u, MAX_SIG_SHORT)?,
            digest: opt_plain(u, MAX_SIG_SHORT)?,
            claimed_time: opt_plain(u, MAX_SIG_SHORT)?,
            stated_reason: opt_plain(u, MAX_SIG_NOTE)?,
            stated_location: opt_plain(u, MAX_SIG_NOTE)?,
            trust_anchor: opt_plain(u, MAX_SIG_NAME)?,
        });
    }
    // The library may list more signatures than are described.
    let total = items.len() as u32
        + if chance(u, 4)? {
            u.int_in_range(0..=1000u32)?
        } else {
            0
        };
    Ok(SignatureReport {
        total,
        unlisted: u.arbitrary()?,
        trust_anchors: u.arbitrary()?,
        items,
    })
}

/// Panics when a report is outside what the protocol promises.
pub fn check_signature_report(report: &SignatureReport) {
    assert!(
        report.items.len() <= MAX_SIGNATURES,
        "{} signatures",
        report.items.len()
    );
    assert!(report.items.len() as u64 <= u64::from(report.total));
    for item in &report.items {
        assert_eq!(
            item.state,
            item.reason.state(),
            "a state that does not fit its reason"
        );
        assert!(item.byte_range.len() <= MAX_BYTE_RANGE);
        assert!(item.doc_mdp <= 3);
        let texts: [(&Option<String>, usize); 13] = [
            (&Some(item.sub_filter.clone()), MAX_SIG_SUBFILTER),
            (&item.signer, MAX_SIG_NAME),
            (&item.organisation, MAX_SIG_NAME),
            (&item.issuer, MAX_SIG_NAME),
            (&item.serial, MAX_SIG_SERIAL),
            (&item.fingerprint, 64),
            (&item.valid_from, MAX_SIG_SHORT),
            (&item.valid_to, MAX_SIG_SHORT),
            (&item.key, MAX_SIG_SHORT),
            (&item.digest, MAX_SIG_SHORT),
            (&item.claimed_time, MAX_SIG_SHORT),
            (&item.stated_reason, MAX_SIG_NOTE),
            (&item.stated_location, MAX_SIG_NOTE),
        ];
        for (text, max) in texts {
            if let Some(text) = text {
                assert!(text.len() <= max, "text of {} bytes", text.len());
                assert!(
                    partridge_protocol::is_plain_text(text),
                    "text that is not plain: {text:?}"
                );
            }
        }
        if let Some(text) = &item.trust_anchor {
            assert!(text.len() <= MAX_SIG_NAME);
            assert!(partridge_protocol::is_plain_text(text));
        }
    }
}
