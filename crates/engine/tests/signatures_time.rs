//! What the check of the signatures reports when its time runs out. The test has a binary of its
//! own because it starts the thread that decodes signatures, slows it down and shortens the time
//! limit for the whole process.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use partridge_engine::sigcheck::{set_parse_delay_for_tests, start_parser};
use partridge_engine::signatures::set_check_time_for_tests;
use partridge_engine::Engine;
use partridge_protocol::{
    ErrorCode, Request, RequestBody, ResponseBody, SignatureReason as R, SignatureReport,
    SignatureState as S,
};
use partridge_testkit as kit;
use partridge_testkit::signed::*;

mod common;

struct Shared(Engine);
unsafe impl Send for Shared {}

fn p256(name: &str) -> Identity {
    Identity::self_signed(KeyKind::P256, &CertSpec::new(name))
}

fn open(engine: &mut Engine, bytes: &[u8]) -> (u32, common::Controls) {
    let (file, controls) = common::ControlledFile::new(bytes);
    engine.set_document_source(Box::new(file));
    let opened = engine
        .handle(Request {
            id: 1,
            body: RequestBody::Open { password: None },
        })
        .body;
    match opened {
        ResponseBody::Opened { doc, .. } => (doc, controls),
        other => panic!("{other:?}"),
    }
}

/// Asks for the signatures; `progress` hears each sign of life.
fn question(
    engine: &mut Engine,
    doc: u32,
    progress: &mut dyn FnMut(u32),
) -> (ResponseBody, Duration) {
    let started = Instant::now();
    let body = engine
        .handle_with_progress(
            Request {
                id: 1,
                body: RequestBody::Signatures { doc, now: NOW },
            },
            progress,
        )
        .body;
    (body, started.elapsed())
}

fn report_of(body: ResponseBody) -> SignatureReport {
    match body {
        ResponseBody::Signatures { report } => report,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_check_that_runs_out_of_time_keeps_what_it_finished_and_never_calls_the_rest_intact() {
    let mut shared = Shared(Engine::new(&kit::pdfium_dir()));
    let engine = &mut shared.0;
    assert!(start_parser());

    // 1. The decoding of one signature is not done in time, and the other was. The report is not
    // lost: the signature that was finished keeps its result (the second signature changed what
    // the first covers, so it says so), the other is "not checked", and the request ends at the
    // time of the signatures, with the time of the last look to spare.
    let one = signed_file(&p256("First Signer"), &SignOptions::default());
    let two = add_signature(&one, &p256("Second Signer"), &SignOptions::default());
    set_check_time_for_tests(3_000);
    set_parse_delay_for_tests(1_500);
    let (doc, _controls) = open(engine, &two.bytes);
    let (body, took) = question(engine, doc, &mut |_| {});
    let report = report_of(body);
    assert_eq!((report.total, report.items.len()), (2, 2));
    // A signature that was not decoded in time never got to say who signed.
    let waiting: Vec<_> = report.items.iter().filter(|i| i.signer.is_none()).collect();
    let finished: Vec<_> = report.items.iter().filter(|i| i.signer.is_some()).collect();
    assert_eq!((waiting.len(), finished.len()), (1, 1), "{report:?}");
    assert_eq!(
        (waiting[0].state, waiting[0].reason),
        (S::Unsupported, R::TooLarge)
    );
    assert_ne!(finished[0].reason, R::TooLarge, "{:?}", finished[0]);
    assert!(took < Duration::from_millis(3_000), "took {took:?}");
    set_parse_delay_for_tests(0);
    // The thread finishes the decoding nobody waited for before it takes work again.
    std::thread::sleep(Duration::from_millis(1_800));

    // 2. The last look at the file has the time that was kept for it, and no more. The file is big
    // enough that the look and the scan tell the app now and then that they are at work. Without
    // the slow-down the signature holds.
    let big = signed_file(
        &p256("Test Signer"),
        &SignOptions {
            hidden_text: Some("x".repeat(1_200_000)),
            ..SignOptions::default()
        },
    );
    set_check_time_for_tests(2_000);
    let (doc, _controls) = open(engine, &big.bytes);
    let report = report_of(question(engine, doc, &mut |_| {}).0);
    assert_eq!(
        (report.items[0].state, report.items[0].reason),
        (S::UnknownSigner, R::NoTrustList)
    );
    // The same file, and now every read takes a tenth of a second from the second sign of life on:
    // the first is the scan, the second comes while the covered bytes are hashed. The look at the
    // file afterwards cannot finish before the end of the time limit. The signature held, but
    // nothing could show that the file was still the one that was read: it is "not checked".
    // (The plain look, with no time limit, would take about 3 seconds more and say "intact".)
    let (doc, controls) = open(engine, &big.bytes);
    let slow = controls.slow.clone();
    let mut signs = 0;
    let (body, took) = question(engine, doc, &mut |_| {
        signs += 1;
        if signs == 2 {
            slow.store(true, Ordering::SeqCst);
        }
    });
    let report = report_of(body);
    assert!(signs >= 2, "{signs} signs of life");
    let item = &report.items[0];
    assert_eq!(
        (item.state, item.reason),
        (S::Unsupported, R::TooLarge),
        "{item:?}"
    );
    assert!(item.trust_anchor.is_none());
    assert!(
        took < Duration::from_millis(2_600),
        "the look at the file after the report took {took:?}"
    );

    // 3. A scan that does not finish in time leaves nothing to report: an error, as before.
    let (doc, controls) = open(engine, &big.bytes);
    controls.slow.store(true, Ordering::SeqCst);
    let (body, _) = question(engine, doc, &mut |_| {});
    assert!(
        matches!(
            body,
            ResponseBody::Error {
                code: ErrorCode::Internal,
                ..
            }
        ),
        "{body:?}"
    );
}
