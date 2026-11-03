//! The region check for images: given the rectangles that were redacted and the page, find the
//! images that are drawn over them (page content, forms, annotation appearances), decode each
//! one, and check that the pixels inside the rectangle are blank. This does not depend on the
//! codec: a pixel is what the decoder gives, however the engine stored the image again.
//!
//! "Blank" means that the covered pixels all have the same colour (within a tolerance for lossy
//! codecs). A pixel counts as covered when its whole square lies inside the rectangle.

use crate::carve;
use crate::content::{for_each_op, Op};
use crate::pdfobj::{Doc, Entry, Obj};
use crate::{Hit, Layer, Region, Report, Subject};

type M = [f64; 6];
const IDENTITY: M = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
/// The tolerance for an image stored with a lossy codec (JPEG).
pub const LOSSY_TOLERANCE: u8 = 16;

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
            _ => {}
        });
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

/// A decoded image: 8-bit samples, `ncomp` per pixel, rows without padding.
struct Pixels {
    w: usize,
    h: usize,
    ncomp: usize,
    data: Vec<u8>,
    lossy: bool,
}

fn components(doc: &Doc, cs: &Obj) -> Result<usize, String> {
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

fn unpack(raw: &[u8], w: usize, h: usize, ncomp: usize, bits: usize) -> Result<Vec<u8>, String> {
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

fn decode_image(doc: &Doc, dict: &Obj, decoded: &carve::Decoded) -> Result<Pixels, String> {
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
                Ok(Pixels {
                    w,
                    h,
                    ncomp,
                    data,
                    lossy: true,
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
        lossy: false,
    })
}

/// Checks the regions against the pages of the normalised copy of a file. `bytes` is that copy.
pub fn check(bytes: &[u8], regions: &[Region], rep: &mut Report) {
    let doc = Doc::scan(bytes);
    let pages = doc.pages();
    if pages.is_empty() {
        rep.problems
            .push("images: no pages were found, so the regions were not checked".into());
        return;
    }
    for (ri, region) in regions.iter().enumerate() {
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
        for pl in &w.out {
            check_placement(&doc, region, ri, pl, rep);
        }
    }
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
            let (tx0, tx1) = pts.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| {
                (lo.min(p.0), hi.max(p.0))
            });
            let (ty0, ty1) = pts.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| {
                (lo.min(p.1), hi.max(p.1))
            });
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

fn check_placement(doc: &Doc, region: &Region, ri: usize, pl: &Placement, rep: &mut Report) {
    let [rx0, ry0, rx1, ry1] = region.rect;
    let (rx0, rx1) = (rx0.min(rx1), rx0.max(rx1));
    let (ry0, ry1) = (ry0.min(ry1), ry0.max(ry1));
    let corners = [
        apply(&pl.ctm, 0.0, 0.0),
        apply(&pl.ctm, 1.0, 0.0),
        apply(&pl.ctm, 0.0, 1.0),
        apply(&pl.ctm, 1.0, 1.0),
    ];
    let (bx0, bx1) = corners
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p.0), h.max(p.0)));
    let (by0, by1) = corners
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p.1), h.max(p.1)));
    if bx1 <= rx0 || bx0 >= rx1 || by1 <= ry0 || by0 >= ry1 {
        return;
    }
    let who = match &pl.image {
        Source::Object(n) => format!("image object {n}"),
        Source::Inline(..) => "an inline image".to_owned(),
    };
    let place = format!("page {}, region {}, {who}", region.page, ri + 1);
    let decoded = match &pl.image {
        Source::Object(n) => {
            let Some(e) = doc.latest(*n) else { return };
            let Some(d) = doc.stream_data(e) else {
                rep.problems
                    .push(format!("images: {place}: the image has no data"));
                return;
            };
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
            return;
        }
    };
    let Some(inv) = invert(&pl.ctm) else { return };
    let uv: Vec<(f64, f64)> = [(rx0, ry0), (rx1, ry0), (rx0, ry1), (rx1, ry1)]
        .iter()
        .map(|(x, y)| apply(&inv, *x, *y))
        .collect();
    let (u0, u1) = uv
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p.0), h.max(p.0)));
    let (v0, v1) = uv
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p.1), h.max(p.1)));
    let (w, h) = (px.w as f64, px.h as f64);
    let col0 = ((u0 * w).floor().max(0.0)) as usize;
    let col1 = ((u1 * w).ceil().min(w)) as usize;
    let row0 = (((1.0 - v1) * h).floor().max(0.0)) as usize;
    let row1 = (((1.0 - v0) * h).ceil().min(h)) as usize;
    let tol = region
        .tolerance
        .unwrap_or(if px.lossy { LOSSY_TOLERANCE } else { 0 });
    let eps = 1e-3;
    let mut reference: Option<Vec<u8>> = None;
    let (mut covered, mut differing) = (0usize, 0usize);
    for row in row0..row1.min(px.h) {
        for col in col0..col1.min(px.w) {
            let (cx0, cx1) = (col as f64 / w, (col + 1) as f64 / w);
            let (cy0, cy1) = (1.0 - (row + 1) as f64 / h, 1.0 - row as f64 / h);
            let inside = [(cx0, cy0), (cx1, cy0), (cx0, cy1), (cx1, cy1)]
                .iter()
                .all(|(u, v)| {
                    let (x, y) = apply(&pl.ctm, *u, *v);
                    x >= rx0 - eps && x <= rx1 + eps && y >= ry0 - eps && y <= ry1 + eps
                });
            if !inside {
                continue;
            }
            covered += 1;
            let s = &px.data[(row * px.w + col) * px.ncomp..][..px.ncomp];
            match &reference {
                None => reference = Some(s.to_vec()),
                Some(r) => {
                    if r.iter().zip(s).any(|(a, b)| a.abs_diff(*b) > tol) {
                        differing += 1;
                    }
                }
            }
        }
    }
    if differing > 0 {
        rep.hits.push(Hit {
            layer: Layer::Images,
            subject: Subject::Rule(format!(
                "image pixels inside a redaction region are not blank ({differing} of {covered} differ)"
            )),
            offset: None,
            location: place,
            form: None,
        });
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
}
