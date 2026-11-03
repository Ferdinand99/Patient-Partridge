//! What a visible signature looks like on the page: a thin frame, an optional picture, and the
//! lines of text (who signed, the reason, the place and the time).
//!
//! The look is a form XObject made of plain drawing instructions, written by the engine itself (not
//! by PDFium), so it can be part of the update that carries the signature. Text is set in the
//! standard Helvetica with the Windows Latin encoding, which every reader has; a letter that
//! encoding does not hold is shown as a question mark (the signature itself holds the whole name).
//! The picture is made the way the picture of a signature is made (drawn lines, a typed name as
//! filled curves, or an image) and proves nothing.

use partridge_protocol::SignatureMark;

use crate::sigmark::{self, Seg};

/// The picture of an imported image, split in colour and transparency as PDF wants it.
#[derive(Debug, Clone, PartialEq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    /// Three bytes for each pixel.
    pub rgb: Vec<u8>,
    /// One byte for each pixel, or none when every pixel is opaque.
    pub alpha: Option<Vec<u8>>,
}

/// A finished look.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    /// The size of the box as it is drawn, in points.
    pub width: f32,
    pub height: f32,
    /// What turns the drawn box into the box on the page: the two axes of the page, so that a page
    /// that is turned shows the signature upright (the form is drawn upright, and the reader
    /// applies this).
    pub matrix: [f32; 4],
    /// The drawing instructions.
    pub content: Vec<u8>,
    pub raster: Option<Raster>,
}

/// A number as PDF text: no exponent, at most three decimals, no trailing zeros.
pub fn num(v: f32) -> String {
    let mut s = format!("{v:.3}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" {
        s = "0".to_string();
    }
    s
}

/// The byte of the Windows Latin encoding for a character, or a question mark.
pub fn winansi(c: char) -> u8 {
    match c {
        ' '..='~' | '\u{a0}'..='\u{ff}' => c as u32 as u8,
        '\u{20ac}' => 0x80,
        '\u{201a}' => 0x82,
        '\u{192}' => 0x83,
        '\u{201e}' => 0x84,
        '\u{2026}' => 0x85,
        '\u{2020}' => 0x86,
        '\u{2021}' => 0x87,
        '\u{2c6}' => 0x88,
        '\u{2030}' => 0x89,
        '\u{160}' => 0x8a,
        '\u{2039}' => 0x8b,
        '\u{152}' => 0x8c,
        '\u{17d}' => 0x8e,
        '\u{2018}' => 0x91,
        '\u{2019}' => 0x92,
        '\u{201c}' => 0x93,
        '\u{201d}' => 0x94,
        '\u{2022}' => 0x95,
        '\u{2013}' => 0x96,
        '\u{2014}' => 0x97,
        '\u{2dc}' => 0x98,
        '\u{2122}' => 0x99,
        '\u{161}' => 0x9a,
        '\u{203a}' => 0x9b,
        '\u{153}' => 0x9c,
        '\u{17e}' => 0x9e,
        '\u{178}' => 0x9f,
        _ => b'?',
    }
}

/// The widths of Helvetica for the characters 32 to 126, in thousandths of the size.
const WIDTHS: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, // space to /
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, // 0 to 9
    278, 278, 584, 584, 584, 556, 1015, // : to @
    667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667,
    611, 722, 667, 944, 667, 667, 611, // A to Z
    278, 278, 278, 469, 556, 333, // [ to `
    556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500,
    278, 556, 500, 722, 500, 500, 500, // a to z
    334, 260, 334, 584, // { to ~
];

/// How wide a byte is in Helvetica, in thousandths of the size. The letters beyond ASCII are
/// counted as the letter they are made from, which is near enough to fit a line in a box.
fn width_of(byte: u8) -> u32 {
    match byte {
        32..=126 => u32::from(WIDTHS[usize::from(byte) - 32]),
        0xc0..=0xde => 667,
        0xdf..=0xff => 556,
        _ => 556,
    }
}

