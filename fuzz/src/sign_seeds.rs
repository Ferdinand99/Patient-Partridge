//! The seeds of the targets for signing: messages for the decoders, files for the loader of the
//! signer, and files that PDFium wrote for the writer of the empty signature.

use std::fs;
use std::path::Path;

use partridge_engine::Engine;
use partridge_protocol::{
    Request, RequestBody, Response, ResponseBody, SignField, SignPlace, SignRequest, SignResponse,
    SignSpec, SignatureFont, SignatureMark,
};
use partridge_signer_protocol::{
    usage, Certificate, ErrorCode, KeyKind, Request as SignerRequest, Response as SignerResponse,
};
use partridge_testkit as kit;

fn write(dir: &Path, target: &str, name: &str, bytes: &[u8]) {
    let folder = dir.join(target);
    fs::create_dir_all(&folder).expect("a seed folder");
    fs::write(folder.join(name), bytes).expect("a seed");
}

/// The file PDFium writes for a document of the test kit.
fn written_by_pdfium(engine: &mut Engine, pdf: &[u8]) -> Vec<u8> {
    let file = kit::TempFile::new("seed.pdf", pdf);
    engine.set_document(std::fs::File::open(file.path()).expect("the seed file"));
    let ResponseBody::Opened { doc, .. } = engine
        .handle(Request {
            id: 1,
            body: RequestBody::Open { password: None },
        })
        .body
    else {
        panic!("a seed does not open");
    };
    let ResponseBody::SaveReady { size } = engine
        .handle(Request {
            id: 2,
            body: RequestBody::SaveBegin { doc },
        })
        .body
    else {
        panic!("a seed does not save");
    };
    let mut out = Vec::new();
    while (out.len() as u32) < size {
        let len = (1 << 20).min(size - out.len() as u32);
        let ResponseBody::SaveData { data, .. } = engine
            .handle(Request {
                id: 3,
                body: RequestBody::SaveChunk {
                    doc,
                    offset: out.len() as u32,
                    len,
                },
            })
            .body
        else {
            panic!("a piece of a seed");
        };
        out.extend_from_slice(&data);
    }
    engine.handle(Request {
        id: 4,
        body: RequestBody::Close { doc },
    });
    out
}

pub fn write_sign_seeds(dir: &Path) {
    let spec = SignSpec {
        doc: 1,
        place: SignPlace::Rect {
            page: 0,
            rect: [72.0, 600.0, 272.0, 660.0],
        },
        name: "Anna Alvorlig".into(),
        reason: "Read".into(),
        location: "Oslo".into(),
        signed_at: 1_781_524_800,
        mark: Some(SignatureMark::Typed {
            font: SignatureFont::GreatVibes,
            text: "Anna".into(),
        }),
        color: [10, 40, 120, 255],
        reserve: 4_000,
    };
    let requests = [
        ("sign_begin", SignRequest::Begin(spec.clone())),
        (
            "sign_begin_field",
            SignRequest::Begin(SignSpec {
                place: SignPlace::Field {
                    page: 0,
                    name: "empty_signature".into(),
                },
                mark: None,
                ..spec.clone()
            }),
        ),
        (
            "sign_begin_invisible",
            SignRequest::Begin(SignSpec {
                place: SignPlace::Invisible,
                mark: None,
                ..spec
            }),
        ),
        ("sign_fields", SignRequest::Fields { doc: 1 }),
        ("sign_fingerprint", SignRequest::Fingerprint { doc: 1 }),
    ];
    for (name, body) in requests {
        let request = Request {
            id: 70,
            body: RequestBody::Sign(body),
        };
        write(dir, "protocol_request_decode", name, &request.encode());
    }
    let responses = [
        (
            "sign_ready",
            SignResponse::Ready {
                size: 20_000,
                hole_start: 10_000,
                hole_end: 10_000 + 2 + 8_000,
                field: "Signature1".into(),
            },
        ),
        (
            "sign_field_list",
            SignResponse::Fields {
                fields: vec![SignField {
                    page: 0,
                    rect: [72.0, 40.0, 250.0, 80.0],
                    name: "empty_signature".into(),
                }],
            },
        ),
    ];
    for (name, body) in responses {
        let response = Response {
            id: 70,
            body: ResponseBody::Sign(body),
        };
        write(dir, "protocol_response_decode", name, &response.encode());
    }

    // The messages of the signer.
    let certificate = Certificate {
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
    };
    for (name, bytes) in [
        ("hello", SignerRequest::Hello { version: 1 }.encode()),
        (
            "load",
            SignerRequest::Load {
                p12: vec![0x30, 0x82, 1, 2, 3],
                password: b"test".to_vec(),
            }
            .encode(),
        ),
        ("sign", SignerRequest::Sign { digest: vec![9; 32] }.encode()),
        ("close", SignerRequest::Close.encode()),
        (
            "hello_answer",
            SignerResponse::Hello {
                version: 1,
                sandbox: "seccomp+landlock".into(),
            }
            .encode(),
        ),
        ("loaded", SignerResponse::Loaded(certificate).encode()),
        ("signed", SignerResponse::Signed { cms: vec![0x30; 120] }.encode()),
        ("closed", SignerResponse::Closed.encode()),
        (
            "error",
            SignerResponse::Error {
                code: ErrorCode::WrongPassword,
                message: "the password is wrong".into(),
            }
            .encode(),
        ),
    ] {
        write(dir, "signer_protocol", name, &bytes);
    }

    // The loader: the test files of the signer, behind a byte that says how long the password is.
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/signer/tests/fixtures");
    for name in [
        "rsa2048", "rsa2048-legacy", "p256", "p384", "ed25519", "no-key", "rsa2048-self-signed",
    ] {
        let p12 = fs::read(fixtures.join(format!("{name}.p12"))).expect("a test certificate file");
        let mut seed = vec![4u8];
        seed.extend_from_slice(b"test");
        seed.extend_from_slice(&p12);
        write(dir, "signer_load", name, &seed);
    }
    write(dir, "signer_load", "garbage", b"\x00\x30\x82\x01\x00not a certificate file");

    // The writer of the signature: files as PDFium writes them. The first byte chooses the page and
    // the place (see the target).
    let mut engine = Engine::new(&kit::pdfium_dir());
    for (name, pdf, choice) in [
        ("plain", kit::simple_pdf(2), 0x01u8),
        ("plain_invisible", kit::simple_pdf(1), 0x00),
        ("plain_look", kit::simple_pdf(3), 0x41 | (1 << 2)),
        ("form", kit::forms_pdf(), 0x41),
        ("form_field", kit::forms_empty_signature_pdf(), 0x42),
        ("form_signed", kit::forms_signed_pdf(), 0x01),
        ("turned", kit::turned_pdf(), 0x41),
        ("hierarchy", kit::inherited_tree_pdf(), 0x01),
    ] {
        let mut seed = vec![choice];
        seed.extend_from_slice(&written_by_pdfium(&mut engine, &pdf));
        write(dir, "sign_prepare", name, &seed);
    }
}
