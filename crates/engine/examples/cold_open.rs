//! Measures what a user waits for: a document opened by a client that has no engine yet, up to
//! the first page, and then the steady rate of drawing pages. Works the same on every version of
//! the client, so a change to the engine start (such as a sandbox) can be compared.
//!
//! Usage: `cargo run --release -p partridge-engine --example cold_open -- <file.pdf> [runs]`
//! Build the engine first (`cargo build --release -p partridge-engine`).

use std::path::PathBuf;
use std::time::Instant;

use partridge_engine_client::{EngineClient, EngineConfig};

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    values[values.len() / 2]
}

fn rss_mib(pid: u32) -> f64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<f64>()
                .ok()
        })
        .map_or(0.0, |kib| kib / 1024.0)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let file = args.next().expect("usage: cold_open <file.pdf> [runs]");
    let runs: usize = args.next().and_then(|r| r.parse().ok()).unwrap_or(20);
    let exe = std::env::current_exe().unwrap();
    let engine: PathBuf = exe
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join(format!("partridge-engine{}", std::env::consts::EXE_SUFFIX)))
        .unwrap();
    let pdfium_dir = partridge_testkit::pdfium_dir();

    let (mut open, mut first, mut total, mut steady, mut rss) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for _ in 0..runs {
        let mut config = EngineConfig::new(&engine);
        config.args = vec!["--pdfium-dir".into(), pdfium_dir.clone().into_os_string()];
        let client = EngineClient::new(config);
        let t0 = Instant::now();
        let doc = client.open(&file, None).expect("open");
        open.push(ms(t0));
        let t1 = Instant::now();
        client.render(doc.doc, 0, 1224, 1584).expect("render");
        first.push(ms(t1));
        total.push(ms(t0));
        let t2 = Instant::now();
        let n = doc.pages.len().min(40);
        for p in 1..n {
            client
                .render(doc.doc, p as u32, 1224, 1584)
                .expect("render");
        }
        if n > 1 {
            steady.push(ms(t2) / (n - 1) as f64);
        }
        rss.push(rss_mib(client.pid().unwrap()));
    }
    println!("runs: {runs}");
    println!(
        "cold open (engine start + open): median {:.1} ms",
        median(&mut open)
    );
    println!(
        "first page render 1224x1584: median {:.1} ms",
        median(&mut first)
    );
    println!(
        "cold open to first page: median {:.1} ms",
        median(&mut total)
    );
    println!(
        "steady page render, mean per page: median {:.1} ms",
        median(&mut steady)
    );
    println!(
        "engine memory after 40 pages (RSS): median {:.1} MiB",
        median(&mut rss)
    );
}
