//! The messages for signing with a certificate for the fuzz targets: valid messages built from
//! fuzzer bytes, the caps as assertions, and the messages of the signer process.

use arbitrary::{Result, Unstructured};
use partridge_protocol::{
    plain_text, SignField, SignPlace, SignRequest, SignResponse, SignSpec, MAX_COORDINATE,
    MAX_SAVE_BYTES, MAX_SIGN_FIELDS, MAX_SIGN_RESERVE, MAX_SIGN_TEXT_BYTES, MAX_UNIX_TIME,
    MIN_SIGN_RESERVE,
};
use partridge_signer_protocol::{
    usage, Certificate, ErrorCode as SignerError, KeyKind, Request as SignerRequest,
    Response as SignerResponse, MAX_CHAIN, MAX_CMS_BYTES, MAX_P12_BYTES, MAX_PASSWORD_BYTES,
    MAX_SANDBOX_TEXT, MAX_TEXT_BYTES,
};

use crate::{arb_mark, chance, count_up_to, string_up_to};

fn plain_up_to(u: &mut Unstructured, max: usize) -> Result<String> {
    Ok(plain_text(&string_up_to(u, max)?, max))
}

fn coordinate(u: &mut Unstructured) -> Result<f32> {
    Ok(u.int_in_range(-(MAX_COORDINATE as i32)..=MAX_COORDINATE as i32)? as f32 / 4.0)
}

pub fn arb_sign_request(u: &mut Unstructured) -> Result<SignRequest> {
    let doc: u32 = u.arbitrary()?;
    Ok(match u.int_in_range(0..=2u8)? {
        0 => {
            let place = match u.int_in_range(0..=2u8)? {
                0 => SignPlace::Invisible,
                1 => SignPlace::Rect {
                    page: u.arbitrary()?,
                    rect: [coordinate(u)?, coordinate(u)?, coordinate(u)?, coordinate(u)?],
                },
                _ => SignPlace::Field {
                    page: u.arbitrary()?,
                    name: plain_up_to(u, MAX_SIGN_TEXT_BYTES)?,
                },
            };
            SignRequest::Begin(SignSpec {
                doc,
                place,
                name: plain_up_to(u, MAX_SIGN_TEXT_BYTES)?,
                reason: plain_up_to(u, MAX_SIGN_TEXT_BYTES)?,
                location: plain_up_to(u, MAX_SIGN_TEXT_BYTES)?,
                signed_at: u.int_in_range(0..=MAX_UNIX_TIME)?,
                mark: if u.arbitrary()? {
                    Some(arb_mark(u)?)
                } else {
                    None
                },
                color: u.arbitrary()?,
                reserve: u.int_in_range(MIN_SIGN_RESERVE..=MAX_SIGN_RESERVE)?,
            })
        }
        1 => SignRequest::Fields { doc },
        _ => SignRequest::Fingerprint { doc },
    })
}

pub fn arb_sign_response(u: &mut Unstructured) -> Result<SignResponse> {
    if u.arbitrary()? {
        let digits = 2 * u.int_in_range(MIN_SIGN_RESERVE..=MAX_SIGN_RESERVE)?;
        let hole_start = u.int_in_range(0..=1_000_000u32)?;
        let hole_end = hole_start + 2 + digits;
        let size = hole_end + u.int_in_range(0..=1_000_000u32)?;
        return Ok(SignResponse::Ready {
            size,
            hole_start,
            hole_end,
            field: plain_up_to(u, MAX_SIGN_TEXT_BYTES)?,
        });
    }
    let count = count_up_to(u, MAX_SIGN_FIELDS)?;
    let mut fields = Vec::new();
    for _ in 0..count {
        fields.push(SignField {
            page: u.arbitrary()?,
            rect: [coordinate(u)?, coordinate(u)?, coordinate(u)?, coordinate(u)?],
            name: plain_up_to(u, MAX_SIGN_TEXT_BYTES)?,
        });
    }
    Ok(SignResponse::Fields { fields })
}

pub fn check_sign_request_caps(request: &SignRequest) {
    if let SignRequest::Begin(spec) = request {
        for text in [&spec.name, &spec.reason, &spec.location] {
            assert!(text.len() <= MAX_SIGN_TEXT_BYTES, "text of {} bytes", text.len());
        }
        if let SignPlace::Field { name, .. } = &spec.place {
            assert!(name.len() <= MAX_SIGN_TEXT_BYTES);
        }
        assert!((0..=MAX_UNIX_TIME).contains(&spec.signed_at));
        assert!((MIN_SIGN_RESERVE..=MAX_SIGN_RESERVE).contains(&spec.reserve));
        if let Some(mark) = &spec.mark {
            crate::check_mark_caps(mark);
        }
    }
}

