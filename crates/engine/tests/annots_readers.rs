//! A second reader for the files the engine writes: `qpdf` checks the structure, and Poppler
//! (`pdfinfo`, `pdftoppm`) opens the file and draws the annotations from their appearance streams.
//! PDFium reads back its own files in the other tests; this one asks programs that share no code with
//! it, so a stream that only PDFium can read does not pass.
//!
//! The tools are looked for on the path. When one is missing the test says so and goes on, unless
//! `PARTRIDGE_REQUIRE_READERS` is set (CI sets it), in which case a missing tool is a failure.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;
use partridge_protocol::{AnnotKind, RequestBody, StampKind};
use partridge_testkit as kit;

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
        let dir =
            std::env::temp_dir().join(format!("partridge-readers-{name}-{}", std::process::id()));
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

/// A file with every kind of annotation on the first page, written by the engine.
fn everything() -> Vec<u8> {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::features_pdf(), None).unwrap();
    for (query, kind) in [
        ("needle", AnnotKind::Highlight),
        ("haystack", AnnotKind::Underline),
        ("brown fox", AnnotKind::Strikeout),
    ] {
        let (start, len) = find(e, doc, 0, query);
        annot(ask(
            e,
            RequestBody::AddMarkup {
                doc,
                page: 0,
                kind,
                color: if kind == AnnotKind::Highlight { YELLOW } else { [200, 0, 0, 255] },
                start,
                end: start + len,
            },
        ));
    }
    annot(ask(
        e,
        RequestBody::AddNote {
            doc,
            page: 0,
            x: 400.0,
            y: 150.0,
            color: YELLOW,
            text: "A note \u{2713} blåbær".into(),
        },
    ));
    annot(ask(
        e,
        RequestBody::AddInk {
            doc,
            page: 0,
            color: [0, 0, 255, 255],
            width: 4.0,
            strokes: vec![vec![[300.0, 500.0], [350.0, 540.0], [400.0, 500.0]]],
        },
    ));
    annot(ask(
        e,
        RequestBody::AddStamp {
            doc,
            page: 0,
            x: 100.0,
            y: 600.0,
            stamp: StampKind::Approved,
        },
    ));
    save_all(e, doc)
}

/// Reads a binary PPM from `pdftoppm`: width, height and the RGB bytes.
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

