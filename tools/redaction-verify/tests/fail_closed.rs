//! Every way in which the check cannot be done must give exit status 2, not a clean report. Each
//! test breaks exactly one thing and runs exactly one layer, so that a path that stops reporting
//! turns its own test red: another layer cannot cover for it.
//!
//! The tools are replaced through the environment variables that the verifier reads (`QPDF`,
//! `PDFTOTEXT`, `PDFIMAGES`) by small scripts that fail in one way and otherwise call the real
//! tool.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use common::jpeg::*;
use common::review::*;
use common::*;

const SPEC: &str = r#"{"text":["Ola Nordmann"]}"#;
const TEX: &[u8] = b"BT /F1 12 Tf 72 680 Td [(Ola)-333(Nordmann)] TJ ET\n";

fn clean() -> Bytes {
    Fx::new(BENIGN).build()
}

fn real(tool: &str) -> String {
    let out = Command::new("sh")
        .args(["-c", &format!("command -v {tool}")])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// A stand-in for qpdf that fails (exit status 2 and a message) when one of its arguments starts
/// with `fail_on`, and otherwise runs the real qpdf.
fn qpdf_failing_on(dir: &Dir, fail_on: &str, status: &str) -> PathBuf {
    let script = format!(
        "#!/bin/sh\nfor a in \"$@\"; do\n  case \"$a\" in\n    {fail_on}*) echo \"WARNING: stand-in qpdf fails\" >&2; {status};;\n  esac\ndone\nexec {} \"$@\"\n",
        real("qpdf")
    );
    let p = dir.0.path().join("qpdf-stand-in");
    std::fs::write(&p, script).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn only(layer: &str, bytes: &[u8], spec: &str, env: &[(&str, &str)]) -> Run {
    cli(bytes, Some(spec), &["--only", layer], env)
}

#[track_caller]
fn assert_could_not_check(r: &Run, what: &str) {
    assert_eq!(r.code, 2, "exit status\n{}", r.out);
    assert!(r.layers().is_empty(), "{}", r.out);
    assert!(
        r.problems().iter().any(|p| p.contains(what)),
        "no problem about {what}\n{}",
        r.out
    );
    assert!(!r.out.contains("clean"), "{}", r.out);
}

/// The content stream of object 4 with a `/Length` that is wrong. qpdf reads the file, and warns.
fn wrong_length(bytes: &[u8]) -> Bytes {
    let at = bytes.windows(7).position(|w| w == b"4 0 obj").unwrap();
    let l = at
        + bytes[at..]
            .windows(8)
            .position(|w| w == b"/Length ")
            .unwrap()
        + 8;
    let end = l + bytes[l..].iter().position(|c| !c.is_ascii_digit()).unwrap();
    let n: usize = std::str::from_utf8(&bytes[l..end])
        .unwrap()
        .parse()
        .unwrap();
    let mut out = bytes[..l].to_vec();
    out.extend(format!("{}", n + 7).into_bytes());
    out.extend_from_slice(&bytes[end..]);
    out
}

// ---------------------------------------------------------------------------------------------
// qpdf, one call at a time.

#[test]
fn a_qpdf_check_warning_alone_is_a_problem() {
    let bytes = wrong_length(&clean());
    // Only the structure layer, and only the validity check can say anything here.
    let r = only("structure", &bytes, SPEC, &[]);
    assert_could_not_check(&r, "qpdf --check");
}

#[test]
fn a_qpdf_check_that_fails_is_a_problem() {
    let dir = Dir::new();
    let q = qpdf_failing_on(&dir, "--check", "exit 3");
    let r = only(
        "structure",
        &clean(),
        SPEC,
        &[("QPDF", q.to_str().unwrap())],
    );
    assert_could_not_check(&r, "qpdf --check");
    // Without the check the same file is clean: the stand-in only fails on --check.
    let ok = cli(&clean(), Some(SPEC), &["--only", "structure"], &[]);
    assert_eq!(ok.code, 0, "{}", ok.out);
}

#[test]
fn a_qdf_copy_that_qpdf_cannot_make_is_a_problem() {
    let dir = Dir::new();
    let q = qpdf_failing_on(&dir, "--qdf", "exit 2");
    let r = only("qdf", &clean(), SPEC, &[("QPDF", q.to_str().unwrap())]);
    assert_could_not_check(&r, "qdf:");
    let ok = only("qdf", &clean(), SPEC, &[]);
    assert_eq!(ok.code, 0, "{}", ok.out);
}

#[test]
fn a_qdf_copy_for_the_regions_that_qpdf_cannot_make_is_a_problem() {
    let dir = Dir::new();
    let q = qpdf_failing_on(&dir, "--qdf", "exit 2");
    let bytes = with_image(Fx::image(2, 2, 3, &[9; 12])).build();
    let spec = r#"{"regions":[{"page":1,"rect":[72,450,172,500]}]}"#;
    let r = only("images", &bytes, spec, &[("QPDF", q.to_str().unwrap())]);
    assert_could_not_check(&r, "regions were not checked");
}

#[test]
fn a_structure_dump_that_qpdf_cannot_make_is_a_problem() {
    let dir = Dir::new();
    let q = qpdf_failing_on(&dir, "--json=2", "exit 2");
    let r = only(
        "structure",
        &clean(),
        SPEC,
        &[("QPDF", q.to_str().unwrap())],
    );
    assert_could_not_check(&r, "structure: qpdf could not read the structure");
    let ok = only("structure", &clean(), SPEC, &[]);
    assert_eq!(ok.code, 0, "{}", ok.out);
}

#[test]
fn a_structure_dump_from_a_qpdf_that_is_killed_is_a_problem() {
    let dir = Dir::new();
    let q = qpdf_failing_on(&dir, "--json=2", "kill -9 $$");
    let r = only(
        "structure",
        &clean(),
        SPEC,
        &[("QPDF", q.to_str().unwrap())],
    );
    assert_could_not_check(&r, "structure: qpdf could not read the structure");
}

#[test]
fn a_qpdf_that_cannot_run_is_an_error() {
    let r = only("qdf", &clean(), SPEC, &[("QPDF", "/nonexistent/qpdf")]);
    assert_eq!(r.code, 2, "{}", r.out);
    assert!(r.out.contains("cannot run"), "{}", r.out);
}

#[test]
fn a_file_that_qpdf_cannot_say_is_encrypted_or_not_is_a_problem_in_carve() {
    let dir = Dir::new();
    let q = qpdf_failing_on(&dir, "--show-encryption", "exit 2");
    let r = only("carve", &clean(), SPEC, &[("QPDF", q.to_str().unwrap())]);
    assert_could_not_check(&r, "whether it is encrypted");
}

#[test]
fn a_decrypted_copy_that_qpdf_cannot_make_is_a_problem_in_carve() {
    let dir = Dir::new();
    let encrypted = encrypt_keep(&dir, &clean(), "");
    let q = qpdf_failing_on(&dir, "--preserve-unreferenced", "exit 2");
    let r = only("carve", &encrypted, SPEC, &[("QPDF", q.to_str().unwrap())]);
    assert_could_not_check(&r, "could not decrypt");
    // With the real qpdf the same file is clean.
    let ok = only("carve", &encrypted, SPEC, &[]);
    assert_eq!(ok.code, 0, "{}", ok.out);
}

// ---------------------------------------------------------------------------------------------
// Poppler.

#[test]
fn pdftotext_that_fails_is_a_problem() {
    let r = only("text", &clean(), SPEC, &[("PDFTOTEXT", "/bin/false")]);
    assert_could_not_check(&r, "text: pdftotext");
    let ok = only("text", &clean(), SPEC, &[]);
    assert_eq!(ok.code, 0, "{}", ok.out);
}

#[test]
fn pdftotext_that_cannot_run_is_an_error() {
    let r = only(
        "text",
        &clean(),
        SPEC,
        &[("PDFTOTEXT", "/nonexistent/pdftotext")],
    );
    assert_eq!(r.code, 2, "{}", r.out);
}

#[test]
fn pdfimages_that_fails_is_a_problem() {
    let bytes = with_image(Fx::image(2, 2, 3, &[9; 12])).build();
    let spec = format!(r#"{{"image_sha256":["{}"]}}"#, sha(&[1; 12]));
    let r = only("images", &bytes, &spec, &[("PDFIMAGES", "/bin/false")]);
    assert_could_not_check(&r, "images: pdfimages failed");
    let ok = only("images", &bytes, &spec, &[]);
    assert_eq!(ok.code, 0, "{}", ok.out);
}

#[test]
fn poppler_errors_on_stderr_are_a_problem_even_with_exit_status_0() {
    // A page whose font Poppler cannot use: it says so on stderr and still exits with 0.
    let bytes = Fx::new(b"BT /F9 12 Tf 72 720 Td (x) Tj ET /GS gs [ 1 2 foo\n").build();
    let r = only("text", &bytes, SPEC, &[]);
    assert_could_not_check(&r, "pdftotext");
    assert!(
        r.problems().iter().any(|p| p.contains("reported")),
        "{}",
        r.out
    );
}

// ---------------------------------------------------------------------------------------------
// Files that are locked.

fn locked() -> (Bytes, Bytes) {
    let dir = Dir::new();
    let leaky = Fx::new(BENIGN).hidden(TEX).build();
    (encrypt_keep(&dir, &leaky, "open-sesame"), leaky)
}

#[test]
fn a_file_with_a_user_password_is_a_problem_in_every_layer() {
    let (locked, _) = locked();
    let with_image = {
        let dir = Dir::new();
        encrypt_keep(
            &dir,
            &with_image(Fx::image(2, 2, 3, &[9; 12])).build(),
            "open-sesame",
        )
    };
    let image_spec = format!(r#"{{"image_sha256":["{}"]}}"#, sha(&[1; 12]));
    for layer in ["qdf", "carve", "text", "structure"] {
        let r = only(layer, &locked, SPEC, &[]);
        assert_eq!(r.code, 2, "{layer}\n{}", r.out);
        assert!(!r.problems().is_empty(), "{layer}\n{}", r.out);
        assert!(r.layers().is_empty(), "{layer}\n{}", r.out);
    }
    let r = only("images", &with_image, &image_spec, &[]);
    assert_eq!(r.code, 2, "images\n{}", r.out);
    assert!(!r.problems().is_empty(), "{}", r.out);
}

#[test]
fn the_user_password_opens_the_file_for_every_layer() {
    let (locked, _) = locked();
    let dir = Dir::new();
    let pw = dir.put("pw.txt", b"open-sesame\n");
    let file = dir.put("locked.pdf", &locked);
    let spec = dir.put("spec.json", SPEC.as_bytes());
    let pw = pw.to_str().unwrap();
    let args = |layer: &'static str| {
        vec![
            "--spec",
            spec.to_str().unwrap(),
            "--password-file",
            pw,
            "--only",
            layer,
        ]
    };
    // The text is hidden: the byte layers and the decoder see it, Poppler does not.
    for layer in ["qdf", "carve", "decode"] {
        let r = cli_on(&file, &args(layer), &[]);
        assert_eq!(r.code, 1, "{layer}\n{}", r.out);
        assert_eq!(r.layers().len(), 1, "{}", r.out);
    }
    for layer in ["text", "structure"] {
        let r = cli_on(&file, &args(layer), &[]);
        assert_eq!(r.code, 0, "{layer}\n{}", r.out);
    }
    // A wrong password is no better than none.
    let wrong = dir.put("wrong.txt", b"not it");
    let r = cli_on(
        &file,
        &[
            "--spec",
            spec.to_str().unwrap(),
            "--password-file",
            wrong.to_str().unwrap(),
            "--only",
            "carve",
        ],
        &[],
    );
    assert_eq!(r.code, 2, "{}", r.out);
}

#[test]
fn a_stream_of_an_encrypted_file_that_qpdf_does_not_reach_is_a_problem() {
    let dir = Dir::new();
    let mut encrypted = encrypt_keep(&dir, &clean(), "");
    // An object after the last %%EOF: no cross-reference table has it, so qpdf cannot decrypt it.
    encrypted.extend_from_slice(b"\n99 0 obj\n<< /Length 4 >>\nstream\nabcd\nendstream\nendobj\n");
    let r = only("carve", &encrypted, SPEC, &[]);
    assert_could_not_check(&r, "not reached");
}

#[test]
fn an_encrypted_file_has_no_false_scrub_hit_on_the_version_of_its_encryption() {
    let dir = Dir::new();
    let encrypted = encrypt_keep(&dir, &clean(), "");
    let r = cli(
        &encrypted,
        Some(r#"{"text":["Nothing here"],"scrubbed":true}"#),
        &[],
        &[],
    );
    assert_eq!(r.code, 0, "{}", r.out);
}

// ---------------------------------------------------------------------------------------------
// Streams that cannot be decoded.

fn orphan(dict: &str, data: &[u8]) -> Bytes {
    let mut fx = Fx::new(BENIGN);
    fx.extra.insert(18, stream_obj(dict, data, false));
    fx.build()
}

#[test]
fn a_stream_that_carve_cannot_decode_is_a_problem() {
    let plain = b"BT /F1 12 Tf 72 720 Td (nothing) Tj ET\n";
    let cases: Vec<(&str, String, Bytes)> = vec![
        ("a Crypt filter", "/Filter /Crypt".into(), plain.to_vec()),
        (
            "an unknown filter",
            "/Filter /FooDecode".into(),
            plain.to_vec(),
        ),
        (
            "an indirect filter that is not there",
            "/Filter 77 0 R".into(),
            plain.to_vec(),
        ),
        (
            "a filter list with a number in it",
            "/Filter [/FlateDecode 5]".into(),
            flate(plain),
        ),
        (
            "a /Filter that is a number",
            "/Filter 5".into(),
            plain.to_vec(),
        ),
        ("damaged Flate data", "/Filter /FlateDecode".into(), {
            let mut f = flate(&[plain.as_slice(), &[b'x'; 300]].concat());
            f.truncate(f.len() / 2);
            f
        }),
    ];
    for (name, dict, data) in cases {
        let r = only("carve", &orphan(&dict, &data), SPEC, &[]);
        assert_eq!(r.code, 2, "{name}\n{}", r.out);
        assert!(
            r.problems().iter().any(|p| p.starts_with("carve:")),
            "{name}\n{}",
            r.out
        );
    }
    // Streams that are fine: the same orphan with filters that are undone, a name written with
    // an escape, and an image codec that carve leaves alone.
    for (name, dict, data) in [
        ("Flate", "/Filter /FlateDecode", flate(plain)),
        ("Flate with an escape in its name", "/Filter /Flate#44ecode", flate(plain)),
        (
            "an image codec",
            "/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode",
            vec![0xff, 0xd8, 0xff, 0xd9],
        ),
    ] {
        let r = only("carve", &orphan(dict, &data), SPEC, &[]);
        assert_eq!(r.code, 0, "{name}\n{}", r.out);
    }
}

#[test]
fn a_filter_name_with_an_escape_is_still_decoded() {
    // Padded, so that the compressor packs the text and it is not in the file as it is.
    let mut text = b"BT /F1 12 Tf 72 720 Td (Ola Nordmann) Tj ET\n% ".to_vec();
    text.extend(std::iter::repeat_n(*b"filler ", 60).flatten());
    let r = only(
        "carve",
        &orphan("/Filter /Flate#44ecode", &flate(&text)),
        SPEC,
        &[],
    );
    assert_eq!(r.code, 1, "{}", r.out);
}

// ---------------------------------------------------------------------------------------------
// A run that checks nothing.

#[test]
fn an_empty_spec_is_an_error() {
    let r = cli(&clean(), Some("{}"), &[], &[]);
    assert_eq!(r.code, 2, "{}", r.out);
    assert!(r.out.contains("nothing to check"), "{}", r.out);
    let r = cli(&clean(), None, &[], &[]);
    assert_eq!(r.code, 2, "{}", r.out);
}

#[test]
fn a_run_with_every_layer_skipped_is_an_error() {
    let leak =
        Fx::new(&[BENIGN, b"BT /F1 12 Tf 72 680 Td (Ola Nordmann) Tj ET\n"].concat()).build();
    let r = cli(
        &leak,
        None,
        &[
            "--text",
            NEEDLE,
            "--skip",
            "raw,qdf,carve,decode,text,structure,images",
        ],
        &[],
    );
    assert_eq!(r.code, 2, "{}", r.out);
    assert!(!r.out.contains("clean"), "{}", r.out);
}

#[test]
fn layers_that_have_nothing_to_look_for_are_not_reported_as_run() {
    let leak =
        Fx::new(&[BENIGN, b"BT /F1 12 Tf 72 680 Td (Ola Nordmann) Tj ET\n"].concat()).build();
    // A text spec has nothing for the image layer.
    let r = cli(&leak, Some(r#"{"text":["Nordmann Ola"]}"#), &[], &[]);
    assert_eq!(r.code, 0, "{}", r.out);
    let line = r.out.lines().find(|l| l.starts_with("clean:")).unwrap();
    assert!(!line.contains("images"), "{line}");
    assert!(line.contains("decode"), "{line}");
    // Only the image layer, but only text to look for: nothing was checked.
    let r = cli(&leak, Some(SPEC), &["--only", "images"], &[]);
    assert_eq!(r.code, 2, "{}", r.out);
    assert!(r.out.contains("nothing was checked"), "{}", r.out);
    // A spec of rules only has nothing for the byte layers.
    let r = cli(&leak, Some(r#"{"scrubbed":true}"#), &["--only", "raw"], &[]);
    assert_eq!(r.code, 2, "{}", r.out);
}

#[test]
fn a_needle_of_white_space_is_refused() {
    let r = cli(&clean(), Some(r#"{"text":["  "]}"#), &[], &[]);
    assert_eq!(r.code, 2, "{}", r.out);
}

// ---------------------------------------------------------------------------------------------
// The region check says when it cannot check.

fn jpeg_image(hex: &str) -> Bytes {
    stream_obj(
        "/Type /XObject /Subtype /Image /Width 16 /Height 16 /ColorSpace /DeviceGray \
         /BitsPerComponent 8 /Filter /DCTDecode",
        &unhex(hex),
        false,
    )
}

const TOP_HALF: &str = r#"{"regions":[{"page":1,"rect":[72,450,172,500]}]}"#;

#[test]
fn an_image_codec_that_is_not_decoded_over_a_region_is_a_problem() {
    let jpx = stream_obj(
        "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB \
         /BitsPerComponent 8 /Filter /JPXDecode",
        b"not really a JPEG 2000 file",
        false,
    );
    let r = cli(&with_image(jpx.clone()).build(), Some(TOP_HALF), &[], &[]);
    assert_could_not_check(&r, "JPXDecode");
    // The same image far from the region needs no decoding.
    let far = r#"{"regions":[{"page":1,"rect":[300,300,400,400]}]}"#;
    let r = cli(&with_image(jpx).build(), Some(far), &[], &[]);
    assert_eq!(r.code, 0, "{}", r.out);
}

#[test]
fn a_jpeg_that_cannot_be_decoded_over_a_region_is_a_problem() {
    let broken = stream_obj(
        "/Type /XObject /Subtype /Image /Width 16 /Height 16 /ColorSpace /DeviceGray \
         /BitsPerComponent 8 /Filter /DCTDecode",
        &[0xff, 0xd8, 0xff, 0xe0, 0, 1, 2, 3],
        false,
    );
    let r = cli(&with_image(broken).build(), Some(TOP_HALF), &[], &[]);
    assert_could_not_check(&r, "JPEG");
}

#[test]
fn a_region_on_a_page_that_is_not_there_is_a_problem() {
    let spec = r#"{"regions":[{"page":2,"rect":[0,0,10,10]}]}"#;
    let r = cli(&clean(), Some(spec), &[], &[]);
    assert_could_not_check(&r, "names page 2");
}

#[test]
fn jpeg_pixels_are_checked_whatever_the_codec_did() {
    let secret = with_image(jpeg_image(SECRET_HEX)).build();
    let r = cli(&secret, Some(TOP_HALF), &[], &[]);
    assert_eq!(r.code, 1, "{}", r.out);
    assert_eq!(r.layers().len(), 1, "{}", r.out);
    let blank = with_image(jpeg_image(BLANK_TOP_HEX)).build();
    let r = cli(&blank, Some(TOP_HALF), &[], &[]);
    assert_eq!(r.code, 0, "{}", r.out);
    // The whole picture is not blank.
    let whole = r#"{"regions":[{"page":1,"rect":[72,400,172,500]}]}"#;
    let r = cli(&blank, Some(whole), &[], &[]);
    assert_eq!(r.code, 1, "{}", r.out);
}

#[test]
fn a_region_can_name_how_far_pixels_may_differ() {
    // The lower half of the JPEG is a pattern of 0 and 255: no tolerance below 255 passes it.
    let bytes = with_image(jpeg_image(BLANK_TOP_HEX)).build();
    let low = r#"{"regions":[{"page":1,"rect":[72,400,172,450],"tolerance":100}]}"#;
    assert_eq!(cli(&bytes, Some(low), &[], &[]).code, 1);
    let high = r#"{"regions":[{"page":1,"rect":[72,400,172,450],"tolerance":255}]}"#;
    assert_eq!(cli(&bytes, Some(high), &[], &[]).code, 0);
}

#[test]
fn a_structure_dump_that_is_not_json_is_a_problem() {
    let dir = Dir::new();
    let script = format!(
        "#!/bin/sh\nfor a in \"$@\"; do\n  case \"$a\" in\n    --json=2*) echo '{{ not json'; exit 0;;\n  esac\ndone\nexec {} \"$@\"\n",
        real("qpdf")
    );
    let q = dir.0.path().join("qpdf-json-garbage");
    std::fs::write(&q, script).unwrap();
    std::fs::set_permissions(&q, std::fs::Permissions::from_mode(0o755)).unwrap();
    let r = only(
        "structure",
        &clean(),
        SPEC,
        &[("QPDF", q.to_str().unwrap())],
    );
    assert_could_not_check(&r, "the qpdf JSON cannot be read");
}

#[test]
fn a_structure_dump_without_objects_is_a_problem() {
    let dir = Dir::new();
    let script = format!(
        "#!/bin/sh\nfor a in \"$@\"; do\n  case \"$a\" in\n    --json=2*) echo '{{\"qpdf\": []}}'; exit 0;;\n  esac\ndone\nexec {} \"$@\"\n",
        real("qpdf")
    );
    let q = dir.0.path().join("qpdf-json-empty");
    std::fs::write(&q, script).unwrap();
    std::fs::set_permissions(&q, std::fs::Permissions::from_mode(0o755)).unwrap();
    let r = only(
        "structure",
        &clean(),
        SPEC,
        &[("QPDF", q.to_str().unwrap())],
    );
    assert_could_not_check(&r, "the qpdf JSON has no objects");
}

#[test]
fn a_region_in_a_file_without_pages_is_a_problem() {
    let mut o = Fx::new(BENIGN).objs();
    o.insert(2, b"<< /Type /Pages /Kids [] /Count 0 >>".to_vec());
    o.remove(&3);
    o.remove(&4);
    let spec = r#"{"regions":[{"page":1,"rect":[0,0,10,10]}]}"#;
    let r = cli(&file(&o), Some(spec), &["--only", "images"], &[]);
    assert_could_not_check(&r, "no pages were found");
}
