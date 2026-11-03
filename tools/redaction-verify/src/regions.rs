//! The region check: given the rectangles that were redacted, find out whether anything can still
//! be seen inside them. There are two checks, and a region must pass both.
//!
//! 1. The render. The page is drawn by Poppler (`pdftoppm`, a renderer that is not the engine under
//!    test) at [`RENDER_DPI`]. Every device pixel whose whole square lies inside the region, less
//!    a margin of [`RENDER_MARGIN`] pixels at the edge (anti-aliasing), must have the fill colour
//!    of the region, or, if the spec names none, one colour. This sees what a reader sees: masks,
//!    patterns, vector shapes and text that was turned into outlines.
//! 2. The images. The images drawn over the region (page content, forms, annotation appearances)
//!    are decoded, with their soft mask and mask, and the samples whose whole square lies inside
//!    the region must all be one value (within a tolerance for a lossy codec). This does not
//!    depend on the codec: a sample is what the decoder gives, however the engine stored the
//!    image again.
//!
//! Where the check cannot be done (a codec that is not decoded, a mask that cannot be read, a
//! region too small to leave a pixel after the margin, a render that fails) it is a problem.
//!
//! A JPEG changes the pixels near the edge of what was blanked: the blocks that straddle the edge
//! of the region (8 x 8 samples, 16 x 16 with subsampled colour, and the colour that spreads one
//! sample further) are left out of both checks, and the rest may differ from one value by what the
//! quantisation of a flat block allows (derived from the quantisation tables of the file).

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};

use crate::carve;
use crate::content::{for_each_op, Op};
use crate::layers::Ctx;
use crate::pdfobj::{Doc, Entry, Obj};
use crate::tools;
use crate::{Hit, Layer, Region, Report, Result, Subject, RENDER_DPI, RENDER_MARGIN};

type M = [f64; 6];
const IDENTITY: M = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
/// The most device pixels one render may have.
const MAX_RENDER_PIXELS: usize = 80_000_000;

fn mul(a: &M, b: &M) -> M {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

fn apply(m: &M, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn invert(m: &M) -> Option<M> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 {
        return None;
    }
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    Some([a, b, c, d, -(m[4] * a + m[5] * c), -(m[4] * b + m[5] * d)])
}

/// Whether the matrix maps the axes of the image onto the axes of the page (a turn by a multiple
/// of 90 degrees, with or without a flip).
fn axis_aligned(m: &M) -> bool {
    let size = m[..4].iter().fold(0.0f64, |a, b| a.max(b.abs()));
    let tiny = |v: f64| v.abs() < 1e-6 * (1.0 + size);
    (tiny(m[1]) && tiny(m[2])) || (tiny(m[0]) && tiny(m[3]))
}

fn matrix_of(o: &Obj, doc: &Doc) -> M {
    match doc.resolve(o) {
        Obj::Array(a) if a.len() == 6 => {
            let mut m = IDENTITY;
            for (i, x) in a.iter().enumerate() {
                m[i] = doc.resolve(x).num().unwrap_or(m[i]);
            }
            m
        }
        _ => IDENTITY,
    }
}

fn numbers(o: &Obj, doc: &Doc) -> Vec<f64> {
    doc.resolve(o)
        .array()
        .map(|a| a.iter().filter_map(|x| doc.resolve(x).num()).collect())
        .unwrap_or_default()
}

/// An image that is drawn, with the matrix that maps its unit square to the page.
struct Placement {
    ctm: M,
    /// The image object, or the dictionary text and data of an inline image.
    image: Source,
}

enum Source {
    Object(u32),
    Inline(Vec<u8>, Vec<u8>),
}

struct Walker<'a, 'd> {
    doc: &'a Doc<'d>,
    out: Vec<Placement>,
    problems: Vec<String>,
}

impl Walker<'_, '_> {
    fn content(&mut self, data: &[u8], res: &Obj, ctm: M, depth: usize) {
        if depth > 8 {
            self.problems
                .push("forms are nested more than 8 deep".to_owned());
            return;
        }
        let mut ctm = ctm;
        let mut stack: Vec<M> = Vec::new();
        for_each_op(data, &mut |op: &Op| match op.name {
            b"q" => stack.push(ctm),
            b"Q" => {
                if let Some(m) = stack.pop() {
                    ctm = m;
                }
            }
            b"cm" => {
                let n = op.nums();
                if n.len() == 6 {
                    ctm = mul(&[n[0], n[1], n[2], n[3], n[4], n[5]], &ctm);
                }
            }
            b"BI" => {
                if let Some((d, data)) = op.inline {
                    self.out.push(Placement {
                        ctm,
                        image: Source::Inline(d.to_vec(), data.to_vec()),
                    });
                }
            }
            b"Do" => {
                let Some(name) = op.name_arg() else { return };
                let name: String = name.iter().map(|b| char::from(*b)).collect();
                let xo = self.doc.dget(res, "XObject");
                let Some(Obj::Ref(n)) = self.doc.resolve(xo).get(&name) else {
                    return;
                };
                self.xobject(*n, res, ctm, depth);
            }
            // A pattern colour: its images are drawn wherever the pattern is painted.
            b"scn" | b"SCN" | b"sc" | b"SC" => {
                if let Some(name) = op.name_arg() {
                    self.pattern(name, res);
                }
            }
            // A soft mask is a form that is drawn in the space of the moment it is set.
            b"gs" => {
                if let Some(name) = op.name_arg() {
                    let name: String = name.iter().map(|b| char::from(*b)).collect();
                    let state = self.doc.dget(self.doc.dget(res, "ExtGState"), &name);
                    let mask = self.doc.dget(state, "SMask");
                    if let Some(Obj::Ref(n)) = mask.get("G") {
                        self.xobject(*n, res, ctm, depth);
                    }
                }
            }
            _ => {}
        });
    }

    /// A tiling pattern that draws an image: where it is painted is not followed (that needs the
    /// path), so the image is not placed. The render check sees the pixels; the object-level check
    /// cannot, and says so.
    fn pattern(&mut self, name: &[u8], res: &Obj) {
        let name: String = name.iter().map(|b| char::from(*b)).collect();
        let pats = self.doc.dget(res, "Pattern");
        let Some(Obj::Ref(n)) = self.doc.resolve(pats).get(&name) else {
            return;
        };
        let Some(e) = self.doc.latest(*n) else { return };
        if e.stream.is_some() && draws_image(self.doc, e, 0) {
            let msg = format!(
                "an image is drawn through the tiling pattern /{name}: where the pattern is painted is not followed, so the pixels of that image were checked only in the render"
            );
            if !self.problems.contains(&msg) {
                self.problems.push(msg);
            }
        }
    }

    fn xobject(&mut self, n: u32, res: &Obj, ctm: M, depth: usize) {
        let Some(e) = self.doc.latest(n) else { return };
        match self.doc.dget(&e.value, "Subtype").name() {
            Some("Image") => self.out.push(Placement {
                ctm,
                image: Source::Object(n),
            }),
            Some("Form") => {
                let Some(d) = self.doc.stream_data(e) else {
                    return;
                };
                if let Some(p) = &d.problem {
                    self.problems.push(format!("form object {n}: {p}"));
                }
                let m = e
                    .value
                    .get("Matrix")
                    .map_or(IDENTITY, |m| matrix_of(m, self.doc));
                let own = e
                    .value
                    .get("Resources")
                    .map(|r| self.doc.resolve(r).clone());
                let res2 = own.unwrap_or_else(|| res.clone());
                self.content(&d.data, &res2, mul(&m, &ctm), depth + 1);
            }
            _ => {}
        }
    }
}

/// Whether the content of a form or pattern draws an image (also through a form it draws).
fn draws_image(doc: &Doc, e: &Entry, depth: usize) -> bool {
    let Some(d) = doc.stream_data(e) else {
        return false;
    };
    let res = e
        .value
        .get("Resources")
        .map_or(Obj::Null, |r| doc.resolve(r).clone());
    let mut found = false;
    for_each_op(&d.data, &mut |op: &Op| match op.name {
        b"BI" => found = true,
        b"Do" => {
            let Some(name) = op.name_arg() else { return };
            let name: String = name.iter().map(|b| char::from(*b)).collect();
            let xo = doc.dget(&res, "XObject");
            let Some(Obj::Ref(n)) = doc.resolve(xo).get(&name) else {
                return;
            };
            let Some(x) = doc.latest(*n) else { return };
            match doc.dget(&x.value, "Subtype").name() {
                Some("Image") => found = true,
                Some("Form") if depth < 4 => found |= draws_image(doc, x, depth + 1),
                _ => {}
            }
        }
        _ => {}
    });
    found
}

fn contents(doc: &Doc, page: &Entry) -> Option<Vec<u8>> {
    let c = page.value.get("Contents")?;
    let mut data = Vec::new();
    let mut add = |o: &Obj| {
        if let Obj::Ref(n) = o {
            if let Some(d) = doc.latest(*n).and_then(|e| doc.stream_data(e)) {
                data.extend_from_slice(&d.data);
                data.push(b'\n');
            }
        }
    };
    match c {
        Obj::Array(a) => a.iter().for_each(&mut add),
        Obj::Ref(n) => match doc.latest(*n).map(|e| &e.value) {
            Some(Obj::Array(a)) => a.iter().for_each(&mut add),
            _ => add(c),
        },
        _ => {}
    }
    Some(data)
}

// ---------------------------------------------------------------------------------------------
// Decoding images.

/// What a JPEG says about the error that it makes in a flat block, and the blocks it works in.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Lossy {
    /// How far a flat block may be from its value in one component after decoding.
    tolerance: u8,
    /// The size of a block of samples that the codec codes together, and how many samples the
    /// colour of a block spreads into its neighbours.
    mcu: (usize, usize),
    spread: (usize, usize),
}

