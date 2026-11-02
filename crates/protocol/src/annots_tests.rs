//! Tests of the annotation messages: every cap, at the cap and one past it.

use crate::annots::*;
use crate::{ProtocolError, Request, RequestBody, Response, ResponseBody};

fn info(id: u32) -> AnnotInfo {
    AnnotInfo {
        id,
        kind: AnnotKind::Note,
        abilities: ability::EDIT_TEXT | ability::REMOVE,
        rect: [10.0, 20.0, 30.0, 40.0],
        color: [255, 235, 59, 255],
        text: "A note \u{2713} <b>not markup</b>".into(),
        author: "Someone".into(),
    }
}

fn requests() -> Vec<RequestBody> {
    vec![
        RequestBody::ListAnnots { doc: 7, page: 3 },
        RequestBody::AddMarkup {
            doc: 7,
            page: 3,
            kind: AnnotKind::Highlight,
            color: [255, 235, 59, 255],
            start: 4,
            end: 19,
        },
        RequestBody::AddMarkup {
            doc: 7,
            page: 0,
            kind: AnnotKind::Strikeout,
            color: [0, 0, 0, 0],
            start: 0,
            end: MAX_MARKUP_CHARS as u32,
        },
        RequestBody::AddNote {
            doc: 7,
            page: 3,
            x: 72.5,
            y: -3.0,
            color: [1, 2, 3, 4],
            text: "blåbær \u{1F600}".into(),
        },
        RequestBody::AddInk {
            doc: 7,
            page: 3,
            color: [0, 0, 255, 255],
            width: 2.5,
            strokes: vec![vec![[1.0, 2.0], [3.0, 4.0]], vec![[5.0, 6.0]]],
        },
        RequestBody::AddStamp {
            doc: 7,
            page: 3,
            x: 100.0,
            y: 200.0,
            stamp: StampKind::Confidential,
        },
        RequestBody::UpdateAnnot {
            doc: 7,
            page: 3,
            id: 9,
            text: Some("new".into()),
            color: Some([9, 8, 7, 255]),
            dx: 1.5,
            dy: -2.5,
        },
        RequestBody::UpdateAnnot {
            doc: 7,
            page: 3,
            id: 9,
            text: None,
            color: None,
            dx: 0.0,
            dy: 0.0,
        },
        RequestBody::RemoveAnnot {
            doc: 7,
            page: 3,
            id: 9,
        },
        RequestBody::RestoreAnnot {
            doc: 7,
            page: 3,
            id: 9,
        },
    ]
}

fn encoded(body: RequestBody) -> Vec<u8> {
    Request { id: 1, body }.encode()
}

