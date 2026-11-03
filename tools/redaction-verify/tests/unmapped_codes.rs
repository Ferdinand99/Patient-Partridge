//! A code that a font cannot map must never make a file "clean". Text that is shown with a font
//! that has no character for some of its codes may hold a phrase behind them, and the check says
//! "could not check" (exit status 2) unless the codes cannot hide the phrase, or another way to
//! read the fonts leaves no code unmapped. A stream that shows text and does not parse as content
//! is the same.

mod common;

use common::hard_cases::*;
use common::*;

fn spec() -> String {
    format!(r#"{{"text":["{NEEDLE}"]}}"#)
}

#[track_caller]
fn assert_not_checked(r: &Run, what: &str) {
    assert_eq!(r.code, 2, "exit status\n{}", r.out);
    assert!(r.layers().is_empty(), "{}", r.out);
    assert!(
        r.problems().iter().any(|p| p.contains(what)),
        "no problem about {what}\n{}",
        r.out
    );
    assert!(!r.out.contains("clean"), "{}", r.out);
    assert!(!r.out.contains(NEEDLE), "the text was printed\n{}", r.out);
}

/// A page whose font `/F2` is a Type 3 font with these glyph names, showing `shown`.
fn type3_page(names: &[String], shown: &[u8], hidden: bool, to_unicode: Option<Bytes>) -> Bytes {
    let ops = [b"BT /F2 12 Tf 72 600 Td ".as_slice(), shown, b" Tj ET\n"].concat();
    let mut fx = if hidden {
        Fx::new(BENIGN).hidden(&ops)
    } else {
        Fx::new(&[BENIGN, ops.as_slice()].concat())
    };
    fx.extra.insert(
        6,
        fonts::type3_font(65, names, 20, to_unicode.as_ref().map(|_| 8)),
    );
    fx.extra
        .insert(20, stream_obj("", b"500 0 d0 0 0 400 400 re f", false));
    if let Some(tu) = to_unicode {
        fx.extra.insert(8, stream_obj("", &tu, true));
    }
    fx.build()
}

fn numbered(i: usize, _: char) -> String {
    format!("g{}", i + 3)
}

// ---------------------------------------------------------------------------------------------
// Glyph names that tell nothing.

#[test]
fn a_type3_font_with_numbered_glyph_names_and_no_map_is_could_not_check() {
    let (names, shown) = fonts::type3_for(65, NEEDLE, numbered);
    for hidden in [true, false] {
        let bytes = type3_page(&names, &shown, hidden, None);
        let r = cli(&bytes, Some(&spec()), &[], &[]);
        assert_not_checked(&r, "decode:");
        assert!(
            r.problems().iter().any(|p| p.contains("may hold text #0")),
            "{}",
            r.out
        );
        let r = cli(&bytes, Some(&spec()), &["--only", "decode"], &[]);
        assert_not_checked(&r, "behind codes that its fonts cannot map");
    }
}

#[test]
fn the_same_font_with_a_character_map_is_read() {
    let (names, shown) = fonts::type3_for(65, NEEDLE, numbered);
    let table = fonts::codes(65, 1, NEEDLE);
    let tu = fonts::cmap_one_byte(&fonts::bfchar_width(&table, 2));
    let r = cli(
        &type3_page(&names, &shown, true, Some(tu)),
        Some(&spec()),
        &[],
        &[],
    );
    assert_eq!(r.code, 1, "{}", r.out);
}

#[test]
fn text_in_such_a_font_that_is_too_short_to_hold_the_phrase_is_clean() {
    let (names, shown) = fonts::type3_for(65, "Ola", numbered);
    let r = cli(
        &type3_page(&names, &shown, true, None),
        Some(&spec()),
        &[],
        &[],
    );
    assert_eq!(r.code, 0, "{}", r.out);
    // Letters that are known and disagree with the phrase.
    let (names, shown) = fonts::type3_for(65, "Quarterly", |i, c| {
        if i % 2 == 0 {
            c.to_string()
        } else {
            numbered(i, c)
        }
    });
    let r = cli(
        &type3_page(&names, &shown, true, None),
        Some(&spec()),
        &[],
        &[],
    );
    assert_eq!(r.code, 0, "{}", r.out);
}

#[test]
fn one_unknown_code_may_be_a_ligature() {
    // "o", "f", a glyph with a number for a name (ffi?), "c", "e".
    let names: Vec<String> = ["o", "f", "g1", "c", "e"].map(str::to_owned).to_vec();
    let shown = b"(ABCDE)".to_vec();
    let bytes = type3_page(&names, &shown, true, None);
    let r = cli(&bytes, Some(r#"{"text":["office"]}"#), &[], &[]);
    assert_eq!(r.code, 2, "{}", r.out);
    assert!(
        r.problems().iter().any(|p| p.contains("may hold text #0")),
        "{}",
        r.out
    );
    let r = cli(&bytes, Some(r#"{"text":["Zxqkw"]}"#), &[], &[]);
    assert_eq!(r.code, 0, "{}", r.out);
}

// ---------------------------------------------------------------------------------------------
// A character map that stops.

fn cut_map_page(hidden: bool) -> Bytes {
    let table = fonts::codes(0x0101, 7, NEEDLE);
    let full = String::from_utf8(fonts::cmap(&fonts::bfchar(&table))).unwrap();
    // Two entries, and then nothing sensible: a map cut short.
    let at = full.find("<0101>").unwrap();
    let mut cut = full[..at].to_owned().into_bytes();
    cut.extend(b"<0101> <004F>\n<0108> <006C>\n");
    cut.extend([0, 1]);
    cut.extend(b"garbage]]>>");
    let ops = format!(
        "BT /F2 12 Tf 72 600 Td {} Tj ET\n",
        fonts::hex_codes(&table, NEEDLE)
    )
    .into_bytes();
    let mut fx = if hidden {
        Fx::new(BENIGN).hidden(&ops)
    } else {
        Fx::new(&[BENIGN, ops.as_slice()].concat())
    };
    fx.extra.insert(8, packed("", &cut));
    fx.build()
}

#[test]
fn a_character_map_that_stops_early_is_could_not_check() {
    for hidden in [true, false] {
        let r = cli(&cut_map_page(hidden), Some(&spec()), &[], &[]);
        assert_not_checked(&r, "may hold text #0");
    }
}

// ---------------------------------------------------------------------------------------------
// Codes that draw nothing, and another reading.

fn simple_font_page(font: &str, string: &[u8]) -> Bytes {
    let mut fx = Fx::new(&[BENIGN, b"BT /F1 12 Tf 72 600 Td (", string, b") Tj ET\n"].concat());
    fx.extra.insert(5, font.as_bytes().to_vec());
    fx.build()
}

#[test]
fn control_codes_of_a_latin_font_hide_nothing() {
    let r = cli(
        &simple_font_page(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
            b"Quarterly\x01report\x02 for the board",
        ),
        Some(&spec()),
        &[],
        &[],
    );
    assert_eq!(r.code, 0, "{}", r.out);
}

#[test]
fn another_reading_that_leaves_no_code_unmapped_makes_it_clean() {
    // Nine codes that StandardEncoding does not have and WinAnsi does, between two letters and
    // the end of the phrase. A font without an encoding is read both ways.
    let mut string = b"xy".to_vec();
    string.extend(0x80..=0x88u8);
    string.extend(b"ann");
    let both = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";
    let r = cli(&simple_font_page(both, &string), Some(&spec()), &[], &[]);
    assert_eq!(r.code, 0, "{}", r.out);
    // With StandardEncoding named, there is no other reading: the codes may hide the phrase.
    let one = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /StandardEncoding >>";
    let r = cli(&simple_font_page(one, &string), Some(&spec()), &[], &[]);
    assert_not_checked(&r, "may hold text #0");
}

// ---------------------------------------------------------------------------------------------
// Streams that may be content.

#[test]
fn a_stream_that_shows_text_and_does_not_parse_as_content_is_could_not_check() {
    // A text, and then words that are no operators of the language of content streams.
    let mut data = b"BT /F1 12 Tf 72 600 Td (nothing) Tj ET\n".to_vec();
    data.extend(b"foo bar baz qux ".repeat(40));
    let mut fx = Fx::new(BENIGN);
    fx.extra.insert(13, stream_obj("", &data, false));
    let r = cli(&fx.build(), Some(&scrubbed_spec()), &[], &[]);
    assert_not_checked(&r, "may be a content stream");
}

fn scrubbed_spec() -> String {
    format!(r#"{{"text":["{NEEDLE}"],"scrubbed":true}}"#)
}

#[test]
fn a_stream_that_is_neither_image_font_nor_content_is_listed_and_checked_by_the_bytes_only() {
    let junk: Bytes = (0..3000u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 11) as u8)
        .collect();
    let mut fx = Fx::new(BENIGN);
    fx.extra.insert(13, stream_obj("", &junk, false));
    let r = cli(&fx.build(), Some(&scrubbed_spec()), &[], &[]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(
        r.out.lines().any(|l| l.starts_with("NOTE ")
            && l.contains("object 13 0")
            && l.contains("neither an image, a font nor a content stream")),
        "{}",
        r.out
    );
    // The same bytes are found by the byte layers when the phrase is in them.
    let mut with_phrase = junk.clone();
    with_phrase.extend(NEEDLE.bytes());
    let mut fx = Fx::new(BENIGN);
    fx.extra.insert(13, stream_obj("", &with_phrase, false));
    let r = cli(&fx.build(), Some(&scrubbed_spec()), &[], &[]);
    assert_eq!(r.code, 1, "{}", r.out);
    assert!(r.layers().contains("carve"), "{}", r.out);
}
