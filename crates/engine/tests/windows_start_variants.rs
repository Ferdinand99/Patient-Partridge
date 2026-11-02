//! Starts the real engine many times with layers taken away, and says how each start ended.

#![cfg(windows)]

use std::io::Write;
use std::sync::mpsc;
use std::time::Duration;

use partridge_engine_client::{spawn_engine, SandboxPolicy, SpawnRequest};
use partridge_protocol::{read_frame, write_request, Request, RequestBody, Response};
use partridge_testkit as kit;

fn start_once() -> String {
    let program = std::path::PathBuf::from(env!("CARGO_BIN_EXE_partridge-engine"));
    let args: Vec<std::ffi::OsString> =
        vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
    let spawned = spawn_engine(&SpawnRequest {
        program: &program,
        args: &args,
        document: None,
        memory_limit: None,
        policy: SandboxPolicy::Required,
        fonts: &[],
    });
    let mut engine = match spawned {
        Ok(engine) => engine,
        Err(e) => return format!("not started: {e}"),
    };
    let write = write_request(
        &mut engine.stdin,
        &Request {
            id: 1,
            body: RequestBody::Hello {
                protocol_version: partridge_protocol::PROTOCOL_VERSION,
            },
        },
    )
    .is_ok()
        && engine.stdin.flush().is_ok();
    let mut stdout = engine.stdout;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(read_frame(&mut stdout));
    });
    let answer = rx.recv_timeout(Duration::from_secs(10));
    let text = match answer {
        Ok(Ok(Some(frame))) => match Response::decode(frame) {
            Ok(r) => format!("{:?}", r.body).chars().take(200).collect(),
            Err(e) => format!("undecodable: {e:?}"),
        },
        Ok(Ok(None)) | Ok(Err(_)) => {
            std::thread::sleep(Duration::from_millis(200));
            format!(
                "no answer (write {}), {}",
                write,
                engine
                    .process
                    .try_wait()
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "still running".to_string())
            )
        }
        Err(_) => "no answer in 10 seconds".to_string(),
    };
    let _ = engine.process.kill();
    let _ = engine.process.wait();
    text
}

#[test]
#[ignore = "diagnosis, run explicitly"]
fn how_starts_end_with_layers_taken_away() {
    for variant in [
        "",
        "nomit",
        "nojob",
        "nodesk",
        "nolowbox",
        "nomit,nojob,nodesk",
    ] {
        std::env::set_var("PARTRIDGE_DIAG", variant);
        let mut tally: std::collections::BTreeMap<String, u32> = Default::default();
        for _ in 0..10 {
            *tally.entry(start_once()).or_default() += 1;
        }
        println!("variant [{variant}]:");
        for (what, n) in tally {
            println!("   {n} x {what}");
        }
    }
}
