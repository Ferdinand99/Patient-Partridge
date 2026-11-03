//! A tool that runs too long must be killed, with everything it started, and the check must end
//! with exit status 2 and a message, never with a report. The tools are replaced through the
//! environment variables that the verifier reads (`QPDF`, `PDFTOTEXT`, `PDFTOPPM`) by scripts that
//! sleep.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::hard_cases::*;
use common::*;

const SPEC: &str = r#"{"text":["Ola Nordmann"]}"#;
const REGION_SPEC: &str = r#"{"regions":[{"page":1,"rect":[72,400,172,500]}]}"#;

fn clean() -> Bytes {
    Fx::new(BENIGN).build()
}

/// A stand-in that writes its own process number and that of a child it starts to files in `dir`,
/// and then waits for the child, which sleeps for a minute. A question for the version is
/// answered by the real tool, so that the sleep comes at the call that does the work.
fn sleeper(dir: &Dir, name: &str) -> PathBuf {
    let d = dir.0.path().display();
    let script = format!(
        "#!/bin/sh\ncase \"$1\" in\n  --version) exec qpdf --version;;\n  -v) exec pdftotext -v;;\nesac\nsleep 60 &\necho $! > {d}/{name}.child\necho $$ > {d}/{name}.self\nwait\n"
    );
    let p = dir.0.path().join(name);
    std::fs::write(&p, script).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

/// A stand-in that ends at once and leaves a child with the pipes open.
fn leaver(dir: &Dir, name: &str) -> PathBuf {
    let d = dir.0.path().display();
    let script = format!(
        "#!/bin/sh\ncase \"$1\" in\n  -v) exec pdftotext -v;;\nesac\nsleep 60 &\necho $! > {d}/{name}.child\nexit 0\n"
    );
    let p = dir.0.path().join(name);
    std::fs::write(&p, script).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn alive(pid: &str) -> bool {
    // A process that was killed and not yet collected by its parent is a zombie: it is dead.
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        return stat
            .rsplit(')')
            .next()
            .is_some_and(|rest| !rest.trim_start().starts_with('Z'));
    }
    Command::new("kill")
        .args(["-0", pid])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Every process that the stand-in wrote down is gone (the kill takes a moment to be seen).
#[track_caller]
fn assert_killed(dir: &Dir, name: &str, which: &[&str]) {
    for w in which {
        let text = std::fs::read_to_string(dir.0.path().join(format!("{name}.{w}")))
            .unwrap_or_else(|_| panic!("the stand-in did not run ({name}.{w})"));
        let pid = text.trim();
        let until = Instant::now() + Duration::from_secs(3);
        while alive(pid) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(pid), "process {pid} ({name} {w}) is still running");
    }
}

#[track_caller]
fn assert_stopped(r: &Run, what: &str, started: Instant) {
    assert_eq!(r.code, 2, "exit status\n{}", r.out);
    assert!(r.out.contains(what), "no \"{what}\"\n{}", r.out);
    assert!(!r.out.contains("clean"), "{}", r.out);
    assert!(!r.out.contains("LEAK"), "{}", r.out);
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "the run waited for the tool: {:?}",
        started.elapsed()
    );
}

#[test]
fn a_tool_that_sleeps_is_killed_with_its_children_and_the_run_exits_2() {
    // Each tool that the verifier drives, on the layer that needs it.
    for (var, layer, spec) in [
        ("QPDF", "qdf", SPEC),
        ("PDFTOTEXT", "text", SPEC),
        ("PDFTOPPM", "images", REGION_SPEC),
    ] {
        let dir = Dir::new();
        let stand_in = sleeper(&dir, "stand-in");
        let started = Instant::now();
        let r = cli(
            &clean(),
            Some(spec),
            &["--only", layer],
            &[
                (var, stand_in.to_str().unwrap()),
                ("VERIFIER_TOOL_TIMEOUT", "1"),
            ],
        );
        assert_stopped(&r, "took longer than 1 s and was killed", started);
        assert!(r.out.contains("VERIFIER_TOOL_TIMEOUT"), "{}", r.out);
        // The script and the sleep that it started are both gone.
        assert_killed(&dir, "stand-in", &["self", "child"]);
    }
}

