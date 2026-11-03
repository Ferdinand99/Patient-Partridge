//! A small reader for what the region check needs of an embedded font program: the bounding box of
//! all its glyphs (the `head` table of a TrueType or OpenType program, the `FontBBox` of a CFF or
//! Type 1 program) and, for a program with an `hmtx` table, the advance of every glyph. It does not
//! read outlines, names or encodings: the walk bounds where ink can be with the box, and checks the
//! widths of the file against the advances. A program that this reader does not fully understand is
//! an error, and the text shown with its font is "could not check".

/// The kind of an embedded font program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// A TrueType program (`FontFile2`).
    TrueType,
    /// An OpenType program with CFF outlines (`FontFile3`).
    OpenTypeCff,
    /// A bare CFF program (`FontFile3`).
    Cff,
    /// A Type 1 program (`FontFile`).
    Type1,
}

/// The numbers of an embedded font program that the region check uses, in em.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub format: Format,
    /// `[xmin, ymin, xmax, ymax]` of all glyphs.
    pub bbox: [f64; 4],
    /// The advance of each glyph, in the order of the glyph numbers (TrueType and OpenType).
    pub advances: Option<Vec<f64>>,
    /// The same, sorted.
    sorted: Vec<f64>,
}

impl Program {
    fn new(format: Format, bbox: [f64; 4], advances: Option<Vec<f64>>) -> Result<Program, String> {
        if bbox.iter().any(|v| !v.is_finite() || v.abs() > 100.0)
            || bbox[2] <= bbox[0]
            || bbox[3] <= bbox[1]
        {
            return Err("its bounding box is empty or not a box".to_owned());
        }
        let mut sorted = advances.clone().unwrap_or_default();
        sorted.sort_by(f64::total_cmp);
        Ok(Program {
            format,
            bbox,
            advances,
            sorted,
        })
    }

    /// The advance of a glyph, if the program has advances.
    pub fn advance(&self, gid: usize) -> Option<f64> {
        self.advances.as_ref()?.get(gid).copied()
    }

    /// Whether some glyph of the program has an advance within the tolerance of `w`.
    pub fn has_advance_near(&self, w: f64, within: impl Fn(f64) -> f64) -> bool {
        let tol = within(w);
        let i = self.sorted.partition_point(|a| *a < w - tol);
        self.sorted.get(i).is_some_and(|a| *a <= w + tol)
    }
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]))
}

fn i16_at(b: &[u8], i: usize) -> Option<i16> {
    u16_at(b, i).map(|v| v as i16)
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *b.get(i)?,
        *b.get(i + 1)?,
        *b.get(i + 2)?,
        *b.get(i + 3)?,
    ]))
}

/// The program of a font file: TrueType, OpenType, bare CFF or Type 1.
pub fn parse(data: &[u8]) -> Result<Program, String> {
    match data.get(..4) {
        Some([0, 1, 0, 0] | b"true") => sfnt(data, Format::TrueType),
        Some(b"OTTO") => sfnt(data, Format::OpenTypeCff),
        Some(b"ttcf") => Err("a TrueType collection is not read".to_owned()),
        Some([1, ..]) => cff(data),
        Some([b'%', b'!', ..] | [0x80, 1, ..]) => type1(data),
        _ => Err("it is not a TrueType, OpenType, CFF or Type 1 program".to_owned()),
    }
}

fn table(data: &[u8], tag: &[u8; 4]) -> Option<(usize, usize)> {
    let n = usize::from(u16_at(data, 4)?);
    for i in 0..n {
        let rec = 12 + 16 * i;
        if data.get(rec..rec + 4)? == tag {
            let off = usize::try_from(u32_at(data, rec + 8)?).ok()?;
            let len = usize::try_from(u32_at(data, rec + 12)?).ok()?;
            let end = off.checked_add(len)?;
            if end <= data.len() {
                return Some((off, len));
            }
            return None;
        }
    }
    None
}