/// A decoded image: 8-bit samples, `ncomp` per pixel, rows without padding.
struct Pixels {
    w: usize,
    h: usize,
    ncomp: usize,
    data: Vec<u8>,
    lossy: Option<Lossy>,
}

/// The tolerance and the blocks of a JPEG, from the first value of its quantisation tables and the
/// largest sampling factors in the markers of the file.
fn jpeg_lossy(data: &[u8]) -> Option<Lossy> {
    if data.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    let mut steps: Vec<u16> = Vec::new();
    let (mut hmax, mut vmax) = (1usize, 1usize);
    let mut sof = false;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let m = data[i + 1];
        if m == 0xFF {
            i += 1;
            continue;
        }
        if m == 0x00 || m == 0x01 || (0xD0..=0xD7).contains(&m) {
            i += 2;
            continue;
        }
        if m == 0xD9 || m == 0xDA {
            break;
        }
        let len = usize::from(data[i + 2]) << 8 | usize::from(data[i + 3]);
        if len < 2 {
            return None;
        }
        let seg = data.get(i + 4..i + 2 + len)?;
        match m {
            0xDB => {
                let mut p = 0;
                while p < seg.len() {
                    let wide = seg[p] >> 4 != 0;
                    p += 1;
                    let size = if wide { 2 } else { 1 };
                    if p + 64 * size > seg.len() {
                        return None;
                    }
                    steps.push(if wide {
                        u16::from(seg[p]) << 8 | u16::from(seg[p + 1])
                    } else {
                        u16::from(seg[p])
                    });
                    p += 64 * size;
                }
            }
            0xC0..=0xC2 => {
                let n = usize::from(*seg.get(5)?);
                for c in 0..n {
                    let hv = *seg.get(6 + c * 3 + 1)?;
                    hmax = hmax.max(usize::from(hv >> 4));
                    vmax = vmax.max(usize::from(hv & 15));
                }
                sof = true;
            }
            _ => {}
        }
        i += 2 + len;
    }
    if !sof || steps.is_empty() {
        return None;
    }
    // A flat block has one coefficient, the DC value; quantising it is off by half a step, and the
    // pixel value is one eighth of the coefficient. The colour conversion adds the error of the
    // colour components to the one of the luma (a factor of 1.8 at most), and 2 more levels are
    // rounding.
    let step = f64::from(*steps.iter().max()?);
    let tolerance = (step / 16.0 * 2.8 + 2.0).ceil().min(255.0) as u8;
    let (hmax, vmax) = (hmax.clamp(1, 4), vmax.clamp(1, 4));
    Some(Lossy {
        tolerance,
        mcu: (8 * hmax, 8 * vmax),
        spread: (
            if hmax > 1 { hmax } else { 0 },
            if vmax > 1 { vmax } else { 0 },
        ),
    })
}