fn count_near(
    image: &(usize, usize, Vec<u8>),
    b: [f32; 4],
    color: [u8; 3],
    tolerance: i32,
) -> usize {
    let (w, _, rgb) = image;
    let mut n = 0;
    for y in b[1] as usize..b[3] as usize {
        for x in b[0] as usize..b[2] as usize {
            let at = (y * w + x) * 3;
            if (0..3).all(|i| (rgb[at + i] as i32 - color[i] as i32).abs() <= tolerance) {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn qpdf_accepts_the_files_and_shows_real_annotations_with_appearances() {
    if !have("qpdf") {
        return;
    }
    let dir = Dir::new("qpdf");
    let files = [
        ("all.pdf", everything(), None),
        ("turned.pdf", turned_with_marks(), None),
        ("protected.pdf", protected_with_note(), Some("secret")),
    ];
    for (name, bytes, password) in files {
        let path = dir.write(name, &bytes);
        let path = path.to_str().unwrap();
        let mut args = vec!["--check"];
        let option;
        if let Some(p) = password {
            option = format!("--password={p}");
            args.push(&option);
        }
        args.push(path);
        let (ok, output) = run("qpdf", &args);
        assert!(ok, "qpdf --check {name} failed:\n{output}");
        assert!(output.contains("No syntax or stream encoding errors"), "{name}: {output}");
    }
    // Look inside the first one: unpacked, every annotation of ours is there with its stream.
    let path = dir.write("all.pdf", &everything());
    let qdf = dir.0.join("all.qdf");
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
    for subtype in ["/Highlight", "/Underline", "/StrikeOut", "/Text", "/Ink", "/Stamp"] {
        assert!(
            unpacked.contains(&format!("/Subtype {subtype}")),
            "no {subtype} annotation in the file"
        );
    }
    assert!(unpacked.matches("/AP <<").count() >= 6, "fewer than six appearance dictionaries");
    assert!(unpacked.contains("/QuadPoints"));
    assert!(unpacked.contains("/InkList"), "the drawing has no list of strokes");
    assert!(unpacked.contains("/Rect"));
}

fn turned_with_marks() -> Vec<u8> {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::turned_pdf(), None).unwrap();
    for page in 0..5u32 {
        let (start, len) = find(e, doc, page, "quick brown fox");
        annot(ask(
            e,
            RequestBody::AddMarkup {
                doc,
                page,
                kind: AnnotKind::Highlight,
                color: YELLOW,
                start,
                end: start + len,
            },
        ));
        annot(ask(
            e,
            RequestBody::AddNote {
                doc,
                page,
                x: 50.0,
                y: 50.0,
                color: YELLOW,
                text: "n".into(),
            },
        ));
    }
    save_all(e, doc)
}

fn protected_with_note() -> Vec<u8> {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::encrypted_pdf("secret", 1), Some("secret")).unwrap();
    annot(ask(
        e,
        RequestBody::AddNote { doc, page: 0, x: 60.0, y: 60.0, color: YELLOW, text: "n".into() },
    ));
    save_all(e, doc)
}

#[test]
fn poppler_opens_the_files_and_draws_the_annotations() {
    if !have("pdfinfo") || !have("pdftoppm") {
        return;
    }
    let dir = Dir::new("poppler");
    let all = dir.write("all.pdf", &everything());
    let (ok, info) = run("pdfinfo", &[all.to_str().unwrap()]);
    assert!(ok, "{info}");
    assert!(info.contains("Pages:"), "{info}");
    assert!(!info.contains("Syntax Error"), "{info}");

    let turned = dir.write("turned.pdf", &turned_with_marks());
    let (ok, info) = run("pdfinfo", &[turned.to_str().unwrap()]);
    assert!(ok, "{info}");
    assert!(info.contains("Pages:           5"), "{info}");

    let protected = dir.write("protected.pdf", &protected_with_note());
    let (ok, info) = run("pdfinfo", &[protected.to_str().unwrap()]);
    assert!(!ok, "a protected file opened without its password: {info}");
    let (ok, info) = run("pdfinfo", &["-upw", "secret", protected.to_str().unwrap()]);
    assert!(ok, "{info}");

    // Poppler draws the first page of the first file at one pixel to the point.
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
            all.to_str().unwrap(),
            prefix.to_str().unwrap(),
        ],
    );
    assert!(ok, "{output}");
    assert!(!output.contains("Syntax Error"), "{output}");
    let image_path = std::fs::read_dir(&dir.0)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "ppm"))
        .expect("pdftoppm wrote no picture");
    let image = read_ppm(&image_path);
    assert_eq!((image.0, image.1), (612, 792));

    // The highlight, the drawing and the stamp are drawn where the engine put them.
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &std::fs::read(&all).unwrap(), None).unwrap();
    let items = list(e, doc, 0);
    let rect_of = |kind: AnnotKind| items.iter().find(|i| i.kind == kind).unwrap().rect;
    let highlight = rect_of(AnnotKind::Highlight);
    assert!(
        count_near(&image, highlight, [255, 235, 59], 50) > 40,
        "Poppler does not draw the highlight"
    );
    let ink = rect_of(AnnotKind::Ink);
    assert!(
        count_near(&image, ink, [0, 0, 255], 70) > 80,
        "Poppler does not draw the drawing"
    );
    let stamp = rect_of(AnnotKind::Stamp);
    assert!(
        count_near(&image, stamp, [26, 127, 55], 60) > 80,
        "Poppler does not draw the stamp"
    );
}
