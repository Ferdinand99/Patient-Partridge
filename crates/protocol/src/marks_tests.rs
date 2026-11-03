//! Tests of the messages that carry the picture of a signature: every cap, at the cap and one
//! past it.

use crate::marks::*;
use crate::{ProtocolError, Request, RequestBody};

fn strokes_mark(strokes: Vec<Vec<[f32; 2]>>) -> SignatureMark {
    SignatureMark::Strokes {
        width: 300.0,
        height: 100.0,
        line: 3.0,
        strokes,
    }
}

fn typed(text: &str) -> SignatureMark {
    SignatureMark::Typed {
        font: SignatureFont::Allura,
        text: text.to_string(),
    }
}

fn image(width: u32, height: u32) -> SignatureMark {
    SignatureMark::Image {
        width,
        height,
        rgba: vec![7; width as usize * height as usize * 4],
    }
}

fn add(mark: SignatureMark) -> RequestBody {
    RequestBody::AddSignature {
        doc: 3,
        page: 2,
        rect: [10.0, 20.0, 210.0, 90.0],
        color: [10, 20, 120, 255],
        mark,
    }
}

fn encoded(mark: SignatureMark) -> Vec<u8> {
    Request {
        id: 5,
        body: add(mark),
    }
    .encode()
}

fn accepted(mark: SignatureMark) -> bool {
    Request::decode(&encoded(mark)).is_ok()
}

