//! Tests of the messages for signing with a certificate: every cap at the cap and one past it, and
//! the numbers that must stay clear of the other features.

use crate::sign::*;
use crate::{
    ErrorCode, ProtocolError, Request, RequestBody, Response, ResponseBody, SignatureFont,
    SignatureMark, MAX_UNIX_TIME,
};

fn spec() -> SignSpec {
    SignSpec {
        doc: 3,
        place: SignPlace::Rect {
            page: 2,
            rect: [72.0, 600.0, 272.0, 660.0],
        },
        name: "Anna Alvorlig".into(),
        reason: "I have read this".into(),
        location: "Oslo".into(),
        signed_at: 1_781_524_800,
        mark: None,
        color: [10, 40, 120, 255],
        reserve: 6_000,
    }
}

fn begin(spec: SignSpec) -> Vec<u8> {
    Request {
        id: 9,
        body: RequestBody::Sign(SignRequest::Begin(spec)),
    }
    .encode()
}

fn accepted(spec: SignSpec) -> bool {
    Request::decode(&begin(spec)).is_ok()
}

#[test]
fn every_request_round_trips_and_has_one_encoding() {
    let bodies = [
        SignRequest::Begin(spec()),
        SignRequest::Begin(SignSpec {
            place: SignPlace::Invisible,
            ..spec()
        }),
        SignRequest::Begin(SignSpec {
            place: SignPlace::Field {
                page: 0,
                name: "Signature1".into(),
            },
            mark: Some(SignatureMark::Typed {
                font: SignatureFont::Allura,
                text: "Anna".into(),
            }),
            ..spec()
        }),
        SignRequest::Begin(SignSpec {
            mark: Some(SignatureMark::Strokes {
                width: 300.0,
                height: 100.0,
                line: 3.0,
                strokes: vec![vec![[1.0, 2.0], [3.0, 4.0]]],
            }),
            ..spec()
        }),
        SignRequest::Begin(SignSpec {
            mark: Some(SignatureMark::Image {
                width: 2,
                height: 2,
                rgba: vec![9; 16],
            }),
            name: "\u{c5}se \u{d8}deg\u{e5}rd".into(),
            ..spec()
        }),
        SignRequest::Fields { doc: 4 },
        SignRequest::Fingerprint { doc: 4 },
    ];
    for body in bodies {
        let request = Request {
            id: 77,
            body: RequestBody::Sign(body),
        };
        let bytes = request.encode();
        assert_eq!(Request::decode(&bytes).unwrap(), request);
        for cut in 0..bytes.len() {
            assert!(Request::decode(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(matches!(
            Request::decode(&longer),
            Err(ProtocolError::TrailingBytes(1))
        ));
    }
}

#[test]
fn every_answer_round_trips_and_is_cut_nowhere() {
    let answers = [
        SignResponse::Ready {
            size: 50_000,
            hole_start: 30_000,
            hole_end: 30_000 + 2 + 12_000,
            field: "Signature1".into(),
        },
        SignResponse::Fields { fields: vec![] },
        SignResponse::Fields {
            fields: vec![
                SignField {
                    page: 0,
                    rect: [1.0, 2.0, 3.0, 4.0],
                    name: "Sign here".into(),
                },
                SignField {
                    page: 5,
                    rect: [10.0, 20.0, 130.0, 60.0],
                    name: "Approval.Signature".into(),
                },
            ],
        },
    ];
    for body in answers {
        let response = Response {
            id: 5,
            body: ResponseBody::Sign(body),
        };
        let bytes = response.encode();
        assert_eq!(Response::decode(bytes.clone()).unwrap(), response);
        for cut in 0..bytes.len() {
            assert!(
                Response::decode(bytes[..cut].to_vec()).is_err(),
                "cut at {cut}"
            );
        }
    }
}

#[test]
fn the_texts_are_capped_and_must_be_plain() {
    let long = |n: usize| "a".repeat(n);
    assert!(accepted(SignSpec {
        name: long(MAX_SIGN_TEXT_BYTES),
        reason: long(MAX_SIGN_TEXT_BYTES),
        location: long(MAX_SIGN_TEXT_BYTES),
        ..spec()
    }));
    // The encoder cuts what is too long, so a decoder never sees it; a frame that says more is
    // refused by the decoder.
    let mut bytes = begin(spec());
    let at = 1 + 4 + 4 + 1 + 4 + 16 + 4; // tag, id, doc, place tag, page, box, length of the name
    let long_name = (MAX_SIGN_TEXT_BYTES as u32 + 1).to_le_bytes();
    bytes[at - 4..at].copy_from_slice(&long_name);
    assert!(Request::decode(&bytes).is_err());
    for text in [
        "tab\there",
        "line\nbreak",
        "\u{202e}flipped",
        "zero\u{200b}width",
    ] {
        assert!(
            !accepted(SignSpec {
                reason: text.into(),
                ..spec()
            }),
            "{text:?}"
        );
        assert!(
            !accepted(SignSpec {
                place: SignPlace::Field {
                    page: 0,
                    name: text.into()
                },
                ..spec()
            }),
            "{text:?}"
        );
    }
}

#[test]
fn the_reserve_and_the_time_are_held_to_their_caps() {
    for reserve in [MIN_SIGN_RESERVE, 6_000, MAX_SIGN_RESERVE] {
        assert!(accepted(SignSpec { reserve, ..spec() }), "{reserve}");
    }
    for reserve in [0, MIN_SIGN_RESERVE - 1, MAX_SIGN_RESERVE + 1, u32::MAX] {
        assert!(!accepted(SignSpec { reserve, ..spec() }), "{reserve}");
    }
    for signed_at in [0, 1, MAX_UNIX_TIME] {
        assert!(
            accepted(SignSpec {
                signed_at,
                ..spec()
            }),
            "{signed_at}"
        );
    }
    for signed_at in [-1, MAX_UNIX_TIME + 1, i64::MIN, i64::MAX] {
        assert!(
            !accepted(SignSpec {
                signed_at,
                ..spec()
            }),
            "{signed_at}"
        );
    }
}

#[test]
fn a_box_that_is_not_a_box_is_refused() {
    for bad in [f32::NAN, f32::INFINITY, -f32::INFINITY, 1.0e12] {
        assert!(!accepted(SignSpec {
            place: SignPlace::Rect {
                page: 0,
                rect: [0.0, 0.0, bad, 10.0]
            },
            ..spec()
        }));
    }
}

#[test]
fn a_picture_over_its_caps_is_refused_with_the_same_rules_as_elsewhere() {
    let ok = |mark: SignatureMark| {
        accepted(SignSpec {
            mark: Some(mark),
            ..spec()
        })
    };
    assert!(ok(SignatureMark::Typed {
        font: SignatureFont::Kristi,
        text: "x".repeat(crate::MAX_MARK_TEXT_CHARS)
    }));
    assert!(!ok(SignatureMark::Typed {
        font: SignatureFont::Kristi,
        text: String::new()
    }));
    // The encoder leaves out what is over the caps, so the frame is checked by hand: an image of
    // 1025 by 1 pixels is over the cap of a side.
    let mut bytes = begin(SignSpec {
        mark: Some(SignatureMark::Image {
            width: 4,
            height: 1,
            rgba: vec![0; 16],
        }),
        ..spec()
    });
    // The width comes 32 bytes before the end: width, height, pixels, colour, reserve.
    let at = bytes.len() - 32;
    bytes[at..at + 4].copy_from_slice(&(crate::MAX_MARK_IMAGE_SIDE + 1).to_le_bytes());
    assert!(Request::decode(&bytes).is_err());
}

#[test]
fn a_hole_that_cannot_be_in_the_file_is_refused() {
    let answer = |size: u32, start: u32, end: u32| {
        Response {
            id: 1,
            body: ResponseBody::Sign(SignResponse::Ready {
                size,
                hole_start: start,
                hole_end: end,
                field: "S".into(),
            }),
        }
        .encode()
    };
    let good = 2 + 2 * MIN_SIGN_RESERVE;
    assert!(Response::decode(answer(10_000, 100, 100 + good)).is_ok());
    // Past the end of the file, backwards, odd, too small, too big.
    assert!(Response::decode(answer(10_000, 9_900, 9_900 + good)).is_err());
    assert!(Response::decode(answer(10_000, 200, 100)).is_err());
    assert!(Response::decode(answer(10_000, 100, 100 + good + 1)).is_err());
    assert!(Response::decode(answer(10_000, 100, 100 + good - 2)).is_err());
    assert!(Response::decode(answer(1 << 24, 100, 100 + 2 + 2 * MAX_SIGN_RESERVE + 2)).is_err());
    assert!(Response::decode(answer(u32::MAX, 100, 100 + good)).is_err());
}

#[test]
fn the_list_of_fields_is_capped_before_it_is_read() {
    let mut bytes = Response {
        id: 1,
        body: ResponseBody::Sign(SignResponse::Fields { fields: vec![] }),
    }
    .encode();
    let at = bytes.len() - 4;
    bytes[at..].copy_from_slice(&(MAX_SIGN_FIELDS as u32 + 1).to_le_bytes());
    assert!(Response::decode(bytes.clone()).is_err());
    // A count that the rest of the message cannot hold is refused without an allocation.
    bytes[at..].copy_from_slice(&(MAX_SIGN_FIELDS as u32).to_le_bytes());
    assert!(Response::decode(bytes).is_err());
}

#[test]
fn the_numbers_of_signing_meet_no_other_feature() {
    // The page tools (80 to 91, 200 to 205), the form designer (100 to 102, 210 and 211) and the
    // reader keep their own numbers; the codes of the designer are 70 and 71.
    let ours = [
        Request {
            id: 1,
            body: RequestBody::Sign(SignRequest::Fields { doc: 1 }),
        }
        .encode()[0],
        Request {
            id: 1,
            body: RequestBody::Sign(SignRequest::Begin(spec())),
        }
        .encode()[0],
        Request {
            id: 1,
            body: RequestBody::Sign(SignRequest::Fingerprint { doc: 1 }),
        }
        .encode()[0],
    ];
    let taken: [std::ops::RangeInclusive<u8>; 8] = [
        1..=24,
        40..=42,
        60..=60,
        80..=91,
        100..=102,
        129..=146,
        170..=171,
        190..=190,
    ];
    for tag in ours {
        assert!((FIRST_REQUEST_TAG..=LAST_REQUEST_TAG).contains(&tag));
        for range in &taken {
            assert!(!range.contains(&tag), "request tag {tag} is in {range:?}");
        }
    }
    let answers = [
        Response {
            id: 1,
            body: ResponseBody::Sign(SignResponse::Fields { fields: vec![] }),
        }
        .encode()[0],
        Response {
            id: 1,
            body: ResponseBody::Sign(SignResponse::Ready {
                size: 4000,
                hole_start: 10,
                hole_end: 10 + 2 + 1024,
                field: String::new(),
            }),
        }
        .encode()[0],
    ];
    let taken: [std::ops::RangeInclusive<u8>; 5] =
        [129..=146, 170..=171, 190..=190, 200..=205, 210..=211];
    for tag in answers {
        assert!((FIRST_RESPONSE_TAG..=LAST_RESPONSE_TAG).contains(&tag));
        for range in &taken {
            assert!(!range.contains(&tag), "answer tag {tag} is in {range:?}");
        }
    }
    for code in [
        ErrorCode::SignEncrypted,
        ErrorCode::SignXfa,
        ErrorCode::SignAlreadySigned,
        ErrorCode::SignStructure,
        ErrorCode::SignRefused,
    ] {
        let number = code as u8;
        assert!((90..=94).contains(&number));
        assert_eq!(ErrorCode::from_u8(number), Some(code));
        assert!(code.name().starts_with("sign_"));
    }
}
