#![no_main]
//! Arbitrary bytes as a certificate file (a PKCS #12 file) and a password, into the loader of the
//! signer: the code that opens the file, pairs the key with its certificate and builds the chain.
//! The file comes from outside and is parsed by crates that are young, in the one process that
//! holds the key.
//!
//! The first byte says how long the password is (it is cut from the front of the rest, and at most
//! sixteen bytes long); the other bytes are the file. The seeds are the files of the tests, so that
//! the fuzzer starts from something that gets past the first checks.
//!
//! The loader must not panic, hang or use more memory than its limits allow. A file that is taken
//! must give a certificate inside the caps of the protocol, a chain of at most the cap, and a
//! signature (over a digest of the right length) that is no longer than the loader said it would be.

use libfuzzer_sys::fuzz_target;
use partridge_signer::{load, sign};
use partridge_signer_protocol::{ErrorCode, MAX_CHAIN, MAX_CMS_BYTES};

fuzz_target!(|data: &[u8]| {
    let Some((&n, rest)) = data.split_first() else {
        return;
    };
    let n = (n as usize).min(rest.len()).min(16);
    let (password, file) = rest.split_at(n);
    let password = String::from_utf8_lossy(password).into_owned();
    match load(file, &password) {
        Ok(identity) => {
            let c = identity.certificate();
            assert!((1..=MAX_CHAIN).contains(&c.chain));
            assert!(c.signature_bytes as usize <= MAX_CMS_BYTES);
            assert_eq!(c.fingerprint.len(), 64);
            assert!(c
                .fingerprint
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
            let digest = vec![7u8; c.hash().len()];
            let cms = sign(&identity, &digest).expect("a key that was taken signs");
            assert!(cms.len() <= c.signature_bytes as usize);
            assert_eq!(cms[0], 0x30);
        }
        Err(e) => {
            assert!(!e.message.is_empty());
            assert_ne!(e.code, ErrorCode::NotLoaded);
        }
    }
});