fn text_width(bytes: &[u8], size: f32) -> f32 {
    bytes.iter().map(|b| width_of(*b)).sum::<u32>() as f32 * size / 1000.0
}

/// The line as bytes of the encoding, cut with three dots when it is wider than `room` at `size`.
fn fit_line(line: &str, size: f32, room: f32) -> Vec<u8> {
    let mut bytes: Vec<u8> = line.chars().map(winansi).collect();
    if text_width(&bytes, size) <= room {
        return bytes;
    }
    while !bytes.is_empty() && text_width(&bytes, size) + text_width(b"...", size) > room {
        bytes.pop();
    }
    bytes.extend_from_slice(b"...");
    bytes
}

/// A string of PDF text for the bytes of the encoding.
fn string_of(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![b'('];
    for &b in bytes {
        match b {
            b'\\' | b'(' | b')' => {
                out.push(b'\\');
                out.push(b);
            }
            0x20..=0x7e => out.push(b),
            _ => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
        }
    }
    out.push(b')');
    out
}

/// A box of the look: `[left, bottom, right, top]` in the units of the drawn box (y up).
type Area = [f32; 4];

/// Where a picture of the proportions `aspect` goes in `area`: whole, centred.
fn fit_in(aspect: f32, area: Area) -> Option<(f32, f32, f32, f32)> {
    let (aw, ah) = (area[2] - area[0], area[3] - area[1]);
    if !(aw > 1.0 && ah > 1.0 && aspect.is_finite() && aspect > 0.0) {
        return None;
    }
    let (w, h) = if aw / ah > aspect {
        (ah * aspect, ah)
    } else {
        (aw, aw / aspect)
    };
    Some((area[0] + (aw - w) / 2.0, area[1] + (ah - h) / 2.0, w, h))
}

fn rgb_of(color: [u8; 4]) -> String {
    format!(
        "{} {} {}",
        num(f32::from(color[0]) / 255.0),
        num(f32::from(color[1]) / 255.0),
        num(f32::from(color[2]) / 255.0)
    )
}

/// Draws the picture in `area`. Returns the instructions and the raster, if the picture is one.
fn draw_mark(mark: &SignatureMark, color: [u8; 4], area: Area) -> Result<(String, Option<Raster>), String> {
    let aspect = sigmark::aspect_of(mark)?;
    let Some((ox, oy, w, h)) = fit_in(aspect, area) else {
        return Ok((String::new(), None));
    };
    let mut out = String::new();
    match mark {
        SignatureMark::Strokes {
            width,
            height: _,
            line,
            strokes,
        } => {
            let s = w / width;
            out.push_str(&format!(
                "q {} RG {} w 1 J 1 j\n",
                rgb_of(color),
                num((line * s).max(0.05))
            ));
            for stroke in strokes {
                let at = |p: &[f32; 2]| {
                    format!("{} {}", num(ox + p[0] * s), num(oy + h - p[1] * s))
                };
                let Some(first) = stroke.first() else { continue };
                out.push_str(&format!("{} m ", at(first)));
                if stroke.len() == 1 {
                    // A dot: a line so short that its round ends are all there is.
                    out.push_str(&format!("{} {} l ", num(ox + first[0] * s + 0.01), num(oy + h - first[1] * s)));
                }
                for p in &stroke[1..] {
                    out.push_str(&format!("{} l ", at(p)));
                }
                out.push_str("S\n");
            }
            out.push_str("Q\n");
            Ok((out, None))
        }
        SignatureMark::Typed { font, text } => {
            let outline = sigmark::typed_outline(*font, text)
                .map_err(|_| "that text cannot be set in this font".to_string())?;
            let b = outline.bounds;
            let s = w / (b[2] - b[0]);
            let at = |x: f32, y: f32| format!("{} {}", num(ox + (x - b[0]) * s), num(oy + (y - b[1]) * s));
            out.push_str(&format!("q {} rg\n", rgb_of(color)));
            for path in &outline.paths {
                for seg in path {
                    match *seg {
                        Seg::Move(x, y) => out.push_str(&format!("{} m ", at(x, y))),
                        Seg::Line(x, y) => out.push_str(&format!("{} l ", at(x, y))),
                        Seg::Cubic(x1, y1, x2, y2, x3, y3) => out.push_str(&format!(
                            "{} {} {} c ",
                            at(x1, y1),
                            at(x2, y2),
                            at(x3, y3)
                        )),
                        Seg::Close => out.push_str("h "),
                    }
                }
                out.push('\n');
            }
            out.push_str("f\nQ\n");
            Ok((out, None))
        }
        SignatureMark::Image {
            width,
            height,
            rgba,
        } => {
            let pixels = *width as usize * *height as usize;
            let mut rgb = Vec::with_capacity(pixels * 3);
            let mut alpha = Vec::with_capacity(pixels);
            for p in rgba.chunks_exact(4) {
                rgb.extend_from_slice(&p[..3]);
                alpha.push(p[3]);
            }
            let opaque = alpha.iter().all(|a| *a == 255);
            out.push_str(&format!(
                "q {} 0 0 {} {} {} cm /Im0 Do Q\n",
                num(w),
                num(h),
                num(ox),
                num(oy)
            ));
            Ok((
                out,
                Some(Raster {
                    width: *width,
                    height: *height,
                    rgb,
                    alpha: (!opaque).then_some(alpha),
                }),
            ))
        }
    }
}

