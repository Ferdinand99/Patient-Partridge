#![no_main]
//! Valid requests built from the fuzzer's bytes must come through encode, framing and decode
//! unchanged. Includes strings of exactly the longest allowed size and multi-byte characters.
//!
//! One run in eight builds a request that is one byte over a cap instead (open password, search
//! text, a piece of a saved file longer than a piece may be). The encoder cannot write such a request, so the bytes are written by
//! hand, and the decoder must refuse it.

use std::io::Cursor;

use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;
use partridge_fuzz::{arb_request, chance, over_cap_request, ChunkReader};
use partridge_protocol::{read_frame, write_request, Request};

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    if matches!(chance(&mut u, 8), Ok(true)) {
        let Ok((name, payload)) = over_cap_request(&mut u) else {
            return;
        };
        assert!(
            Request::decode(&payload).is_err(),
            "the decoder accepted: {name}"
        );
        return;
    }
    let Ok(request) = arb_request(&mut u) else {
        return;
    };
    let payload = request.encode();
    assert_eq!(
        Request::decode(&payload).expect("decode own encoding"),
        request
    );

    let mut wire = Vec::new();
    write_request(&mut wire, &request).expect("write");
    assert_eq!(&wire[..4], &(payload.len() as u32).to_le_bytes());
    assert_eq!(&wire[4..], &payload[..]);
    let frame = read_frame(&mut Cursor::new(&wire))
        .expect("read")
        .expect("one frame");
    assert_eq!(frame, payload);
    // The same through a reader that gives one byte at a time.
    let one_byte = read_frame(&mut ChunkReader::new(&wire, &[1]))
        .expect("read")
        .expect("one frame");
    assert_eq!(one_byte, payload);

    // One byte less is never a valid message.
    assert!(Request::decode(&payload[..payload.len() - 1]).is_err());
});