fn components(doc: &Doc, cs: &Obj) -> std::result::Result<usize, String> {
    match doc.resolve(cs) {
        Obj::Name(n) => match n.as_str() {
            "DeviceGray" | "G" | "CalGray" => Ok(1),
            "DeviceRGB" | "RGB" | "CalRGB" => Ok(3),
            "DeviceCMYK" | "CMYK" => Ok(4),
            other => Err(format!("colour space {other}")),
        },
        Obj::Array(a) => match a.first().map(|x| doc.resolve(x)) {
            Some(Obj::Name(n)) => match n.as_str() {
                "Indexed" | "I" | "Separation" => Ok(1),
                "CalGray" => Ok(1),
                "CalRGB" | "Lab" => Ok(3),
                "ICCBased" => a
                    .get(1)
                    .map(|s| doc.resolve(s))
                    .and_then(|s| doc.dget(s, "N").int())
                    .map(|n| n as usize)
                    .ok_or_else(|| "an ICC profile without /N".to_owned()),
                "DeviceN" => a
                    .get(1)
                    .and_then(|x| doc.resolve(x).array())
                    .map(<[Obj]>::len)
                    .ok_or_else(|| "a DeviceN colour space without names".to_owned()),
                other => Err(format!("colour space {other}")),
            },
            _ => Err("a colour space that cannot be read".to_owned()),
        },
        _ => Err("an image without a colour space".to_owned()),
    }
}

fn unpack(
    raw: &[u8],
    w: usize,
    h: usize,
    ncomp: usize,
    bits: usize,
) -> std::result::Result<Vec<u8>, String> {
    if ![1usize, 2, 4, 8, 16].contains(&bits) {
        return Err(format!("{bits} bits per component"));
    }
    let row = (w * ncomp * bits).div_ceil(8);
    if row == 0 || raw.len() < row * h {
        return Err(format!(
            "the image data is shorter than the image ({} of {} bytes)",
            raw.len(),
            row * h
        ));
    }
    let mut out = Vec::with_capacity(w * h * ncomp);
    for y in 0..h {
        let line = &raw[y * row..(y + 1) * row];
        for i in 0..w * ncomp {
            let v = match bits {
                8 => line[i],
                16 => line[i * 2],
                b => {
                    let per = 8 / b;
                    let byte = line[i / per];
                    let shift = 8 - b * (i % per + 1);
                    let max = (1u16 << b) - 1;
                    ((u16::from(byte >> shift) & max) * 255 / max) as u8
                }
            };
            out.push(v);
        }
    }
    Ok(out)
}

fn decode_image(
    doc: &Doc,
    dict: &Obj,
    decoded: &carve::Decoded,
) -> std::result::Result<Pixels, String> {
    let w = doc
        .dget(dict, "Width")
        .int()
        .or_else(|| doc.dget(dict, "W").int())
        .ok_or("an image without /Width")?;
    let h = doc
        .dget(dict, "Height")
        .int()
        .or_else(|| doc.dget(dict, "H").int())
        .ok_or("an image without /Height")?;
    if w <= 0 || h <= 0 || w > 100_000 || h > 100_000 || w * h > 100_000_000 {
        return Err(format!("an image of {w} x {h} pixels"));
    }
    let (w, h) = (w as usize, h as usize);
    if let Some(stop) = &decoded.stopped_at {
        return match stop.as_str() {
            "DCTDecode" | "DCT" => {
                let mut dec = jpeg_decoder::Decoder::new(decoded.data.as_slice());
                let px = dec.decode().map_err(|e| format!("the JPEG data: {e}"))?;
                let info = dec.info().ok_or("a JPEG without a header")?;
                if usize::from(info.width) != w || usize::from(info.height) != h {
                    return Err("the JPEG size differs from the image size".to_owned());
                }
                let (ncomp, data) = match info.pixel_format {
                    jpeg_decoder::PixelFormat::L8 => (1, px),
                    jpeg_decoder::PixelFormat::L16 => {
                        (1, px.chunks(2).map(|p| p[0]).collect::<Vec<u8>>())
                    }
                    jpeg_decoder::PixelFormat::RGB24 => (3, px),
                    jpeg_decoder::PixelFormat::CMYK32 => (4, px),
                };
                if data.len() < w * h * ncomp {
                    return Err("the JPEG data is shorter than the image".to_owned());
                }
                // A JPEG whose tables cannot be read is judged by the widest blocks and a loose
                // tolerance, never by none.
                let lossy = jpeg_lossy(&decoded.data).unwrap_or(Lossy {
                    tolerance: 16,
                    mcu: (16, 16),
                    spread: (2, 2),
                });
                Ok(Pixels {
                    w,
                    h,
                    ncomp,
                    data,
                    lossy: Some(lossy),
                })
            }
            other => Err(format!("the filter {other} is not decoded")),
        };
    }
    if let Some(p) = &decoded.problem {
        return Err(p.clone());
    }
    let is_mask = matches!(doc.dget(dict, "ImageMask"), Obj::Bool(true))
        || matches!(doc.dget(dict, "IM"), Obj::Bool(true));
    let (ncomp, bits) = if is_mask {
        (1, 1)
    } else {
        let cs = dict.get("ColorSpace").or_else(|| dict.get("CS"));
        let ncomp = components(doc, cs.unwrap_or(&Obj::Null))?;
        let bits = doc
            .dget(dict, "BitsPerComponent")
            .int()
            .or_else(|| doc.dget(dict, "BPC").int())
            .unwrap_or(8) as usize;
        (ncomp, bits)
    };
    Ok(Pixels {
        w,
        h,
        ncomp,
        data: unpack(&decoded.data, w, h, ncomp, bits)?,
        lossy: None,
    })
}

