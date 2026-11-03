//! The region check: given the rectangles that were redacted, find out whether anything can still
//! be seen inside them, or be seen again. There are three checks, and a region must pass all.
//!
//! 1. The render. The page is drawn by Poppler (`pdftoppm`, a renderer that is not the engine under
//!    test) at [`RENDER_DPI`]. Every device pixel whose whole square lies inside the region, less
//!    a margin of [`RENDER_MARGIN`] pixels at the edge (anti-aliasing), must have the fill colour
//!    of the region, or, if the spec names none, the middle colour of the region. This sees what
//!    a reader sees: masks, patterns, vector shapes and text that was turned into outlines.
//! 2. The images. The images drawn over the region (page content, forms, every state of every
//!    annotation appearance, the icons of widgets, soft mask groups), also when they are turned,
//!    are decoded, with their soft mask and mask, and the samples whose whole square lies inside
//!    the region must all be one value (within a tolerance for a lossy codec). This does not
//!    depend on the codec: a sample is what the decoder gives, however the engine stored the
//!    image again.
//! 3. The geometry. One interpreter ([`crate::walk`]) walks the content of the page, with a
//!    complete graphics state, and every painting event (a glyph of any font, a path that is
//!    filled or stroked, a clip path, a shading) is placed on the page. Any that meets the region
//!    is a leak whatever its colour, clip, transparency, blend mode, render mode or optional
//!    content, except geometry that carries nothing about what was under the region (the box that
//!    redacts, a fill of one colour that contains the region, a rule that crosses it). What a
//!    viewer cannot show now can be shown later. The glyphs of ordinary fonts are leaks when the
//!    spec has no text to look for: the text layers cover them otherwise.
//!
//! Where the check cannot be done (a codec that is not decoded, a mask that cannot be read, a
//! region too small to leave a pixel after the margin, a render that fails, content whose bounds
//! are not known, a walk that needs more steps or time than its budget) it is a problem.
//!
//! A JPEG changes the pixels near the edge of what was blanked. A block of samples that the edge of
//! the region cuts (8 x 8 samples, 16 x 16 with subsampled colour) has the ringing of the codec all
//! over it, and the colour of its samples spreads into the samples next to it. Those samples are
//! left out of both the image check and the render check; a sample is compared only when every
//! block that its colour comes from lies wholly inside the region. The rest may differ from one
//! value by what the quantisation of a flat block allows, from the tables that the components of the
//! image use (see [`JPEG_TOLERANCE_CAP`]). The blocks are in the grid of the image, so a turned
//! image is looked at the same way as one that is not.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};

use crate::carve;
use crate::content::apply;
use crate::layers::Ctx;
use crate::pdfobj::{Doc, Entry, Obj};
use crate::tools;
use crate::walk::{
    bounds, matrix_of, numbers, overlaps, Budget, Found, Placement, Source, Target, Walker, M,
};
use crate::{Hit, Layer, NeedleKind, Region, Report, Result, Subject, RENDER_DPI, RENDER_MARGIN};

/// The most device pixels one render may have.
const MAX_RENDER_PIXELS: usize = 80_000_000;
/// The most samples of one image that the check keeps track of.
const MAX_IMAGE_SAMPLES: usize = 40_000_000;

fn invert(m: &M) -> Option<M> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 {
        return None;
    }
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    Some([a, b, c, d, -(m[4] * a + m[5] * c), -(m[4] * b + m[5] * d)])
}