/// Makes the look of a signature of `width` by `height` points (as drawn) with the lines of text
/// and the picture, if there is one. `matrix` is carried along (see [`Look::matrix`]).
pub fn build(
    width: f32,
    height: f32,
    matrix: [f32; 4],
    lines: &[String],
    mark: Option<&SignatureMark>,
    color: [u8; 4],
) -> Result<Look, String> {
    if !(width.is_finite() && height.is_finite() && width >= 1.0 && height >= 1.0) {
        return Err("the box of the signature is too small".to_string());
    }
    let pad = (width.min(height) * 0.06).clamp(1.5, 6.0);
    let horizontal = width >= height * 1.15;
    let (mark_area, text_area): (Option<Area>, Area) = match (mark.is_some(), horizontal) {
        (false, _) => (None, [pad, pad, width - pad, height - pad]),
        (true, true) => (
            Some([pad, pad, width * 0.42, height - pad]),
            [width * 0.42 + pad, pad, width - pad, height - pad],
        ),
        (true, false) => (
            Some([pad, height * 0.45, width - pad, height - pad]),
            [pad, pad, width - pad, height * 0.45 - pad * 0.5],
        ),
    };
    let mut content = String::new();
    // The frame, inside the box so that no reader cuts it off.
    content.push_str(&format!(
        "q 0.45 0.45 0.5 RG 0.6 w 0.3 0.3 {} {} re S Q\n",
        num(width - 0.6),
        num(height - 0.6)
    ));
    let mut raster = None;
    if let (Some(mark), Some(area)) = (mark, mark_area) {
        let (drawn, image) = draw_mark(mark, color, area)?;
        content.push_str(&drawn);
        raster = image;
    }

    // The text: as large as the area allows, up to ten points, and every line whole.
    let (room_w, room_h) = (text_area[2] - text_area[0], text_area[3] - text_area[1]);
    let mut lines: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
    let spacing = 1.2f32;
    let mut size = 10.0f32;
    // Fewer lines when even the smallest size does not fit them all (the last ones go first).
    while lines.len() > 1 && lines.len() as f32 * 3.0 * spacing > room_h {
        lines.pop();
    }
    if !lines.is_empty() && room_w > 2.0 && room_h > 2.0 {
        size = size.min(room_h / (lines.len() as f32 * spacing));
        let widest = lines
            .iter()
            .map(|l| text_width(&l.chars().map(winansi).collect::<Vec<u8>>(), 1.0))
            .fold(0.0f32, f32::max);
        if widest > 0.0 {
            size = size.min(room_w / widest);
        }
        size = size.max(3.0);
        let mut y = text_area[3] - size;
        for line in &lines {
            let bytes = fit_line(line, size, room_w);
            content.push_str(&format!(
                "BT /F1 {} Tf 0.1 0.1 0.1 rg {} {} Td ",
                num(size),
                num(text_area[0]),
                num(y)
            ));
            content.push_str(&String::from_utf8_lossy(&string_of(&bytes)).replace('\u{fffd}', "?"));
            content.push_str(" Tj ET\n");
            y -= size * spacing;
        }
    }
    // The string of a line holds bytes above 127 only as octal escapes, so the content is ASCII.
    debug_assert!(content.is_ascii());
    Ok(Look {
        width,
        height,
        matrix,
        content: content.into_bytes(),
        raster,
    })
}