/// The soft mask or the stencil mask of an image, decoded like an image. `None` when there is
/// none (or a colour key, which is a function of the samples that are checked anyway).
fn decode_mask(doc: &Doc, dict: &Obj, key: &str) -> Option<std::result::Result<Pixels, String>> {
    let v = dict.get(key)?;
    match doc.resolve(v) {
        Obj::Array(_) => return None,
        Obj::Null => return Some(Err(format!("its /{key} points to nothing"))),
        _ => {}
    }
    let Obj::Ref(n) = v else {
        return Some(Err(format!("its /{key} is not a stream")));
    };
    let Some(e) = doc.latest(*n) else {
        return Some(Err(format!("its /{key} points to nothing")));
    };
    let Some(d) = doc.stream_data(e) else {
        return Some(Err(format!("its /{key} is not a stream")));
    };
    Some(decode_image(doc, &e.value, &d).map_err(|why| format!("its /{key} ({why})")))
}

// ---------------------------------------------------------------------------------------------
// The image check.

/// An image with the pixels that its codec changes near an edge: the render check leaves out the
/// part of the image that lies in the region and outside `safe`.
struct Zone {
    /// The bounds of the placed image, in points.
    bbox: [f64; 4],
    /// The blocks that lie wholly in the region, in points (`None`: none, or not axis aligned).
    safe: Option<[f64; 4]>,
    /// How far a rendered pixel may be from the others.
    tolerance: u8,
}

fn bounds(points: &[(f64, f64)]) -> [f64; 4] {
    let (x0, x1) = points
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p.0), h.max(p.0)));
    let (y0, y1) = points
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p.1), h.max(p.1)));
    [x0, y0, x1, y1]
}

/// The result of looking at one set of samples under a region.
struct Scan {
    covered: usize,
    differing: usize,
    /// For a lossy image: the part that was looked at, in points.
    safe: Option<[f64; 4]>,
}

/// The samples of `px` (drawn with `ctm`) whose whole square lies inside `rect`, and how many of
/// them differ from the first by more than `tolerance`. For a JPEG whose region names its fill,
/// the samples are compared with the fill, within what the quantisation of a flat block allows.
fn scan(px: &Pixels, ctm: &M, rect: [f64; 4], tolerance: u8, fill: Option<[u8; 3]>) -> Scan {
    let [rx0, ry0, rx1, ry1] = rect;
    let mut out = Scan {
        covered: 0,
        differing: 0,
        safe: None,
    };
    let Some(inv) = invert(ctm) else { return out };
    let uv: Vec<(f64, f64)> = [(rx0, ry0), (rx1, ry0), (rx0, ry1), (rx1, ry1)]
        .iter()
        .map(|(x, y)| apply(&inv, *x, *y))
        .collect();
    let [u0, v0, u1, v1] = bounds(&uv);
    let (w, h) = (px.w as f64, px.h as f64);
    let col0 = ((u0 * w).floor().max(0.0)) as usize;
    let col1 = ((u1 * w).ceil().min(w)) as usize;
    let row0 = (((1.0 - v1) * h).floor().max(0.0)) as usize;
    let row1 = (((1.0 - v0) * h).ceil().min(h)) as usize;
    let eps = 1e-3;
    let inside = |col: usize, row: usize| -> bool {
        let (cx0, cx1) = (col as f64 / w, (col + 1) as f64 / w);
        let (cy0, cy1) = (1.0 - (row + 1) as f64 / h, 1.0 - row as f64 / h);
        [(cx0, cy0), (cx1, cy0), (cx0, cy1), (cx1, cy1)]
            .iter()
            .all(|(u, v)| {
                let (x, y) = apply(ctm, *u, *v);
                x >= rx0 - eps && x <= rx1 + eps && y >= ry0 - eps && y <= ry1 + eps
            })
    };
    // The samples that lie inside, as a box.
    let (mut c0, mut c1, mut r0, mut r1) = (usize::MAX, 0usize, usize::MAX, 0usize);
    for row in row0..row1.min(px.h) {
        for col in col0..col1.min(px.w) {
            if inside(col, row) {
                c0 = c0.min(col);
                c1 = c1.max(col + 1);
                r0 = r0.min(row);
                r1 = r1.max(row + 1);
            }
        }
    }
    if c0 == usize::MAX {
        return out;
    }
    let mut tol = i32::from(tolerance);
    // The fill as samples of this image (red, green, blue, or the gray of it).
    let expected: Option<Vec<u8>> =
        fill.filter(|_| px.lossy.is_some())
            .and_then(|f| match px.ncomp {
                3 => Some(f.to_vec()),
                1 => Some(vec![
                    ((u32::from(f[0]) * 299 + u32::from(f[1]) * 587 + u32::from(f[2]) * 114) / 1000)
                        as u8,
                ]),
                _ => None,
            });
    if let Some(l) = &px.lossy {
        // Leave out the blocks that straddle the edge of the region, and the samples next to
        // them that take colour from them. At the edge of the image there is no neighbour.
        if !axis_aligned(ctm) {
            return out;
        }
        let shrink = |lo: usize, hi: usize, size: usize, spread: usize, total: usize| {
            let lo2 = if lo == 0 {
                0
            } else {
                (lo + spread).div_ceil(size) * size
            };
            let hi2 = if hi == total {
                total
            } else {
                hi.saturating_sub(spread) / size * size
            };
            (lo2, hi2)
        };
        (c0, c1) = shrink(c0, c1, l.mcu.0, l.spread.0, px.w);
        (r0, r1) = shrink(r0, r1, l.mcu.1, l.spread.1, px.h);
        if c1 <= c0 || r1 <= r0 {
            return out;
        }
        let corners: Vec<(f64, f64)> = [
            (c0 as f64 / w, 1.0 - r0 as f64 / h),
            (c1 as f64 / w, 1.0 - r0 as f64 / h),
            (c0 as f64 / w, 1.0 - r1 as f64 / h),
            (c1 as f64 / w, 1.0 - r1 as f64 / h),
        ]
        .iter()
        .map(|(u, v)| apply(ctm, *u, *v))
        .collect();
        out.safe = Some(bounds(&corners));
        // With the fill named, each sample is off from it by the tolerance; without it, two
        // samples are each off by the tolerance, in opposite directions.
        tol = tol.max(i32::from(l.tolerance) * if expected.is_some() { 1 } else { 2 });
    }
    let mut reference: Option<&[u8]> = expected.as_deref();
    for row in r0..r1 {
        for col in c0..c1 {
            if !inside(col, row) {
                continue;
            }
            out.covered += 1;
            let s = &px.data[(row * px.w + col) * px.ncomp..][..px.ncomp];
            match reference {
                None => reference = Some(s),
                Some(r) => {
                    if r.iter()
                        .zip(s)
                        .any(|(a, b)| i32::from(a.abs_diff(*b)) > tol)
                    {
                        out.differing += 1;
                    }
                }
            }
        }
    }
    out
}