pub fn check_sign_response_caps(response: &SignResponse) {
    match response {
        SignResponse::Ready {
            size,
            hole_start,
            hole_end,
            field,
        } => {
            assert!(*size as usize <= MAX_SAVE_BYTES);
            assert!(hole_start < hole_end && hole_end <= size);
            assert!((hole_end - hole_start) % 2 == 0);
            assert!(field.len() <= MAX_SIGN_TEXT_BYTES);
        }
        SignResponse::Fields { fields } => {
            assert!(fields.len() <= MAX_SIGN_FIELDS, "{} fields", fields.len());
            for f in fields {
                assert!(f.name.len() <= MAX_SIGN_TEXT_BYTES);
            }
        }
    }
}

// ---- the signer process ------------------------------------------------------------------------

pub fn arb_signer_request(u: &mut Unstructured) -> Result<SignerRequest> {
    Ok(match u.int_in_range(0..=3u8)? {
        0 => SignerRequest::Hello {
            version: u.arbitrary()?,
        },
        1 => {
            let max = if chance(u, 24)? { MAX_P12_BYTES } else { 2_000 };
            let n = u.int_in_range(0..=max.min(u.len()))?;
            let p12 = u.bytes(n)?.to_vec();
            let pw = u.int_in_range(0..=MAX_PASSWORD_BYTES.min(u.len()))?;
            SignerRequest::Load {
                p12,
                password: u.bytes(pw)?.to_vec(),
            }
        }
        2 => {
            let len = u.int_in_range(0..=48usize.min(u.len()))?;
            SignerRequest::Sign {
                digest: u.bytes(len)?.to_vec(),
            }
        }
        _ => SignerRequest::Close,
    })
}

pub fn arb_signer_response(u: &mut Unstructured) -> Result<SignerResponse> {
    Ok(match u.int_in_range(0..=4u8)? {
        0 => SignerResponse::Hello {
            version: u.arbitrary()?,
            sandbox: plain_up_to(u, MAX_SANDBOX_TEXT)?,
        },
        1 => {
            let key = match u.int_in_range(0..=2u8)? {
                0 => KeyKind::Rsa(u.int_in_range(2048..=4096u32)?),
                1 => KeyKind::EcdsaP256,
                _ => KeyKind::EcdsaP384,
            };
            let fingerprint: String = (0..64)
                .map(|_| char::from_digit(u.int_in_range(0..=15u32).unwrap_or(0), 16).unwrap_or('0'))
                .collect();
            SignerResponse::Loaded(Certificate {
                name: plain_up_to(u, MAX_TEXT_BYTES)?,
                issuer: plain_up_to(u, MAX_TEXT_BYTES)?,
                self_signed: u.arbitrary()?,
                serial: plain_up_to(u, MAX_TEXT_BYTES)?,
                fingerprint,
                not_before: u.arbitrary()?,
                not_after: u.arbitrary()?,
                key,
                usage: u.int_in_range(0..=7u8)? & (usage::HAS_EXTENSION | usage::DIGITAL_SIGNATURE | usage::NON_REPUDIATION),
                chain: u.int_in_range(1..=MAX_CHAIN)?,
                signature_bytes: u.int_in_range(1..=MAX_CMS_BYTES as u32)?,
            })
        }
        2 => {
            let len = u.int_in_range(0..=MAX_CMS_BYTES.min(u.len()))?;
            SignerResponse::Signed {
                cms: u.bytes(len)?.to_vec(),
            }
        }
        3 => SignerResponse::Closed,
        _ => SignerResponse::Error {
            code: [
                SignerError::NotACertificateFile,
                SignerError::WrongPassword,
                SignerError::NoKey,
                SignerError::KeyUnsupported,
                SignerError::KeyMismatch,
                SignerError::TooLarge,
                SignerError::NotLoaded,
                SignerError::InvalidRequest,
                SignerError::SandboxUnavailable,
                SignerError::Internal,
                SignerError::ProtocolMismatch,
            ][u.int_in_range(0..=10usize)?],
            message: plain_up_to(u, MAX_TEXT_BYTES)?,
        },
    })
}
