#![no_main]
//! Arbitrary bytes into `Response::decode`. This is the app's view of the engine, and the engine
//! is the part that handles hostile PDFs: after an exploit the bytes could be anything.
//!
//! A response that decodes must survive encode and decode again, with the same bytes the second
//! time. (Not the same as the input: boxes that are not finite are turned into zeros.)

use libfuzzer_sys::fuzz_target;
use partridge_protocol::Response;

fuzz_target!(|data: &[u8]| {
    match Response::decode(data.to_vec()) {
        Ok(response) => {
            let encoded = response.encode();
            let again = Response::decode(encoded.clone()).expect("re-decode of own encoding");
            assert_eq!(again.encode(), encoded, "encoding is not stable");
        }
        Err(e) => {
            let _ = e.to_string();
        }
    }
});