/// The decoded content of a page (its streams, one after the other), or why it cannot be read.
fn contents(doc: &Doc, page: &Entry) -> std::result::Result<Vec<u8>, String> {
    let Some(c) = page.value.get("Contents") else {
        return Err("the page has no content".to_owned());
    };
    let mut data = Vec::new();
    let mut why: Option<String> = None;
    let mut add = |o: &Obj| {
        if let Obj::Ref(n) = o {
            if let Some(d) = doc.latest(*n).and_then(|e| doc.stream_data(e)) {
                if let Some(p) = &d.problem {
                    why.get_or_insert(format!("content stream {n}: {p}"));
                }
                if let Some(s) = &d.stopped_at {
                    why.get_or_insert(format!(
                        "content stream {n} has the filter {s}, which is not decoded"
                    ));
                }
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
    match why {
        Some(w) => Err(w),
        None => Ok(data),
    }
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

/// The most that the check lets a JPEG be off in a flat block, in levels of 255. A flat block is
/// off by at most half a quantisation step of its DC coefficient divided by 8, widened for the
/// colour conversion and the rounding (see [`jpeg_lossy`]); 16 stands for a DC step of 80, about
/// quality 10 of libjpeg, below which a scan is too coarse for any check to separate what was
/// there from what the codec made of it. The light gray of a faint stamp is 40 levels from white,
/// so 16 still tells it from a blank block. A JPEG that needs more is "could not check".
const JPEG_TOLERANCE_CAP: u8 = 16;

/// The tolerance and the blocks of a JPEG, from the first value of the quantisation tables that
/// its components use and the largest sampling factors in the markers of the file. A table that no
/// component uses says nothing about the image and is not read. The error is from the file under
/// test, so it is capped at [`JPEG_TOLERANCE_CAP`]: a JPEG that would need more is refused, with
/// the reason.
fn jpeg_lossy(data: &[u8]) -> std::result::Result<Lossy, String> {
    if data.get(..2) != Some(&[0xFF, 0xD8]) {
        return Err("it does not start like a JPEG".to_owned());
    }
    let unreadable = || {
        "its marker segments cannot be read, so its quantisation tables are not known".to_owned()
    };
    let mut i = 2;
    // The first (DC) entry of each table, by table number; a table that is defined twice counts
    // with the larger one.
    let mut steps: [Option<u16>; 4] = [None; 4];
    let (mut hmax, mut vmax) = (1usize, 1usize);
    // The tables that the components select.
    let mut used: Vec<usize> = Vec::new();
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
        // Markers without a segment, and the bytes of the entropy coded data that look like one.
        if m == 0x00 || m == 0x01 || (0xD0..=0xD7).contains(&m) {
            i += 2;
            continue;
        }
        if m == 0xD9 {
            break;
        }
        let len = usize::from(data[i + 2]) << 8 | usize::from(data[i + 3]);
        if len < 2 {
            return Err(unreadable());
        }
        let seg = data.get(i + 4..i + 2 + len).ok_or_else(unreadable)?;
        match m {
            0xDB => {
                let mut p = 0;
                while p < seg.len() {
                    let wide = seg[p] >> 4 != 0;
                    let id = usize::from(seg[p] & 15);
                    p += 1;
                    let size = if wide { 2 } else { 1 };
                    if p + 64 * size > seg.len() || id > 3 {
                        return Err(unreadable());
                    }
                    let first = if wide {
                        u16::from(seg[p]) << 8 | u16::from(seg[p + 1])
                    } else {
                        u16::from(seg[p])
                    };
                    steps[id] = Some(steps[id].map_or(first, |s| s.max(first)));
                    p += 64 * size;
                }
            }
            0xC0..=0xC2 => {
                let n = usize::from(*seg.get(5).ok_or_else(unreadable)?);
                for c in 0..n {
                    let hv = *seg.get(6 + c * 3 + 1).ok_or_else(unreadable)?;
                    let table = usize::from(*seg.get(6 + c * 3 + 2).ok_or_else(unreadable)?);
                    hmax = hmax.max(usize::from(hv >> 4));
                    vmax = vmax.max(usize::from(hv & 15));
                    if !used.contains(&table) {
                        used.push(table);
                    }
                }
            }
            _ => {}
        }
        i += 2 + len;
    }
    if used.is_empty() {
        return Err("it has no frame header, so its quantisation tables are not known".to_owned());
    }
    let mut step = 0u16;
    for t in used {
        match steps.get(t).copied().flatten() {
            Some(s) => step = step.max(s),
            None => {
                return Err(format!(
                    "a component uses the quantisation table {t}, which it does not define"
                ))
            }
        }
    }
    // A flat block has one coefficient, the DC value; quantising it is off by half a step, and the
    // pixel value is one eighth of the coefficient. The colour conversion adds the error of the
    // colour components to the one of the luma (a factor of 1.8 at most), and 2 more levels are
    // rounding.
    let tolerance = (f64::from(step) / 16.0 * 2.8 + 2.0).ceil();
    if tolerance > f64::from(JPEG_TOLERANCE_CAP) {
        return Err(format!(
            "its quantisation tables are so coarse that a flat block may be off by {tolerance} levels, more than the {JPEG_TOLERANCE_CAP} that can be told from content"
        ));
    }
    let (hmax, vmax) = (hmax.clamp(1, 4), vmax.clamp(1, 4));
    Ok(Lossy {
        tolerance: tolerance as u8,
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
                // A JPEG whose tables cannot be read, or whose tables are too coarse, cannot be
                // judged: it is refused, with the reason.
                let lossy = jpeg_lossy(&decoded.data)?;
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

/// The samples of a lossy image that were compared: the blocks that lie wholly inside the region.
/// The render check leaves out the rest of the image, where the codec has changed the pixels near
/// the edge of what was blanked.
struct Checked {
    /// Nothing of the image was compared (it could not be decoded): the render check leaves out
    /// all of it, and the problem that was reported says so.
    none: bool,
    /// The window of the image that the region can touch.
    col0: usize,
    row0: usize,
    cols: usize,
    rows: usize,
    bits: Vec<bool>,
}

impl Checked {
    /// Whether the sample was compared. A sample outside the window is under no pixel of the
    /// region, so it is not left out.
    fn has(&self, col: usize, row: usize) -> bool {
        if self.none {
            return false;
        }
        if col < self.col0 || row < self.row0 {
            return true;
        }
        let (c, r) = (col - self.col0, row - self.row0);
        c >= self.cols || r >= self.rows || self.bits[r * self.cols + c]
    }
}

/// A lossy image under a region, for the render check: where it is, which of its samples were
/// compared, and how far a rendered pixel of it may be from the others.
struct Zone {
    ctm: M,
    w: usize,
    h: usize,
    checked: Checked,
    tolerance: u8,
}

/// The result of looking at one set of samples under a region.
struct Scan {
    covered: usize,
    differing: usize,
    /// The region is over more samples than the check keeps track of.
    too_large: bool,
    /// For a lossy image: the part that was looked at.
    checked: Option<Checked>,
}

/// The middle value of each component of the samples.
fn median(samples: &[&[u8]], ncomp: usize) -> Vec<u8> {
    (0..ncomp)
        .map(|c| {
            let mut v: Vec<u8> = samples.iter().map(|s| s[c]).collect();
            let mid = v.len() / 2;
            *v.select_nth_unstable(mid).1
        })
        .collect()
}

/// The samples of `px` (drawn with `ctm`, which may turn it) whose whole square lies inside `rect`,
/// and how many of them differ from the others by more than `tolerance`: from the middle value of
/// all of them, or, for a JPEG whose region names its fill, from the fill. A JPEG is looked at
/// only where a whole block, with the samples that take colour from it, lies inside the region.
fn scan(px: &Pixels, ctm: &M, rect: [f64; 4], tolerance: u8, fill: Option<[u8; 3]>) -> Scan {
    let [rx0, ry0, rx1, ry1] = rect;
    let mut out = Scan {
        covered: 0,
        differing: 0,
        too_large: false,
        checked: None,
    };
    let Some(inv) = invert(ctm) else { return out };
    let uv: Vec<(f64, f64)> = [(rx0, ry0), (rx1, ry0), (rx0, ry1), (rx1, ry1)]
        .iter()
        .map(|(x, y)| apply(&inv, *x, *y))
        .collect();
    let [u0, v0, u1, v1] = bounds(&uv);
    let (w, h) = (px.w as f64, px.h as f64);
    let col0 = ((u0 * w).floor().max(0.0)) as usize;
    let col1 = (((u1 * w).ceil().min(w)) as usize).min(px.w);
    let row0 = (((1.0 - v1) * h).floor().max(0.0)) as usize;
    let row1 = (((1.0 - v0) * h).ceil().min(h) as usize).min(px.h);
    if col1 <= col0 || row1 <= row0 {
        return out;
    }
    let (cols, rows) = (col1 - col0, row1 - row0);
    if cols.saturating_mul(rows) > MAX_IMAGE_SAMPLES {
        out.too_large = true;
        return out;
    }
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
    let mut ins = vec![false; cols * rows];
    for r in 0..rows {
        for c in 0..cols {
            ins[r * cols + c] = inside(col0 + c, row0 + r);
        }
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
    let sel: Vec<bool> = match &px.lossy {
        None => ins,
        Some(l) => {
            // The blocks that lie wholly inside the region, with the samples next to them that
            // take colour from them (at the edge of the image there is no neighbour). The blocks
            // are in the grid of the image, so a turned image is looked at the same way.
            let stride = cols + 1;
            let mut sum = vec![0u32; stride * (rows + 1)];
            for r in 0..rows {
                for c in 0..cols {
                    sum[(r + 1) * stride + c + 1] = u32::from(ins[r * cols + c])
                        + sum[r * stride + c + 1]
                        + sum[(r + 1) * stride + c]
                        - sum[r * stride + c];
                }
            }
            let count = |x0: usize, y0: usize, x1: usize, y1: usize| -> usize {
                // The samples of the window in [x0, x1) x [y0, y1), image coordinates.
                let (a, b, c, d) = (x0 - col0, y0 - row0, x1 - col0, y1 - row0);
                (sum[d * stride + c] + sum[b * stride + a]
                    - sum[b * stride + c]
                    - sum[d * stride + a]) as usize
            };
            let (mw, mh) = l.mcu;
            let (sx, sy) = l.spread;
            // A block is clean when all of it lies inside the region. A block that the edge of
            // the region cuts has the ringing of the codec all over it, and the colour of its
            // samples spreads into the samples next to it.
            let (blocks_x, blocks_y) = (px.w.div_ceil(mw), px.h.div_ceil(mh));
            let (first_bx, first_by) = (col0 / mw, row0 / mh);
            let (nbx, nby) = (
                (col1 - 1) / mw + 1 - first_bx,
                (row1 - 1) / mh + 1 - first_by,
            );
            let mut clean = vec![false; nbx * nby];
            for j in 0..nby {
                for i in 0..nbx {
                    let (bx, by) = (first_bx + i, first_by + j);
                    let (x0, x1) = (bx * mw, ((bx + 1) * mw).min(px.w));
                    let (y0, y1) = (by * mh, ((by + 1) * mh).min(px.h));
                    clean[j * nbx + i] = x0 >= col0
                        && y0 >= row0
                        && x1 <= col1
                        && y1 <= row1
                        && count(x0, y0, x1, y1) == (x1 - x0) * (y1 - y0);
                }
            }
            let is_clean = |bx: usize, by: usize| -> bool {
                if bx >= blocks_x || by >= blocks_y {
                    return true;
                }
                bx >= first_bx
                    && by >= first_by
                    && bx - first_bx < nbx
                    && by - first_by < nby
                    && clean[(by - first_by) * nbx + bx - first_bx]
            };
            let mut sel = vec![false; cols * rows];
            for r in 0..rows {
                for c in 0..cols {
                    let (x, y) = (col0 + c, row0 + r);
                    let (bx, by) = (x / mw, y / mh);
                    if !is_clean(bx, by) {
                        continue;
                    }
                    // Every block that the colour of this sample comes from must be clean.
                    let (bx0, bx1) = (x.saturating_sub(sx) / mw, (x + sx).min(px.w - 1) / mw);
                    let (by0, by1) = (y.saturating_sub(sy) / mh, (y + sy).min(px.h - 1) / mh);
                    sel[r * cols + c] = (by0..=by1).all(|j| (bx0..=bx1).all(|i| is_clean(i, j)));
                }
            }
            tol = tol.max(i32::from(l.tolerance));
            sel
        }
    };
    let mut samples: Vec<&[u8]> = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            if sel[r * cols + c] {
                samples.push(&px.data[((row0 + r) * px.w + col0 + c) * px.ncomp..][..px.ncomp]);
            }
        }
    }
    out.covered = samples.len();
    if px.lossy.is_some() {
        out.checked = Some(Checked {
            none: false,
            col0,
            row0,
            cols,
            rows,
            bits: sel,
        });
    }
    if samples.is_empty() {
        return out;
    }
    let reference = expected.unwrap_or_else(|| median(&samples, px.ncomp));
    out.differing = samples
        .iter()
        .filter(|s| {
            reference
                .iter()
                .zip(**s)
                .any(|(a, b)| i32::from(a.abs_diff(*b)) > tol)
        })
        .count();
    out
}

/// The size of an image that is stored as a JPEG.
fn jpeg_size(doc: &Doc, image: &Source, dict: Option<&Obj>) -> Option<(usize, usize)> {
    let Source::Object(_) = image else {
        return None;
    };
    let dict = dict?;
    let filter = match doc.dget(dict, "Filter") {
        Obj::Name(n) => Some(n.as_str()),
        Obj::Array(a) => a.last().and_then(|x| doc.resolve(x).name()),
        _ => None,
    };
    if !matches!(filter, Some("DCTDecode" | "DCT")) {
        return None;
    }
    let w = usize::try_from(doc.dget(dict, "Width").int()?).ok()?;
    let h = usize::try_from(doc.dget(dict, "Height").int()?).ok()?;
    (w > 0 && h > 0).then_some((w, h))
}

/// Looks at the image of a placement and its masks. Returns the lossy images, for the render check
/// to leave out the pixels that their codec changed near the edge.
fn check_placement(
    doc: &Doc,
    region: &Region,
    ri: usize,
    pl: &Placement,
    rep: &mut Report,
) -> Vec<Zone> {
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
        return Vec::new();
    }
    let who = match &pl.image {
        Source::Object(n) => format!("image object {n}"),
        Source::Inline(..) => "an inline image".to_owned(),
    };
    let place = format!("page {}, region {}, {who}", region.page, ri + 1);
    let mut image_dict: Option<Obj> = None;
    let decoded = match &pl.image {
        Source::Object(n) => {
            let Some(e) = doc.latest(*n) else {
                return Vec::new();
            };
            let Some(d) = doc.stream_data(e) else {
                rep.problems
                    .push(format!("images: {place}: the image has no data"));
                return Vec::new();
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
            // A JPEG that cannot be judged is reported once, as "could not check": the render
            // check leaves out its pixels, which the codec has changed by an amount that is not
            // known.
            return match jpeg_size(doc, &pl.image, image_dict.as_ref()) {
                Some((w, h)) => vec![Zone {
                    ctm: pl.ctm,
                    w,
                    h,
                    checked: Checked {
                        none: true,
                        col0: 0,
                        row0: 0,
                        cols: 0,
                        rows: 0,
                        bits: Vec::new(),
                    },
                    tolerance: 0,
                }],
                None => Vec::new(),
            };
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
                    return Vec::new();
                }
            }
        }
    }
    let own = region.tolerance.unwrap_or(0);
    let mut zones: Vec<Zone> = Vec::new();
    for (name, p) in sets {
        // The fill is the colour of the image; a mask holds opacities.
        let fill = if name == "the image" {
            region.fill
        } else {
            None
        };
        let r = scan(&p, &pl.ctm, rect, own, fill);
        if r.too_large {
            rep.problems.push(format!(
                "images: {place}: the region is over more than {MAX_IMAGE_SAMPLES} samples of {name}, which is more than the check keeps track of"
            ));
            continue;
        }
        if let Some(l) = &p.lossy {
            // A JPEG is compared only where a whole block lies inside the region, and what
            // lies outside those blocks is left out of the render check: with no block inside,
            // nothing of it is checked, whether or not the region names its fill.
            if r.covered == 0 {
                rep.problems.push(format!(
                    "images: {place}: {name} is a JPEG that lies over the region, and no whole block of it ({} x {} samples, with the neighbours that take colour from it) lies inside the region, so it cannot be told from the rounding of the codec; make the region cover whole blocks, or store the image without loss",
                    l.mcu.0, l.mcu.1
                ));
            }
        } else if r.covered < 2 && region.fill.is_none() {
            // One sample is one value whether or not it was redacted.
            rep.problem(format!(
                "images: {place}: the region holds {} whole sample(s) of {name}, and one value cannot be told from an image that was not redacted; name the fill colour of the region, so that the render is compared with it",
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
        if let (Some(l), Some(checked)) = (&p.lossy, r.checked) {
            zones.push(Zone {
                ctm: pl.ctm,
                w: p.w,
                h: p.h,
                checked,
                tolerance: l.tolerance,
            });
        }
    }
    zones
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
    // Where each lossy image is in the render: device pixel -> position in the unit square of the
    // image.
    let to_unit: Vec<Option<M>> = zones
        .iter()
        .map(|z| {
            let at = |u: f64, v: f64| {
                let (x, y) = apply(&z.ctm, u, v);
                map(x, y)
            };
            let (o, ex, ey) = (at(0.0, 0.0), at(1.0, 0.0), at(0.0, 1.0));
            invert(&[ex.0 - o.0, ex.1 - o.1, ey.0 - o.0, ey.1 - o.1, o.0, o.1])
        })
        .collect();
    let own = region.tolerance.map_or(0, i32::from);
    // The pixels to compare, and how far each may be from the others: a pixel of a JPEG may be off
    // by what its codec makes of a flat block, and a pixel of anything else by the tolerance of the
    // region. The pixels of a JPEG outside the blocks that were compared (the codec changes those
    // near the edge of what was blanked) are left out.
    let mut kept: Vec<(usize, usize, i32)> = Vec::new();
    for y in lo_y..hi_y {
        for x in lo_x..hi_x {
            let (cx, cy) = (x as f64 + 0.5, y as f64 + 0.5);
            let mut tol = own;
            let mut left_out = false;
            for (z, inv) in zones.iter().zip(&to_unit) {
                let Some(inv) = inv else { continue };
                let (u, v) = apply(inv, cx, cy);
                if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                    continue;
                }
                let col = ((u * z.w as f64) as usize).min(z.w - 1);
                let row = (((1.0 - v) * z.h as f64) as usize).min(z.h - 1);
                if z.checked.has(col, row) {
                    tol = tol.max(i32::from(z.tolerance));
                } else {
                    left_out = true;
                }
            }
            if !left_out {
                kept.push((x, y, tol));
            }
        }
    }
    if kept.is_empty() {
        rep.problems.push(format!(
            "images: {place}: the region lies over a JPEG image whose blocks all straddle its edge, so no pixel of it can be told from the rounding of the codec"
        ));
        return;
    }
    let reference: [u8; 3] = region.fill.unwrap_or_else(|| {
        let mut channels: [Vec<u8>; 3] = Default::default();
        for &(x, y, _) in &kept {
            let p = &img.rgb[(y * img.w + x) * 3..][..3];
            for (c, v) in channels.iter_mut().zip(p) {
                c.push(*v);
            }
        }
        let mid = |v: &mut Vec<u8>| {
            let m = v.len() / 2;
            *v.select_nth_unstable(m).1
        };
        let [r, g, b] = &mut channels;
        [mid(r), mid(g), mid(b)]
    });
    let (mut differing, mut first_bad) = (0usize, None);
    for &(x, y, tol) in &kept {
        let px = &img.rgb[(y * img.w + x) * 3..][..3];
        if reference
            .iter()
            .zip(px)
            .any(|(a, b)| i32::from(a.abs_diff(*b)) > tol)
        {
            differing += 1;
            first_bad.get_or_insert((x, y));
        }
    }
    let checked = kept.len();
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

/// Geometry that meets a redaction region is in the file whatever its colour, clip, transparency,
/// blend mode or visibility: a reader that shows it later (a layer turned on, the fill taken away)
/// shows what was to be redacted. The walk has already left out the geometry that carries nothing
/// (see [`crate::walk`]); what is left is reported here.
fn report_found(found: &Found, region: &Region, ri: usize, rep: &mut Report) {
    let Some((place, bbox)) = &found.first else {
        return;
    };
    let kinds: Vec<&str> = found.kinds.iter().copied().collect();
    rep.hits.push(Hit {
        layer: Layer::Images,
        subject: Subject::Rule(format!(
            "painted geometry inside a redaction region: {} object(s) ({}) are painted there, and stay in the file whatever their colour, clip, transparency or visibility; the first is in {} and reaches from ({:.1}, {:.1}) to ({:.1}, {:.1}) points",
            found.count,
            kinds.join(", "),
            place,
            bbox[0],
            bbox[1],
            bbox[2],
            bbox[3]
        )),
        offset: None,
        location: format!("page {}, region {}", region.page, ri + 1),
        form: None,
    });
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
    let mut budget = Budget::new(ctx.opts.walk_steps, ctx.opts.walk_budget);
    // The regions of each page, in the order of the spec: a page is walked once.
    let mut by_page: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (ri, region) in ctx.spec.regions.iter().enumerate() {
        if region
            .page
            .checked_sub(1)
            .and_then(|i| pages.get(i))
            .is_none()
        {
            rep.problems.push(format!(
                "images: region {} names page {}, the file has {} pages",
                ri + 1,
                region.page,
                pages.len()
            ));
            continue;
        }
        by_page.entry(region.page).or_default().push(ri);
    }
    // The glyphs of ordinary fonts are the business of the text layers when the spec has a text to
    // look for; when it has none, a glyph under a region is a leak.
    let glyphs_leak = !ctx.spec.needles.iter().any(|n| n.kind == NeedleKind::Text);
    for (page_no, ris) in by_page {
        let (page, res) = &pages[page_no - 1];
        // Nothing is painted outside the media box.
        let media = numbers(inherited(&doc, page, "MediaBox"), &doc);
        let container = if media.len() == 4 {
            [
                media[0].min(media[2]),
                media[1].min(media[3]),
                media[0].max(media[2]),
                media[1].max(media[3]),
            ]
        } else {
            [f64::MIN, f64::MIN, f64::MAX, f64::MAX]
        };
        let targets: Vec<Target> = ris
            .iter()
            .map(|ri| {
                let r = &ctx.spec.regions[*ri];
                Target {
                    rect: normalised(r.rect),
                    fill: r.fill,
                }
            })
            .collect();
        let mut w = Walker::new(&doc, container, targets, glyphs_leak, budget);
        match contents(&doc, page) {
            Ok(data) => w.page(&data, &doc.resolve(res).clone()),
            Err(why) => w.problem(why),
        }
        annotations(&mut w, page, container);
        budget = w.budget;
        for p in w.problems.drain(..) {
            rep.problems.push(format!("images: page {page_no}: {p}"));
        }
        for (k, ri) in ris.iter().enumerate() {
            let region = &ctx.spec.regions[*ri];
            let rect = normalised(region.rect);
            let zones: Vec<Zone> = w
                .out
                .iter()
                .flat_map(|pl| check_placement(&doc, region, *ri, pl, rep))
                .collect();
            report_found(&w.found[k], region, *ri, rep);
            for (r, sub) in &w.bare_text {
                if overlaps(r, &rect) {
                    rep.hits.push(Hit {
                        layer: Layer::Images,
                        subject: Subject::Rule(format!(
                            "painted geometry inside a redaction region: the text of a {sub} annotation with no appearance stream lies over the region; a viewer draws that text from the entries of the annotation (/Contents, /RC, /V)"
                        )),
                        offset: None,
                        location: format!("page {}, region {}", region.page, ri + 1),
                        form: None,
                    });
                }
            }
            for (r, sub) in &w.bare {
                if overlaps(r, &rect) {
                    rep.problem(format!(
                        "images: page {}, region {}: a {sub} annotation with no appearance stream lies over the region; a viewer draws it from its own entries, which this check does not place, so what it draws was not checked",
                        region.page,
                        ri + 1
                    ));
                }
            }
            check_render(
                ctx,
                &doc,
                region.page,
                page,
                region,
                *ri,
                &zones,
                &mut cache,
                rep,
            );
        }
    }
    Ok(())
}

fn normalised(r: [f64; 4]) -> [f64; 4] {
    [
        r[0].min(r[2]),
        r[1].min(r[3]),
        r[0].max(r[2]),
        r[1].max(r[3]),
    ]
}

/// The streams of an appearance, in every state: `/N`, `/R` and `/D` hold a stream or a dictionary
/// of streams (one per state, nested as deep as a file makes it).
fn appearance_streams(doc: &Doc, v: &Obj, out: &mut Vec<u32>, depth: usize) {
    if depth > 4 {
        return;
    }
    match v {
        Obj::Ref(n) => match doc.latest(*n) {
            Some(e) if e.stream.is_some() => {
                if !out.contains(n) {
                    out.push(*n);
                }
            }
            Some(e) => appearance_streams(doc, &e.value, out, depth + 1),
            None => {}
        },
        Obj::Dict(d) => d
            .iter()
            .for_each(|(_, x)| appearance_streams(doc, x, out, depth + 1)),
        Obj::Array(a) => a
            .iter()
            .for_each(|x| appearance_streams(doc, x, out, depth + 1)),
        _ => {}
    }
}

/// The annotations that draw something of their own, and that a viewer draws from their entries
/// (`/InkList`, `/L`, `/Vertices`, `/QuadPoints`, `/Contents`, the value of a field) when they
/// have no appearance stream.
const DRAWN_BY_VIEWER: [&str; 17] = [
    "Ink",
    "Line",
    "Square",
    "Circle",
    "Polygon",
    "PolyLine",
    "Stamp",
    "Highlight",
    "Underline",
    "Squiggly",
    "StrikeOut",
    "Caret",
    "FreeText",
    "Widget",
    "Text",
    "FileAttachment",
    "Popup",
];

/// The entries that a viewer draws text from.
const TEXT_KEYS: [&str; 3] = ["Contents", "RC", "V"];

/// Whether an annotation has text in the entries that a viewer draws it from. A pop-up shows the
/// text of the annotation that it belongs to.
fn has_entry_text(doc: &Doc, ad: &Obj, depth: usize) -> bool {
    let text = |o: &Obj| match doc.resolve(o) {
        Obj::Str(s) => s.iter().any(|b| !crate::strings::is_space(*b)),
        // A stream (rich text) or anything else that is not empty.
        Obj::Null | Obj::Name(_) | Obj::Bool(_) | Obj::Int(_) | Obj::Real(_) => false,
        Obj::Array(a) => !a.is_empty(),
        Obj::Dict(_) | Obj::Ref(_) => true,
    };
    if TEXT_KEYS.iter().any(|k| ad.get(k).is_some_and(text)) {
        return true;
    }
    depth < 2
        && ad
            .get("Parent")
            .is_some_and(|p| has_entry_text(doc, doc.resolve(p), depth + 1))
}

/// Walks every form that a viewer can draw for an annotation: all states of the normal, rollover
/// and down appearances, and the icons of a widget (`/MK /I`, `/RI`, `/IX`), each fitted to the
/// rectangle of the annotation as a viewer does.
fn annotations(w: &mut Walker, page: &Entry, container: [f64; 4]) {
    let doc = w.doc();
    let Obj::Array(annots) = doc.dget(&page.value, "Annots") else {
        return;
    };
    for a in annots {
        let ad = doc.resolve(a);
        let mut forms: Vec<u32> = Vec::new();
        if let Some(ap) = ad.get("AP").map(|x| doc.resolve(x)) {
            for state in ["N", "R", "D"] {
                if let Some(s) = ap.get(state) {
                    appearance_streams(doc, s, &mut forms, 0);
                }
            }
        }
        if let Some(mk) = ad.get("MK").map(|x| doc.resolve(x)) {
            for icon in ["I", "RI", "IX"] {
                if let Some(s) = mk.get(icon) {
                    appearance_streams(doc, s, &mut forms, 0);
                }
            }
        }
        let rect = ad.get("Rect").map(|r| numbers(r, doc)).unwrap_or_default();
        if forms.is_empty() {
            // No appearance stream: a viewer draws these from their own entries.
            let sub = doc.dget(ad, "Subtype").name().unwrap_or("");
            if !DRAWN_BY_VIEWER.contains(&sub) {
                continue;
            }
            if rect.len() != 4 {
                w.problem(format!(
                    "a {sub} annotation with no appearance stream has no /Rect, so where a viewer draws it is not known"
                ));
                continue;
            }
            let r = normalised([rect[0], rect[1], rect[2], rect[3]]);
            if has_entry_text(doc, ad, 0) {
                w.bare_text.push((r, sub.to_owned()));
            } else {
                w.bare.push((r, sub.to_owned()));
            }
            continue;
        }
        if rect.len() != 4 {
            w.problem("an annotation with an appearance has no /Rect, so where a viewer draws it is not known".to_owned());
            continue;
        }
        for n in forms {
            let Some(e) = doc.latest(n) else { continue };
            let bbox = e
                .value
                .get("BBox")
                .map(|b| numbers(b, doc))
                .unwrap_or_default();
            if bbox.len() != 4 {
                w.problem(format!(
                    "the appearance form {n} has no /BBox, so where a viewer draws it is not known"
                ));
                continue;
            }
            let m = e
                .value
                .get("Matrix")
                .map_or(crate::content::IDENTITY, |m| matrix_of(m, doc));
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
            w.appearance(n, &res, a_m, container);
        }
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
    use crate::content::{mul, IDENTITY};

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
        assert!(jpeg_lossy(b"not a jpeg").is_err());
    }

    /// A DQT segment for the table `id` with this first value, 8 or 16 bits wide.
    fn dqt(id: u8, first: u16, wide: bool) -> Vec<u8> {
        let mut seg = vec![id | if wide { 0x10 } else { 0 }];
        for i in 0..64 {
            let v = if i == 0 { first } else { 1 };
            if wide {
                seg.extend(v.to_be_bytes());
            } else {
                seg.push(v as u8);
            }
        }
        let mut out = vec![0xFF, 0xDB];
        out.extend(u16::try_from(seg.len() + 2).unwrap().to_be_bytes());
        out.extend(seg);
        out
    }

    #[test]
    fn only_the_tables_that_a_component_uses_count() {
        let head = jpeg_head(8, 0x11);
        let base = jpeg_lossy(&head).unwrap().tolerance;
        for table in [dqt(3, 255, false), dqt(3, 65535, true), dqt(1, 255, false)] {
            let mut v = head[..2].to_vec();
            v.extend(&table);
            v.extend(&head[2..]);
            assert_eq!(jpeg_lossy(&v).unwrap().tolerance, base);
        }
        // A table that a component uses and the file does not define.
        let mut v = head[..2].to_vec();
        v.extend(&head[2 + 69..]);
        assert!(jpeg_lossy(&v).unwrap_err().contains("does not define"));
        // The table that is used decides, 16 bits or not, and a later definition of it counts.
        let mut v = head[..2].to_vec();
        v.extend(dqt(0, 40, true));
        v.extend(&head[2 + 69..]);
        assert_eq!(jpeg_lossy(&v).unwrap().tolerance, 9);
        // A scan header, some entropy coded bytes, and then the definition.
        let mut late = head[..head.len() - 2].to_vec();
        late.extend([
            0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00, 0x12, 0x34,
        ]);
        late.extend(dqt(0, 40, false));
        assert_eq!(jpeg_lossy(&late).unwrap().tolerance, 9);
    }

    #[test]
    fn a_tolerance_above_the_cap_is_refused() {
        let at = |q0: u8| {
            let mut v = vec![0xFF, 0xD8];
            v.extend(dqt(0, u16::from(q0), false));
            v.extend(&jpeg_head(0, 0x11)[2 + 69..]);
            jpeg_lossy(&v)
        };
        // 80 / 16 * 2.8 + 2 = 16: the most that is accepted.
        assert_eq!(at(80).unwrap().tolerance, JPEG_TOLERANCE_CAP);
        assert!(at(81).unwrap_err().contains("so coarse"));
        assert!(at(255).is_err());
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