fn sfnt(data: &[u8], format: Format) -> Result<Program, String> {
    let bad = || "its tables cannot be read".to_owned();
    let (head, hl) = table(data, b"head").ok_or_else(bad)?;
    if hl < 54 {
        return Err(bad());
    }
    let upem = f64::from(u16_at(data, head + 18).ok_or_else(bad)?);
    if !(16.0..=16384.0).contains(&upem) {
        return Err("its units per em are not usable".to_owned());
    }
    let b = |o: usize| i16_at(data, head + o).map(|v| f64::from(v) / upem);
    let bbox = [
        b(36).ok_or_else(bad)?,
        b(38).ok_or_else(bad)?,
        b(40).ok_or_else(bad)?,
        b(42).ok_or_else(bad)?,
    ];
    // The advances, when the program has the tables for them.
    let advances = match (
        table(data, b"hhea"),
        table(data, b"maxp"),
        table(data, b"hmtx"),
    ) {
        (Some((hhea, hl)), Some((maxp, ml)), Some((hmtx, xl))) if hl >= 36 && ml >= 6 => {
            let nh = usize::from(u16_at(data, hhea + 34).ok_or_else(bad)?);
            let ng = usize::from(u16_at(data, maxp + 4).ok_or_else(bad)?);
            if nh == 0 || nh > ng || xl < 4 * nh {
                return Err(bad());
            }
            let mut adv = Vec::with_capacity(ng);
            for g in 0..ng {
                let i = g.min(nh - 1);
                adv.push(f64::from(u16_at(data, hmtx + 4 * i).ok_or_else(bad)?) / upem);
            }
            Some(adv)
        }
        _ => None,
    };
    Program::new(format, bbox, advances)
}

/// An INDEX of a CFF program at `at`: the ranges of its entries and the offset after it.
fn cff_index(d: &[u8], at: usize) -> Option<(Vec<std::ops::Range<usize>>, usize)> {
    let count = usize::from(u16_at(d, at)?);
    if count == 0 {
        return Some((Vec::new(), at + 2));
    }
    let off_size = usize::from(*d.get(at + 2)?);
    if !(1..=4).contains(&off_size) {
        return None;
    }
    let off = |i: usize| -> Option<usize> {
        let p = at + 3 + i * off_size;
        let mut v = 0usize;
        for k in 0..off_size {
            v = v << 8 | usize::from(*d.get(p + k)?);
        }
        Some(v)
    };
    let base = at + 3 + (count + 1) * off_size - 1;
    let mut out = Vec::with_capacity(count.min(4096));
    for i in 0..count {
        let (a, b) = (base + off(i)?, base + off(i + 1)?);
        if a > b || b > d.len() {
            return None;
        }
        out.push(a..b);
    }
    let end = base + off(count)?;
    Some((out, end))
}

/// The entries of a DICT of a CFF program: each operator (two bytes for 12 xx) with the numbers
/// before it.
fn cff_dict(d: &[u8]) -> Option<Vec<(u16, Vec<f64>)>> {
    let mut out = Vec::new();
    let mut stack: Vec<f64> = Vec::new();
    let mut i = 0;
    while i < d.len() {
        let b0 = d[i];
        match b0 {
            0..=21 => {
                let op = if b0 == 12 {
                    i += 1;
                    0x0C00 | u16::from(*d.get(i)?)
                } else {
                    u16::from(b0)
                };
                out.push((op, std::mem::take(&mut stack)));
                i += 1;
            }
            28 => {
                stack.push(f64::from(i16_at(d, i + 1)?));
                i += 3;
            }
            29 => {
                stack.push(f64::from(u32_at(d, i + 1)? as i32));
                i += 5;
            }
            30 => {
                // A real: nibbles up to the nibble 15.
                let mut s = String::new();
                i += 1;
                'real: loop {
                    let b = *d.get(i)?;
                    i += 1;
                    for n in [b >> 4, b & 15] {
                        match n {
                            0..=9 => s.push(char::from(b'0' + n)),
                            10 => s.push('.'),
                            11 => s.push('E'),
                            12 => s.push_str("E-"),
                            14 => s.push('-'),
                            15 => break 'real,
                            _ => return None,
                        }
                    }
                }
                stack.push(s.parse::<f64>().ok()?);
            }
            32..=246 => {
                stack.push(f64::from(b0) - 139.0);
                i += 1;
            }
            247..=250 => {
                stack.push((f64::from(b0) - 247.0) * 256.0 + f64::from(*d.get(i + 1)?) + 108.0);
                i += 2;
            }
            251..=254 => {
                stack.push(-(f64::from(b0) - 251.0) * 256.0 - f64::from(*d.get(i + 1)?) - 108.0);
                i += 2;
            }
            _ => return None,
        }
        if stack.len() > 48 {
            return None;
        }
    }
    Some(out)
}

