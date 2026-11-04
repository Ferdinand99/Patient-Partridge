#![no_main]
//! Arbitrary bytes into the decoders of the messages between the app and the signer process, and
//! into the frame reader. The signer reads what the app sends and the app reads what the signer
//! sends: either side may be damaged, and the signer is the one that holds the key.
//!
//! A message that decodes must encode back to the same bytes (every message has one encoding, and
//! the encoder cuts what is over a cap, so a missing cap in the decoder shows up here), must be
//! inside the caps, and the frame reader must refuse a length over the cap before it allocates.

use libfuzzer_sys::fuzz_target;
use partridge_signer_protocol::{
    read_frame, Request, Response, MAX_CHAIN, MAX_CMS_BYTES, MAX_P12_BYTES, MAX_PASSWORD_BYTES,
    MAX_TEXT_BYTES,
};

fuzz_target!(|data: &[u8]| {
    match Request::decode(data) {
        Ok(request) => {
            assert_eq!(request.encode(), data, "a request has more than one encoding");
            match &request {
                Request::Load { p12, password } => {
                    assert!(p12.len() <= MAX_P12_BYTES && password.len() <= MAX_PASSWORD_BYTES);
                }
                Request::Sign { digest } => assert!(digest.len() <= 48),
                _ => {}
            }
        }
        Err(e) => {
            let _ = e.to_string();
        }
    }
    match Response::decode(data) {
        Ok(response) => {
            assert_eq!(response.encode(), data, "a response has more than one encoding");
            match &response {
                Response::Signed { cms } => assert!(cms.len() <= MAX_CMS_BYTES),
                Response::Loaded(c) => {
                    assert!(c.name.len() <= MAX_TEXT_BYTES && c.issuer.len() <= MAX_TEXT_BYTES);
                    assert!(c.signature_bytes as usize <= MAX_CMS_BYTES);
                    assert!((1..=MAX_CHAIN).contains(&c.chain));
                    assert!(c.fingerprint.len() == 64);
                }
                _ => {}
            }
        }
        Err(e) => {
            let _ = e.to_string();
        }
    }
    // The same bytes as a stream of frames.
    let mut stream = data;
    while let Ok(Some(frame)) = read_frame(&mut stream) {
        let _ = Request::decode(&frame);
        let _ = Response::decode(&frame);
    }
});
