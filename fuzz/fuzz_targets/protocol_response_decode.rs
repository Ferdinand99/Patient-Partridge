#![no_main]
//! Arbitrary bytes into `Response::decode`. This is the app's view of the engine, and the engine
//! is the part that handles hostile PDFs: after an exploit the bytes could be anything.
//!
//! A response that decodes must be inside every cap of the protocol (list lengths, string
//! lengths, outline depth, bitmap size), and must survive encode and decode again with the same
//! bytes the second time. (Not the same as the input: boxes that are not finite are turned into
//! zeros.)
//!
//! The seeds include responses that are one over a cap (an outline with 5001 entries, 501 links,
//! 2001 matches, strings one byte too long, a bitmap one pixel too wide), so the cap checks are
//! reached without luck. That needs a large `-max_len`.

use libfuzzer_sys::fuzz_target;
use partridge_fuzz::check_response_caps;
use partridge_protocol::Response;

fuzz_target!(|data: &[u8]| {
    match Response::decode(data.to_vec()) {
        Ok(response) => {
            check_response_caps(&response);
            let encoded = response.encode();
            let again = Response::decode(encoded.clone()).expect("re-decode of own encoding");
            assert_eq!(again.encode(), encoded, "encoding is not stable");
        }
        Err(e) => {
            let _ = e.to_string();
        }
    }
});
