//! The pictures of signatures for the fuzz targets: valid ones made out of fuzzer bytes, the caps
//! of the protocol as assertions, and requests that break exactly one cap.

use arbitrary::{Result, Unstructured};
use partridge_protocol::{
    mark_char_ok, SignatureFont, SignatureMark, MAX_MARK_IMAGE_PIXELS, MAX_MARK_IMAGE_SIDE,
    MAX_MARK_LINE, MAX_MARK_POINTS, MAX_MARK_SPACE, MAX_MARK_STROKES, MAX_MARK_TEXT_CHARS,
    MIN_MARK_LINE,
};

use crate::chance;

/// A number inside the space of a drawn picture, sometimes on its edge.
fn space(u: &mut Unstructured) -> Result<f32> {
    if chance(u, 16)? {
        return Ok(if u.arbitrary()? {
            MAX_MARK_SPACE * 2.0
        } else {
            -MAX_MARK_SPACE * 2.0
        });
    }
    Ok(u.int_in_range(-2_000i32..=2_000)? as f32 * 0.5)
}

/// A side of the box of a drawn picture: more than nothing, at most the most.
fn side(u: &mut Unstructured) -> Result<f32> {
    if chance(u, 16)? {
        return Ok(MAX_MARK_SPACE);
    }
    Ok(u.int_in_range(1..=4_096u32)? as f32)
}

fn arb_font(u: &mut Unstructured) -> Result<SignatureFont> {
    Ok(SignatureFont::ALL[u.int_in_range(0..=SignatureFont::ALL.len() - 1)?])
}

/// The picture of a signature inside every cap: a few short strokes mostly, and now and then the
/// most strokes, the most points, the longest text, or the biggest image the protocol allows (made
/// of repeated values, so that they cost the fuzzer's input nothing).
pub fn arb_mark(u: &mut Unstructured) -> Result<SignatureMark> {
    Ok(match u.int_in_range(0..=2u8)? {
        0 => {
            let strokes = if chance(u, 24)? {
                if u.arbitrary()? {
                    vec![vec![[1.0, 1.0]]; MAX_MARK_STROKES]
                } else {
                    vec![vec![[2.0, 3.0]; MAX_MARK_POINTS]]
                }
            } else {
                let count = u.int_in_range(1..=4usize)?;
                let mut strokes = Vec::new();
                for _ in 0..count {
                    let points = u.int_in_range(1..=6usize)?;
                    let mut stroke = Vec::new();
                    for _ in 0..points {
                        stroke.push([space(u)?, space(u)?]);
                    }
                    strokes.push(stroke);
                }
                strokes
            };
            SignatureMark::Strokes {
                width: side(u)?,
                height: side(u)?,
                line: if chance(u, 8)? {
                    [MIN_MARK_LINE, MAX_MARK_LINE][u.int_in_range(0..=1usize)?]
                } else {
                    u.int_in_range(1..=2_560u32)? as f32 * 0.1
                },
                strokes,
            }
        }
        1 => {
            let alphabet = ['A', 'n', 'a', ' ', '\u{e6}', '\u{4e2d}', '\u{1f4d5}', '-'];
            let count = if chance(u, 8)? {
                MAX_MARK_TEXT_CHARS
            } else {
                u.int_in_range(1..=12usize)?
            };
            let mut text = String::new();
            for i in 0..count {
                // The first character has ink, so that the text is not only spaces.
                let c = alphabet[u.int_in_range(if i == 0 { 0..=2usize } else { 0..=7 })?];
                if mark_char_ok(c) {
                    text.push(c);
                }
            }
            SignatureMark::Typed {
                font: arb_font(u)?,
                text,
            }
        }
        _ => {
            let (width, height) = if chance(u, 24)? {
                (
                    MAX_MARK_IMAGE_SIDE,
                    MAX_MARK_IMAGE_PIXELS / MAX_MARK_IMAGE_SIDE,
                )
            } else {
                (u.int_in_range(1..=16u32)?, u.int_in_range(1..=16u32)?)
            };
            let pixel: [u8; 4] = u.arbitrary()?;
            let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
            for _ in 0..width as usize * height as usize {
                rgba.extend_from_slice(&pixel);
            }
            SignatureMark::Image {
                width,
                height,
                rgba,
            }
        }
    })
}

/// Panics when a picture is outside the caps the protocol promises.
pub fn check_mark_caps(mark: &SignatureMark) {
    assert!(mark.within_caps(), "a picture outside the caps");
    match mark {
        SignatureMark::Strokes { strokes, .. } => {
            assert!(strokes.len() <= MAX_MARK_STROKES);
            assert!(strokes.iter().map(Vec::len).sum::<usize>() <= MAX_MARK_POINTS);
        }
        SignatureMark::Typed { text, .. } => {
            assert!(text.chars().count() <= MAX_MARK_TEXT_CHARS);
        }
        SignatureMark::Image {
            width,
            height,
            rgba,
        } => {
            assert!(*width <= MAX_MARK_IMAGE_SIDE && *height <= MAX_MARK_IMAGE_SIDE);
            assert!(width * height <= MAX_MARK_IMAGE_PIXELS);
            assert_eq!(rgba.len(), *width as usize * *height as usize * 4);
        }
    }
}

// ---- requests that break exactly one cap -----------------------------------------------------