/// Looks at the image of a placement and its masks. Returns the zone to leave out of the render
/// check when the image is lossy.
fn check_placement(
    doc: &Doc,
    region: &Region,
    ri: usize,
    pl: &Placement,
    rep: &mut Report,
) -> Option<Zone> {
    let [rx0, ry0, rx1, ry1] = region.rect;
    let rect = [rx0.min(rx1), ry0.min(ry1), rx0.max(rx1), ry0.max(ry1)];
    let corners = [
        apply(&pl.ctm, 0.0, 0.0),
        apply(&pl.ctm, 1.0, 0.0),
        apply(&pl.ctm, 0.0, 1.0),
        apply(&pl.ctm, 1.0, 1.0),
    ];
    let bbox = bounds(&corners);
    if bbox[2] <= rect[0] || bbox[0] >= rect[2] || bbox[3] <= rect[1] || bbox[1] >= rect[3] {
        return None;
    }
    let who = match &pl.image {
        Source::Object(n) => format!("image object {n}"),
        Source::Inline(..) => "an inline image".to_owned(),
    };
    let place = format!("page {}, region {}, {who}", region.page, ri + 1);
    let mut image_dict: Option<Obj> = None;
    let decoded = match &pl.image {
        Source::Object(n) => {
            let e = doc.latest(*n)?;
            let Some(d) = doc.stream_data(e) else {
                rep.problems
                    .push(format!("images: {place}: the image has no data"));
                return None;
            };
            image_dict = Some(e.value.clone());
            decode_image(doc, &e.value, &d)
        }
        Source::Inline(dict, data) => {
            let text = [b"<< ".as_slice(), dict, b" >>"].concat();
            match crate::pdfobj::parse(&text, 0) {
                Some((d, _)) if d.get("F").is_none() && d.get("Filter").is_none() => {
                    let d = expand_inline(&d);
                    decode_image(
                        doc,
                        &d,
                        &carve::decode_spec(data, &carve::FilterSpec::default()),
                    )
                }
                _ => Err("an inline image with a filter".to_owned()),
            }
        }
    };
    let px = match decoded {
        Ok(p) => p,
        Err(why) => {
            rep.problems.push(format!(
                "images: {place} lies over a redaction region and could not be checked: {why}"
            ));
            return None;
        }
    };
    let mut sets: Vec<(&str, Pixels)> = vec![("the image", px)];
    if let Some(d) = &image_dict {
        for (key, name) in [("SMask", "its soft mask"), ("Mask", "its mask")] {
            match decode_mask(doc, d, key) {
                None => {}
                Some(Ok(m)) => sets.push((name, m)),
                Some(Err(why)) => {
                    rep.problems.push(format!(
                        "images: {place} lies over a redaction region and could not be checked: {why}"
                    ));
                    return None;
                }
            }
        }
    }
    let own = region.tolerance.unwrap_or(0);
    let mut zone: Option<Zone> = None;
    for (name, p) in &sets {
        // The fill is the colour of the image; a mask holds opacities.
        let fill = if *name == "the image" {
            region.fill
        } else {
            None
        };
        let r = scan(p, &pl.ctm, rect, own, fill);
        if let (Some(l), None) = (&p.lossy, &zone) {
            zone = Some(Zone {
                bbox,
                safe: r.safe,
                tolerance: own.max(l.tolerance.saturating_mul(2)),
            });
        }
        // One sample is one value whether or not it was redacted. Without the fill colour to
        // compare with, a region over fewer than two whole samples cannot be checked.
        if r.covered < 2 && region.fill.is_none() {
            rep.problem(format!(
                "images: {place}: the region holds {} whole sample(s) of {name}, and one value cannot be told from an image that was not redacted; name the fill colour of the region",
                r.covered
            ));
        }
        if r.differing > 0 {
            rep.hits.push(Hit {
                layer: Layer::Images,
                subject: Subject::Rule(format!(
                    "{name} inside a redaction region is not blank ({} of {} samples differ)",
                    r.differing, r.covered
                )),
                offset: None,
                location: place.clone(),
                form: None,
            });
        }
    }
    zone
}

// ---------------------------------------------------------------------------------------------
// The render check.

/// A page drawn by Poppler: RGB, 8 bits, rows without padding.
struct Rendered {
    w: usize,
    h: usize,
    rgb: Vec<u8>,
}

/// The size and samples of a binary PPM (P6) file.
fn parse_ppm(b: &[u8]) -> Option<Rendered> {
    let mut pos = 0usize;
    let mut token = || -> Option<&[u8]> {
        loop {
            while pos < b.len() && b[pos].is_ascii_whitespace() {
                pos += 1;
            }
            if b.get(pos) == Some(&b'#') {
                while pos < b.len() && b[pos] != b'\n' {
                    pos += 1;
                }
            } else {
                break;
            }
        }
        let s = pos;
        while pos < b.len() && !b[pos].is_ascii_whitespace() {
            pos += 1;
        }
        (pos > s).then(|| &b[s..pos])
    };
    if token()? != b"P6" {
        return None;
    }
    let num = |t: &[u8]| std::str::from_utf8(t).ok()?.parse::<usize>().ok();
    let w = num(token()?)?;
    let h = num(token()?)?;
    if num(token()?)? != 255 {
        return None;
    }
    // One white-space byte separates the header from the samples.
    let rgb = b.get(pos + 1..)?;
    let size = w.checked_mul(h)?.checked_mul(3)?;
    (w > 0 && h > 0 && rgb.len() >= size).then(|| Rendered {
        w,
        h,
        rgb: rgb[..size].to_vec(),
    })
}

