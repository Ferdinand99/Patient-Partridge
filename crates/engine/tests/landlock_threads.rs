//! Landlock holds for the thread that applies it and the threads that thread starts afterwards, and
//! for no thread that already runs. The engine applies it before it starts any thread of its own
//! (`sandbox::restrict_files`); this test shows why that order is the one that works, on a kernel
//! that has Landlock.
//!
//! Landlock cannot be taken off again, so the test runs itself a second time as a child process and
//! reads what that one reports. Without Landlock in the kernel the test does nothing (and says so);
//! where the kernel lists it among its security modules, a refusal is a failure.

#![cfg(target_os = "linux")]

use std::process::Command;
use std::sync::mpsc;

use partridge_engine::sandbox;

const CHILD: &str = "PARTRIDGE_LANDLOCK_THREADS_CHILD";

/// Whether this thread may look into the root folder. Landlock with no rule allows nothing.
fn may_open_the_root() -> bool {
    std::fs::read_dir("/").is_ok()
}

/// A thread that answers, each time it is asked, whether it may open the root folder.
fn asker() -> mpsc::Sender<mpsc::Sender<bool>> {
    let (jobs, queue) = mpsc::channel::<mpsc::Sender<bool>>();
    std::thread::spawn(move || {
        for reply in queue {
            let _ = reply.send(may_open_the_root());
        }
    });
    jobs
}

fn ask(thread: &mpsc::Sender<mpsc::Sender<bool>>) -> bool {
    let (reply, answer) = mpsc::channel();
    thread.send(reply).unwrap();
    answer.recv().unwrap()
}

fn child() {
    // A thread that runs before Landlock is applied, and the control: it may open the root now.
    let before = asker();
    assert!(ask(&before), "the root can be opened before Landlock");
    assert!(may_open_the_root());
    match sandbox::restrict_files() {
        Err(why) => println!("\nSKIP {why}"),
        Ok(()) => {
            let started_after = std::thread::spawn(may_open_the_root).join().unwrap();
            println!(
                "\nRESULT this={} started_before={} started_after={started_after}",
                may_open_the_root(),
                ask(&before),
            );
        }
    }
}

#[test]
fn a_thread_started_after_landlock_is_under_it_and_one_started_before_is_not() {
    if std::env::var_os(CHILD).is_some() {
        child();
        return;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "a_thread_started_after_landlock_is_under_it_and_one_started_before_is_not",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, "1")
        .output()
        .expect("the test program starts itself");
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "the child failed:\n{text}");
    let lsm = std::fs::read_to_string("/sys/kernel/security/lsm").unwrap_or_default();
    let listed = lsm.split(',').any(|m| m.trim() == "landlock");
    if let Some(skip) = text.lines().find(|l| l.starts_with("SKIP")) {
        assert!(
            !listed,
            "the kernel lists Landlock, but it was not applied: {skip}"
        );
        println!("no Landlock on this kernel, nothing shown here: {skip}");
        return;
    }
    let result = text
        .lines()
        .find(|l| l.starts_with("RESULT"))
        .unwrap_or_else(|| panic!("the child reported nothing:\n{text}"));
    // The thread that applied it and the one started after it are refused; the one that was
    // already running is not, which is why the engine applies Landlock before it starts any.
    assert_eq!(
        result,
        "RESULT this=false started_before=true started_after=false"
    );
}
