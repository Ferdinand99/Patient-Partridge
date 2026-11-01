#![no_main]
//! Arbitrary bytes into `Request::decode`. This is the engine's input: the app is trusted, but
//! the engine must still never panic, hang or allocate wildly on a damaged frame.
//!
//! Every request has exactly one encoding, so a payload that decodes must encode back to the
//! same bytes.

use libfuzzer_sys::fuzz_target;
use partridge_protocol::Request;

fuzz_target!(|data: &[u8]| {
    match Request::decode(data) {
        Ok(request) => {
            let encoded = request.encode();
            assert_eq!(encoded, data, "a request has more than one encoding");
            assert_eq!(Request::decode(&encoded).expect("re-decode"), request);
        }
        Err(e) => {
            // The message text is what the logs show: it must be possible to build it.
            let _ = e.to_string();
        }
    }
});