fn render_page(ctx: &Ctx, page: usize) -> std::result::Result<Rendered, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let prefix = dir.path().join("page");
    let n = page.to_string();
    let dpi = RENDER_DPI.to_string();
    let mut args: Vec<OsString> = ["-r", dpi.as_str(), "-f", n.as_str(), "-l", n.as_str()]
        .iter()
        .map(OsString::from)
        .collect();
    args.extend(ctx.poppler_password());
    args.push(ctx.path.as_os_str().to_owned());
    args.push(prefix.as_os_str().to_owned());
    let refs: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
    let out = tools::pdftoppm(&refs).map_err(|e| e.0)?;
    let first = out
        .stderr
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    if out.status != 0 {
        return Err(format!("pdftoppm failed (exit {}): {first}", out.status));
    }
    // Poppler says what it could not read on stderr and still exits with 0.
    if let Some(l) = out.stderr.lines().find(|l| l.contains("Error")) {
        return Err(format!("pdftoppm reported: {}", l.trim()));
    }
    let file = std::fs::read_dir(dir.path())
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().is_some_and(|x| x == "ppm"))
        .ok_or("pdftoppm wrote no image")?;
    let bytes = std::fs::read(file).map_err(|e| e.to_string())?;
    parse_ppm(&bytes).ok_or_else(|| "the image that pdftoppm wrote cannot be read".to_owned())
}

/// The value of an inheritable page attribute.
fn inherited<'d>(doc: &'d Doc, page: &'d Entry, key: &str) -> &'d Obj {
    let mut cur = &page.value;
    for _ in 0..40 {
        if let Some(v) = cur.get(key) {
            return doc.resolve(v);
        }
        match cur.get("Parent") {
            Some(p) => cur = doc.resolve(p),
            None => break,
        }
    }
    &crate::pdfobj::NULL
}

/// The box of a page in points (y up), and how it is turned.
struct Geometry {
    media: [f64; 4],
    rotate: i64,
}

fn geometry(doc: &Doc, page: &Entry) -> std::result::Result<Geometry, String> {
    let mb = numbers(inherited(doc, page, "MediaBox"), doc);
    if mb.len() != 4 {
        return Err("the page has no MediaBox".to_owned());
    }
    let media = [
        mb[0].min(mb[2]),
        mb[1].min(mb[3]),
        mb[0].max(mb[2]),
        mb[1].max(mb[3]),
    ];
    let rotate = inherited(doc, page, "Rotate")
        .int()
        .unwrap_or(0)
        .rem_euclid(360);
    if rotate % 90 != 0 {
        return Err(format!("the page is turned by {rotate} degrees"));
    }
    Ok(Geometry { media, rotate })
}

impl Geometry {
    /// Where a point of the page (in points, y up) is on a render of `w` x `h` pixels. An error
    /// if the render is not the size of the page.
    fn mapper(
        &self,
        w: usize,
        h: usize,
    ) -> std::result::Result<impl Fn(f64, f64) -> (f64, f64), String> {
        let (pw, ph) = (self.media[2] - self.media[0], self.media[3] - self.media[1]);
        let (rw, rh) = if self.rotate % 180 == 0 {
            (pw, ph)
        } else {
            (ph, pw)
        };
        let s = f64::from(RENDER_DPI) / 72.0;
        if (rw * s - w as f64).abs() > 2.0 || (rh * s - h as f64).abs() > 2.0 {
            return Err(format!(
                "the render is {w} x {h} pixels, and a page of {rw:.1} x {rh:.1} points at {RENDER_DPI} dpi should be about {:.0} x {:.0}",
                rw * s,
                rh * s
            ));
        }
        let (sx, sy) = (w as f64 / rw, h as f64 / rh);
        let (media, rotate) = (self.media, self.rotate);
        Ok(move |x: f64, y: f64| {
            // From the top left of the page, in points, before the page is turned.
            let (u, v) = (x - media[0], media[3] - y);
            let (u, v) = match rotate {
                90 => (ph - v, u),
                180 => (pw - u, ph - v),
                270 => (v, pw - u),
                _ => (u, v),
            };
            (u * sx, v * sy)
        })
    }
}

type Cache = HashMap<usize, std::result::Result<Rendered, String>>;

