//! The digital signatures of an open document, for the signature panel.
//!
//! The engine describes and checks the signatures (it reads the hostile data, in its sandbox); this
//! module only passes the answer on. Everything that came from the file reaches the window as plain
//! text in a field of its own, and the state of a signature and the reason for it are short codes
//! that the window has its own wording for. Nothing here reads a signature, a certificate or a
//! date, and nothing is sent anywhere.

use std::time::{SystemTime, UNIX_EPOCH};

use partridge_engine_client::{CertificateTime, SignatureInfo, SignatureReport};
use serde::Serialize;
use tauri::State;

use super::{blocking, CommandError, Engine};

/// One signature, as the window gets it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureView {
    index: u32,
    /// `trusted`, `unknown_signer`, `changed`, `broken` or `unsupported`.
    state: &'static str,
    /// Why, as a short stable code (`data_after_signature`, `no_trust_list`, ...).
    reason: &'static str,
    /// `unknown`, `valid`, `expired` or `not_yet_valid`: the certificate against the clock.
    certificate_time: &'static str,
    sub_filter: String,
    byte_range: Vec<u64>,
    file_len: u64,
    doc_mdp: u8,
    certificates: u8,
    signer: Option<String>,
    organisation: Option<String>,
    issuer: Option<String>,
    serial: Option<String>,
    fingerprint: Option<String>,
    valid_from: Option<String>,
    valid_to: Option<String>,
    key: Option<String>,
    digest: Option<String>,
    claimed_time: Option<String>,
    stated_reason: Option<String>,
    stated_location: Option<String>,
    trust_anchor: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignaturesView {
    /// How many signatures the PDF library lists in all.
    total: u32,
    /// How many places in the file look like a signature that the library does not list.
    unlisted: u32,
    /// How many certificates are on the trust list that ships with the app.
    trust_anchors: u32,
    items: Vec<SignatureView>,
}

fn time_name(time: CertificateTime) -> &'static str {
    match time {
        CertificateTime::Unknown => "unknown",
        CertificateTime::Valid => "valid",
        CertificateTime::Expired => "expired",
        CertificateTime::NotYetValid => "not_yet_valid",
    }
}

impl From<SignatureInfo> for SignatureView {
    fn from(info: SignatureInfo) -> Self {
        SignatureView {
            index: info.index,
            state: info.state.name(),
            reason: info.reason.name(),
            certificate_time: time_name(info.certificate_time),
            sub_filter: info.sub_filter,
            byte_range: info.byte_range,
            file_len: info.file_len,
            doc_mdp: info.doc_mdp,
            certificates: info.certificates,
            signer: info.signer,
            organisation: info.organisation,
            issuer: info.issuer,
            serial: info.serial,
            fingerprint: info.fingerprint,
            valid_from: info.valid_from,
            valid_to: info.valid_to,
            key: info.key,
            digest: info.digest,
            claimed_time: info.claimed_time,
            stated_reason: info.stated_reason,
            stated_location: info.stated_location,
            trust_anchor: info.trust_anchor,
        }
    }
}

impl From<SignatureReport> for SignaturesView {
    fn from(report: SignatureReport) -> Self {
        SignaturesView {
            total: report.total,
            unlisted: report.unlisted,
            trust_anchors: report.trust_anchors,
            items: report.items.into_iter().map(SignatureView::from).collect(),
        }
    }
}

/// The time of the check, in seconds since 1970. Certificates are judged against it.
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

/// The signatures of a document, described and checked by its engine.
#[tauri::command]
pub async fn get_signatures(
    engine: State<'_, Engine>,
    doc: u64,
) -> Result<SignaturesView, CommandError> {
    let pool = engine.0.clone();
    let report = blocking(move || pool.signatures(doc, now())).await?;
    Ok(report.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use partridge_engine_client::{SignatureReason, SignatureState};

    fn info() -> SignatureInfo {
        SignatureInfo {
            index: 0,
            state: SignatureState::UnknownSigner,
            reason: SignatureReason::NoTrustList,
            certificate_time: CertificateTime::Valid,
            sub_filter: "adbe.pkcs7.detached".into(),
            byte_range: vec![0, 10, 20, 30],
            file_len: 50,
            doc_mdp: 0,
            certificates: 1,
            signer: Some("<b>Name</b>".into()),
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

    #[test]
    fn the_window_gets_codes_for_the_state_and_text_for_the_rest() {
        let view = SignaturesView::from(SignatureReport {
            total: 1,
            unlisted: 2,
            trust_anchors: 0,
            items: vec![info()],
        });
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["total"], 1);
        assert_eq!(json["unlisted"], 2);
        assert_eq!(json["trustAnchors"], 0);
        let item = &json["items"][0];
        assert_eq!(item["state"], "unknown_signer");
        assert_eq!(item["reason"], "no_trust_list");
        assert_eq!(item["certificateTime"], "valid");
        assert_eq!(item["byteRange"], serde_json::json!([0, 10, 20, 30]));
        // Text is passed on as it is: the window shows it as text, never as markup.
        assert_eq!(item["signer"], "<b>Name</b>");
        assert_eq!(item["trustAnchor"], serde_json::Value::Null);
    }

    #[test]
    fn the_time_of_the_check_is_after_the_year_2025() {
        assert!(now() > 1_735_689_600);
    }
}
