//! The strict profile of the region check (see `profile.rs` and the README): whatever the walk does
//! not fully understand is "could not check", never a pass. Each case is a file in which a secret
//! (a blue signature, or the text `Ola Nordmann`) lies under a black box, drawn in a way that readers
//! read differently or that the tool does not place; none of them may give exit status 0. Every
//! case that is refused has a control that is inside the profile and is understood.

mod common;

use common::hard_cases::*;
use common::*;

const SIG: &[u8] = b"0 0 1 rg 75 405 m 90 440 l 100 410 l 115 445 l 118 405 l f\n";
const BOX: &[u8] = b"0 0 0 rg 72 400 50 50 re f\n";
const SPEC: &str = r##"{"regions":[{"page":1,"rect":[72,400,122,450],"fill":"#000000"}]}"##;
const SPEC_NEEDLE: &str =
    r##"{"text":["Nordmann"],"regions":[{"page":1,"rect":[72,400,122,450],"fill":"#000000"}]}"##;

/// The page with the benign text, this content, and the box last.
fn page(content: &[u8], build: impl FnOnce(&mut Fx)) -> Bytes {
    let mut fx = Fx::new(&[BENIGN, content, BOX].concat());
    build(&mut fx);
    fx.build()
}

/// A form (a signature, or empty) of the full page.
fn form(content: &[u8], extra: &str) -> Bytes {
    stream_obj(
        &format!("/Type /XObject /Subtype /Form /BBox [0 0 612 792]{extra}"),
        content,
        true,
    )
}

/// The file is refused or a leak: never clean. `msg` is in the problems when the profile refuses it.
#[track_caller]
fn never_clean(name: &str, bytes: &[u8], spec: &str, msg: &str) {
    let r = cli(bytes, Some(spec), &[], &[]);
    assert!(
        r.code == 1 || r.code == 2,
        "{name}: exit {}\n{}",
        r.code,
        r.out
    );
    assert!(
        r.problems().iter().any(|p| p.contains(msg)),
        "{name}: no problem with \"{msg}\"\n{}",
        r.out
    );
}

#[track_caller]
fn clean(name: &str, bytes: &[u8], spec: &str) {
    let r = cli(bytes, Some(spec), &[], &[]);
    assert_eq!(r.code, 0, "{name}\n{}", r.out);
}

#[track_caller]
fn leak(name: &str, bytes: &[u8], spec: &str) {
    let r = cli(bytes, Some(spec), &[], &[]);
    assert_eq!(r.code, 1, "{name}\n{}", r.out);
    assert!(
        r.out.contains("painted geometry inside a redaction region"),
        "{name}\n{}",
        r.out
    );
}

// ---------------------------------------------------------------------------------------------
// Operators: exactly the operands of the specification.