fn cff(d: &[u8]) -> Result<Program, String> {
    let bad = || "its CFF data cannot be read".to_owned();
    let hdr = usize::from(*d.get(2).ok_or_else(bad)?);
    let (_names, after_names) = cff_index(d, hdr).ok_or_else(bad)?;
    let (tops, _) = cff_index(d, after_names).ok_or_else(bad)?;
    let top = tops.first().ok_or_else(bad)?;
    let dict = cff_dict(&d[top.clone()]).ok_or_else(bad)?;
    let get = |op: u16| dict.iter().find(|(o, _)| *o == op).map(|(_, v)| v);
    // FontMatrix (12 7): only the default of 1/1000 is read.
    if let Some(m) = get(0x0C07) {
        let default = [0.001, 0.0, 0.0, 0.001, 0.0, 0.0];
        if m.len() != 6 || m.iter().zip(default).any(|(a, b)| (a - b).abs() > 1e-9) {
            return Err("its FontMatrix is not 0.001".to_owned());
        }
    }
    // FontBBox is operator 5; without it the default is 0 0 0 0, which is no box.
    let Some(b) = get(5) else {
        return Err("it has no FontBBox".to_owned());
    };
    if b.len() != 4 {
        return Err(bad());
    }
    Program::new(
        Format::Cff,
        [b[0] / 1000.0, b[1] / 1000.0, b[2] / 1000.0, b[3] / 1000.0],
        None,
    )
}