/// How many cases `mark_over_cap_request_case` has.
pub const MARK_OVER_REQUESTS: usize = 9;

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// The start of an `AddSignature` request: the tag, the id, the document, the page, a box and a
/// colour. The picture follows.
fn add_head(id: u32, rect: [f32; 4]) -> Vec<u8> {
    let mut p = vec![23u8];
    put_u32(&mut p, id);
    put_u32(&mut p, 1);
    put_u32(&mut p, 0);
    for v in rect {
        put_f32(&mut p, v);
    }
    p.extend_from_slice(&[0, 0, 0, 255]);
    p
}

const BOX: [f32; 4] = [10.0, 10.0, 200.0, 100.0];

/// Strokes with a good box and the given line, and the given strokes of the given number of points.
fn strokes_head(id: u32, line: f32, strokes: usize, points: &[usize]) -> Vec<u8> {
    let mut p = add_head(id, BOX);
    p.push(1);
    for v in [100.0f32, 50.0, line] {
        put_f32(&mut p, v);
    }
    put_u32(&mut p, strokes as u32);
    for n in points {
        put_u32(&mut p, *n as u32);
        for _ in 0..*n {
            put_f32(&mut p, 1.0);
            put_f32(&mut p, 1.0);
        }
    }
    p
}

fn typed_with(id: u32, text: &str) -> Vec<u8> {
    let mut p = add_head(id, BOX);
    p.extend_from_slice(&[2, 1]);
    put_u32(&mut p, text.len() as u32);
    p.extend_from_slice(text.as_bytes());
    p
}

fn image_head(id: u32, width: u32, height: u32) -> Vec<u8> {
    let mut p = add_head(id, BOX);
    p.push(3);
    put_u32(&mut p, width);
    put_u32(&mut p, height);
    p
}

/// An `AddSignature` request that is valid except that one cap is broken by one. The decoder must
/// refuse it. Images are given without the pixels they name where that does not matter: the size
/// is refused before any pixel is read.
pub fn mark_over_cap_request_case(case: usize, id: u32) -> (&'static str, Vec<u8>) {
    match case % MARK_OVER_REQUESTS {
        0 => {
            let points = vec![1; MAX_MARK_STROKES + 1];
            (
                "signature drawn with one stroke too many",
                strokes_head(id, 2.0, MAX_MARK_STROKES + 1, &points),
            )
        }
        1 => (
            "signature drawn with one point too many",
            strokes_head(id, 2.0, 2, &[MAX_MARK_POINTS - 1, 2]),
        ),
        2 => (
            "signature drawn with a line too thin",
            strokes_head(id, MIN_MARK_LINE / 2.0, 1, &[1]),
        ),
        3 => (
            "signature typed with one character too many",
            typed_with(id, &"a".repeat(MAX_MARK_TEXT_CHARS + 1)),
        ),
        4 => (
            "signature typed with a control character",
            typed_with(id, "A\u{0}b"),
        ),
        5 => (
            "signature image one pixel too wide",
            image_head(id, MAX_MARK_IMAGE_SIDE + 1, 1),
        ),
        6 => {
            let mut p = image_head(
                id,
                MAX_MARK_IMAGE_SIDE,
                MAX_MARK_IMAGE_PIXELS / MAX_MARK_IMAGE_SIDE + 1,
            );
            p.resize(
                p.len()
                    + MAX_MARK_IMAGE_SIDE as usize
                        * (MAX_MARK_IMAGE_PIXELS / MAX_MARK_IMAGE_SIDE + 1) as usize
                        * 4,
                0,
            );
            ("signature image with one row too many", p)
        }
        7 => {
            let mut p = add_head(id, BOX);
            p.push(9);
            ("signature of a kind nobody defined", p)
        }
        _ => {
            let mut p = add_head(id, [f32::NAN, 10.0, 200.0, 100.0]);
            p.push(2);
            p.push(1);
            put_u32(&mut p, 1);
            p.push(b'a');
            ("signature box that is not a number", p)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use partridge_protocol::{Request, RequestBody};

    #[test]
    fn every_request_that_breaks_one_cap_is_refused_by_the_decoder() {
        for case in 0..MARK_OVER_REQUESTS {
            let (name, payload) = mark_over_cap_request_case(case, 7);
            assert!(Request::decode(&payload).is_err(), "{name} was accepted");
        }
    }

    #[test]
    fn the_head_of_a_request_is_good_so_only_the_one_cap_is_to_blame() {
        // The same requests with the one fault put right are accepted.
        let mut p = add_head(7, BOX);
        p.extend_from_slice(&[2, 1]);
        put_u32(&mut p, 4);
        p.extend_from_slice(b"Anna");
        assert!(matches!(
            Request::decode(&p).unwrap().body,
            RequestBody::AddSignature { .. }
        ));
        let p = strokes_head(7, 2.0, MAX_MARK_STROKES, &vec![1; MAX_MARK_STROKES]);
        assert!(Request::decode(&p).is_ok());
        let p = strokes_head(7, 2.0, 2, &[MAX_MARK_POINTS - 2, 2]);
        assert!(Request::decode(&p).is_ok());
        let p = typed_with(7, &"a".repeat(MAX_MARK_TEXT_CHARS));
        assert!(Request::decode(&p).is_ok());
    }

    #[test]
    fn made_up_pictures_are_always_inside_the_caps() {
        let mut seed = 0x1234_5678u32;
        for _ in 0..300 {
            let bytes: Vec<u8> = (0..300)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 24) as u8
                })
                .collect();
            let mut u = Unstructured::new(&bytes);
            if let Ok(mark) = arb_mark(&mut u) {
                check_mark_caps(&mark);
            }
        }
    }
}
