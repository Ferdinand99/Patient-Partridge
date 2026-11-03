//! Other programs read the pictures of signatures the engine writes: `qpdf` checks the structure,
//! Poppler opens the file and draws the picture, and Poppler's print path (`pdftocairo`) keeps it
//! on the paper because the stamp has the print flag.
//!
//! The tools are looked for on the path. When one is missing the test says so and goes on, unless
//! `PARTRIDGE_REQUIRE_READERS` is set (CI sets it), in which case a missing tool is a failure.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;
use partridge_protocol::{RequestBody, SignatureFont, SignatureMark};
use partridge_testkit as kit;

const INK: [u8; 4] = [10, 40, 120, 255];

fn required() -> bool {
    std::env::var_os("PARTRIDGE_REQUIRE_READERS").is_some()
}

fn have(tool: &str) -> bool {
    let flag = if tool == "qpdf" { "--version" } else { "-v" };
    let found = Command::new(tool)
        .arg(flag)
        .output()
        .map(|o| o.status.success() || !o.stderr.is_empty() || !o.stdout.is_empty())
        .unwrap_or(false);
    if !found {
        assert!(!required(), "{tool} is required and is not installed");
        eprintln!("{tool} is not installed: its checks are skipped");
    }
    found
}

fn run(tool: &str, args: &[&str]) -> (bool, String) {
    let out = Command::new(tool)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("{tool} could not be started: {e}"));
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), text)
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let dir = std::env::temp_dir().join(format!(
            "partridge-signature-readers-{name}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A binary PPM from `pdftoppm`: width, height and the RGB bytes.
fn read_ppm(path: &Path) -> (usize, usize, Vec<u8>) {
    let bytes = std::fs::read(path).unwrap();
    let mut fields = Vec::new();
    let mut at = 0;
    while fields.len() < 4 {
        while bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let start = at;
        while !bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        fields.push(String::from_utf8_lossy(&bytes[start..at]).into_owned());
    }
    at += 1;
    assert_eq!(fields[0], "P6");
    (
        fields[1].parse().unwrap(),
        fields[2].parse().unwrap(),
        bytes[at..].to_vec(),
    )
}

/// How many pixels of the box are not white.
fn inked(image: &(usize, usize, Vec<u8>), b: [f32; 4]) -> usize {
    let (w, h, rgb) = image;
    let mut n = 0;
    for y in b[1].max(0.0) as usize..(b[3].ceil() as usize).min(*h) {
        for x in b[0].max(0.0) as usize..(b[2].ceil() as usize).min(*w) {
            let at = (y * w + x) * 3;
            if rgb[at..at + 3].iter().any(|c| *c < 200) {
                n += 1;
            }
        }
    }
    n
}

fn scribble() -> SignatureMark {
    let mut a = Vec::new();
    for i in 0..=60 {
        let t = i as f32 / 60.0;
        a.push([
            10.0 + t * 200.0,
            50.0 + 30.0 * (t * 17.0).sin() * (1.0 - t * 0.5),
        ]);
    }
    SignatureMark::Strokes {
        width: 240.0,
        height: 100.0,
        line: 5.0,
        strokes: vec![a],
    }
}

fn image() -> SignatureMark {
    let mut rgba = Vec::new();
    for y in 0..40u32 {
        for x in 0..120u32 {
            if (x / 10 + y / 10) % 2 == 0 {
                rgba.extend_from_slice(&[200, 30, 30, 255]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    SignatureMark::Image {
        width: 120,
        height: 40,
        rgba,
    }
}

/// A file with the three kinds of picture on its page, each in a place of its own, and where
/// those places are.
fn signed_page() -> (Vec<u8>, Vec<(&'static str, [f32; 4])>) {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::blank_pages_pdf(1), None).unwrap();
    let mut places = Vec::new();
    for (name, rect, mark) in [
        ("drawn", [60.0, 80.0, 300.0, 180.0], scribble()),
        (
            "typed",
            [60.0, 260.0, 360.0, 360.0],
            SignatureMark::Typed {
                font: SignatureFont::GreatVibes,
                text: "Anna Alvorlig".into(),
            },
        ),
        ("image", [60.0, 440.0, 300.0, 540.0], image()),
    ] {
        let item = annot(ask(
            e,
            RequestBody::AddSignature {
                doc,
                page: 0,
                rect,
                color: INK,
                mark,
            },
        ));
        places.push((name, item.rect));
    }
    (save_all(e, doc), places)
}

#[test]
fn qpdf_accepts_the_file_and_finds_stamps_with_the_print_flag() {
    if !have("qpdf") {
        return;
    }
    let dir = Dir::new("qpdf");
    let (bytes, _) = signed_page();
    let path = dir.write("signed.pdf", &bytes);
    let (ok, output) = run("qpdf", &["--check", path.to_str().unwrap()]);
    assert!(ok, "qpdf --check failed:\n{output}");
    assert!(
        output.contains("No syntax or stream encoding errors"),
        "{output}"
    );
    let qdf = dir.0.join("signed.qdf");
    let (ok, output) = run(
        "qpdf",
        &[
            "--qdf",
            "--object-streams=disable",
            path.to_str().unwrap(),
            qdf.to_str().unwrap(),
        ],
    );
    assert!(ok, "{output}");
    let unpacked = String::from_utf8_lossy(&std::fs::read(&qdf).unwrap()).into_owned();
    // Three stamps, each with the print flag and an appearance stream.
    assert_eq!(unpacked.matches("/Subtype /Stamp").count(), 3);
    assert_eq!(unpacked.matches("/F 4\n").count(), 3, "{unpacked}");
    assert!(unpacked.matches("/AP <<").count() >= 3);
    // A picture is a picture: no signature field, no signature dictionary, no certificate data.
    for forbidden in ["/Sig", "/ByteRange", "/AcroForm", "/PKCS7"] {
        assert!(!unpacked.contains(forbidden), "{forbidden} is in the file");
    }
}

#[test]
fn poppler_draws_each_picture_where_the_engine_put_it() {
    if !have("pdfinfo") || !have("pdftoppm") {
        return;
    }
    let dir = Dir::new("poppler");
    let (bytes, places) = signed_page();
    let path = dir.write("signed.pdf", &bytes);
    let (ok, info) = run("pdfinfo", &[path.to_str().unwrap()]);
    assert!(ok, "{info}");
    assert!(!info.contains("Syntax Error"), "{info}");
    let prefix = dir.0.join("page");
    let (ok, output) = run(
        "pdftoppm",
        &[
            "-r",
            "72",
            "-f",
            "1",
            "-l",
            "1",
            path.to_str().unwrap(),
            prefix.to_str().unwrap(),
        ],
    );
    assert!(ok, "{output}");
    assert!(!output.contains("Syntax Error"), "{output}");
    let image_path = std::fs::read_dir(&dir.0)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().is_some_and(|x| x == "ppm"))
        .expect("no picture of the page");
    let page = read_ppm(&image_path);
    let mut total = 0;
    for (name, rect) in &places {
        let grown = [rect[0] - 2.0, rect[1] - 2.0, rect[2] + 2.0, rect[3] + 2.0];
        let n = inked(&page, grown);
        assert!(n > 80, "{name}: Poppler drew only {n} pixels at {rect:?}");
        total += n;
    }
    // And nothing anywhere else.
    assert_eq!(
        inked(&page, [0.0, 0.0, page.0 as f32, page.1 as f32]),
        total
    );
}

#[test]
fn the_pictures_are_still_there_when_poppler_prints_the_page() {
    if !have("pdftocairo") || !have("pdftoppm") {
        return;
    }
    let dir = Dir::new("print");
    let (bytes, places) = signed_page();
    let path = dir.write("signed.pdf", &bytes);
    // The print path leaves out what does not carry the print flag.
    let printed = dir.0.join("printed.pdf");
    let (ok, output) = run(
        "pdftocairo",
        &["-pdf", path.to_str().unwrap(), printed.to_str().unwrap()],
    );
    assert!(ok, "{output}");
    let prefix = dir.0.join("print");
    let (ok, output) = run(
        "pdftoppm",
        &[
            "-r",
            "72",
            "-f",
            "1",
            "-l",
            "1",
            printed.to_str().unwrap(),
            prefix.to_str().unwrap(),
        ],
    );
    assert!(ok, "{output}");
    let image_path = std::fs::read_dir(&dir.0)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| {
            p.file_name().is_some_and(|n| {
                n.to_string_lossy().starts_with("print")
                    && p.extension().is_some_and(|x| x == "ppm")
            })
        })
        .expect("no picture of the printed page");
    let page = read_ppm(&image_path);
    for (name, rect) in &places {
        let grown = [rect[0] - 2.0, rect[1] - 2.0, rect[2] + 2.0, rect[3] + 2.0];
        let n = inked(&page, grown);
        assert!(n > 80, "{name}: only {n} pixels reached the printed page");
    }
}