fn type1(d: &[u8]) -> Result<Program, String> {
    // The clear-text part comes first: /FontBBox {a b c d} (or [a b c d]), /FontMatrix [..].
    let head = &d[..d.len().min(1 << 16)];
    let find = |key: &[u8]| head.windows(key.len()).position(|w| w == key);
    let numbers = |at: usize| -> Option<Vec<f64>> {
        let rest = &head[at..];
        let open = rest.iter().position(|b| matches!(b, b'{' | b'['))?;
        let close = rest.iter().position(|b| matches!(b, b'}' | b']'))?;
        if close < open || close - open > 200 {
            return None;
        }
        std::str::from_utf8(&rest[open + 1..close])
            .ok()?
            .split_ascii_whitespace()
            .map(|t| t.parse::<f64>().ok())
            .collect()
    };
    if let Some(at) = find(b"/FontMatrix") {
        let m = numbers(at + 11).ok_or("its FontMatrix cannot be read")?;
        let default = [0.001, 0.0, 0.0, 0.001, 0.0, 0.0];
        if m.len() != 6 || m.iter().zip(default).any(|(a, b)| (a - b).abs() > 1e-9) {
            return Err("its FontMatrix is not 0.001".to_owned());
        }
    }
    let at = find(b"/FontBBox").ok_or("it has no FontBBox")?;
    let b = numbers(at + 9).ok_or("its FontBBox cannot be read")?;
    if b.len() != 4 {
        return Err("its FontBBox cannot be read".to_owned());
    }
    Program::new(
        Format::Type1,
        [b[0] / 1000.0, b[1] / 1000.0, b[2] / 1000.0, b[3] / 1000.0],
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A TrueType program with these advances (units per em 1000) and bounding box.
    fn truetype(bbox: [i16; 4], advances: &[u16]) -> Vec<u8> {
        let mut head = vec![0u8; 54];
        head[18..20].copy_from_slice(&1000u16.to_be_bytes());
        for (i, v) in bbox.iter().enumerate() {
            head[36 + 2 * i..38 + 2 * i].copy_from_slice(&v.to_be_bytes());
        }
        let mut hhea = vec![0u8; 36];
        hhea[34..36].copy_from_slice(&(advances.len() as u16).to_be_bytes());
        let mut maxp = vec![0u8; 6];
        maxp[4..6].copy_from_slice(&(advances.len() as u16).to_be_bytes());
        let hmtx: Vec<u8> = advances
            .iter()
            .flat_map(|a| [a.to_be_bytes(), [0, 0]].concat())
            .collect();
        let tables: [(&[u8; 4], &Vec<u8>); 4] = [
            (b"head", &head),
            (b"hhea", &hhea),
            (b"hmtx", &hmtx),
            (b"maxp", &maxp),
        ];
        let mut out = vec![0, 1, 0, 0];
        out.extend((tables.len() as u16).to_be_bytes());
        out.extend([0u8; 6]);
        let mut off = 12 + 16 * tables.len();
        for (tag, t) in &tables {
            out.extend(tag.iter());
            out.extend(0u32.to_be_bytes());
            out.extend((off as u32).to_be_bytes());
            out.extend((t.len() as u32).to_be_bytes());
            off += t.len();
        }
        for (_, t) in &tables {
            out.extend(t.iter());
        }
        out
    }

    #[test]
    fn a_truetype_program_gives_its_box_and_advances() {
        let p = parse(&truetype([-100, -250, 1100, 950], &[500, 600, 250])).unwrap();
        assert_eq!(p.bbox, [-0.1, -0.25, 1.1, 0.95]);
        assert_eq!(p.advance(1), Some(0.6));
        assert!(p.has_advance_near(0.598, |w| 0.02 * w));
        assert!(!p.has_advance_near(0.4, |w| 0.02 * w));
        assert!(parse(&truetype([0, 0, 0, 0], &[500])).is_err());
        assert!(parse(&truetype([0, 0, 10, 10], &[500])[..40]).is_err());
    }

    #[test]
    fn a_cff_program_gives_its_font_bbox() {
        // Header, an empty Name INDEX, a Top DICT INDEX with FontBBox -100 -250 1100 950.
        let dict: Vec<u8> = vec![
            29, 0xFF, 0xFF, 0xFF, 0x9C, // -100
            29, 0xFF, 0xFF, 0xFF, 0x06, // -250
            28, 0x04, 0x4C, // 1100
            28, 0x03, 0xB6, // 950
            5,
        ];
        let mut d = vec![1, 0, 4, 1];
        d.extend([0, 0]); // Name INDEX, no entries
        d.extend([0, 1, 1, 1, (dict.len() + 1) as u8]);
        d.extend(&dict);
        let p = parse(&d).unwrap();
        assert_eq!(p.bbox, [-0.1, -0.25, 1.1, 0.95]);
        assert!(p.advances.is_none());
    }

    #[test]
    fn a_type1_program_gives_its_font_bbox() {
        let t = b"%!PS-AdobeFont-1.0: X\n/FontMatrix [0.001 0 0 0.001 0 0] readonly def\n/FontBBox{-168 -218 1000 898}readonly def\n";
        assert_eq!(parse(t).unwrap().bbox, [-0.168, -0.218, 1.0, 0.898]);
        let odd = b"%!PS-AdobeFont-1.0: X\n/FontMatrix [0.002 0 0 0.002 0 0]\n/FontBBox{0 0 1 1}\n";
        assert!(parse(odd).is_err());
        assert!(parse(b"%!PS-AdobeFont-1.0: X\n").is_err());
        assert!(parse(b"junk").is_err());
    }
}
