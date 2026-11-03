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

// ---- documents written by hand, with bytes that are not text ----------------------------------

/// A one-page document from raw objects (`objects[i]` is object `i + 1`, and the first is the
/// catalog), with a plain table. The bytes of the objects are written as they are.
pub fn raw_pdf(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut at = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        at.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let table = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in at {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// The strings of `latin1_form_pdf`, as bytes: Latin-1 and UTF-16 with a byte order mark.
pub const LATIN1_FIELD: &[u8] = b"(Underskrift s\xf8ker)";
pub const LATIN1_TIP: &[u8] = b"(Signer h\xe9r)";
pub const UTF16_FIELD: &[u8] = b"(\xfe\xff\x00U\x00n\x00d\x00e\x00r\x00\xc5)";
pub const LABEL: &[u8] = b"(Side \xc5-)";
pub const LANGUAGE: &[u8] = b"(nb-\xf8)";
pub const FORM_DEFAULT: &[u8] = b"(/Helv 0 Tf 0 g \xe5)";
pub const PAGE_NOTE: &[u8] = b"(\xe5\xf8 \xfe\xff)";

/// The names of the two signature fields of `latin1_form_pdf` as the engine reads them.
pub const LATIN1_NAME: &str = "Underskrift s\u{f8}ker";
pub const UTF16_NAME: &str = "Under\u{c5}";

/// A form with two empty signature fields whose names (Latin-1 and UTF-16BE), a page label, a
/// language, the default appearance of the form and a note on the page are all bytes that are not
/// valid UTF-8.
pub fn latin1_form_pdf() -> Vec<u8> {
    let join = |parts: &[&[u8]]| parts.concat();
    raw_pdf(&[
        join(&[
            b"<< /Type /Catalog /Pages 2 0 R /Lang ",
            LANGUAGE,
            b" /PageLabels << /Nums [0 << /P ",
            LABEL,
            b" >>] >> /AcroForm << /Fields [5 0 R 6 0 R] /DA ",
            FORM_DEFAULT,
            b" /DR << /Font << /Helv 4 0 R >> >> >> >>",
        ]),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        join(&[
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /Helv 4 0 R >> >> /Annots [5 0 R 6 0 R] /Note ",
            PAGE_NOTE,
            b" >>",
        ]),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        join(&[
            b"<< /Type /Annot /Subtype /Widget /FT /Sig /T ",
            LATIN1_FIELD,
            b" /TU ",
            LATIN1_TIP,
            b" /Rect [72 100 250 140] /F 132 /P 3 0 R >>",
        ]),
        join(&[
            b"<< /Type /Annot /Subtype /Widget /FT /Sig /T ",
            UTF16_FIELD,
            b" /Rect [72 160 250 200] /F 132 /P 3 0 R >>",
        ]),
    ])
}

/// A form whose one empty signature field is described by `extra` (a seed value, a lock, an old
/// look), on a page whose annotation list may be an object of its own.
pub fn one_field_pdf(field_extra: &[u8], shared_annots: bool) -> Vec<u8> {
    let join = |parts: &[&[u8]]| parts.concat();
    let page_annots: &[u8] = if shared_annots { b"7 0 R" } else { b"[5 0 R]" };
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>".to_vec(),
        join(&[
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots ",
            page_annots,
            b" >>",
        ]),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        join(&[
            b"<< /Type /Annot /Subtype /Widget /FT /Sig /T (box) /Rect [72 100 250 140] /F 132 /P 3 0 R ",
            field_extra,
            b" >>",
        ]),
        join(&[
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots ",
            if shared_annots { b"7 0 R" } else { b"[]" },
            b" >>",
        ]),
    ];
    if shared_annots {
        objects.push(b"[5 0 R]".to_vec());
    }
    raw_pdf(&objects)
}
