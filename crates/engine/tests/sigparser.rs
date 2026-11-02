//! The thread that decodes signatures: the caller waits for it only until the time of the check is
//! up, and a decoding that nobody waits for any more does not queue more work behind it. The test
//! has a binary of its own because it starts the thread and slows it down for the whole process.

use std::time::{Duration, Instant};

use partridge_engine::sigcheck;
use partridge_protocol::SignatureReason as R;
use partridge_testkit::signed::*;

/// The signature of a file made by the testkit, as DER without the padding.
fn signature_der() -> Vec<u8> {
    let signer = Identity::self_signed(KeyKind::P256, &CertSpec::new("Test Signer"));
    let file = signed_file(&signer, &SignOptions::default());
    let hex = &file.bytes[file.contents_at + 1..file.contents_end - 1];
    let contents: Vec<u8> = hex
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    let len = sigcheck::der_element_len(&contents).expect("one element");
    contents[..len].to_vec()
}

#[test]
fn the_decoding_is_waited_for_only_until_the_time_is_up() {
    assert!(sigcheck::start_parser());
    // Starting it twice is no harm.
    assert!(sigcheck::start_parser());
    let der = signature_der();

    // Quick work comes back as it would without the thread, with a time or without one.
    assert!(sigcheck::parse_within(&der, None).is_ok());
    let soon = Some(Instant::now() + Duration::from_secs(30));
    assert!(sigcheck::parse_within(&der, soon).is_ok());
    // A shape that is refused is refused before the thread is asked.
    assert_eq!(
        sigcheck::parse_within(&der[..der.len() - 1], None).err(),
        Some(R::EncodingUnsupported)
    );

    // A thread that takes long: the caller is told "too large" when its time is up, long before
    // the thread is done.
    sigcheck::set_parse_delay_for_tests(2_000);
    let started = Instant::now();
    let answer = sigcheck::parse_within(&der, Some(Instant::now() + Duration::from_millis(200)));
    assert_eq!(answer.err(), Some(R::TooLarge));
    assert!(
        started.elapsed() < Duration::from_millis(1_500),
        "waited {:?}",
        started.elapsed()
    );

    // While it is still busy with that one, the next is refused at once and nothing is queued.
    let started = Instant::now();
    let answer = sigcheck::parse_within(&der, Some(Instant::now() + Duration::from_secs(30)));
    assert_eq!(answer.err(), Some(R::TooLarge));
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "waited {:?}",
        started.elapsed()
    );

    // When it is done it takes work again.
    sigcheck::set_parse_delay_for_tests(0);
    let give_up = Instant::now() + Duration::from_secs(20);
    loop {
        match sigcheck::parse_within(&der, Some(Instant::now() + Duration::from_secs(5))) {
            Ok(_) => break,
            Err(R::TooLarge) if Instant::now() < give_up => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(other) => panic!("the thread did not take work again: {other:?}"),
        }
    }
}
