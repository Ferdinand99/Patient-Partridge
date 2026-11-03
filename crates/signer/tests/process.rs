//! The real signer program, started the way the app starts it: its protocol on standard input and
//! output, nothing else. It must be locked into the sandbox when it answers the greeting (the test
//! insists on that where the system has one: the signer is told to refuse without it), and it must
//! do what the in-process tests say it does.

use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use partridge_signer_protocol::{
    read_frame, write_frame, ErrorCode, Request, Response, PROTOCOL_VERSION,
};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

struct Running {
    child: Child,
    input: BufWriter<ChildStdin>,
    output: BufReader<ChildStdout>,
}

/// A debug build goes on without a sandbox unless it is told to insist. On Windows the sandbox is
/// made by the app when it starts the program (an AppContainer), not by the program itself, so a
/// program started here has none to find; the tests that start the signer through the client
/// (crates/engine/tests/sign_process.rs) insist on it there.
fn insist(command: &mut Command) -> &mut Command {
    if cfg!(windows) {
        command
    } else {
        command.env("PARTRIDGE_TEST_ENFORCE", "1")
    }
}

fn start() -> Running {
    let mut child = insist(Command::new(env!("CARGO_BIN_EXE_partridge-signer")).env_clear())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the signer starts");
    let input = BufWriter::new(child.stdin.take().unwrap());
    let output = BufReader::new(child.stdout.take().unwrap());
    Running {
        child,
        input,
        output,
    }
}

impl Running {
    fn ask(&mut self, request: Request) -> Response {
        write_frame(&mut self.input, &request.encode()).unwrap();
        let frame = read_frame(&mut self.output).unwrap().expect("an answer");
        Response::decode(&frame).unwrap()
    }
}

#[test]
fn the_signer_program_greets_loads_signs_and_ends() {
    let mut signer = start();
    let Response::Hello { version, sandbox } = signer.ask(Request::Hello {
        version: PROTOCOL_VERSION,
    }) else {
        panic!("no greeting");
    };
    assert_eq!(version, PROTOCOL_VERSION);
    eprintln!("signer sandbox: {sandbox}");
    #[cfg(target_os = "linux")]
    assert!(sandbox.contains("seccomp"), "{sandbox}");
    #[cfg(not(windows))]
    assert!(!sandbox.starts_with("none"), "{sandbox}");

    let wrong = signer.ask(Request::Load {
        p12: fixture("rsa2048.p12"),
        password: b"wrong".to_vec(),
    });
    assert!(matches!(
        wrong,
        Response::Error {
            code: ErrorCode::WrongPassword,
            ..
        }
    ));
    let Response::Loaded(info) = signer.ask(Request::Load {
        p12: fixture("rsa2048.p12"),
        password: b"test".to_vec(),
    }) else {
        panic!("not loaded");
    };
    assert_eq!(info.name, "Test Signer RSA 2048");
    let Response::Signed { cms } = signer.ask(Request::Sign {
        digest: vec![3; 32],
    }) else {
        panic!("not signed");
    };
    assert!(cms.len() <= info.signature_bytes as usize);
    assert_eq!(signer.ask(Request::Close), Response::Closed);
    let status = signer.child.wait().unwrap();
    assert!(status.success(), "{status}");
}

#[test]
fn the_signer_makes_every_kind_of_signature_after_it_is_locked_in() {
    // The elliptic curve keys and the longest RSA key use code that the first test does not: all of
    // it has to run without a file, a socket or a new process.
    for (file, digest) in [
        ("rsa4096.p12", 32),
        ("p256.p12", 32),
        ("p384.p12", 48),
        ("rsa2048-legacy.p12", 32),
    ] {
        let mut signer = start();
        signer.ask(Request::Hello {
            version: PROTOCOL_VERSION,
        });
        let Response::Loaded(info) = signer.ask(Request::Load {
            p12: fixture(file),
            password: b"test".to_vec(),
        }) else {
            panic!("{file} was not loaded");
        };
        let Response::Signed { cms } = signer.ask(Request::Sign {
            digest: vec![8; digest],
        }) else {
            panic!("{file} was not signed");
        };
        assert!(cms.len() <= info.signature_bytes as usize);
        signer.ask(Request::Close);
        assert!(signer.child.wait().unwrap().success());
    }
}

#[test]
fn the_signer_ends_when_the_app_closes_its_input() {
    let mut signer = start();
    signer.ask(Request::Hello {
        version: PROTOCOL_VERSION,
    });
    drop(signer.input);
    let status = signer.child.wait().unwrap();
    assert!(status.success(), "{status}");
}

#[test]
fn a_message_that_cannot_be_read_ends_the_signer() {
    let mut signer = start();
    signer.ask(Request::Hello {
        version: PROTOCOL_VERSION,
    });
    write_frame(&mut signer.input, &[0xEE, 1, 2, 3]).unwrap();
    let status = signer.child.wait().unwrap();
    assert!(!status.success());
}

#[test]
fn the_signer_takes_the_memory_limit_and_nothing_else() {
    // The limit that the app puts on every program it starts is taken (and can only lower the one
    // the signer has).
    let mut child = insist(
        Command::new(env!("CARGO_BIN_EXE_partridge-signer"))
            .args(["--memory-limit", "268435456"])
            .env_clear(),
    )
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .spawn()
    .unwrap();
    let mut input = BufWriter::new(child.stdin.take().unwrap());
    let mut output = BufReader::new(child.stdout.take().unwrap());
    write_frame(
        &mut input,
        &Request::Hello {
            version: PROTOCOL_VERSION,
        }
        .encode(),
    )
    .unwrap();
    let frame = read_frame(&mut output).unwrap().unwrap();
    assert!(matches!(
        Response::decode(&frame).unwrap(),
        Response::Hello { .. }
    ));
    drop(input);
    assert!(child.wait().unwrap().success());
    // Nothing else is taken.
    for args in [
        &["--memory-limit"][..],
        &["--memory-limit", "many"],
        &["--other", "1"],
        &["x", "y", "z"],
    ] {
        let status = Command::new(env!("CARGO_BIN_EXE_partridge-signer"))
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn the_signer_takes_no_arguments() {
    let status = Command::new(env!("CARGO_BIN_EXE_partridge-signer"))
        .arg("--anything")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(2));
}

#[test]
fn a_signer_that_cannot_lock_itself_in_does_not_serve() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_partridge-signer"))
        .env_clear()
        .env("PARTRIDGE_TEST_ENFORCE", "1")
        .env("PARTRIDGE_TEST_FAIL_SANDBOX", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = BufWriter::new(child.stdin.take().unwrap());
    let mut output = BufReader::new(child.stdout.take().unwrap());
    write_frame(
        &mut input,
        &Request::Hello {
            version: PROTOCOL_VERSION,
        }
        .encode(),
    )
    .unwrap();
    let frame = read_frame(&mut output).unwrap().unwrap();
    assert!(matches!(
        Response::decode(&frame).unwrap(),
        Response::Error {
            code: ErrorCode::SandboxUnavailable,
            ..
        }
    ));
    assert!(!child.wait().unwrap().success());
}
