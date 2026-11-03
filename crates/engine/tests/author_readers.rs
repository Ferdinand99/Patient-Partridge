//! Other readers for the forms the designer makes: `qpdf` checks the structure of the saved file,
//! Poppler (`pdfinfo`, `pdftoppm`, `pdftotext`) opens it and draws every field from the pictures the
//! designer wrote, and `pdftk` reads the fields the way a form tool does, fills them in, and flattens
//! them. None of them shares code with PDFium or with the writer, so a file that only this engine can
//! read does not pass.
//!
//! The tools are looked for on the path. When one is missing the test says so and goes on, unless
//! `PARTRIDGE_REQUIRE_READERS` is set (CI sets it), which makes a missing `qpdf` or Poppler a failure,
//! and `PARTRIDGE_REQUIRE_PDFTK`, which does the same for `pdftk`.

mod common;

use std::path::PathBuf;
use std::process::Command;

use common::designer::*;
use common::{engine, save_all};
use partridge_protocol::{field_flag, AuthorOp, DesignField, DesignKind, DesignProps, FormValue};
use partridge_testkit as kit;

fn required(name: &str) -> bool {
    std::env::var_os(name).is_some()
}

fn have(tool: &str, flag: &str, require: &str) -> bool {
    let found = Command::new(tool)
        .arg(flag)
        .output()
        .map(|o| o.status.success() || !o.stderr.is_empty() || !o.stdout.is_empty())
        .unwrap_or(false);
    if !found {
        assert!(
            !required(require),
            "{tool} is required and is not installed"
        );
        eprintln!("{tool} is not installed: its checks are skipped");
    }
    found
}

fn have_qpdf() -> bool {
    have("qpdf", "--version", "PARTRIDGE_REQUIRE_READERS")
}

fn have_poppler() -> bool {
    have("pdftoppm", "-v", "PARTRIDGE_REQUIRE_READERS")
        && have("pdfinfo", "-v", "PARTRIDGE_REQUIRE_READERS")
        && have("pdftotext", "-v", "PARTRIDGE_REQUIRE_READERS")
}

