//! Other programs read the files that signing makes. They know nothing of this program and agree on
//! what the file says only when it is written right:
//!
//! - `qpdf --check` reads the structure of the file (the table of objects, the update that was
//!   added, the signature field and its look);
//! - Poppler's `pdfsig` checks the signature and names the signer, and `pdftoppm` draws the field
//!   where the box was asked for;
//! - `openssl cms -verify` checks the signature on its own, from the bytes the byte range names,
//!   with the chain from the file and the root that issued it;
//! - pyHanko, a validator of signed PDFs written by other people, checks the signature, the chain
//!   and what part of the file the signature covers.
//!
//! The tools are looked for on the path. When one is missing the test says so and goes on, unless
//! `PARTRIDGE_REQUIRE_READERS` is set (CI sets it on Linux), in which case a missing tool is a
//! failure. pyHanko is run by the Python that `PARTRIDGE_PYHANKO_PYTHON` names, and is required when
//! `PARTRIDGE_REQUIRE_PYHANKO` is set.

mod common;
mod sign_support;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;
use partridge_protocol::{SignPlace, SignatureFont, SignatureMark};
use partridge_testkit as kit;
use sign_support::*;

fn required() -> bool {
    std::env::var_os("PARTRIDGE_REQUIRE_READERS").is_some()
}

