//! Tests of the form messages: that they come back the same, that the encoder keeps to the caps,
//! and that the decoder refuses what is over them.

use super::*;
use crate::{Request, RequestBody, Response, ResponseBody};

fn field(index: u32, kind: FormFieldKind) -> FormField {
    FormField {
        index,
        kind,
        flags: field_flag::REQUIRED | field_flag::CHECKED,
        turns: 1,
        rect: [10.5, 20.25, 110.0, 40.0],
        font_size: 11.5,
        name: "person.first".into(),
        tooltip: "Your first name \u{2713}".into(),
        value: "Ada\nLovelace".into(),
        max_len: 40,
        options: vec![
            FormOption {
                selected: true,
                label: "Norway".into(),
            },
            FormOption {
                selected: false,
                label: "\u{c5}land".into(),
            },
        ],
    }
}

fn roundtrip_response(fields: Vec<FormField>) -> Vec<FormField> {
    let bytes = Response {
        id: 9,
        body: ResponseBody::FormFields { fields },
    }
    .encode();
    match Response::decode(bytes).expect("decodes").body {
        ResponseBody::FormFields { fields } => fields,
        other => panic!("{other:?}"),
    }
}

#[test]
fn every_form_message_comes_back_the_same() {
    let requests = [
        RequestBody::FormInfo { doc: 3 },
        RequestBody::ListFormFields { doc: 3, page: 7 },
        RequestBody::SetFormField {
            doc: 3,
            page: 7,
            index: 2,
            name: "agree".into(),
            value: FormValue::Checked(true),
        },
        RequestBody::SetFormField {
            doc: 3,
            page: 0,
            index: 0,
            name: "notes".into(),
            value: FormValue::Text("one\ntwo \u{2713}".into()),
        },
        RequestBody::SetFormField {
            doc: 3,
            page: 0,
            index: 1,
            name: "colours".into(),
            value: FormValue::Choice(vec![0, 2, 5]),
        },
    ];
    for body in requests {
        let request = Request {
            id: 11,
            body: body.clone(),
        };
        assert_eq!(Request::decode(&request.encode()).unwrap().body, body);
    }
    let responses = [
        ResponseBody::FormInfo {
            kind: FormKind::Xfa,
            has_actions: true,
        },
        ResponseBody::FormInfo {
            kind: FormKind::None,
            has_actions: false,
        },
        ResponseBody::FormFields { fields: vec![] },
        ResponseBody::FormFields {
            fields: vec![
                field(0, FormFieldKind::Text),
                field(1, FormFieldKind::Combo),
            ],
        },
    ];
    for body in responses {
        let response = Response {
            id: 12,
            body: body.clone(),
        };
        assert_eq!(Response::decode(response.encode()).unwrap().body, body);
    }
}

#[test]
fn the_encoder_cuts_what_is_over_the_caps_so_the_result_still_decodes() {
    let mut long = field(0, FormFieldKind::List);
    long.name = "n".repeat(MAX_FORM_NAME + 100);
    long.tooltip = "\u{2713}".repeat(MAX_FORM_TOOLTIP);
    long.value = "v".repeat(MAX_FORM_VALUE + 1);
    long.options = (0..MAX_FORM_OPTIONS + 50)
        .map(|i| FormOption {
            selected: i % 2 == 0,
            label: "\u{c5}".repeat(MAX_FORM_OPTION_LABEL),
        })
        .collect();
    let back = roundtrip_response(vec![long]);
    assert_eq!(back.len(), 1);
    let f = &back[0];
    assert_eq!(f.name.len(), MAX_FORM_NAME);
    assert!(f.tooltip.len() <= MAX_FORM_TOOLTIP && f.tooltip.chars().all(|c| c == '\u{2713}'));
    assert_eq!(f.value.len(), MAX_FORM_VALUE);
    assert_eq!(f.options.len(), MAX_FORM_OPTIONS);
    assert!(f
        .options
        .iter()
        .all(|o| o.label.len() <= MAX_FORM_OPTION_LABEL));
}