#[test]
fn signature_requests_round_trip_and_have_one_encoding() {
    for body in [
        add(strokes_mark(vec![
            vec![[1.0, 2.0], [3.0, 4.0]],
            vec![[5.0, 6.0]],
        ])),
        add(typed("Anna Alvorlig \u{e6}\u{f8}\u{e5}")),
        add(image(3, 2)),
        RequestBody::PlaceSignature {
            doc: 3,
            page: 2,
            id: 9,
            rect: [1.0, 2.0, 3.0, 4.0],
        },
    ] {
        let req = Request { id: 77, body };
        let bytes = req.encode();
        assert_eq!(Request::decode(&bytes).unwrap(), req);
        // One byte too many or too few is never a valid message.
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
fn a_drawn_picture_is_held_to_its_caps() {
    // At the caps: accepted.
    let full: Vec<Vec<[f32; 2]>> = (0..MAX_MARK_STROKES)
        .map(|_| vec![[1.0, 1.0]; MAX_MARK_POINTS / MAX_MARK_STROKES])
        .collect();
    assert!(accepted(strokes_mark(full.clone())));
    assert!(accepted(strokes_mark(vec![vec![
        [1.0, 1.0];
        MAX_MARK_POINTS
    ]])));
    assert!(strokes_mark(full).within_caps());
    // One stroke too many, one point too many.
    let too_many_strokes = vec![vec![[1.0, 1.0]]; MAX_MARK_STROKES + 1];
    assert!(!strokes_mark(too_many_strokes.clone()).within_caps());
    let one_over = vec![vec![[1.0, 1.0]; MAX_MARK_POINTS + 1]];
    assert!(!strokes_mark(one_over).within_caps());
    // The wire says the same: a hand-made frame with 65 strokes, or 4097 points, is refused.
    let mut bytes = encoded(strokes_mark(vec![vec![[1.0, 1.0]]]));
    let count_at = 1 + 4 + 4 + 4 + 16 + 4 + 1 + 12;
    bytes[count_at..count_at + 4].copy_from_slice(&(MAX_MARK_STROKES as u32 + 1).to_le_bytes());
    assert!(Request::decode(&bytes).is_err());
    let mut bytes = encoded(strokes_mark(vec![vec![[1.0, 1.0]]]));
    let points_at = count_at + 4;
    bytes[points_at..points_at + 4].copy_from_slice(&(MAX_MARK_POINTS as u32 + 1).to_le_bytes());
    assert!(Request::decode(&bytes).is_err());
    // No strokes, an empty stroke.
    let mut bytes = encoded(strokes_mark(vec![vec![[1.0, 1.0]]]));
    bytes[count_at..count_at + 4].copy_from_slice(&0u32.to_le_bytes());
    assert!(Request::decode(&bytes).is_err());
    let mut bytes = encoded(strokes_mark(vec![vec![[1.0, 1.0]]]));
    bytes[points_at..points_at + 4].copy_from_slice(&0u32.to_le_bytes());
    assert!(Request::decode(&bytes).is_err());
}

#[test]
fn the_box_the_line_and_the_points_of_a_drawn_picture_are_numbers_in_range() {
    let one = || vec![vec![[1.0f32, 1.0]]];
    let with = |width: f32, height: f32, line: f32| SignatureMark::Strokes {
        width,
        height,
        line,
        strokes: one(),
    };
    assert!(accepted(with(
        MAX_MARK_SPACE,
        MAX_MARK_SPACE,
        MAX_MARK_LINE
    )));
    assert!(accepted(with(1.0, 1.0, MIN_MARK_LINE)));
    for (w, h, l) in [
        (MAX_MARK_SPACE + 1.0, 100.0, 3.0),
        (100.0, MAX_MARK_SPACE + 1.0, 3.0),
        (0.0, 100.0, 3.0),
        (100.0, -1.0, 3.0),
        (f32::NAN, 100.0, 3.0),
        (100.0, f32::INFINITY, 3.0),
        (100.0, 100.0, MIN_MARK_LINE / 2.0),
        (100.0, 100.0, MAX_MARK_LINE + 1.0),
        (100.0, 100.0, f32::NAN),
    ] {
        assert!(!accepted(with(w, h, l)), "{w} {h} {l}");
    }
    for p in [
        [f32::NAN, 1.0],
        [1.0, f32::INFINITY],
        [MAX_MARK_SPACE * 2.0 + 1.0, 1.0],
        [1.0, -MAX_MARK_SPACE * 2.0 - 1.0],
    ] {
        assert!(!accepted(strokes_mark(vec![vec![p]])), "{p:?}");
    }
    assert!(accepted(strokes_mark(vec![vec![[
        -MAX_MARK_SPACE,
        MAX_MARK_SPACE * 2.0
    ]]])));
}

#[test]
fn the_encoder_leaves_out_what_the_decoder_would_refuse() {
    let many: Vec<Vec<[f32; 2]>> = vec![vec![[1.0, 2.0]]; MAX_MARK_STROKES + 10];
    let RequestBody::AddSignature {
        mark: SignatureMark::Strokes { strokes, .. },
        ..
    } = Request::decode(&encoded(strokes_mark(many))).unwrap().body
    else {
        panic!("not strokes");
    };
    assert_eq!(strokes.len(), MAX_MARK_STROKES);
    let heavy = vec![vec![[1.0, 2.0]; MAX_MARK_POINTS], vec![[3.0, 4.0]]];
    let RequestBody::AddSignature {
        mark: SignatureMark::Strokes { strokes, .. },
        ..
    } = Request::decode(&encoded(strokes_mark(heavy))).unwrap().body
    else {
        panic!("not strokes");
    };
    assert_eq!(strokes.len(), 1);
    assert_eq!(strokes[0].len(), MAX_MARK_POINTS);
}

#[test]
fn a_typed_picture_is_held_to_its_length_and_its_characters() {
    assert!(accepted(typed(&"a".repeat(MAX_MARK_TEXT_CHARS))));
    assert!(typed(&"a".repeat(MAX_MARK_TEXT_CHARS)).within_caps());
    // One character too many.
    assert!(!typed(&"a".repeat(MAX_MARK_TEXT_CHARS + 1)).within_caps());
    let mut bytes = encoded(typed("a"));
    let text_at = 1 + 4 + 4 + 4 + 16 + 4 + 1 + 1;
    bytes.truncate(text_at);
    bytes.extend(((MAX_MARK_TEXT_CHARS + 1) as u32).to_le_bytes());
    bytes.extend(vec![b'a'; MAX_MARK_TEXT_CHARS + 1]);
    assert!(Request::decode(&bytes).is_err());
    // Four-byte characters: forty of them fit, and the byte cap is the length of forty of them.
    assert!(accepted(typed(&"\u{1F600}".repeat(MAX_MARK_TEXT_CHARS))));
    for bad in [
        "",
        "   ",
        "a\u{0}b",
        "a\nb",
        "a\u{202E}b",
        "a\u{200B}b",
        "a\u{2028}b",
        "\u{feff}",
    ] {
        assert!(!typed(bad).within_caps(), "{bad:?}");
        let mut bytes = encoded(typed("a"));
        bytes.truncate(text_at);
        bytes.extend((bad.len() as u32).to_le_bytes());
        bytes.extend(bad.as_bytes());
        assert!(Request::decode(&bytes).is_err(), "{bad:?}");
    }
}

#[test]
fn the_fonts_are_a_closed_set() {
    let mut bytes = encoded(typed("Anna"));
    let font_at = 1 + 4 + 4 + 4 + 16 + 4 + 1;
    for (i, font) in SignatureFont::ALL.iter().enumerate() {
        assert_eq!(*font as u8, i as u8 + 1);
        assert_eq!(SignatureFont::from_u8(*font as u8).unwrap(), *font);
        assert_eq!(SignatureFont::from_name(font.name()), Some(*font));
    }
    for bad in [0u8, 4, 255] {
        bytes[font_at] = bad;
        assert!(matches!(
            Request::decode(&bytes),
            Err(ProtocolError::UnknownValue(_))
        ));
    }
    assert_eq!(SignatureFont::from_name("comic-sans"), None);
}

#[test]
fn an_imported_picture_is_held_to_its_size() {
    // At the caps: the widest, the most pixels.
    assert!(accepted(image(
        MAX_MARK_IMAGE_SIDE,
        MAX_MARK_IMAGE_PIXELS / MAX_MARK_IMAGE_SIDE
    )));
    assert!(accepted(image(1, 1)));
    assert!(accepted(image(512, 1_024)));
    // One pixel too many in a row, in a column, or in all.
    assert!(!image(MAX_MARK_IMAGE_SIDE + 1, 1).within_caps());
    assert!(!image(1, MAX_MARK_IMAGE_SIDE + 1).within_caps());
    assert!(!image(
        MAX_MARK_IMAGE_SIDE,
        MAX_MARK_IMAGE_PIXELS / MAX_MARK_IMAGE_SIDE + 1
    )
    .within_caps());
    for (w, h) in [
        (MAX_MARK_IMAGE_SIDE + 1, 1),
        (1, MAX_MARK_IMAGE_SIDE + 1),
        (
            MAX_MARK_IMAGE_SIDE,
            MAX_MARK_IMAGE_PIXELS / MAX_MARK_IMAGE_SIDE + 1,
        ),
        (0, 5),
        (5, 0),
        (u32::MAX, u32::MAX),
    ] {
        // A frame that says so, with the pixels it names (or none).
        let mut bytes = encoded(image(1, 1));
        let size_at = 1 + 4 + 4 + 4 + 16 + 4 + 1;
        bytes.truncate(size_at);
        bytes.extend(w.to_le_bytes());
        bytes.extend(h.to_le_bytes());
        if (w as u64) * (h as u64) < 8_000_000 {
            bytes.extend(vec![0u8; w as usize * h as usize * 4]);
        }
        assert!(Request::decode(&bytes).is_err(), "{w}x{h}");
    }
    // A size that says more pixels than are there is cut short, not trusted.
    let mut bytes = encoded(image(2, 2));
    bytes.truncate(bytes.len() - 1);
    assert!(matches!(
        Request::decode(&bytes),
        Err(ProtocolError::Truncated)
    ));
    // A picture whose pixel bytes do not match its size is not within the caps.
    let wrong = SignatureMark::Image {
        width: 2,
        height: 2,
        rgba: vec![0; 15],
    };
    assert!(!wrong.within_caps());
}

#[test]
fn a_kind_of_picture_nobody_defined_is_refused() {
    let mut bytes = encoded(image(1, 1));
    let kind_at = 1 + 4 + 4 + 4 + 16 + 4;
    for bad in [0u8, 4, 255] {
        bytes[kind_at] = bad;
        assert!(matches!(
            Request::decode(&bytes),
            Err(ProtocolError::UnknownValue(_))
        ));
    }
}

#[test]
fn the_box_of_a_signature_is_a_place_not_a_number_that_is_absurd() {
    let good = encoded(typed("Anna"));
    for bad in [f32::NAN, f32::INFINITY, crate::MAX_COORDINATE * 2.0] {
        let mut bytes = good.clone();
        bytes[13..17].copy_from_slice(&bad.to_le_bytes());
        assert!(Request::decode(&bytes).is_err(), "{bad}");
    }
    let place = |v: f32| {
        let mut bytes = Request {
            id: 1,
            body: RequestBody::PlaceSignature {
                doc: 1,
                page: 0,
                id: 2,
                rect: [0.0; 4],
            },
        }
        .encode();
        let at = bytes.len() - 4;
        bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
        Request::decode(&bytes)
    };
    assert!(place(10.0).is_ok());
    assert!(place(f32::NAN).is_err());
    assert!(place(crate::MAX_COORDINATE * 2.0).is_err());
}

#[test]
fn the_weight_of_a_picture_follows_its_size() {
    assert!(image(100, 100).weight() > 40_000);
    assert!(typed("Anna").weight() < 200);
    assert!(strokes_mark(vec![vec![[0.0, 0.0]; 1000]]).weight() > 8_000);
}