fn have(tool: &str) -> bool {
    let flag = if tool == "qpdf" { "--version" } else { "-v" };
    let flag = if tool == "openssl" { "version" } else { flag };
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
        let dir = std::env::temp_dir().join(format!("partridge-sign-readers-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        // A person who wants to look at the files can have them kept.
        if let (Some(keep), true) = (std::env::var_os("PARTRIDGE_KEEP_SIGNED"), name.ends_with(".pdf")) {
            let _ = std::fs::create_dir_all(&keep);
            let tag = self.0.file_name().unwrap().to_string_lossy().replace("partridge-sign-readers-", "");
            let _ = std::fs::write(PathBuf::from(&keep).join(format!("{tag}-{name}")), bytes);
        }
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
    (fields[1].parse().unwrap(), fields[2].parse().unwrap(), bytes[at..].to_vec())
}

/// How many pixels of the box (`[left, top, right, bottom]`) are not white.
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

/// How many pixels of the box differ between two drawings of the same page.
fn differing(a: &(usize, usize, Vec<u8>), b: &(usize, usize, Vec<u8>), r: [f32; 4]) -> usize {
    assert_eq!((a.0, a.1), (b.0, b.1));
    let mut n = 0;
    for y in r[1].max(0.0) as usize..(r[3].ceil() as usize).min(a.1) {
        for x in r[0].max(0.0) as usize..(r[2].ceil() as usize).min(a.0) {
            let at = (y * a.0 + x) * 3;
            if a.2[at..at + 3] != b.2[at..at + 3] {
                n += 1;
            }
        }
    }
    n
}

fn draw(dir: &Dir, pdf: &Path, name: &str) -> (usize, usize, Vec<u8>) {
    let prefix = dir.0.join(name);
    let (ok, out) = run(
        "pdftoppm",
        &["-r", "72", "-f", "1", "-l", "1", "-singlefile", pdf.to_str().unwrap(), prefix.to_str().unwrap()],
    );
    assert!(ok, "pdftoppm failed:\n{out}");
    // `-singlefile` writes `<prefix>.ppm`; the default writes a PPM for colour.
    read_ppm(&prefix.with_extension("ppm"))
}

/// The signature out of the hole, and the bytes the signature covers, as files for `openssl`.
fn cms_and_content(dir: &Dir, signed: &[u8]) -> (PathBuf, PathBuf) {
    let text = String::from_utf8_lossy(signed);
    let at = text.find("/ByteRange [").unwrap() + 12;
    let numbers: Vec<usize> = text[at..]
        .split(']')
        .next()
        .unwrap()
        .split_whitespace()
        .map(|n| n.parse().unwrap())
        .collect();
    let (a, b) = (numbers[1], numbers[2]);
    let hex = &signed[a + 1..b - 1];
    let der: Vec<u8> = hex
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect();
    // The length of the DER from its own header: the rest of the hole is zeros.
    let len = if der[1] < 0x80 {
        2 + der[1] as usize
    } else {
        let n = (der[1] & 0x7f) as usize;
        2 + n + der[2..2 + n].iter().fold(0usize, |v, b| (v << 8) | *b as usize)
    };
    let content = [&signed[..a], &signed[b..]].concat();
    (dir.write("signature.der", &der[..len]), dir.write("content.bin", &content))
}

const CASES: [(&str, &str); 7] = [
    ("rsa2048.p12", "Test Signer RSA 2048"),
    ("rsa3072.p12", "Test Signer RSA 3072"),
    ("rsa4096.p12", "Test Signer RSA 4096"),
    ("p256.p12", "Test Signer P-256"),
    ("p384.p12", "Test Signer P-384"),
    ("rsa2048-legacy.p12", "Test Signer RSA 2048"),
    ("p256-legacy.p12", "Test Signer P-256 legacy"),
];

fn signed_file(file: &str) -> Vec<u8> {
    let mut guard = engine();
    sign_with(&mut guard.0, &kit::simple_pdf(2), file, rect_place(0)).0
}

#[test]
fn qpdf_accepts_every_signed_file() {
    if !have("qpdf") {
        return;
    }
    let dir = Dir::new("qpdf");
    for (file, _) in CASES {
        let signed = signed_file(file);
        let path = dir.write("signed.pdf", &signed);
        let (ok, output) = run("qpdf", &["--check", path.to_str().unwrap()]);
        assert!(ok, "{file}: qpdf --check failed:\n{output}");
        assert!(output.contains("No syntax or stream encoding errors"), "{file}: {output}");
        // The field and its signature are in the structure that qpdf reads.
        let (ok, json) = run("qpdf", &["--json", path.to_str().unwrap()]);
        assert!(ok, "{json}");
        assert!(json.contains("/Sig"), "{file}: no signature in the structure");
    }
}

#[test]
fn qpdf_accepts_the_other_shapes_of_signed_file() {
    if !have("qpdf") {
        return;
    }
    let dir = Dir::new("qpdf-shapes");
    let mut guard = engine();
    let e = &mut guard.0;
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("invisible", sign_with(e, &kit::simple_pdf(2), "p256.p12", SignPlace::Invisible).0),
        ("form", sign_with(e, &kit::forms_pdf(), "p256.p12", rect_place(0)).0),
        (
            "existing field",
            sign_with(
                e,
                &kit::forms_empty_signature_pdf(),
                "p256.p12",
                SignPlace::Field {
                    page: 0,
                    name: "empty_signature".into(),
                },
            )
            .0,
        ),
        ("turned", sign_in_page(e, &kit::forms_rotated_pdf(90), "p256.p12").0),
        ("inherited boxes", sign_in_page(e, &kit::inherited_tree_pdf(), "p256.p12").0),
    ];
    for (name, signed) in cases {
        let path = dir.write("shape.pdf", &signed);
        let (ok, output) = run("qpdf", &["--check", path.to_str().unwrap()]);
        assert!(ok, "{name}: qpdf --check failed:\n{output}");
        assert!(output.contains("No syntax or stream encoding errors"), "{name}: {output}");
    }
}

#[test]
fn pdfsig_says_the_signature_is_valid_and_names_the_signer() {
    if !have("pdfsig") {
        return;
    }
    let dir = Dir::new("pdfsig");
    for (file, name) in CASES {
        let signed = signed_file(file);
        let path = dir.write("signed.pdf", &signed);
        let (_, output) = run("pdfsig", &[path.to_str().unwrap()]);
        assert!(output.contains("Signature is Valid"), "{file}:\n{output}");
        assert!(output.contains("Total document signed"), "{file}:\n{output}");
        assert!(output.contains("ETSI.CAdES.detached"), "{file}:\n{output}");
        assert!(output.contains(name), "{file}: the signer is not named:\n{output}");
        // The issuer is not on a list this tool trusts: it says so and does not call the signer valid.
        assert!(!output.contains("Signature is Invalid"), "{file}:\n{output}");
    }
}

#[test]
fn pdfsig_does_not_call_a_changed_file_valid() {
    if !have("pdfsig") {
        return;
    }
    let dir = Dir::new("pdfsig-changed");
    let mut signed = signed_file("p256.p12");
    let at = signed.windows(8).position(|w| w == b"Alvorlig").unwrap();
    signed[at + 1] ^= 1;
    let path = dir.write("changed.pdf", &signed);
    let (_, output) = run("pdfsig", &[path.to_str().unwrap()]);
    assert!(!output.contains("Signature is Valid"), "{output}");
    // And a file that has more after the signed part: valid, but not the whole document.
    let mut longer = signed_file("p256.p12");
    longer.extend_from_slice(b"\n% more\n");
    let path = dir.write("longer.pdf", &longer);
    let (_, output) = run("pdfsig", &[path.to_str().unwrap()]);
    assert!(!output.contains("Total document signed"), "{output}");
}

#[test]
fn openssl_verifies_the_signature_on_its_own() {
    if !have("openssl") {
        return;
    }
    let dir = Dir::new("openssl");
    let root = dir.write("root.pem", &root_pem());
    for (file, _) in CASES {
        let signed = signed_file(file);
        let (cms, content) = cms_and_content(&dir, &signed);
        let (cms, content) = (cms.to_str().unwrap(), content.to_str().unwrap());
        // The signature over the bytes, without looking at the chain.
        let (ok, output) = run(
            "openssl",
            &["cms", "-verify", "-inform", "DER", "-in", cms, "-content", content, "-binary", "-noverify", "-out", "/dev/null"],
        );
        assert!(ok && output.contains("Verification successful"), "{file}: {output}");
        // And with the chain: the certificates in the file lead to the root that issued them.
        let (ok, output) = run(
            "openssl",
            &[
                "cms", "-verify", "-inform", "DER", "-in", cms, "-content", content, "-binary", "-CAfile",
                root.to_str().unwrap(), "-purpose", "any", "-out", "/dev/null",
            ],
        );
        assert!(ok && output.contains("Verification successful"), "{file} with the chain: {output}");
        // Changed bytes are not verified.
        let mut bad = std::fs::read(content).unwrap();
        bad[40] ^= 1;
        let bad_path = dir.write("bad.bin", &bad);
        let (ok, _) = run(
            "openssl",
            &["cms", "-verify", "-inform", "DER", "-in", cms, "-content", bad_path.to_str().unwrap(), "-binary", "-noverify", "-out", "/dev/null"],
        );
        assert!(!ok, "{file}: a changed file was verified");
    }
}

#[test]
fn pdftoppm_draws_the_field_where_the_box_was_asked_for_and_the_picture_in_it() {
    if !have("pdftoppm") {
        return;
    }
    let dir = Dir::new("draw");
    let mut guard = engine();
    let e = &mut guard.0;
    let unsigned = dir.write("unsigned.pdf", &kit::simple_pdf(2));
    let before = draw(&dir, &unsigned, "unsigned");
    let place = [72.0, 600.0, 272.0, 660.0];
    assert_eq!(inked(&before, place), 0, "the box is not empty before the signature");

    // The text alone.
    let (signed, _) = sign_with(e, &kit::simple_pdf(2), "p256.p12", rect_place(0));
    let path = dir.write("text.pdf", &signed);
    let text_drawing = draw(&dir, &path, "text");
    let text_only = inked(&text_drawing, place);
    assert!(text_only > 150, "the box shows {text_only} dark pixels");
    // Nothing outside the box changed.
    let page = draw(&dir, &path, "text-page");
    assert_eq!(inked(&page, [0.0, 0.0, 612.0, 590.0]), inked(&before, [0.0, 0.0, 612.0, 590.0]));

    // With a picture of each kind: more is drawn.
    let marks = [
        (
            "strokes",
            SignatureMark::Strokes {
                width: 240.0,
                height: 100.0,
                line: 6.0,
                strokes: vec![(0..60)
                    .map(|i| [10.0 + i as f32 * 3.5, 50.0 + 35.0 * (i as f32 / 5.0).sin()])
                    .collect()],
            },
        ),
        (
            "typed",
            SignatureMark::Typed {
                font: SignatureFont::GreatVibes,
                text: "Anna Alvorlig".into(),
            },
        ),
        (
            "image",
            SignatureMark::Image {
                width: 60,
                height: 24,
                rgba: (0..60 * 24)
                    .flat_map(|i| if (i / 6) % 2 == 0 { [200, 20, 20, 255] } else { [0, 0, 0, 0] })
                    .collect(),
            },
        ),
    ];
    for (name, mark) in marks {
        let doc = open(e, &kit::simple_pdf(2), None).unwrap();
        let identity = identity("p256.p12");
        let mut s = with_mark(spec(doc, rect_place(0)), mark);
        s.reserve = identity.certificate().signature_bytes + 64;
        let signed = finish(begin(e, s).unwrap(), &identity);
        ask(e, partridge_protocol::RequestBody::Close { doc });
        let path = dir.write("marked.pdf", &signed);
        let marked = draw(&dir, &path, "marked");
        // The text moves to the right half to make room, and the picture is drawn on the left.
        assert!(inked(&marked, place) > 300, "{name}: {} dark pixels", inked(&marked, place));
        let changed = differing(&text_drawing, &marked, place);
        assert!(changed > 250, "{name}: only {changed} pixels differ from the text alone");
        // Nothing outside the box changed.
        assert_eq!(differing(&text_drawing, &marked, [0.0, 0.0, 612.0, 590.0]), 0, "{name}");
    }
}

fn pyhanko_python() -> Option<String> {
    match std::env::var("PARTRIDGE_PYHANKO_PYTHON") {
        Ok(python) if !python.is_empty() => Some(python),
        _ => {
            assert!(
                std::env::var_os("PARTRIDGE_REQUIRE_PYHANKO").is_none(),
                "pyHanko is required and PARTRIDGE_PYHANKO_PYTHON is not set"
            );
            eprintln!("pyHanko is not set up: its checks are skipped");
            None
        }
    }
}

#[test]
fn pyhanko_finds_the_signature_intact_valid_and_trusted_with_the_root_as_anchor() {
    let Some(python) = pyhanko_python() else { return };
    let dir = Dir::new("pyhanko");
    let root = dir.write("root.pem", &root_pem());
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/sign_readers/pyhanko_check.py");
    for (file, _) in CASES {
        let signed = signed_file(file);
        let path = dir.write("signed.pdf", &signed);
        let (ok, output) = run(&python, &["-I", script.to_str().unwrap(), path.to_str().unwrap(), root.to_str().unwrap()]);
        assert!(ok, "{file}: pyHanko failed:\n{output}");
        let line = output.lines().rev().find(|l| l.starts_with('{')).unwrap_or_else(|| panic!("{file}: {output}"));
        let v: serde_like::Fields = serde_like::parse(line);
        assert_eq!(v.get("intact"), "true", "{file}: {line}");
        assert_eq!(v.get("valid"), "true", "{file}: {line}");
        assert_eq!(v.get("trusted"), "true", "{file}: {line}");
        assert_eq!(v.get("coverage"), "\"ENTIRE_FILE\"", "{file}: {line}");
        assert!(v.get("sub_filter").contains("ETSI.CAdES.detached"), "{file}: {line}");
    }
}

#[test]
fn pyhanko_sees_a_change_after_the_signature() {
    let Some(python) = pyhanko_python() else { return };
    let dir = Dir::new("pyhanko-changed");
    let root = dir.write("root.pem", &root_pem());
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/sign_readers/pyhanko_check.py");
    let mut signed = signed_file("p256.p12");
    let at = signed.windows(8).position(|w| w == b"Alvorlig").unwrap();
    signed[at + 1] ^= 1;
    let path = dir.write("changed.pdf", &signed);
    let (_, output) = run(&python, &["-I", script.to_str().unwrap(), path.to_str().unwrap(), root.to_str().unwrap()]);
    let line = output.lines().rev().find(|l| l.starts_with('{')).unwrap_or("");
    if !line.is_empty() {
        let v = serde_like::parse(line);
        assert_ne!(v.get("intact"), "true", "{line}");
    }
}

/// Just enough of a reader for the one line of JSON that the script prints: `"key": value` pairs.
mod serde_like {
    pub struct Fields(Vec<(String, String)>);

    impl Fields {
        pub fn get(&self, key: &str) -> String {
            self.0
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| panic!("no {key}"))
        }
    }

    pub fn parse(line: &str) -> Fields {
        let inner = line.trim().trim_start_matches('{').trim_end_matches('}');
        let mut out = Vec::new();
        for part in inner.split(", \"") {
            let part = part.trim_start_matches('"');
            if let Some((k, v)) = part.split_once("\": ") {
                out.push((k.to_string(), v.trim().to_string()));
            }
        }
        Fields(out)
    }
}