#[test]
fn the_limit_can_be_given_as_a_flag() {
    let dir = Dir::new();
    let stand_in = sleeper(&dir, "stand-in");
    let started = Instant::now();
    let r = cli(
        &clean(),
        Some(SPEC),
        &["--only", "text", "--tool-timeout", "0.5"],
        &[("PDFTOTEXT", stand_in.to_str().unwrap())],
    );
    assert_stopped(&r, "took longer than 0.5 s and was killed", started);
    assert_killed(&dir, "stand-in", &["self", "child"]);
}

#[test]
fn a_tool_that_ends_and_leaves_a_child_with_the_pipes_open_is_held_to_the_limit() {
    let dir = Dir::new();
    let stand_in = leaver(&dir, "stand-in");
    let started = Instant::now();
    let r = cli(
        &clean(),
        Some(SPEC),
        &["--only", "text"],
        &[
            ("PDFTOTEXT", stand_in.to_str().unwrap()),
            ("VERIFIER_TOOL_TIMEOUT", "1"),
        ],
    );
    assert_stopped(&r, "took longer than 1 s and was killed", started);
    assert_killed(&dir, "stand-in", &["child"]);
}

#[test]
fn a_leak_that_another_layer_found_does_not_make_a_run_with_a_stopped_tool_a_report() {
    // The phrase is on the page, so the byte layers find it; pdftotext sleeps. The answer is
    // still "could not check": the file was not fully checked.
    let dir = Dir::new();
    let stand_in = sleeper(&dir, "stand-in");
    let leaking =
        Fx::new(&[BENIGN, b"BT /F1 12 Tf 72 680 Td (Ola Nordmann) Tj ET\n"].concat()).build();
    let started = Instant::now();
    let r = cli(
        &leaking,
        Some(SPEC),
        &[],
        &[
            ("PDFTOTEXT", stand_in.to_str().unwrap()),
            ("VERIFIER_TOOL_TIMEOUT", "1"),
        ],
    );
    assert_stopped(&r, "took longer than 1 s and was killed", started);
}

#[test]
fn the_calls_for_one_file_share_a_budget() {
    // The tool limit is long; the budget for the file is short.
    let dir = Dir::new();
    let stand_in = sleeper(&dir, "stand-in");
    let started = Instant::now();
    let r = cli(
        &clean(),
        Some(SPEC),
        &["--only", "text", "--file-budget", "1"],
        &[
            ("PDFTOTEXT", stand_in.to_str().unwrap()),
            ("VERIFIER_TOOL_TIMEOUT", "100"),
        ],
    );
    assert_stopped(&r, "the time budget of 1 s for this file ran out", started);
    assert!(r.out.contains("VERIFIER_FILE_BUDGET"), "{}", r.out);
    assert_killed(&dir, "stand-in", &["self", "child"]);
}

#[test]
fn a_limit_that_is_not_a_number_is_refused() {
    for flag in ["--tool-timeout", "--file-budget"] {
        for bad in ["0", "-3", "soon"] {
            let r = cli(&clean(), Some(SPEC), &[flag, bad], &[]);
            assert_eq!(r.code, 2, "{flag} {bad}\n{}", r.out);
            assert!(r.out.contains("seconds above 0"), "{}", r.out);
        }
    }
}

#[test]
fn tools_that_finish_in_time_are_not_affected() {
    let r = cli(&clean(), Some(SPEC), &["--tool-timeout", "60"], &[]);
    assert_eq!(r.code, 0, "{}", r.out);
}

#[test]
fn a_process_that_a_tool_leaves_behind_in_its_group_is_stopped_when_the_tool_ends() {
    // The tool starts a child that holds no pipe and ends with status 0. Nothing waits for the
    // child, and nothing may leave it running.
    let dir = Dir::new();
    let d = dir.0.path().display();
    let script = format!(
        "#!/bin/sh\ncase \"$1\" in\n  -v) exec pdftotext -v;;\nesac\nsleep 60 >/dev/null 2>&1 &\necho $! > {d}/stand-in.child\nexit 0\n"
    );
    let p = dir.0.path().join("stand-in");
    std::fs::write(&p, script).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    let r = cli(
        &clean(),
        Some(SPEC),
        &["--only", "text"],
        &[("PDFTOTEXT", p.to_str().unwrap())],
    );
    assert!(r.code == 0 || r.code == 2, "{}", r.out);
    assert_killed(&dir, "stand-in", &["child"]);
}