#[test]
fn a_page_lists_at_most_the_most_fields_and_the_most_bytes() {
    let many: Vec<FormField> = (0..MAX_FORM_FIELDS_PER_PAGE as u32 + 20)
        .map(|i| field(i, FormFieldKind::Text))
        .collect();
    assert_eq!(roundtrip_response(many).len(), MAX_FORM_FIELDS_PER_PAGE);
    // A thousand fields with the longest values are more than the answer may hold: the ones that
    // do not fit are left out, and what is sent is under the limit.
    let heavy: Vec<FormField> = (0..MAX_FORM_FIELDS_PER_PAGE as u32)
        .map(|i| {
            let mut f = field(i, FormFieldKind::Text);
            f.value = "w".repeat(MAX_FORM_VALUE);
            f
        })
        .collect();
    let bytes = Response {
        id: 1,
        body: ResponseBody::FormFields { fields: heavy },
    }
    .encode();
    assert!(bytes.len() < MAX_FORM_ANSWER_BYTES, "{} bytes", bytes.len());
    match Response::decode(bytes).unwrap().body {
        ResponseBody::FormFields { fields } => {
            assert!(!fields.is_empty() && fields.len() < MAX_FORM_FIELDS_PER_PAGE);
        }
        other => panic!("{other:?}"),
    }
}

/// The bytes of a `FormFields` answer for one field, written by hand so that a test can make one
/// number wrong.
fn raw_field(
    kind: u8,
    flags: u16,
    turns: u8,
    name_len: usize,
    value_len: usize,
    options: u32,
) -> Vec<u8> {
    let mut out = vec![171u8];
    out.extend_from_slice(&5u32.to_le_bytes()); // id
    out.extend_from_slice(&1u32.to_le_bytes()); // one field
    out.extend_from_slice(&0u32.to_le_bytes()); // index
    out.push(kind);
    out.extend_from_slice(&flags.to_le_bytes());
    out.push(turns);
    for v in [0.0f32, 0.0, 10.0, 10.0, 0.0] {
        out.extend_from_slice(&v.to_le_bytes()); // box and font size
    }
    out.extend_from_slice(&(name_len as u32).to_le_bytes());
    out.extend(std::iter::repeat_n(b'n', name_len));
    out.extend_from_slice(&0u32.to_le_bytes()); // tooltip
    out.extend_from_slice(&(value_len as u32).to_le_bytes());
    out.extend(std::iter::repeat_n(b'v', value_len));
    out.extend_from_slice(&0u32.to_le_bytes()); // max length
    out.extend_from_slice(&options.to_le_bytes());
    out
}

#[test]
fn the_decoder_refuses_a_field_that_is_over_a_cap_or_has_a_value_it_does_not_know() {
    assert!(Response::decode(raw_field(1, 0, 0, 3, 3, 0)).is_ok());
    // At each cap, and one byte over it.
    assert!(Response::decode(raw_field(1, 0, 0, MAX_FORM_NAME, 3, 0)).is_ok());
    assert!(Response::decode(raw_field(1, 0, 0, MAX_FORM_NAME + 1, 3, 0)).is_err());
    assert!(Response::decode(raw_field(1, 0, 0, 3, MAX_FORM_VALUE, 0)).is_ok());
    assert!(Response::decode(raw_field(1, 0, 0, 3, MAX_FORM_VALUE + 1, 0)).is_err());
    // More choices than the cap, or than the bytes that are there.
    assert!(Response::decode(raw_field(4, 0, 0, 3, 3, MAX_FORM_OPTIONS as u32 + 1)).is_err());
    assert!(Response::decode(raw_field(4, 0, 0, 3, 3, 1)).is_err());
    assert!(Response::decode(raw_field(4, 0, 0, 3, 3, u32::MAX)).is_err());
    // A kind, flag bits and a turn that do not exist.
    assert!(Response::decode(raw_field(0, 0, 0, 3, 3, 0)).is_err());
    assert!(Response::decode(raw_field(6, 0, 0, 3, 3, 0)).is_err());
    assert!(Response::decode(raw_field(1, 1 << 12, 0, 3, 3, 0)).is_err());
    assert!(Response::decode(raw_field(1, 0x8000, 0, 3, 3, 0)).is_err());
    assert!(Response::decode(raw_field(1, 0, 4, 3, 3, 0)).is_err());
    assert!(Response::decode(raw_field(1, field_flag::ALL, 3, 3, 3, 0)).is_ok());
}