/// Run-length encoding as PDF's `RunLengthDecode` reads it. Pictures with flat parts (a signature
/// on a transparent background) get much smaller; nothing else is promised.
pub fn run_length(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 2 + 16);
    let mut i = 0;
    while i < data.len() {
        // A run of at least two equal bytes, up to 128.
        let mut run = 1;
        while i + run < data.len() && run < 128 && data[i + run] == data[i] {
            run += 1;
        }
        if run >= 2 {
            out.push((257 - run) as u8);
            out.push(data[i]);
            i += run;
            continue;
        }
        // Literal bytes up to the next run of two, at most 128.
        let start = i;
        i += 1;
        while i < data.len() && i - start < 128 && !(i + 1 < data.len() && data[i] == data[i + 1]) {
            i += 1;
        }
        out.push((i - start - 1) as u8);
        out.extend_from_slice(&data[start..i]);
    }
    out.push(128);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unrun(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < data.len() {
            let l = data[i] as usize;
            i += 1;
            match l {
                128 => break,
                0..=127 => {
                    out.extend_from_slice(&data[i..i + l + 1]);
                    i += l + 1;
                }
                _ => {
                    out.extend(std::iter::repeat(data[i]).take(257 - l));
                    i += 1;
                }
            }
        }
        out
    }

    #[test]
    fn run_length_encoding_reads_back_as_it_was() {
        let cases: Vec<Vec<u8>> = vec![
            vec![],
            vec![1],
            vec![1, 1],
            vec![1, 2, 3, 4, 5],
            vec![0; 1000],
            (0..=255u8).collect(),
            [vec![7; 3], vec![1, 2, 3], vec![9; 200], vec![4]].concat(),
            (0..3000u32).map(|i| (i * i / 7 % 251) as u8).collect(),
        ];
        for case in cases {
            assert_eq!(unrun(&run_length(&case)), case, "{} bytes", case.len());
        }
        // Flat data gets small.
        assert!(run_length(&[0u8; 100_000]).len() < 2_000);
    }

    #[test]
    fn numbers_have_no_exponent_and_no_tail() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(0.5), "0.5");
        assert_eq!(num(72.12345), "72.123");
        assert_eq!(num(-0.0001), "0");
        assert_eq!(num(100000.0), "100000");
        assert_eq!(num(-3.2), "-3.2");
    }

    #[test]
    fn letters_the_font_has_are_kept_and_others_are_a_question_mark() {
        assert_eq!(winansi('A'), b'A');
        assert_eq!(winansi('\u{e5}'), 0xe5);
        assert_eq!(winansi('\u{d8}'), 0xd8);
        assert_eq!(winansi('\u{20ac}'), 0x80);
        assert_eq!(winansi('\u{6d4b}'), b'?');
        assert_eq!(winansi('\u{1f600}'), b'?');
    }

    #[test]
    fn strings_escape_what_pdf_wants_escaped() {
        assert_eq!(string_of(b"a(b)c\\"), b"(a\\(b\\)c\\\\)".to_vec());
        assert_eq!(string_of(&[0xe5, b'x']), b"(\\345x)".to_vec());
    }

    #[test]
    fn a_long_line_is_cut_with_three_dots_to_fit() {
        let line = "A very long name that cannot possibly fit in a small box on a page";
        let bytes = fit_line(line, 10.0, 80.0);
        assert!(bytes.ends_with(b"..."));
        assert!(text_width(&bytes, 10.0) <= 80.0 + 0.01);
        assert_eq!(fit_line("Anna", 10.0, 80.0), b"Anna".to_vec());
    }

    #[test]
    fn the_look_holds_the_frame_and_the_lines_and_is_plain_text() {
        let look = build(
            200.0,
            60.0,
            [1.0, 0.0, 0.0, 1.0],
            &[
                "Digitally signed by \u{c5}se".to_string(),
                "Reason: read".to_string(),
                String::new(),
                "Date: 2026-11-03 14:05 UTC".to_string(),
            ],
            None,
            [0, 0, 0, 255],
        )
        .unwrap();
        let text = String::from_utf8(look.content.clone()).unwrap();
        assert!(text.contains(" re S"));
        assert_eq!(text.matches(" Tj ET").count(), 3);
        assert!(text.contains("\\305"));
        assert!(look.raster.is_none());
    }

    #[test]
    fn a_picture_takes_the_left_of_a_wide_box_and_the_top_of_a_tall_one() {
        let mark = SignatureMark::Strokes {
            width: 100.0,
            height: 40.0,
            line: 2.0,
            strokes: vec![vec![[5.0, 5.0], [90.0, 30.0]]],
        };
        let wide = build(300.0, 80.0, [1.0, 0.0, 0.0, 1.0], &["x".into()], Some(&mark), [0, 0, 0, 255])
            .unwrap();
        let tall = build(80.0, 300.0, [1.0, 0.0, 0.0, 1.0], &["x".into()], Some(&mark), [0, 0, 0, 255])
            .unwrap();
        let first_x = |look: &Look| {
            let text = String::from_utf8(look.content.clone()).unwrap();
            let at = text.find(" w 1 J 1 j\n").unwrap() + 11;
            text[at..].split_whitespace().next().unwrap().parse::<f32>().unwrap()
        };
        let first_y = |look: &Look| {
            let text = String::from_utf8(look.content.clone()).unwrap();
            let at = text.find(" w 1 J 1 j\n").unwrap() + 11;
            text[at..].split_whitespace().nth(1).unwrap().parse::<f32>().unwrap()
        };
        assert!(first_x(&wide) < 300.0 * 0.42);
        assert!(first_y(&tall) > 300.0 * 0.45);
    }

    #[test]
    fn a_typed_picture_is_filled_curves_and_an_image_is_a_raster() {
        let typed = SignatureMark::Typed {
            font: partridge_protocol::SignatureFont::GreatVibes,
            text: "Anna".into(),
        };
        let look = build(240.0, 70.0, [1.0, 0.0, 0.0, 1.0], &["x".into()], Some(&typed), [10, 20, 120, 255]).unwrap();
        let text = String::from_utf8(look.content).unwrap();
        assert!(text.contains(" c "));
        assert!(text.contains("f\nQ"));

        let image = SignatureMark::Image {
            width: 2,
            height: 1,
            rgba: vec![1, 2, 3, 255, 4, 5, 6, 0],
        };
        let look = build(240.0, 70.0, [1.0, 0.0, 0.0, 1.0], &["x".into()], Some(&image), [0, 0, 0, 255]).unwrap();
        let raster = look.raster.unwrap();
        assert_eq!((raster.width, raster.height), (2, 1));
        assert_eq!(raster.rgb, vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(raster.alpha, Some(vec![255, 0]));
        assert!(String::from_utf8(look.content).unwrap().contains("/Im0 Do"));
    }

    #[test]
    fn a_box_too_small_to_draw_in_is_refused() {
        assert!(build(0.5, 20.0, [1.0, 0.0, 0.0, 1.0], &[], None, [0; 4]).is_err());
        assert!(build(f32::NAN, 20.0, [1.0, 0.0, 0.0, 1.0], &[], None, [0; 4]).is_err());
    }
}