/// Checks the rendered pixels inside one region.
#[allow(clippy::too_many_arguments)]
fn check_render(
    ctx: &Ctx,
    doc: &Doc,
    page_no: usize,
    page: &Entry,
    region: &Region,
    ri: usize,
    zones: &[Zone],
    cache: &mut Cache,
    rep: &mut Report,
) {
    let place = format!("page {page_no}, region {}", ri + 1);
    let geo = match geometry(doc, page) {
        Ok(g) => g,
        Err(why) => {
            rep.problems.push(format!(
                "images: {place}: the render check could not place the region: {why}"
            ));
            return;
        }
    };
    let rendered = cache.entry(page_no).or_insert_with(|| {
        let (pw, ph) = (geo.media[2] - geo.media[0], geo.media[3] - geo.media[1]);
        let s = f64::from(RENDER_DPI) / 72.0;
        if pw * s * ph * s > MAX_RENDER_PIXELS as f64 {
            return Err("the page is too large to render for the region check".to_owned());
        }
        render_page(ctx, page_no)
    });
    let img = match rendered {
        Ok(i) => i,
        Err(why) => {
            rep.problem(format!(
                "images: {place}: the render check could not be done: {why}"
            ));
            return;
        }
    };
    let map = match geo.mapper(img.w, img.h) {
        Ok(m) => m,
        Err(why) => {
            rep.problems.push(format!(
                "images: {place}: the render check could not place the region: {why}"
            ));
            return;
        }
    };
    let [x0, y0, x1, y1] = region.rect;
    let rect = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];
    let dev = |r: [f64; 4]| {
        bounds(&[
            map(r[0], r[1]),
            map(r[2], r[1]),
            map(r[0], r[3]),
            map(r[2], r[3]),
        ])
    };
    let [dx0, dy0, dx1, dy1] = dev(rect);
    let margin = RENDER_MARGIN as i64;
    let lo_x = (dx0.ceil() as i64 + margin).max(0);
    let hi_x = (dx1.floor() as i64 - margin).min(img.w as i64);
    let lo_y = (dy0.ceil() as i64 + margin).max(0);
    let hi_y = (dy1.floor() as i64 - margin).min(img.h as i64);
    if hi_x <= lo_x || hi_y <= lo_y {
        rep.problems.push(format!(
            "images: {place}: the region is smaller than {} device pixels at {RENDER_DPI} dpi (or outside the page), so no pixel is left to check after the edge of {RENDER_MARGIN} pixels is left out",
            2 * RENDER_MARGIN + 1
        ));
        return;
    }
    let (lo_x, hi_x, lo_y, hi_y) = (lo_x as usize, hi_x as usize, lo_y as usize, hi_y as usize);
    // Pixels of a lossy image that its codec changes near an edge are left out.
    type Skip = ([f64; 4], Option<[f64; 4]>);
    let skip: Vec<Skip> = zones
        .iter()
        .map(|z| (dev(z.bbox), z.safe.map(dev)))
        .collect();
    let inside = |b: &[f64; 4], x: f64, y: f64| x >= b[0] && x <= b[2] && y >= b[1] && y <= b[3];
    let tolerance = region.tolerance.map(i32::from).unwrap_or_else(|| {
        zones
            .iter()
            .map(|z| i32::from(z.tolerance))
            .max()
            .unwrap_or(0)
    });
    let mut reference: Option<[u8; 3]> = region.fill;
    let (mut checked, mut differing) = (0usize, 0usize);
    let mut first_bad: Option<(usize, usize)> = None;
    for y in lo_y..hi_y {
        for x in lo_x..hi_x {
            let (cx, cy) = (x as f64 + 0.5, y as f64 + 0.5);
            let left_out = skip.iter().any(|(bb, safe)| {
                inside(bb, cx, cy) && !safe.as_ref().is_some_and(|s| inside(s, cx, cy))
            });
            if left_out {
                continue;
            }
            let px = &img.rgb[(y * img.w + x) * 3..][..3];
            checked += 1;
            match reference {
                None => reference = Some([px[0], px[1], px[2]]),
                Some(r) => {
                    if r.iter()
                        .zip(px)
                        .any(|(a, b)| i32::from(a.abs_diff(*b)) > tolerance)
                    {
                        differing += 1;
                        first_bad.get_or_insert((x, y));
                    }
                }
            }
        }
    }
    if checked == 0 {
        rep.problems.push(format!(
            "images: {place}: the region lies over a JPEG image whose blocks all straddle its edge, so no pixel of it can be told from the rounding of the codec"
        ));
        return;
    }
    if differing > 0 {
        let what = if region.fill.is_some() {
            "the fill colour"
        } else {
            "of one colour"
        };
        let (bx, by) = first_bad.unwrap_or((0, 0));
        rep.hits.push(Hit {
            layer: Layer::Images,
            subject: Subject::Rule(format!(
                "rendered pixels inside a redaction region are not {what} ({differing} of {checked} differ)"
            )),
            offset: None,
            location: format!(
                "{place}, drawn by pdftoppm at {RENDER_DPI} dpi, first at pixel ({bx}, {by})"
            ),
            form: None,
        });
    }
}

/// Checks the regions against the pages of the normalised copy of a file. `bytes` is that copy.
pub fn check(ctx: &Ctx, bytes: &[u8], rep: &mut Report) -> Result<()> {
    let doc = Doc::scan(bytes);
    let pages = doc.pages();
    if pages.is_empty() {
        rep.problems
            .push("images: no pages were found, so the regions were not checked".into());
        return Ok(());
    }
    let mut cache: Cache = HashMap::new();
    for (ri, region) in ctx.spec.regions.iter().enumerate() {
        let Some((page, res)) = region.page.checked_sub(1).and_then(|i| pages.get(i)) else {
            rep.problems.push(format!(
                "images: region {} names page {}, the file has {} pages",
                ri + 1,
                region.page,
                pages.len()
            ));
            continue;
        };
        let mut w = Walker {
            doc: &doc,
            out: Vec::new(),
            problems: Vec::new(),
        };
        match contents(&doc, page) {
            Some(data) => w.content(&data, &doc.resolve(res).clone(), IDENTITY, 0),
            None => w.problems.push("the page has no content".into()),
        }
        annotations(&mut w, page);
        for p in w.problems.drain(..) {
            rep.problems
                .push(format!("images: page {}: {p}", region.page));
        }
        let zones: Vec<Zone> = w
            .out
            .iter()
            .filter_map(|pl| check_placement(&doc, region, ri, pl, rep))
            .collect();
        check_render(
            ctx,
            &doc,
            region.page,
            page,
            region,
            ri,
            &zones,
            &mut cache,
            rep,
        );
    }
    Ok(())
}

fn annotations(w: &mut Walker, page: &Entry) {
    let doc = w.doc;
    let Obj::Array(annots) = doc.dget(&page.value, "Annots") else {
        return;
    };
    for a in annots {
        let ad = doc.resolve(a);
        let rect = ad.get("Rect").map(|r| numbers(r, doc)).unwrap_or_default();
        if rect.len() != 4 {
            continue;
        }
        let Some(ap) = ad.get("AP").map(|x| doc.resolve(x)) else {
            continue;
        };
        let mut forms: Vec<u32> = Vec::new();
        match ap.get("N") {
            Some(Obj::Ref(n)) => match doc.latest(*n) {
                Some(e) if e.stream.is_some() => forms.push(*n),
                Some(e) => forms.extend(e.value.entries().iter().filter_map(|(_, v)| match v {
                    Obj::Ref(k) => Some(*k),
                    _ => None,
                })),
                None => {}
            },
            Some(Obj::Dict(d)) => forms.extend(d.iter().filter_map(|(_, v)| match v {
                Obj::Ref(k) => Some(*k),
                _ => None,
            })),
            _ => {}
        }
        for n in forms {
            let Some(e) = doc.latest(n) else { continue };
            let bbox = e
                .value
                .get("BBox")
                .map(|b| numbers(b, doc))
                .unwrap_or_default();
            if bbox.len() != 4 {
                continue;
            }
            let m = e
                .value
                .get("Matrix")
                .map_or(IDENTITY, |m| matrix_of(m, doc));
            let pts = [
                apply(&m, bbox[0], bbox[1]),
                apply(&m, bbox[2], bbox[1]),
                apply(&m, bbox[0], bbox[3]),
                apply(&m, bbox[2], bbox[3]),
            ];
            let [tx0, ty0, tx1, ty1] = bounds(&pts);
            let (rx0, rx1) = (rect[0].min(rect[2]), rect[0].max(rect[2]));
            let (ry0, ry1) = (rect[1].min(rect[3]), rect[1].max(rect[3]));
            let sx = if tx1 - tx0 > 1e-9 {
                (rx1 - rx0) / (tx1 - tx0)
            } else {
                1.0
            };
            let sy = if ty1 - ty0 > 1e-9 {
                (ry1 - ry0) / (ty1 - ty0)
            } else {
                1.0
            };
            let a_m: M = [sx, 0.0, 0.0, sy, rx0 - tx0 * sx, ry0 - ty0 * sy];
            let res = e
                .value
                .get("Resources")
                .map(|r| doc.resolve(r).clone())
                .unwrap_or(Obj::Null);
            w.xobject_form(n, &res, a_m);
        }
    }
}

