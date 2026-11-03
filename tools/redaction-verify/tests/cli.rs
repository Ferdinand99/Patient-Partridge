//! The command line: exit status 0 for a clean file, 1 for a leak, 2 when the check cannot be
//! done.

mod common;

use std::process::Command;

use common::*;

fn run(args: &[&str], file: &std::path::Path) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_redaction-verify"))
        .args(args)
        .arg(file)
        .output()
        .expect("the binary runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr),
    )
}

fn leaking() -> Bytes {
    let mut d = Doc::rich();
    d.info = Some(b"/Title (CLI-SECRET-TITLE)".to_vec());
    d.build()
}

#[test]
fn exit_status_tells_clean_leak_and_error_apart() {
    let dir = Dir::new();
    let leak = dir.put("leak.pdf", &leaking());
    let clean = dir.put("clean.pdf", &Doc::rich().build());

    let (code, out) = run(&["--metadata", "CLI-SECRET-TITLE"], &leak);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("LEAK layer=raw"), "{out}");
    assert!(out.contains("layer=structure"), "{out}");
    assert!(
        !out.contains("CLI-SECRET-TITLE"),
        "the value was printed: {out}"
    );

    let (code, out) = run(&["--metadata", "CLI-SECRET-TITLE", "--show-values"], &leak);
    assert_eq!(code, 1);
    assert!(out.contains("CLI-SECRET-TITLE"), "{out}");

    let (code, out) = run(&["--metadata", "CLI-SECRET-TITLE"], &clean);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("clean"), "{out}");

    let (code, _) = run(&["--text", "x"], &dir.0.path().join("missing.pdf"));
    assert_eq!(code, 2);
    let (code, _) = run(&["--bogus"], &leak);
    assert_eq!(code, 2);
}

#[test]
fn layers_can_be_chosen() {
    let dir = Dir::new();
    let leak = dir.put("leak.pdf", &leaking());
    let (code, out) = run(&["--metadata", "CLI-SECRET-TITLE", "--only", "raw"], &leak);
    assert_eq!(code, 1);
    assert!(!out.contains("layer=structure"), "{out}");
    let (code, _) = run(
        &[
            "--metadata",
            "CLI-SECRET-TITLE",
            "--skip",
            "raw,qdf,structure",
        ],
        &leak,
    );
    assert_eq!(code, 0);
}

#[test]
fn a_spec_file_is_read() {
    let dir = Dir::new();
    let leak = dir.put("leak.pdf", &leaking());
    let spec = dir.put(
        "spec.json",
        br#"{"metadata":["CLI-SECRET-TITLE"],"scrubbed":true}"#,
    );
    let (code, out) = run(&["--spec", spec.to_str().unwrap()], &leak);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("rule:"), "{out}");
}