#[test]
fn annotation_requests_round_trip_and_have_one_encoding() {
    for body in requests() {
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
fn annotation_answers_round_trip() {
    for body in [
        ResponseBody::Annots { items: Vec::new() },
        ResponseBody::Annots {
            items: vec![info(1), info(2)],
        },
        ResponseBody::Annot { item: info(3) },
        ResponseBody::AnnotRemoved,
    ] {
        let res = Response { id: 5, body };
        let bytes = res.encode();
        assert_eq!(Response::decode(bytes.clone()).unwrap(), res);
        for cut in 0..bytes.len() {
            assert!(
                Response::decode(bytes[..cut].to_vec()).is_err(),
                "cut at {cut}"
            );
        }
    }
}

fn markup(kind: AnnotKind, start: u32, end: u32) -> RequestBody {
    RequestBody::AddMarkup {
        doc: 1,
        page: 0,
        kind,
        color: [0; 4],
        start,
        end,
    }
}

/// Where the kind byte of an `AddMarkup` sits: tag, id, doc, page.
const MARKUP_KIND_AT: usize = 1 + 4 + 4 + 4;

#[test]
fn markup_is_held_to_its_range_and_its_kinds() {
    // No characters, and a range that runs backwards.
    for (start, end) in [(5, 5), (9, 4)] {
        assert!(Request::decode(&encoded(markup(AnnotKind::Highlight, start, end))).is_err());
    }
    // One character more than a page holds, and exactly as many.
    let cap = MAX_MARKUP_CHARS as u32;
    assert!(Request::decode(&encoded(markup(AnnotKind::Highlight, 0, cap + 1))).is_err());
    assert!(Request::decode(&encoded(markup(AnnotKind::Highlight, 0, cap))).is_ok());
    assert!(Request::decode(&encoded(markup(AnnotKind::Highlight, u32::MAX - 5, u32::MAX))).is_ok());
    // Only three of the kinds are markup: the wire byte of the others is refused.
    for kind in [
        AnnotKind::Note,
        AnnotKind::Ink,
        AnnotKind::Stamp,
        AnnotKind::Other,
    ] {
        let mut bytes = encoded(markup(AnnotKind::Highlight, 0, 1));
        bytes[MARKUP_KIND_AT] = kind as u8;
        assert!(
            matches!(Request::decode(&bytes), Err(ProtocolError::UnknownValue(_))),
            "{kind:?}"
        );
    }
    let mut bytes = encoded(markup(AnnotKind::Highlight, 0, 1));
    bytes[MARKUP_KIND_AT] = 0;
    assert!(Request::decode(&bytes).is_err());
}

fn note(x: f32, text: String) -> RequestBody {
    RequestBody::AddNote {
        doc: 1,
        page: 0,
        x,
        y: 0.0,
        color: [0; 4],
        text,
    }
}

#[test]
fn note_text_is_held_to_the_cap_and_places_to_finite_numbers() {
    // The encoder cuts the text, so what it writes always decodes ...
    let long = "a".repeat(MAX_ANNOT_TEXT + 100);
    let RequestBody::AddNote { text, .. } = Request::decode(&encoded(note(1.0, long))).unwrap().body
    else {
        panic!("not a note");
    };
    assert_eq!(text.len(), MAX_ANNOT_TEXT);
    // ... and the decoder refuses text that is one byte over the cap.
    let mut bytes = encoded(note(1.0, String::new()));
    let at = bytes.len() - 4;
    bytes[at..at + 4].copy_from_slice(&(MAX_ANNOT_TEXT as u32 + 1).to_le_bytes());
    bytes.extend(std::iter::repeat_n(b'b', MAX_ANNOT_TEXT + 1));
    assert!(matches!(
        Request::decode(&bytes),
        Err(ProtocolError::StringTooLong(_))
    ));
    // Exactly at the cap is fine.
    let at_cap = "c".repeat(MAX_ANNOT_TEXT);
    assert!(Request::decode(&encoded(note(1.0, at_cap))).is_ok());
    // A place that is not a number, or is far off the page, is refused.
    for x in [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        MAX_COORDINATE * 2.0,
    ] {
        let mut bytes = encoded(note(0.0, String::new()));
        let at = 1 + 4 + 4 + 4;
        bytes[at..at + 4].copy_from_slice(&x.to_le_bytes());
        assert!(Request::decode(&bytes).is_err(), "{x}");
    }
    assert!(Request::decode(&encoded(note(MAX_COORDINATE, String::new()))).is_ok());
}

fn ink(width: f32, strokes: Vec<Vec<[f32; 2]>>) -> RequestBody {
    RequestBody::AddInk {
        doc: 1,
        page: 0,
        color: [0; 4],
        width,
        strokes,
    }
}

/// An `AddInk` with the strokes cut off: everything up to the number of strokes.
fn ink_head() -> Vec<u8> {
    let mut bytes = encoded(ink(1.0, vec![vec![[1.0, 1.0]]]));
    let strokes = 4 + 4 + 8;
    bytes.truncate(bytes.len() - strokes);
    bytes
}

#[test]
fn ink_is_held_to_its_caps() {
    // At the caps: accepted.
    let full: Vec<Vec<[f32; 2]>> = (0..MAX_INK_STROKES)
        .map(|_| vec![[1.0, 1.0]; MAX_INK_POINTS / MAX_INK_STROKES])
        .collect();
    assert!(Request::decode(&encoded(ink(1.0, full))).is_ok());
    let one: Vec<Vec<[f32; 2]>> = vec![vec![[1.0, 1.0]; MAX_INK_POINTS]];
    assert!(Request::decode(&encoded(ink(MAX_INK_WIDTH, one))).is_ok());
    assert!(Request::decode(&encoded(ink(MIN_INK_WIDTH, vec![vec![[0.0, 0.0]]]))).is_ok());

    // One stroke too many, built by hand because the encoder would leave it out.
    let mut bytes = ink_head();
    bytes.extend((MAX_INK_STROKES as u32 + 1).to_le_bytes());
    for _ in 0..=MAX_INK_STROKES {
        bytes.extend(1u32.to_le_bytes());
        bytes.extend([0u8; 8]);
    }
    assert!(matches!(
        Request::decode(&bytes),
        Err(ProtocolError::InvalidLength)
    ));

    // One point too many in one stroke.
    let mut bytes = ink_head();
    bytes.extend(1u32.to_le_bytes());
    bytes.extend((MAX_INK_POINTS as u32 + 1).to_le_bytes());
    bytes.extend(vec![0u8; (MAX_INK_POINTS + 1) * 8]);
    assert!(matches!(
        Request::decode(&bytes),
        Err(ProtocolError::InvalidLength)
    ));

    // Two strokes that together are one point over.
    let mut bytes = ink_head();
    bytes.extend(2u32.to_le_bytes());
    for n in [MAX_INK_POINTS - 1, 2] {
        bytes.extend((n as u32).to_le_bytes());
        bytes.extend(vec![0u8; n * 8]);
    }
    assert!(Request::decode(&bytes).is_err());

    // No strokes, a stroke without points, and a width outside the range.
    let mut none = ink_head();
    none.extend(0u32.to_le_bytes());
    assert!(Request::decode(&none).is_err());
    let mut empty = ink_head();
    empty.extend(1u32.to_le_bytes());
    empty.extend(0u32.to_le_bytes());
    assert!(Request::decode(&empty).is_err());
    for width in [0.0, MIN_INK_WIDTH / 2.0, MAX_INK_WIDTH + 0.5, f32::NAN, -1.0] {
        assert!(
            Request::decode(&encoded(ink(width, vec![vec![[1.0, 1.0]]]))).is_err(),
            "{width}"
        );
    }
    // A point that is not a number.
    for bad in [f32::NAN, f32::INFINITY] {
        assert!(Request::decode(&encoded(ink(1.0, vec![vec![[bad, 1.0]]]))).is_err());
    }
}

#[test]
fn ink_counts_that_do_not_fit_the_frame_are_refused_without_allocating() {
    // 256 strokes announced, nothing behind them.
    let mut bytes = ink_head();
    bytes.extend((MAX_INK_STROKES as u32).to_le_bytes());
    assert!(Request::decode(&bytes).is_err());
    // One stroke that announces 20 000 points and carries none.
    let mut bytes = ink_head();
    bytes.extend(1u32.to_le_bytes());
    bytes.extend((MAX_INK_POINTS as u32).to_le_bytes());
    assert!(Request::decode(&bytes).is_err());
}

#[test]
fn the_encoder_leaves_out_what_the_decoder_would_refuse() {
    let many: Vec<Vec<[f32; 2]>> = (0..MAX_INK_STROKES + 10)
        .map(|_| vec![[1.0, 2.0]])
        .collect();
    let RequestBody::AddInk { strokes, .. } =
        Request::decode(&encoded(ink(1.0, many))).unwrap().body
    else {
        panic!("not ink");
    };
    assert_eq!(strokes.len(), MAX_INK_STROKES);
    let heavy: Vec<Vec<[f32; 2]>> = vec![vec![[1.0, 2.0]; MAX_INK_POINTS], vec![[3.0, 4.0]]];
    let RequestBody::AddInk { strokes, .. } =
        Request::decode(&encoded(ink(1.0, heavy))).unwrap().body
    else {
        panic!("not ink");
    };
    assert_eq!(strokes.len(), 1);
    assert_eq!(strokes[0].len(), MAX_INK_POINTS);
}

#[test]
fn stamps_and_update_flags_are_closed_sets() {
    let mut bytes = encoded(RequestBody::AddStamp {
        doc: 1,
        page: 0,
        x: 1.0,
        y: 1.0,
        stamp: StampKind::Draft,
    });
    let last = bytes.len() - 1;
    for bad in [0u8, 7, 255] {
        bytes[last] = bad;
        assert!(matches!(
            Request::decode(&bytes),
            Err(ProtocolError::UnknownValue(_))
        ));
    }
    for (i, stamp) in StampKind::ALL.iter().enumerate() {
        assert_eq!(*stamp as u8, i as u8 + 1);
        assert_eq!(StampKind::from_u8(*stamp as u8).unwrap(), *stamp);
        assert!(stamp.label().is_ascii());
        assert!(!stamp.name().is_empty());
    }
    let mut bytes = encoded(RequestBody::UpdateAnnot {
        doc: 1,
        page: 0,
        id: 1,
        text: None,
        color: None,
        dx: 0.0,
        dy: 0.0,
    });
    let flags = 1 + 4 + 4 + 4 + 4;
    bytes[flags] = 4;
    assert!(matches!(
        Request::decode(&bytes),
        Err(ProtocolError::UnknownValue(4))
    ));
    // The flag says there is text and there is none.
    bytes[flags] = 1;
    assert!(Request::decode(&bytes).is_err());
}

fn annot_answer(item: AnnotInfo) -> Vec<u8> {
    Response {
        id: 1,
        body: ResponseBody::Annot { item },
    }
    .encode()
}

#[test]
fn listed_annotations_are_held_to_their_caps() {
    // The encoder cuts the strings and the list, and the result decodes.
    let mut long = info(1);
    long.text = "t".repeat(MAX_ANNOT_TEXT + 50);
    long.author = "é".repeat(MAX_ANNOT_AUTHOR);
    let many: Vec<AnnotInfo> = (0..MAX_ANNOTS_PER_PAGE as u32 + 20)
        .map(|i| {
            let mut item = info(i);
            item.text = String::new();
            item
        })
        .collect();
    let bytes = Response {
        id: 1,
        body: ResponseBody::Annots { items: many },
    }
    .encode();
    let ResponseBody::Annots { items } = Response::decode(bytes).unwrap().body else {
        panic!("not a list");
    };
    assert_eq!(items.len(), MAX_ANNOTS_PER_PAGE);
    let ResponseBody::Annot { item } = Response::decode(annot_answer(long)).unwrap().body else {
        panic!("not an annotation");
    };
    assert_eq!(item.text.len(), MAX_ANNOT_TEXT);
    assert!(item.author.len() <= MAX_ANNOT_AUTHOR);

    // The decoder refuses a list that is one entry too long, and a count the frame cannot hold.
    let mut bytes = Response {
        id: 1,
        body: ResponseBody::Annots {
            items: vec![info(1)],
        },
    }
    .encode();
    bytes[5..9].copy_from_slice(&(MAX_ANNOTS_PER_PAGE as u32 + 1).to_le_bytes());
    assert!(matches!(
        Response::decode(bytes.clone()),
        Err(ProtocolError::InvalidLength)
    ));
    bytes[5..9].copy_from_slice(&(MAX_ANNOTS_PER_PAGE as u32).to_le_bytes());
    assert!(matches!(
        Response::decode(bytes),
        Err(ProtocolError::InvalidLength)
    ));

    // Text one byte over the cap, an author one byte over, an ability nobody defined, a kind
    // nobody defined.
    let good = annot_answer(info(1));
    let mut bad = good.clone();
    bad[5 + 4 + 1] = 0x80;
    assert!(Response::decode(bad).is_err());
    let mut bad = good.clone();
    bad[5 + 4] = 0;
    assert!(Response::decode(bad).is_err());
    let mut bad = good.clone();
    bad[5 + 4] = 8;
    assert!(Response::decode(bad).is_err());
    let text_at = 5 + 4 + 1 + 1 + 16 + 4;
    let mut bad = good[..text_at].to_vec();
    bad.extend((MAX_ANNOT_TEXT as u32 + 1).to_le_bytes());
    bad.extend(vec![b'x'; MAX_ANNOT_TEXT + 1]);
    bad.extend(0u32.to_le_bytes());
    assert!(matches!(
        Response::decode(bad),
        Err(ProtocolError::StringTooLong(_))
    ));
    let mut bad = good[..text_at].to_vec();
    bad.extend(0u32.to_le_bytes());
    bad.extend((MAX_ANNOT_AUTHOR as u32 + 1).to_le_bytes());
    bad.extend(vec![b'x'; MAX_ANNOT_AUTHOR + 1]);
    assert!(matches!(
        Response::decode(bad),
        Err(ProtocolError::StringTooLong(_))
    ));
}

#[test]
fn a_box_that_is_not_a_number_becomes_zero() {
    let mut item = info(1);
    item.rect = [f32::NAN, f32::INFINITY, 3.0, f32::NEG_INFINITY];
    let ResponseBody::Annot { item } = Response::decode(annot_answer(item)).unwrap().body else {
        panic!("not an annotation");
    };
    assert_eq!(item.rect, [0.0, 0.0, 3.0, 0.0]);
}

#[test]
fn the_abilities_that_leave_the_encoder_are_only_defined_ones() {
    let mut item = info(1);
    item.abilities = 0xFF;
    let ResponseBody::Annot { item } = Response::decode(annot_answer(item)).unwrap().body else {
        panic!("not an annotation");
    };
    assert_eq!(item.abilities, ability::ALL);
}
