#![no_main]
//! Valid responses of all nine kinds built from the fuzzer's bytes must come through encode,
//! framing and decode unchanged (boxes that were not finite come back as zeros). Lists sometimes
//! have the most entries the protocol allows.
//!
//! One run in eight builds a response that is one over a cap instead (an outline with 5001
//! entries, an entry one level too deep, a title one byte too long, 501 links, an address one
//! byte too long, 2001 matches, 100001 characters of page text, strings one byte too long, a
//! bitmap one pixel too wide). The encoder cannot write such a response, so the bytes are written
//! by hand, and the decoder must refuse it. Two cases sit exactly on a cap and must be accepted.

use std::io::Cursor;

use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;
use partridge_fuzz::{arb_response, chance, check_response_caps, over_cap_response};
use partridge_protocol::{read_frame, write_response, Response};

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    if matches!(chance(&mut u, 8), Ok(true)) {
        let Ok((name, payload, accepted)) = over_cap_response(&mut u) else {
            return;
        };
        match Response::decode(payload) {
            Ok(response) => {
                assert!(accepted, "the decoder accepted: {name}");
                check_response_caps(&response);
            }
            Err(e) => assert!(!accepted, "the decoder refused: {name} ({e})"),
        }
        return;
    }
    let Ok(generated) = arb_response(&mut u) else {
        return;
    };
    let payload = generated.sent.encode();
    let decoded = Response::decode(payload.clone()).expect("decode own encoding");
    assert_eq!(decoded, generated.expected);
    check_response_caps(&decoded);

    // `write_response` sends a bitmap without copying it first; the bytes must be the same.
    let mut wire = Vec::new();
    write_response(&mut wire, &generated.sent).expect("write");
    assert_eq!(&wire[..4], &(payload.len() as u32).to_le_bytes());
    assert_eq!(&wire[4..], &payload[..]);
    let frame = read_frame(&mut Cursor::new(&wire))
        .expect("read")
        .expect("one frame");
    assert_eq!(
        Response::decode(frame).expect("decode frame"),
        generated.expected
    );

    // One byte less is never a valid message.
    assert!(Response::decode(payload[..payload.len() - 1].to_vec()).is_err());
});