#[test]
fn an_operator_with_other_operands_than_the_specification_gives_is_could_not_check() {
    // Poppler takes the last operands of an operator that has too many, and says nothing.
    let xo = " /XObject << /Blank 32 0 R /Fm1 31 0 R >>";
    let with_forms = |fx: &mut Fx| {
        fx.resources_extra.push_str(xo);
        fx.extra.insert(31, form(SIG, ""));
        fx.extra.insert(32, form(b"", ""));
    };
    never_clean(
        "Do with two names",
        &page(b"q /Blank /Fm1 Do Q\n", with_forms),
        SPEC,
        "operator Do has 2 operands",
    );
    clean(
        "Do with the one name of the empty form",
        &page(b"q /Blank Do Q\n", with_forms),
        SPEC,
    );
    never_clean(
        "TJ with two arrays",
        &page(
            b"BT /F1 12 Tf 76 420 Td [] [(Ola Nordmann)] TJ ET\n",
            |_| {},
        ),
        SPEC,
        "operator TJ has 2 operands",
    );
    never_clean(
        "the same with a needle in the spec",
        &page(b"BT /F1 12 Tf 76 420 Td [] [(Ola)] TJ ET\n", |_| {}),
        SPEC_NEEDLE,
        "operator TJ has 2 operands",
    );
    never_clean(
        "Tf with two names",
        &page(b"BT /F1 /F2 12 Tf 40 300 Td (A) Tj ET\n", |_| {}),
        SPEC,
        "operator Tf has 3 operands",
    );
    never_clean(
        "w with two numbers",
        &page(b"0 0 1 RG 0.1 60 w 76 390 m 81 390 l S\n", |_| {}),
        SPEC,
        "operator w has 2 operands",
    );
    // Wrong types, too few operands, a name where a number goes, an integer with a fraction.
    for (what, content, msg) in [
        (
            "a string for Tf",
            &b"BT (F1) 12 Tf ET\n"[..],
            "operand 1 of the operator Tf",
        ),
        (
            "too few for Td",
            b"BT 10 Td ET\n",
            "operator Td has 1 operands",
        ),
        (
            "a name for cm",
            b"q 1 0 0 1 /X 0 cm Q\n",
            "operand 5 of the operator cm",
        ),
        (
            "a fraction for Tr",
            b"BT 1.5 Tr ET\n",
            "operand 1 of the operator Tr",
        ),
        (
            "a render mode of 9",
            b"BT 9 Tr ET\n",
            "operand 1 of the operator Tr",
        ),
        (
            "a name in a TJ",
            b"BT [(x) /N] TJ ET\n",
            "operand 1 of the operator TJ",
        ),
        ("an operand for q", b"1 q Q\n", "operator q has 1 operands"),
        (
            "a name in a colour",
            b"0.5 /X sc\n",
            "operator sc has operands",
        ),
        (
            "an operator of no specification",
            b"1 2 zork\n",
            "not one of the operators",
        ),
    ] {
        never_clean(what, &page(content, |_| {}), SPEC, msg);
    }
    // An unknown operator is allowed in a compatibility section.
    clean("BX .. EX", &page(b"BX 1 2 zork EX\n", |_| {}), SPEC);
    // The decode layer reads the operands the same way: a text that hides behind the extra
    // operands is found or the file is could not check, never clean.
    let leaky = page(
        b"BT /F1 12 Tf 76 420 Td [] [(Ola Nordmann)] TJ ET\n",
        |_| {},
    );
    let r = cli(
        &leaky,
        Some(r#"{"text":["Ola Nordmann"]}"#),
        &["--only", "decode"],
        &[],
    );
    assert!(r.code == 1 || r.code == 2, "decode\n{}", r.out);
    let r = cli(
        &page(b"BT /F1 12 Tf 76 420 Td [(x)] [(y)] TJ ET\n", |_| {}),
        Some(r#"{"text":["Zyxwv Absent"]}"#),
        &["--only", "decode"],
        &[],
    );
    assert_eq!(r.code, 2, "decode, an absent needle\n{}", r.out);
}

/// The content of a tiling pattern is held to the profile too, though it is not followed: a fill
/// with the pattern is the fill, and an operator that readers read differently could hide an image.
#[test]
fn the_content_of_a_pattern_is_held_to_the_profile() {
    let pattern = |content: &[u8]| {
        page(b"/Pattern cs /P1 scn 60 380 100 100 re f\n", |fx| {
            fx.resources_extra.push_str(" /Pattern << /P1 19 0 R >>");
            fx.extra.insert(
                19,
                stream_obj(
                    "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 4 4] /XStep 4 /YStep 4 \
                     /Resources << >>",
                    content,
                    false,
                ),
            );
        })
    };
    never_clean(
        "a pattern with an operator of three operands",
        &pattern(b"0 0 2 re f"),
        SPEC,
        "operator re has 3 operands",
    );
    never_clean(
        "a pattern with an unknown operator",
        &pattern(b"0 0 2 2 re zork f"),
        SPEC,
        "not one of the specification",
    );
    // The pattern is a fill that covers the region, which is geometry; the profile is kept.
    leak("a pattern of one square", &pattern(b"0 0 2 2 re f"), SPEC);
}

// ---------------------------------------------------------------------------------------------
// Matrices and boxes: exactly six or four numbers.

#[test]
fn a_matrix_or_a_box_that_is_not_the_numbers_it_should_be_is_could_not_check() {
    let drawn = |matrix: &str, bbox: &str| {
        page(b"q /Fm1 Do Q\n", |fx| {
            fx.resources_extra.push_str(" /XObject << /Fm1 31 0 R >>");
            fx.extra.insert(
                31,
                stream_obj(
                    &format!("/Type /XObject /Subtype /Form{bbox}{matrix}"),
                    // The signature is drawn 300 points up, and the matrix takes it back down.
                    &[b"q 1 0 0 1 0 300 cm ".as_slice(), SIG, b"Q\n"].concat(),
                    true,
                ),
            );
        })
    };
    never_clean(
        "a Matrix of seven numbers",
        &drawn(" /Matrix [1 0 0 1 0 -300 0]", " /BBox [0 0 612 792]"),
        SPEC,
        "/Matrix that is not six numbers",
    );
    never_clean(
        "a Matrix of five numbers",
        &drawn(" /Matrix [1 0 0 1 0]", " /BBox [0 0 612 792]"),
        SPEC,
        "/Matrix that is not six numbers",
    );
    never_clean(
        "a Matrix with a name in it",
        &drawn(" /Matrix [1 0 0 1 0 /X]", " /BBox [0 0 612 792]"),
        SPEC,
        "/Matrix that is not six numbers",
    );
    never_clean(
        "a BBox of three numbers",
        &drawn("", " /BBox [0 0 612]"),
        SPEC,
        "no /BBox of four numbers",
    );
    never_clean(
        "a form with no BBox",
        &drawn("", ""),
        SPEC,
        "no /BBox of four numbers",
    );
    // The same form with a Matrix of six numbers is placed: the signature is under the box.
    leak(
        "a Matrix of six numbers",
        &drawn(" /Matrix [1 0 0 1 0 -300]", " /BBox [0 0 612 792]"),
        SPEC,
    );
    // An appearance, and a rectangle of an annotation.
    let stamp = |rect: &str, ap_matrix: &str| {
        let mut fx = Fx::new(&[BENIGN, BOX].concat());
        fx.page_extra.push_str(" /Annots [44 0 R]");
        fx.extra.insert(
            44,
            format!("<< /Type /Annot /Subtype /Stamp /Rect [{rect}] /F 4 /AP << /N 45 0 R >> >>")
                .into_bytes(),
        );
        fx.extra.insert(
            45,
            stream_obj(
                &format!("/Type /XObject /Subtype /Form /BBox [0 0 50 50]{ap_matrix}"),
                b"q 1 0 0 1 -72 -400 cm 0 0 1 rg 75 405 m 90 440 l 100 410 l f Q\n",
                true,
            ),
        );
        fx.build()
    };
    leak(
        "a stamp over the region",
        &stamp("72 400 122 450", ""),
        SPEC,
    );
    never_clean(
        "an appearance with a Matrix of seven numbers",
        &stamp("72 400 122 450", " /Matrix [1 0 0 1 0 0 0]"),
        SPEC,
        "/Matrix that is not six numbers",
    );
    never_clean(
        "a Rect of five numbers",
        &stamp("72 400 122 450 0", ""),
        SPEC,
        "/Rect that is not four numbers",
    );
}

// ---------------------------------------------------------------------------------------------
// Fonts.

fn helvetica_with(extra: &str) -> Bytes {
    format!("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica {extra} >>").into_bytes()
}

fn std_widths() -> String {
    let h = redaction_verify::stdfonts::for_name("Helvetica").unwrap();
    (32..=126u32)
        .map(|c| format!("{} ", (h.width(c, false).unwrap() * 1000.0).round()))
        .collect()
}

#[test]
fn a_standard_font_is_taken_only_with_the_standard_metrics_and_encoding() {
    let shown = |font: Bytes, text: &str| {
        page(text.as_bytes(), |fx| {
            fx.extra.insert(5, font.clone());
        })
    };
    let ola = "BT /F1 12 Tf 76 420 Td (Ola) Tj ET\n";
    let win = "/Encoding /WinAnsiEncoding";
    let widths = std_widths();
    let range = format!("/FirstChar 32 /LastChar 126 /Widths [{widths}]");
    // The controls: no widths, and the standard widths, are understood.
    leak("no widths", &shown(helvetica_with(win), ola), SPEC);
    leak(
        "the standard widths",
        &shown(helvetica_with(&format!("{win} {range}")), ola),
        SPEC,
    );
    leak("no encoding at all", &shown(helvetica_with(""), ola), SPEC);
    // widths of 10 em (and the text kerned back so that it looks normal).
    let ten_em = format!(
        "/FirstChar 32 /LastChar 126 /Widths [{}]",
        "10000 ".repeat(95)
    );
    never_clean(
        "widths of 10 em",
        &shown(
            helvetica_with(&format!("{win} {ten_em}")),
            "BT /F1 12 Tf 76 420 Td [(O) 9222 (l) 9778 (a)] TJ ET\n",
        ),
        SPEC,
        "outside the strict profile",
    );
    // a descriptor box 5 em tall is no business of a font that is not embedded, but its
    // widths of 600 are not the standard widths.
    let deep = format!(
        "{win} /FirstChar 32 /LastChar 126 /Widths [{}] /FontDescriptor 61 0 R",
        "600 ".repeat(95)
    );
    never_clean(
        "widths of 600 and a descriptor box 5 em tall",
        &page(b"BT /F1 12 Tf 76 410 Td (Ola) Tj ET\n", |fx| {
            fx.extra.insert(5, helvetica_with(&deep));
            fx.extra.insert(
                61,
                b"<< /Type /FontDescriptor /FontName /Helvetica /Flags 32 \
                  /FontBBox [0 -4000 1000 1000] /ItalicAngle 0 /Ascent 718 /Descent -207 \
                  /CapHeight 718 /StemV 88 >>"
                    .to_vec(),
            );
        }),
        SPEC,
        "outside the strict profile",
    );
    // /LastChar 33 with widths that go on to 126 (zeros), and a /MissingWidth.
    let past = format!(
        "{win} /FirstChar 32 /LastChar 33 /Widths [278 278 {}] /FontDescriptor 61 0 R",
        "0 ".repeat(93)
    );
    never_clean(
        "widths past LastChar",
        &page(b"BT /F1 12 Tf 0 420 Td (XO) Tj ET\n", |fx| {
            fx.extra.insert(5, helvetica_with(&past));
            fx.extra.insert(
                61,
                b"<< /Type /FontDescriptor /FontName /Helvetica /Flags 32 \
                  /FontBBox [-166 -225 1000 931] /ItalicAngle 0 /Ascent 718 /Descent -207 \
                  /CapHeight 718 /StemV 88 /MissingWidth 6667 >>"
                    .to_vec(),
            );
        }),
        SPEC,
        "/Widths has 95 entries",
    );
    // code 32 named /O by /Differences, and code 87 named /space.
    let diff = |d: &str| {
        helvetica_with(&format!(
            "/Encoding << /Type /Encoding /BaseEncoding /WinAnsiEncoding /Differences [{d}] >>"
        ))
    };
    never_clean(
        "code 32 is a letter",
        &shown(diff("32 /O"), "BT 76 420 Td /F1 12 Tf ( ) Tj ET\n"),
        SPEC,
        "encoding of a standard font",
    );
    never_clean(
        "code 87 is a space",
        &shown(
            diff("87 /space"),
            "BT /F1 12 Tf 123 420 Td [(WWWWWWWW) 6000 (O)] TJ ET\n",
        ),
        SPEC,
        "encoding of a standard font",
    );
    // A font that is not embedded and not a standard font.
    never_clean(
        "a font that is not embedded",
        &shown(
            b"<< /Type /Font /Subtype /TrueType /BaseFont /Calibri /FirstChar 32 /LastChar 32 /Widths [226] >>"
                .to_vec(),
            ola,
        ),
        SPEC,
        "not embedded and not one of the standard fonts",
    );
    // Symbol is not in the tables.
    never_clean(
        "Symbol",
        &shown(
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Symbol >>".to_vec(),
            ola,
        ),
        SPEC,
        "not embedded and not one of the standard fonts",
    );
}

/// The simple font `/F1` with its program in the file, as `fonts::embedded_simple` writes it.
fn embedded(
    widths: &[u16],
    ascent_descent: (i32, i32),
    bbox: [i16; 4],
    adv: &[u16],
) -> Vec<(u32, Bytes)> {
    let mut objs = fonts::embedded_simple(60, 32, widths, ascent_descent, bbox, adv);
    let font = objs[0].1.clone();
    objs.push((5, font));
    objs
}

fn with_objs(content: &[u8], objs: Vec<(u32, Bytes)>) -> Bytes {
    page(content, |fx| {
        for (n, b) in objs {
            fx.extra.insert(n, b);
        }
    })
}

#[test]
fn the_ink_of_an_embedded_program_must_stay_near_its_cell_and_its_advances() {
    let widths = vec![500u16; 95];
    let ola = b"BT /F1 12 Tf 76 420 Td (Ola) Tj ET\n";
    // The control: a program whose box is the cell.
    leak(
        "a program with a box that is the cell",
        &with_objs(
            ola,
            embedded(&widths, (900, -200), [0, -200, 1000, 900], &[500]),
        ),
        SPEC,
    );
    // the ink of O is 3 em above the origin; the metrics are the ordinary ones.
    never_clean(
        "a program whose box reaches 3.8 em up",
        &with_objs(
            b"BT /F1 12 Tf 78 370 Td (O) Tj ET\n",
            embedded(&widths, (928, -236), [-100, -463, 1793, 3800], &[500]),
        ),
        SPEC,
        "box of its program",
    );
    never_clean(
        "a program whose box reaches 4 em across",
        &with_objs(
            ola,
            embedded(&widths, (900, -200), [-100, -200, 5000, 900], &[500]),
        ),
        SPEC,
        "box of its program",
    );
    // A descriptor that leaves no cell at all.
    never_clean(
        "an ascent of 0.2 and a descent that meet the program box",
        &with_objs(
            ola,
            embedded(&widths, (200, -200), [0, 700, 1000, 900], &[500]),
        ),
        SPEC,
        "leave no cell",
    );
    // Widths that are not advances of the program: the glyph is placed by what the file says.
    never_clean(
        "widths that are no advance of the program",
        &with_objs(
            ola,
            embedded(&widths, (900, -200), [0, -200, 1000, 900], &[320, 700]),
        ),
        SPEC,
        "not the advance of any glyph",
    );
    // A width of 0, outside the least width, for a letter.
    never_clean(
        "a width of zero for a letter",
        &with_objs(
            ola,
            embedded(&[0u16; 95], (900, -200), [0, -200, 1000, 900], &[0, 500]),
        ),
        SPEC,
        "is not a width",
    );
    // A code outside /FirstChar../LastChar is not shown.
    never_clean(
        "a code past LastChar",
        &with_objs(
            ola,
            embedded(&[500u16; 40], (900, -200), [0, -200, 1000, 900], &[500]),
        ),
        SPEC,
        "outside /FirstChar../LastChar",
    );
    // A program that cannot be read.
    let broken = page(ola, |fx| {
        let mut objs = embedded(&widths, (900, -200), [0, -200, 1000, 900], &[500]);
        objs[2].1 = stream_obj("/Length1 4", b"junk", false);
        for (n, b) in objs {
            fx.extra.insert(n, b);
        }
    });
    never_clean("a program that is junk", &broken, SPEC, "not a TrueType");
}

/// The Type0 font of `Fx`, with a program whose advances are these.
fn cid_page(content: &[u8], program: Bytes, dw: u32) -> Bytes {
    let mut fx = Fx::new(&[BENIGN, content, BOX].concat());
    fx.program = Some(program);
    // The descendant font gets the width in /DW.
    let mut objs = fx.objs();
    let desc = String::from_utf8_lossy(&objs[&7]).replace("/DW 600", &format!("/DW {dw}"));
    objs.insert(7, desc.into_bytes());
    file(&objs)
}

#[test]
fn the_widths_of_a_composite_font_are_the_advances_of_its_program() {
    let fx = Fx::new(b"");
    let text = format!("BT /F2 12 Tf 80 420 Td {} Tj ET\n", fx.show("Ola Nordmann"));
    let good = fonts::tiny_truetype([0, -200, 1000, 900], &[600; 400]);
    leak(
        "widths that are the advances",
        &cid_page(text.as_bytes(), good.clone(), 600),
        SPEC,
    );
    never_clean(
        "a width that is not the advance of its glyph",
        &cid_page(text.as_bytes(), good, 300),
        SPEC,
        "is not the advance",
    );
    never_clean(
        "a glyph that the program does not have",
        &cid_page(
            text.as_bytes(),
            fonts::tiny_truetype([0, -200, 1000, 900], &[600; 10]),
            600,
        ),
        SPEC,
        "has no glyph in the font program",
    );
    never_clean(
        "a program whose box reaches 3 em up",
        &cid_page(
            text.as_bytes(),
            fonts::tiny_truetype([0, -200, 1000, 3000], &[600; 400]),
            600,
        ),
        SPEC,
        "box of its program",
    );
    // The font of Fx without a program, a stream as the character map, and the vertical map.
    let no_program = page(text.as_bytes(), |_| {});
    never_clean(
        "a composite font whose program is not in the file",
        &no_program,
        SPEC,
        "program is not in the file",
    );
}

fn t3_font(enc: &str) -> Vec<(u32, Bytes)> {
    let font = format!(
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 10 10] /FontMatrix [0.001 0 0 0.001 0 0] \
         /CharProcs << /A 64 0 R /B 65 0 R >> /Encoding << /Type /Encoding {enc} >> \
         /FirstChar 65 /LastChar 66 /Widths [500 500] /Resources << >> >>"
    );
    vec![
        (6, font.into_bytes()),
        (
            64,
            stream_obj("", b"500 0 d0 0 0 1 rg 3000 8500 3000 3000 re f", false),
        ),
        (65, stream_obj("", b"500 0 d0", false)),
    ]
}

#[test]
fn a_type3_code_must_be_named_by_differences() {
    let shown = |enc: &str, code: &str| {
        with_objs(
            format!("BT /F2 12 Tf 40 300 Td ({code}) Tj ET\n").as_bytes(),
            t3_font(enc),
        )
    };
    // The control: code 66 is named by /Differences, its procedure paints nothing, and the glyph
    // is shown over the region.
    leak(
        "a named code",
        &with_objs(
            b"BT /F2 12 Tf 80 420 Td (B) Tj ET\n",
            t3_font("/Differences [66 /B]"),
        ),
        SPEC,
    );
    // Poppler names code 65 A through the standard encoding, and runs a procedure that paints a
    // square into the region.
    never_clean(
        "a code named by the standard encoding",
        &shown("/Differences [66 /B]", "A"),
        SPEC,
        "not named by /Differences",
    );
    never_clean(
        "a code named by the base encoding",
        &shown("/BaseEncoding /WinAnsiEncoding /Differences [66 /B]", "A"),
        SPEC,
        "not named by /Differences",
    );
    // A matrix that turns the glyphs, and widths that do not fit the codes.
    let turned = with_objs(
        b"BT /F2 12 Tf 40 300 Td (B) Tj ET\n",
        t3_font("/Differences [66 /B]")
            .into_iter()
            .map(|(n, b)| {
                if n == 6 {
                    let s = String::from_utf8(b).unwrap();
                    (
                        n,
                        s.replace("[0.001 0 0 0.001 0 0]", "[0 0.001 -0.001 0 0 0]")
                            .into_bytes(),
                    )
                } else {
                    (n, b)
                }
            })
            .collect(),
    );
    never_clean(
        "a turned font matrix",
        &turned,
        SPEC,
        "turns, skews or collapses",
    );
    let short = with_objs(
        b"BT /F2 12 Tf 40 300 Td (B) Tj ET\n",
        t3_font("/Differences [66 /B]")
            .into_iter()
            .map(|(n, b)| {
                if n == 6 {
                    let s = String::from_utf8(b).unwrap();
                    (n, s.replace("[500 500]", "[500]").into_bytes())
                } else {
                    (n, b)
                }
            })
            .collect(),
    );
    never_clean(
        "widths that are too few",
        &short,
        SPEC,
        "/Widths has 1 entries",
    );
}

// ---------------------------------------------------------------------------------------------
// Writing mode.

#[test]
fn only_horizontal_writing_with_identity_h_is_taken() {
    let fx = Fx::new(b"");
    let text = format!("BT /F2 12 Tf 97 480 Td {} Tj ET\n", fx.show("Ola"));
    let cmap_program = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap \
        /CMapName /Custom-Vert def /CMapType 1 def /WMode 1 def 1 begincodespacerange <0000> <FFFF> \
        endcodespacerange 1 begincidrange <0000> <FFFF> 0 endcidrange endcmap end end";
    // the writing mode is only in the program of the character map, not in its dictionary.
    for (what, dict) in [
        (
            "WMode 1 in the program only",
            "/Type /CMap /CMapName /Custom-Vert",
        ),
        (
            "WMode 1 in the dictionary",
            "/Type /CMap /CMapName /Custom-Vert /WMode 1",
        ),
    ] {
        let bytes = page(text.as_bytes(), |fx| {
            fx.program = Some(fonts::cid_program());
            fx.extra.insert(
                6,
                b"<< /Type /Font /Subtype /Type0 /BaseFont /DejaVuSans /Encoding 10 0 R \
                  /DescendantFonts [7 0 R] /ToUnicode 8 0 R >>"
                    .to_vec(),
            );
            fx.extra.insert(10, stream_obj(dict, cmap_program, false));
        });
        never_clean(
            what,
            &bytes,
            SPEC,
            "character map is not the name Identity-H",
        );
    }
    // A name that is a vertical map, and a map that is not Identity-H.
    for enc in [
        "/Identity-V",
        "/UniJIS-UCS2-V",
        "/UniJIS-UCS2-H",
        "/90ms-RKSJ-H",
    ] {
        let bytes = page(text.as_bytes(), |fx| {
            fx.program = Some(fonts::cid_program());
            fx.extra.insert(
                6,
                format!(
                    "<< /Type /Font /Subtype /Type0 /BaseFont /DejaVuSans /Encoding {enc} \
                     /DescendantFonts [7 0 R] /ToUnicode 8 0 R >>"
                )
                .into_bytes(),
            );
        });
        never_clean(
            enc,
            &bytes,
            SPEC,
            "character map is not the name Identity-H",
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Annotations with the NoRotate flag on a page that is turned.

fn norotate(rotate: i32, rect: &str, flags: u32, with_box: bool) -> Bytes {
    let mut fx = Fx::new(BENIGN);
    fx.page_extra.push_str(&format!(
        " /Rotate {rotate} /Annots [{}]",
        if with_box { "44 0 R 42 0 R" } else { "44 0 R" }
    ));
    fx.extra.insert(
        44,
        format!("<< /Type /Annot /Subtype /Stamp /Rect [{rect}] /F {flags} /AP << /N 45 0 R >> >>")
            .into_bytes(),
    );
    fx.extra.insert(
        45,
        stream_obj(
            "/Type /XObject /Subtype /Form /BBox [0 0 50 50]",
            b"q 1 0 0 1 -72 -400 cm 0 0 1 rg 75 405 m 90 440 l 100 410 l 115 445 l 118 405 l f Q",
            true,
        ),
    );
    fx.extra.insert(
        42,
        b"<< /Type /Annot /Subtype /Square /Rect [72 400 122 450] /F 4 /AP << /N 43 0 R >> >>"
            .to_vec(),
    );
    fx.extra.insert(
        43,
        stream_obj(
            "/Type /XObject /Subtype /Form /BBox [0 0 50 50]",
            b"0 g 0 0 50 50 re f",
            true,
        ),
    );
    fx.build()
}

#[test]
fn a_norotate_annotation_on_a_turned_page_near_a_region_is_could_not_check() {
    for rot in [90, 270] {
        for (where_, rect) in [
            ("below", "72 350 122 400"),
            ("left", "22 400 72 450"),
            ("above", "72 450 122 500"),
            ("right", "122 400 172 450"),
        ] {
            never_clean(
                &format!("a stamp {where_} the region on a page turned by {rot}"),
                &norotate(rot, rect, 20, true),
                SPEC,
                "NoRotate",
            );
        }
    }
    // Far from every region the annotation is no business of it; and a page that is not turned,
    // and an annotation without the flag, are placed as before.
    clean(
        "far from the region",
        &norotate(90, "400 600 450 650", 20, true),
        SPEC,
    );
    clean(
        "a page that is not turned",
        &norotate(0, "400 600 450 650", 20, true),
        SPEC,
    );
    leak(
        "a stamp over the region, no flag, on a page that is turned",
        &norotate(90, "72 400 122 450", 4, true),
        SPEC,
    );
}

// ---------------------------------------------------------------------------------------------
// Strokes.

#[test]
fn a_stroke_with_a_long_miter_or_a_dash_on_a_joined_path_is_could_not_check() {
    // five sharp joins 40 points below the region, 4 points wide. With a miter limit of 100
    // the spikes reach up to 120 points and end inside the region.
    let spikes: String = [
        (78.0, 1.0),
        (88.0, 1.5),
        (98.0, 2.5),
        (108.0, 1.2),
        (118.0, 2.0),
    ]
    .iter()
    .map(|(x, d)| format!("{} 300 m {} 360 l {} 300 l S\n", x - d, x, x + d))
    .collect();
    let stroke = |setup: &str| {
        page(
            format!("q 0 0 1 RG 4 w {setup} 0 j\n{spikes}Q\n").as_bytes(),
            |_| {},
        )
    };
    never_clean(
        "a miter limit of 100",
        &stroke("100 M"),
        SPEC,
        "miter limit above 10",
    );
    clean("the miter limit 10", &stroke("10 M"), SPEC);
    clean("the default miter limit", &stroke(""), SPEC);
    // A dash on a path with joins, near the region; a dash on a single line is placed.
    let dashed = |path: &str| {
        page(
            format!("q 0 0 1 RG 3 w [6 6] 0 d {path} S Q\n").as_bytes(),
            |_| {},
        )
    };
    never_clean(
        "a dashed path with joins",
        &dashed("60 380 m 130 380 l 130 440 l"),
        SPEC,
        "dashed stroke of a path with joins",
    );
    never_clean(
        "a dashed rectangle",
        &dashed("80 405 30 30 re"),
        SPEC,
        "dashed stroke of a path with joins",
    );
    // Far from the region it is no business of it.
    clean(
        "a dashed path with joins far away",
        &dashed("300 300 m 330 300 l 330 330 l"),
        SPEC,
    );
}

// ---------------------------------------------------------------------------------------------
// The glyph rule: a glyph is under a region when its cell overlaps it with positive area. A
// redaction that removes every such glyph is clean.

const FONT_SIZE: f64 = 11.0;

/// The width of a code in the test font, in thousandths of an em: narrow letters, wide ones,
/// digits, a narrow space.
fn width_of(c: char) -> u16 {
    match c {
        ' ' => 250,
        'i' | 'l' | 't' | 'r' | ',' | ':' | '.' | '1' => 280,
        'm' | 'w' | 'M' | 'W' => 800,
        '0'..='9' => 556,
        _ => 520,
    }
}

/// The widths for the codes 32 to 126.
fn font_widths() -> Vec<u16> {
    (32u8..=126).map(|c| width_of(char::from(c))).collect()
}

fn advance(c: char) -> f64 {
    f64::from(width_of(c)) * FONT_SIZE / 1000.0
}

/// A line `before word after` in an embedded font whose cell is 0.2 em below the baseline to 0.9
/// em above it. The glyphs whose cell overlaps the region (the word, enlarged by `margin` points
/// on each side) are removed when `follow_rule`; with `just_the_word` only the glyphs of the word
/// are. The rest is written as runs, each at its old place, and the region is covered by a box.
fn word_redaction(
    before: &str,
    word: &str,
    after: &str,
    margin: f64,
    follow_rule: bool,
    spec_text: bool,
) -> (Bytes, String) {
    let (x0, y) = (72.0, 500.0);
    let line: Vec<char> = format!("{before}{word}{after}").chars().collect();
    let mut x = x0;
    let mut glyphs: Vec<(char, f64, f64)> = Vec::new();
    for c in &line {
        let w = advance(*c);
        glyphs.push((*c, x, w));
        x += w;
    }
    let word_from = x0 + before.chars().map(advance).sum::<f64>();
    let word_to = word_from + word.chars().map(advance).sum::<f64>();
    let region = [
        word_from - margin,
        y - 0.2 * FONT_SIZE - margin,
        word_to + margin,
        y + 0.9 * FONT_SIZE + margin,
    ];
    let n_before = before.chars().count();
    let n_word = word.chars().count();
    let kept: Vec<&(char, f64, f64)> = glyphs
        .iter()
        .enumerate()
        .filter(|(i, (_, gx, gw))| {
            if follow_rule {
                // The cell overlaps the region when it reaches more than 0.01 points into it.
                gx + gw - region[0] <= 0.01 || region[2] - gx <= 0.01
            } else {
                *i < n_before || *i >= n_before + n_word
            }
        })
        .map(|(_, g)| g)
        .collect();
    let mut content = String::new();
    let mut i = 0;
    while i < kept.len() {
        // A run: glyphs that follow each other without a gap.
        let start = kept[i].1;
        let mut text = String::new();
        let mut end = start;
        while i < kept.len() && (kept[i].1 - end).abs() < 1e-9 {
            text.push(kept[i].0);
            end = kept[i].1 + kept[i].2;
            i += 1;
        }
        content.push_str(&format!(
            "BT /F1 {FONT_SIZE} Tf 1 0 0 1 {start:.4} {y} Tm ({text}) Tj ET\n"
        ));
    }
    content.push_str(&format!(
        "0 0 0 rg {:.4} {:.4} {:.4} {:.4} re f\n",
        region[0],
        region[1],
        region[2] - region[0],
        region[3] - region[1]
    ));
    let text = if spec_text {
        format!(
            r#""text":["{}"],"#,
            word.trim_matches(|c| c == ' ' || c == ',')
        )
    } else {
        String::new()
    };
    let spec = format!(
        r##"{{{text}"regions":[{{"page":1,"rect":[{:.4},{:.4},{:.4},{:.4}],"fill":"#000000"}}]}}"##,
        region[0], region[1], region[2], region[3]
    );
    let widths = font_widths();
    let bytes = with_objs_exact(
        content.as_bytes(),
        embedded(
            &widths,
            (900, -200),
            [0, -200, 1000, 900],
            &[250, 280, 520, 556, 800],
        ),
    );
    (bytes, spec)
}

/// The same as `with_objs`, with the content as it is (the box is part of it).
fn with_objs_exact(content: &[u8], objs: Vec<(u32, Bytes)>) -> Bytes {
    let mut fx = Fx::new(&[BENIGN, content].concat());
    for (n, b) in objs {
        fx.extra.insert(n, b);
    }
    fx.build()
}

#[test]
fn a_word_redaction_that_removes_every_glyph_whose_cell_overlaps_the_region_is_clean() {
    let cases = [
        ("Contact: Ola ", "Nordmann,", " Storgata 1"),
        ("Mail til ", "Nordmann", " i dag"),
        ("Navn: ", "Kari", ""),
        ("Konto 1234 ", "5678", " 90"),
        ("Totalt ", "Tilsammen", ", mer"),
    ];
    for (before, word, after) in cases {
        for spec_text in [true, false] {
            // The region is 1 point larger than the word: the glyphs next to it, a space and the
            // end of a narrow letter, overlap it, and are removed with the word.
            let (bytes, spec) = word_redaction(before, word, after, 1.0, true, spec_text);
            clean(&format!("{before}|{word}|{after}: the rule"), &bytes, &spec);
            // The region is the word and nothing more: what ends where it starts, or starts where
            // it ends, stays.
            let (bytes, spec) = word_redaction(before, word, after, 0.0, true, spec_text);
            clean(
                &format!("{before}|{word}|{after}: the exact word box"),
                &bytes,
                &spec,
            );
            // Removing only the word, with the region 1 point larger, leaves the space next to it
            // under the region.
            let (bytes, spec) = word_redaction(before, word, after, 1.0, false, spec_text);
            leak(
                &format!("{before}|{word}|{after}: only the word is removed"),
                &bytes,
                &spec,
            );
        }
    }
}
