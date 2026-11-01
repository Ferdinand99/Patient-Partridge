//! Measures the engine on its own: start-up, opening a document, drawing pages, and memory.
//!
//! Usage: `cargo run --release -p partridge-engine --example measure -- <file.pdf> [runs]`
//! Build the engine first (`cargo build --release -p partridge-engine`); the example starts the
//! binary that sits next to it. Prints one line per measurement.

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use partridge_engine_client::{EngineClient, EngineConfig};

fn rss_kib(pid: u32) -> Option<u64> {
    if cfg!(windows) {
        return None;
    }
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    values[values.len() / 2]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let file = args.next().expect("usage: measure <file.pdf> [runs]");
    let runs: usize = args.next().and_then(|r| r.parse().ok()).unwrap_or(5);

    let exe = std::env::current_exe().unwrap();
    // target/<profile>/examples/measure -> target/<profile>/partridge-engine
    let engine: PathBuf = exe
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join(format!("partridge-engine{}", std::env::consts::EXE_SUFFIX)))
        .unwrap();
    let pdfium_dir = partridge_testkit::pdfium_dir();

    let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
    println!("file: {file} ({size} bytes)");

    let mut start = Vec::new();
    let mut open = Vec::new();
    let mut first = Vec::new();
    let mut later = Vec::new();
    let mut rss_idle = Vec::new();
    let mut rss_after = Vec::new();
    let mut pages = 0;

    for _ in 0..runs {
        let mut config = EngineConfig::new(&engine);
        config.args = vec!["--pdfium-dir".into(), pdfium_dir.clone().into_os_string()];
        let client = EngineClient::new(config);

        let t = Instant::now();
        client.info().expect("engine start");
        start.push(ms(t));
        let pid = client.pid().unwrap();
        rss_idle.push(rss_kib(pid).unwrap_or(0) as f64 / 1024.0);

        let t = Instant::now();
        let doc = client.open(&file, None).expect("open");
        open.push(ms(t));
        pages = doc.pages.len();

        // A page at 100 % on a 2x display: 1224 x 1584 for Letter.
        let t = Instant::now();
        client.render(doc.doc, 0, 1224, 1584).expect("render");
        first.push(ms(t));

        let t = Instant::now();
        let n = pages.min(20);
        for p in 1..n {
            client
                .render(doc.doc, p as u32, 1224, 1584)
                .expect("render");
        }
        if n > 1 {
            later.push(ms(t) / (n - 1) as f64);
        }
        rss_after.push(rss_kib(pid).unwrap_or(0) as f64 / 1024.0);
    }

    println!("pages: {pages}, runs: {runs}");
    println!(
        "engine start (spawn + hello): median {:.1} ms",
        median(&mut start)
    );
    println!(
        "open document (page list incl.): median {:.1} ms",
        median(&mut open)
    );
    println!(
        "first page render 1224x1584: median {:.1} ms",
        median(&mut first)
    );
    if !later.is_empty() {
        println!(
            "next pages render, mean per page: median {:.1} ms",
            median(&mut later)
        );
    }
    println!(
        "engine memory after start (RSS): median {:.1} MiB",
        median(&mut rss_idle)
    );
    println!(
        "engine memory after drawing 20 pages (RSS): median {:.1} MiB",
        median(&mut rss_after)
    );
}