#[test]
fn the_decoder_refuses_more_fields_than_the_cap_or_than_the_bytes_hold() {
    // One field more than a page may have, all of them there: only the cap itself refuses this.
    let one = raw_field(1, 0, 0, 1, 1, 0);
    let list = |count: usize| {
        let mut bytes = vec![171u8];
        bytes.extend_from_slice(&5u32.to_le_bytes());
        bytes.extend_from_slice(&(count as u32).to_le_bytes());
        for _ in 0..count {
            bytes.extend_from_slice(&one[9..]);
        }
        bytes
    };
    assert!(Response::decode(list(MAX_FORM_FIELDS_PER_PAGE)).is_ok());
    assert!(Response::decode(list(MAX_FORM_FIELDS_PER_PAGE + 1)).is_err());
    for count in [MAX_FORM_FIELDS_PER_PAGE as u32 + 1, 50, u32::MAX] {
        let mut bytes = vec![171u8];
        bytes.extend_from_slice(&5u32.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        assert!(Response::decode(bytes).is_err(), "{count}");
    }
}

#[test]
fn a_request_over_a_cap_does_not_decode() {
    let request = |name_len: usize, value: Vec<u8>| {
        let mut out = vec![42u8];
        for v in [1u32, 2, 3, 4] {
            out.extend_from_slice(&v.to_le_bytes()); // id, doc, page, index
        }
        out.extend_from_slice(&(name_len as u32).to_le_bytes());
        out.extend(std::iter::repeat_n(b'n', name_len));
        out.extend(value);
        out
    };
    let text = |len: usize| {
        let mut v = vec![1u8];
        v.extend_from_slice(&(len as u32).to_le_bytes());
        v.extend(std::iter::repeat_n(b't', len));
        v
    };
    assert!(Request::decode(&request(3, text(MAX_FORM_VALUE))).is_ok());
    assert!(Request::decode(&request(3, text(MAX_FORM_VALUE + 1))).is_err());
    assert!(Request::decode(&request(MAX_FORM_NAME, text(1))).is_ok());
    assert!(Request::decode(&request(MAX_FORM_NAME + 1, text(1))).is_err());
    let choice = |count: u32, given: usize| {
        let mut v = vec![3u8];
        v.extend_from_slice(&count.to_le_bytes());
        v.extend(std::iter::repeat_n(0u8, given * 4));
        v
    };
    let most = MAX_FORM_OPTIONS;
    assert!(Request::decode(&request(3, choice(most as u32, most))).is_ok());
    assert!(Request::decode(&request(3, choice(most as u32 + 1, most + 1))).is_err());
    assert!(Request::decode(&request(3, choice(u32::MAX, 2))).is_err());
    // A switch that is neither on nor off, and a kind of value that does not exist.
    assert!(Request::decode(&request(3, vec![2, 2])).is_err());
    assert!(Request::decode(&request(3, vec![9, 0])).is_err());
}

#[test]
fn no_cut_message_decodes_without_an_error_or_a_panic() {
    let bytes = Response {
        id: 3,
        body: ResponseBody::FormFields {
            fields: vec![field(0, FormFieldKind::Combo)],
        },
    }
    .encode();
    for end in 0..bytes.len() {
        assert!(
            Response::decode(bytes[..end].to_vec()).is_err(),
            "cut at {end}"
        );
    }
    let request = Request {
        id: 3,
        body: RequestBody::SetFormField {
            doc: 1,
            page: 1,
            index: 1,
            name: "a".into(),
            value: FormValue::Choice(vec![1, 2]),
        },
    }
    .encode();
    for end in 0..request.len() {
        assert!(Request::decode(&request[..end]).is_err(), "cut at {end}");
    }
}

#[test]
fn a_box_that_is_not_a_number_becomes_zero_and_a_font_size_cannot_be_negative() {
    let mut f = field(0, FormFieldKind::Text);
    f.rect = [f32::NAN, f32::INFINITY, 1.0, 2.0];
    f.font_size = -3.0;
    let back = roundtrip_response(vec![f]);
    assert_eq!(back[0].rect, [0.0, 0.0, 1.0, 2.0]);
    assert_eq!(back[0].font_size, 0.0);
}