impl Walker<'_, '_> {
    /// A form that is drawn with the matrix `ctm` (an appearance stream).
    fn xobject_form(&mut self, n: u32, res: &Obj, ctm: M) {
        self.xobject(n, res, ctm, 0);
    }
}

/// The abbreviated keys of an inline image dictionary, as the full ones.
fn expand_inline(d: &Obj) -> Obj {
    fn full(k: &str) -> &str {
        match k {
            "W" => "Width",
            "H" => "Height",
            "BPC" => "BitsPerComponent",
            "CS" => "ColorSpace",
            "IM" => "ImageMask",
            other => other,
        }
    }
    let cs = |v: &Obj| match v {
        Obj::Name(n) => Obj::Name(
            match n.as_str() {
                "G" => "DeviceGray",
                "RGB" => "DeviceRGB",
                "CMYK" => "DeviceCMYK",
                other => other,
            }
            .to_owned(),
        ),
        other => other.clone(),
    };
    Obj::Dict(
        d.entries()
            .iter()
            .map(|(k, v)| {
                let key = full(k).to_owned();
                let val = if key == "ColorSpace" {
                    cs(v)
                } else {
                    v.clone()
                };
                (key, val)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matrix_and_its_inverse_cancel() {
        let m: M = [2.0, 0.5, -1.0, 3.0, 10.0, 20.0];
        let i = invert(&m).unwrap();
        let id = mul(&m, &i);
        for (a, b) in id.iter().zip(IDENTITY) {
            assert!((a - b).abs() < 1e-9, "{id:?}");
        }
        assert!(invert(&[1.0, 2.0, 2.0, 4.0, 0.0, 0.0]).is_none());
    }

    #[test]
    fn samples_of_few_bits_are_scaled_to_eight() {
        // Four 2-bit samples in one byte, then 1-bit samples.
        assert_eq!(
            unpack(&[0b00_01_10_11], 4, 1, 1, 2).unwrap(),
            [0, 85, 170, 255]
        );
        assert_eq!(
            unpack(&[0b1010_0000], 4, 1, 1, 1).unwrap(),
            [255, 0, 255, 0]
        );
        assert_eq!(unpack(&[1, 2, 3, 4], 2, 1, 1, 16).unwrap(), [1, 3]);
        assert!(unpack(&[1], 4, 4, 1, 8).is_err());
    }

    /// The marker segments of a JPEG with one quantisation table (first value `q0`) and three
    /// components, the first with the sampling factor `hv`.
    fn jpeg_head(q0: u8, hv: u8) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xDB, 0x00, 0x43, 0x00, q0];
        v.extend([1u8; 63]);
        v.extend([0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x10, 0x00, 0x10, 0x03]);
        v.extend([1, hv, 0, 2, 0x11, 0, 3, 0x11, 0]);
        v.extend([0xFF, 0xDA]);
        v
    }

    #[test]
    fn the_quantisation_tables_give_the_tolerance_and_the_blocks() {
        let l = jpeg_lossy(&jpeg_head(8, 0x22)).unwrap();
        assert_eq!((l.mcu, l.spread), ((16, 16), (2, 2)));
        // 8 / 16 * 2.8 + 2, rounded up.
        assert_eq!(l.tolerance, 4);
        let l = jpeg_lossy(&jpeg_head(3, 0x11)).unwrap();
        assert_eq!((l.mcu, l.spread), ((8, 8), (0, 0)));
        assert_eq!(l.tolerance, 3);
        // A coarser table allows more.
        assert!(jpeg_lossy(&jpeg_head(80, 0x22)).unwrap().tolerance > 10);
        assert!(jpeg_lossy(b"not a jpeg").is_none());
    }

    #[test]
    fn a_ppm_is_read_and_a_broken_one_is_not() {
        let mut f = b"P6\n# c\n2 1\n255\n".to_vec();
        f.extend([1, 2, 3, 4, 5, 6]);
        let r = parse_ppm(&f).unwrap();
        assert_eq!((r.w, r.h, r.rgb.len()), (2, 1, 6));
        assert!(parse_ppm(b"P6\n2 1\n255\n\x01").is_none());
        assert!(parse_ppm(b"P5\n1 1\n255\n\x01").is_none());
    }

    #[test]
    fn a_turned_page_maps_the_corner_of_the_page_to_the_corner_of_the_render() {
        // 100 x 200 points; the render at 144 dpi is 200 x 400, or 400 x 200 when turned.
        let g = |rotate| Geometry {
            media: [0.0, 0.0, 100.0, 200.0],
            rotate,
        };
        let m = g(0).mapper(200, 400).unwrap();
        assert_eq!(m(0.0, 200.0), (0.0, 0.0));
        assert_eq!(m(100.0, 0.0), (200.0, 400.0));
        // Turned 90 degrees clockwise: the top left of the page goes to the top right.
        let m = g(90).mapper(400, 200).unwrap();
        assert_eq!(m(0.0, 200.0), (400.0, 0.0));
        assert_eq!(m(0.0, 0.0), (0.0, 0.0));
        let m = g(180).mapper(200, 400).unwrap();
        assert_eq!(m(0.0, 200.0), (200.0, 400.0));
        let m = g(270).mapper(400, 200).unwrap();
        assert_eq!(m(0.0, 200.0), (0.0, 200.0));
        // A render of another size is refused.
        assert!(g(0).mapper(300, 400).is_err());
    }
}