fn have_pdftk() -> bool {
    have("pdftk", "--version", "PARTRIDGE_REQUIRE_PDFTK")
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
            std::env::temp_dir().join(format!("partridge-author-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> String {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A page drawn by Poppler at one pixel to the point, as RGB.
struct Picture {
    width: usize,
    rgb: Vec<u8>,
}

impl Picture {
    fn take(dir: &Dir, pdf: &str, page: u32, name: &str) -> Picture {
        let stem = dir.path(name);
        let (ok, text) = run(
            "pdftoppm",
            &[
                "-r",
                "72",
                "-f",
                &page.to_string(),
                "-l",
                &page.to_string(),
                "-singlefile",
                pdf,
                &stem,
            ],
        );
        assert!(ok, "pdftoppm: {text}");
        let bytes = std::fs::read(format!("{stem}.ppm")).expect("a picture");
        // "P6\nW H\n255\n" and the pixels.
        let mut fields = bytes.splitn(4, |b| *b == b'\n');
        assert_eq!(fields.next(), Some(&b"P6"[..]));
        let size = String::from_utf8_lossy(fields.next().unwrap()).into_owned();
        let (w, _h) = size.split_once(' ').unwrap();
        assert_eq!(fields.next(), Some(&b"255"[..]));
        Picture {
            width: w.parse().unwrap(),
            rgb: fields.next().unwrap().to_vec(),
        }
    }

    fn pixel(&self, x: usize, y: usize) -> [u8; 3] {
        let at = (y * self.width + x) * 3;
        [self.rgb[at], self.rgb[at + 1], self.rgb[at + 2]]
    }

    /// How many pixels of the box differ from the same pixels of the other picture.
    fn differing(&self, other: &Picture, b: [f32; 4]) -> usize {
        let mut n = 0;
        for y in b[1].floor() as usize..b[3].ceil() as usize {
            for x in b[0].floor() as usize..b[2].ceil() as usize {
                if self.pixel(x, y) != other.pixel(x, y) {
                    n += 1;
                }
            }
        }
        n
    }
}

/// The boxes of `everything()` on the page as it is drawn.
const BOXES: [(&str, [f32; 4]); 9] = [
    ("name", [72.0, 72.0, 272.0, 94.0]),
    ("notes", [72.0, 104.0, 272.0, 164.0]),
    ("secret", [72.0, 174.0, 272.0, 196.0]),
    ("code", [72.0, 206.0, 272.0, 228.0]),
    ("agree", [72.0, 238.0, 86.0, 252.0]),
    ("country", [72.0, 290.0, 272.0, 312.0]),
    ("city", [72.0, 322.0, 272.0, 344.0]),
    ("colours", [72.0, 354.0, 272.0, 414.0]),
    ("sign", [72.0, 424.0, 272.0, 474.0]),
];

fn made() -> (Vec<u8>, Vec<u8>) {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open(e, &kit::simple_pdf(1));
    apply_ok(e, doc, everything());
    let empty = save_all(e, doc);
    let list = fields(e, doc, 0);
    let list = fill(
        e,
        doc,
        0,
        &one(&list, "name"),
        FormValue::Text("Bj\u{f8}rn \u{c5}se".into()),
    );
    let list = fill(
        e,
        doc,
        0,
        &one(&list, "notes"),
        FormValue::Text("first line".into()),
    );
    let list = fill(e, doc, 0, &one(&list, "agree"), FormValue::Checked(true));
    let list = fill(
        e,
        doc,
        0,
        &one(&list, "country"),
        FormValue::Choice(vec![0]),
    );
    fill(
        e,
        doc,
        0,
        &one(&list, "colours"),
        FormValue::Choice(vec![0, 2]),
    );
    let filled = save_all(e, doc);
    (empty, filled)
}

#[test]
fn qpdf_finds_no_problem_in_what_the_designer_made() {
    if !have_qpdf() {
        return;
    }
    let dir = Dir::new("qpdf");
    let (empty, filled) = made();
    for (name, bytes) in [("empty.pdf", &empty), ("filled.pdf", &filled)] {
        let path = dir.write(name, bytes);
        let (ok, text) = run("qpdf", &["--check", &path]);
        assert!(ok, "qpdf --check {name}: {text}");
        assert!(
            text.contains("No syntax or stream encoding errors found"),
            "{text}"
        );
        assert!(
            !text.to_lowercase().contains("warning"),
            "qpdf warns about {name}: {text}"
        );
    }
    // Unpacked, every widget has a normal appearance, and the form has the fonts and no flag that
    // makes a reader draw the fields again.
    let path = dir.path("empty.pdf");
    let out = dir.path("empty.qdf");
    let (ok, text) = run("qpdf", &["--qdf", "--object-streams=disable", &path, &out]);
    assert!(ok, "{text}");
    let unpacked = String::from_utf8_lossy(&std::fs::read(&out).unwrap()).into_owned();
    assert_eq!(unpacked.matches("/Subtype /Widget").count(), 12);
    assert!(unpacked.contains("/AcroForm"));
    assert!(unpacked.contains("/DR"));
    assert!(unpacked.contains("/BaseFont /Helvetica"));
    assert!(!unpacked.contains("/NeedAppearances"));
    for forbidden in ["/JavaScript", "/AA ", "/SigFlags", "/ByteRange"] {
        assert!(!unpacked.contains(forbidden), "{forbidden}");
    }
}

#[test]
fn poppler_opens_the_form_and_draws_every_field_and_what_was_filled_in() {
    if !have_poppler() {
        return;
    }
    let dir = Dir::new("poppler");
    let (empty, filled) = made();
    let blank = dir.write("blank.pdf", &kit::simple_pdf(1));
    let empty_path = dir.write("empty.pdf", &empty);
    let filled_path = dir.write("filled.pdf", &filled);
    let (ok, info) = run("pdfinfo", &[&empty_path]);
    assert!(ok, "{info}");
    assert!(info.contains("AcroForm"), "{info}");
    let page = Picture::take(&dir, &blank, 1, "blank");
    let with_fields = Picture::take(&dir, &empty_path, 1, "empty");
    let with_values = Picture::take(&dir, &filled_path, 1, "filled");
    // Every place the designer made is drawn: the page differs from the blank page inside each box.
    for (name, b) in BOXES {
        assert!(
            with_fields.differing(&page, b) > 20,
            "poppler draws nothing in the box of {name}"
        );
    }
    // The radio buttons too (three of them, in a row).
    assert!(with_fields.differing(&page, [72.0, 262.0, 190.0, 276.0]) > 20);
    // What was filled in shows: the name, the first line of the notes, the check mark, the choice
    // and the picked lines of the list differ from the empty form.
    for name in ["name", "notes", "agree", "country", "colours"] {
        let b = BOXES.iter().find(|(n, _)| *n == name).unwrap().1;
        assert!(
            with_values.differing(&with_fields, b) > 5,
            "the filled in form looks the same as the empty one in the box of {name}"
        );
    }
    // The fields that were left alone look the same.
    for name in ["secret", "code", "city", "sign"] {
        let b = BOXES.iter().find(|(n, _)| *n == name).unwrap().1;
        assert_eq!(with_values.differing(&with_fields, b), 0, "{name}");
    }
    let (ok, text) = run("pdftotext", &["-layout", &filled_path, "-"]);
    assert!(ok, "{text}");
    assert!(
        text.contains("Bj\u{f8}rn \u{c5}se"),
        "pdftotext did not find the name: {text}"
    );
}

#[test]
fn poppler_draws_a_field_on_a_turned_page_where_it_was_drawn() {
    if !have_poppler() {
        return;
    }
    let dir = Dir::new("turned");
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open(e, &kit::turned_pdf());
    let drawn = [60.0f32, 80.0, 300.0, 110.0];
    for page in 0..4u32 {
        let mut f = text(&format!("t{page}"), drawn);
        f.widgets[0].page = page;
        apply_ok(e, doc, vec![AuthorOp::Add(f)]);
    }
    let made = save_all(e, doc);
    drop(guard);
    let original = dir.write("turned.pdf", &kit::turned_pdf());
    let with = dir.write("with.pdf", &made);
    for page in 1..=4u32 {
        let before = Picture::take(&dir, &original, page, &format!("before{page}"));
        let after = Picture::take(&dir, &with, page, &format!("after{page}"));
        // Inside the box the picture differs; just outside it, the page is as it was.
        assert!(
            after.differing(&before, drawn) > 20,
            "page {page}: poppler draws nothing in the box"
        );
        assert_eq!(
            after.differing(
                &before,
                [
                    drawn[0] - 20.0,
                    drawn[1] + 100.0,
                    drawn[0] - 4.0,
                    drawn[1] + 160.0
                ]
            ),
            0,
            "page {page}: poppler draws something where there is no field"
        );
    }
}

#[derive(Debug, Default, Clone)]
struct Listed {
    name: String,
    kind: String,
    flags: u32,
    value: Vec<String>,
    default: Option<String>,
    states: Vec<String>,
    max_len: Option<u32>,
    tooltip: Option<String>,
}

/// The fields as `pdftk dump_data_fields_utf8` lists them.
fn pdftk_fields(path: &str) -> Vec<Listed> {
    let (ok, text) = run("pdftk", &[path, "dump_data_fields_utf8"]);
    assert!(ok, "{text}");
    let mut out: Vec<Listed> = Vec::new();
    for line in text.lines() {
        if line == "---" {
            out.push(Listed::default());
            continue;
        }
        let Some(last) = out.last_mut() else { continue };
        let Some((key, value)) = line.split_once(": ") else {
            continue;
        };
        match key {
            "FieldType" => last.kind = value.into(),
            "FieldName" => last.name = value.into(),
            "FieldFlags" => last.flags = value.parse().unwrap_or(0),
            "FieldValue" => last.value.push(value.into()),
            "FieldValueDefault" => last.default = Some(value.into()),
            "FieldStateOption" => last.states.push(value.into()),
            "FieldMaxLength" => last.max_len = value.parse().ok(),
            "FieldNameAlt" => last.tooltip = Some(value.into()),
            _ => {}
        }
    }
    out
}

fn fdf(values: &[(&str, &str)]) -> Vec<u8> {
    let hex = |s: &str| {
        let mut out = String::from("<FEFF");
        for u in s.encode_utf16() {
            out.push_str(&format!("{u:04X}"));
        }
        out.push('>');
        out
    };
    let mut body = String::from("%FDF-1.2\n1 0 obj\n<< /FDF << /Fields [\n");
    for (name, value) in values {
        let v = value
            .strip_prefix('/')
            .map_or_else(|| hex(value), |name| format!("/{name}"));
        body.push_str(&format!("<< /T {} /V {v} >>\n", hex(name)));
    }
    body.push_str("] >> >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n");
    body.into_bytes()
}

#[test]
fn pdftk_reads_the_fields_fills_them_in_and_the_engine_reads_what_it_wrote() {
    if !have_pdftk() {
        return;
    }
    let dir = Dir::new("pdftk");
    let (empty, _) = made();
    let path = dir.write("empty.pdf", &empty);
    let listed = pdftk_fields(&path);
    let find = |name: &str| -> Listed {
        listed
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| {
                panic!(
                    "pdftk lists no field {name}: {:?}",
                    listed.iter().map(|f| &f.name).collect::<Vec<_>>()
                )
            })
            .clone()
    };
    // As made: kinds, flags, tooltips, limits, states and defaults.
    let name = find("name");
    assert_eq!(name.kind, "Text");
    assert_eq!(name.tooltip.as_deref(), Some("Your full name"));
    assert_eq!(name.max_len, Some(30));
    assert_ne!(name.flags & 2, 0, "required");
    assert_eq!(name.default.as_deref(), Some("Ola Nordmann"));
    assert_eq!(name.value, ["Ola Nordmann"]);
    assert_ne!(find("notes").flags & (1 << 12), 0, "several lines");
    assert_ne!(find("secret").flags & (1 << 13), 0, "password");
    let code = find("code");
    assert_ne!(code.flags & (1 << 24), 0, "one box for each character");
    assert_eq!(code.max_len, Some(6));
    let agree = find("agree");
    assert_eq!(agree.kind, "Button");
    assert!(
        agree.states.contains(&"Ja".to_string()),
        "{:?}",
        agree.states
    );
    assert!(agree.states.contains(&"Off".to_string()));
    let size = find("size");
    assert_ne!(size.flags & (1 << 15), 0, "radio");
    for state in ["S", "M", "L"] {
        assert!(
            size.states.contains(&state.to_string()),
            "{state} in {:?}",
            size.states
        );
    }
    assert_eq!(size.value, ["M"]);
    assert_eq!(find("country").kind, "Choice");
    assert_ne!(find("city").flags & (1 << 18), 0, "a box to type in");
    let colours = find("colours");
    assert_ne!(colours.flags & (1 << 21), 0, "several choices");
    assert_eq!(colours.value, ["Gr\u{f8}nn"]);
    assert_eq!(find("sign").kind, "Signature");

    // pdftk fills them in, and the engine reads the values it wrote.
    let form = dir.write(
        "values.fdf",
        &fdf(&[
            ("name", "Kari Nordmann"),
            ("agree", "/Ja"),
            ("size", "/S"),
            ("country", "Danmark"),
            ("city", "Troms\u{f8}"),
        ]),
    );
    let filled = dir.path("filled.pdf");
    let (ok, text) = run("pdftk", &[&path, "fill_form", &form, "output", &filled]);
    assert!(ok, "{text}");
    let bytes = std::fs::read(&filled).unwrap();
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open(e, &bytes);
    let list = fields(e, doc, 0);
    assert_eq!(one(&list, "name").value, "Kari Nordmann");
    assert!(one(&list, "agree").has(field_flag::CHECKED));
    let on: Vec<bool> = named(&list, "size")
        .iter()
        .map(|f| f.has(field_flag::CHECKED))
        .collect();
    assert_eq!(on, [true, false, false]);
    assert_eq!(picked(&one(&list, "country")), ["Danmark"]);
    assert_eq!(one(&list, "city").value, "Troms\u{f8}");
    // And the engine fills in a form pdftk wrote into, which pdftk then reads.
    let list = fill(
        e,
        doc,
        0,
        &one(&list, "notes"),
        FormValue::Text("typed here".into()),
    );
    let again = save_all(e, doc);
    drop(guard);
    let path = dir.write("again.pdf", &again);
    let values: Vec<(String, Vec<String>)> = pdftk_fields(&path)
        .into_iter()
        .map(|f| (f.name, f.value))
        .collect();
    assert!(
        values.contains(&("notes".to_string(), vec!["typed here".to_string()])),
        "{values:?}"
    );
    assert!(values.contains(&("name".to_string(), vec!["Kari Nordmann".to_string()])));
    drop(list);
    // Flattened, the text of the fields is in the page.
    let flat = dir.path("flat.pdf");
    let (ok, text) = run("pdftk", &[&path, "output", &flat, "flatten"]);
    assert!(ok, "{text}");
    if have_poppler() {
        let (ok, text) = run("pdftotext", &[&flat, "-"]);
        assert!(ok);
        assert!(text.contains("Kari Nordmann"), "{text}");
        assert!(text.contains("typed here"), "{text}");
    }
}

#[test]
fn a_form_that_pdftk_filled_takes_a_field_of_the_designer_next_to_the_old_ones() {
    // The other way round: a form that another tool wrote is the document; the designer adds a
    // field to it, and that tool still reads all of them.
    if !have_pdftk() {
        return;
    }
    let dir = Dir::new("old");
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _file) = open(e, &kit::forms_pdf());
    let before = fields(e, doc, 0).len();
    apply_ok(
        e,
        doc,
        vec![AuthorOp::Add(DesignField {
            name: "extra".into(),
            kind: DesignKind::Text,
            props: DesignProps::default(),
            widgets: vec![widget(0, [20.0, 20.0, 120.0, 40.0])],
        })],
    );
    let saved = save_all(e, doc);
    assert_eq!(fields(e, doc, 0).len(), before + 1);
    drop(guard);
    let path = dir.write("old.pdf", &saved);
    let names: Vec<String> = pdftk_fields(&path).into_iter().map(|f| f.name).collect();
    assert!(names.contains(&"extra".to_string()), "{names:?}");
    assert!(names.contains(&"fullname".to_string()), "{names:?}");
}
