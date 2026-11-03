#![allow(dead_code)]
//! Helpers for the tests of signing with a certificate: they do what the app does (ask the engine
//! to prepare the file, hash it except the hole, have the signer make the signature, put it in the
//! hole) and look at the result with the engine's own check.

use std::path::PathBuf;

use partridge_engine::Engine;
use partridge_protocol::{
    ErrorCode, RequestBody, ResponseBody, SignPlace, SignRequest, SignResponse, SignSpec,
    SignatureMark, SignatureReport,
};
use partridge_signer::Identity;
use partridge_signer_protocol::HashKind;
use ring::digest;

use crate::common::{ask, open, save_all};

pub const NOW: i64 = 1_781_524_800; // 2026-06-15 12:00:00 UTC

/// The folder of the certificate files made for the tests.
pub fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../signer/tests/fixtures")
}

pub fn fixture(name: &str) -> Vec<u8> {
    let path = fixtures().join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

pub fn root_pem() -> Vec<u8> {
    fixture("test-root.pem")
}

pub fn identity(file: &str) -> Identity {
    partridge_signer::load(&fixture(file), "test").unwrap_or_else(|e| panic!("{file}: {e}"))
}

/// What the app asks for, with the usual choices.
pub fn spec(doc: u32, place: SignPlace) -> SignSpec {
    SignSpec {
        doc,
        place,
        name: "Anna Alvorlig".into(),
        reason: "I have read this".into(),
        location: "Oslo".into(),
        signed_at: NOW,
        mark: None,
        color: [10, 40, 120, 255],
        reserve: 6_000,
    }
}

pub fn rect_place(page: u32) -> SignPlace {
    SignPlace::Rect {
        page,
        rect: [72.0, 600.0, 272.0, 660.0],
    }
}

/// The file the engine prepared, and where the hole is.
pub struct Prepared {
    pub bytes: Vec<u8>,
    pub hole_start: usize,
    pub hole_end: usize,
    pub field: String,
}

/// `SignBegin`, then the pieces of the file.
pub fn begin(engine: &mut Engine, spec: SignSpec) -> Result<Prepared, ErrorCode> {
    let doc = spec.doc;
    match ask(engine, RequestBody::Sign(SignRequest::Begin(spec))) {
        ResponseBody::Sign(SignResponse::Ready {
            size,
            hole_start,
            hole_end,
            field,
        }) => {
            let mut bytes = Vec::new();
            while (bytes.len() as u32) < size {
                let len = (1 << 20).min(size - bytes.len() as u32);
                match ask(
                    engine,
                    RequestBody::SaveChunk {
                        doc,
                        offset: bytes.len() as u32,
                        len,
                    },
                ) {
                    ResponseBody::SaveData { data, .. } => bytes.extend_from_slice(&data),
                    other => panic!("{other:?}"),
                }
            }
            assert!(matches!(
                ask(engine, RequestBody::SaveEnd { doc }),
                ResponseBody::SaveEnded
            ));
            Ok(Prepared {
                bytes,
                hole_start: hole_start as usize,
                hole_end: hole_end as usize,
                field,
            })
        }
        ResponseBody::Error { code, .. } => Err(code),
        other => panic!("{other:?}"),
    }
}

/// The error message of a refused `SignBegin`.
pub fn refusal_text(engine: &mut Engine, spec: SignSpec) -> (ErrorCode, String) {
    match ask(engine, RequestBody::Sign(SignRequest::Begin(spec))) {
        ResponseBody::Error { code, message } => (code, message),
        other => panic!("{other:?}"),
    }
}

/// The digest of the file except the hole.
pub fn digest_of(p: &Prepared, hash: HashKind) -> Vec<u8> {
    let algorithm = match hash {
        HashKind::Sha256 => &digest::SHA256,
        HashKind::Sha384 => &digest::SHA384,
    };
    let mut context = digest::Context::new(algorithm);
    context.update(&p.bytes[..p.hole_start]);
    context.update(&p.bytes[p.hole_end..]);
    context.finish().as_ref().to_vec()
}

/// Signs the prepared file with `identity` and writes the signature into the hole.
pub fn finish(mut p: Prepared, identity: &Identity) -> Vec<u8> {
    let digest = digest_of(&p, identity.certificate().hash());
    let cms = partridge_signer::sign(identity, &digest).expect("the signer signs");
    let room = (p.hole_end - p.hole_start - 2) / 2;
    assert!(cms.len() <= room, "{} over {room}", cms.len());
    let hex: String = cms.iter().map(|b| format!("{b:02x}")).collect();
    p.bytes[p.hole_start + 1..p.hole_start + 1 + hex.len()].copy_from_slice(hex.as_bytes());
    p.bytes
}

/// Opens `bytes` and asks for its signatures at `NOW`.
pub fn report(engine: &mut Engine, bytes: &[u8]) -> SignatureReport {
    let doc = open(engine, bytes, None).expect("the signed file opens");
    let out = match ask(engine, RequestBody::Signatures { doc, now: NOW }) {
        ResponseBody::Signatures { report } => report,
        other => panic!("{other:?}"),
    };
    ask(engine, RequestBody::Close { doc });
    out
}

/// A document, prepared, signed, and the engine's report on the file.
pub fn sign_with(
    engine: &mut Engine,
    pdf: &[u8],
    file: &str,
    place: SignPlace,
) -> (Vec<u8>, SignatureReport) {
    let doc = open(engine, pdf, None).unwrap();
    let identity = identity(file);
    let mut s = spec(doc, place);
    s.reserve = identity.certificate().signature_bytes + 64;
    let prepared = begin(engine, s).expect("the engine prepares the file");
    let signed = finish(prepared, &identity);
    ask(engine, RequestBody::Close { doc });
    let r = report(engine, &signed);
    (signed, r)
}

/// As `sign_with`, with a box inside the first page whatever its size is.
pub fn sign_in_page(engine: &mut Engine, pdf: &[u8], file: &str) -> (Vec<u8>, SignatureReport) {
    let (doc, sizes) = crate::common::open_sized(engine, pdf, None).unwrap();
    let (w, h) = (sizes[0].0 as f32, sizes[0].1 as f32);
    let identity = identity(file);
    let mut s = spec(
        doc,
        SignPlace::Rect {
            page: 0,
            rect: [10.0, 10.0, (w - 10.0).min(210.0), (h - 10.0).min(70.0)],
        },
    );
    s.reserve = identity.certificate().signature_bytes + 64;
    let prepared = begin(engine, s).expect("the engine prepares the file");
    let signed = finish(prepared, &identity);
    ask(engine, RequestBody::Close { doc });
    let r = report(engine, &signed);
    (signed, r)
}

pub fn with_mark(mut s: SignSpec, mark: SignatureMark) -> SignSpec {
    s.mark = Some(mark);
    s
}

/// What PDFium makes of `bytes` when it writes them out again.
pub fn resaved(engine: &mut Engine, bytes: &[u8]) -> Vec<u8> {
    let doc = open(engine, bytes, None).unwrap();
    let out = save_all(engine, doc);
    ask(engine, RequestBody::Close { doc });
    out
}
