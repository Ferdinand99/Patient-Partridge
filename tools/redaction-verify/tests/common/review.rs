//! Small files for the cases that a byte search cannot see: text in fonts that use glyph codes,
//! words spaced by kerning, text in places that are not drawn, rich text, and images that are
//! only partly covered. Each file is written byte by byte.

use std::collections::BTreeMap;

use super::{flate, Bytes, Pdf};

pub const NEEDLE: &str = "Ola Nordmann";
pub const BENIGN: &[u8] = b"BT /F1 12 Tf 72 720 Td (Quarterly report) Tj ET\n";

pub type Objs = BTreeMap<u32, Bytes>;

/// A stream object body: the dictionary, `/Length`, and the data (compressed with Flate if asked).
pub fn stream_obj(dict: &str, data: &[u8], compress: bool) -> Bytes {
    let (data, dict) = if compress {
        (flate(data), format!("{dict} /Filter /FlateDecode"))
    } else {
        (data.to_vec(), dict.to_owned())
    };
    let mut out = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    out.extend_from_slice(&data);
    out.extend_from_slice(b"\nendstream");
    out
}

/// A Flate stream of text. A short text would be stored by the compressor as it is, which a byte
/// search would find; a comment of filler after it makes the compressor really pack the data.
pub fn packed(dict: &str, data: &[u8]) -> Bytes {
    let mut d = data.to_vec();
    d.extend_from_slice(b"\n% ");
    d.extend(std::iter::repeat_n(*b"filler ", 60).flatten());
    d.push(b'\n');
    stream_obj(dict, &d, true)
}

/// Writes a file from its objects (the catalog is object 1).
pub fn file(objs: &Objs) -> Bytes {
    let mut pdf = Pdf::new("1.7");
    for (n, body) in objs {
        pdf.obj(*n, body);
    }
    pdf.xref("/Root 1 0 R");
    pdf.finish()
}

/// The codes of the Type0 font: each distinct character of `text` gets a glyph number.
pub fn code_of(text: &str, c: char) -> u16 {
    let mut seen: Vec<char> = Vec::new();
    for x in text.chars() {
        if !seen.contains(&x) {
            seen.push(x);
        }
    }
    let i = seen
        .iter()
        .position(|x| *x == c)
        .expect("character in the font");
    0x0101 + (i as u16) * 7
}

/// `<0101..>`: the glyph codes that show `shown` with the font made for `cid_text`.
pub fn show_codes(cid_text: &str, shown: &str) -> String {
    let mut s = String::from("<");
    for c in shown.chars() {
        s.push_str(&format!("{:04X}", code_of(cid_text, c)));
    }
    s.push('>');
    s
}

/// A `ToUnicode` map for the font made for `cid_text`. `maps_to` gives the text for a character
/// (the character itself, normally).
fn cmap(cid_text: &str, maps_to: &dyn Fn(char) -> String) -> Bytes {
    let mut chars: Vec<char> = Vec::new();
    for c in cid_text.chars() {
        if !chars.contains(&c) {
            chars.push(c);
        }
    }
    let mut out = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    out.push_str(&format!("{} beginbfchar\n", chars.len()));
    for c in &chars {
        let units: String = maps_to(*c)
            .encode_utf16()
            .map(|u| format!("{u:04X}"))
            .collect();
        out.push_str(&format!("<{:04X}> <{units}>\n", code_of(cid_text, *c)));
    }
    out.push_str("endbfchar\nendcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    out.into_bytes()
}

/// The parts of a small file with a Helvetica font (`/F1`) and a Type0 font with glyph codes
/// (`/F2`). Object numbers: 1 catalog, 2 pages, 3 page, 4 contents, 5 Helvetica, 6 Type0, 7 its
/// descendant, 8 its ToUnicode map, 9 its descriptor.
pub struct Fx {
    /// The characters the Type0 font has glyphs and ToUnicode entries for.
    pub cid_text: String,
    pub content: Bytes,
    pub page_extra: String,
    pub resources_extra: String,
    pub catalog_extra: String,
    pub compress: bool,
    pub extra: Objs,
    /// Give the Type0 font a `ToUnicode` map.
    pub to_unicode: bool,
    /// What a code maps to in the `ToUnicode` map; the character itself if `None`.
    pub maps_to: Option<fn(char) -> String>,
    /// Name the fonts as subsets (`ABCDEF+..`).
    pub subset: bool,
}

impl Fx {
    pub fn new(content: &[u8]) -> Fx {
        Fx {
            cid_text: NEEDLE.to_owned(),
            content: content.to_vec(),
            page_extra: String::new(),
            resources_extra: String::new(),
            catalog_extra: String::new(),
            compress: true,
            extra: Objs::new(),
            to_unicode: true,
            maps_to: None,
            subset: true,
        }
    }

    pub fn show(&self, text: &str) -> String {
        show_codes(&self.cid_text, text)
    }

    pub fn objs(&self) -> Objs {
        let base = if self.subset {
            "ABCDEF+DejaVuSans"
        } else {
            "DejaVuSans"
        };
        let tu = if self.to_unicode {
            " /ToUnicode 8 0 R"
        } else {
            ""
        };
        let mut o = Objs::new();
        o.insert(
            1,
            format!("<< /Type /Catalog /Pages 2 0 R{} >>", self.catalog_extra).into_bytes(),
        );
        o.insert(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec());
        o.insert(
            3,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
                 /Resources << /Font << /F1 5 0 R /F2 6 0 R >>{} >>{} >>",
                self.resources_extra, self.page_extra
            )
            .into_bytes(),
        );
        o.insert(
            4,
            if self.compress {
                packed("", &self.content)
            } else {
                stream_obj("", &self.content, false)
            },
        );
        o.insert(
            5,
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .to_vec(),
        );
        o.insert(
            6,
            format!(
                "<< /Type /Font /Subtype /Type0 /BaseFont /{base} /Encoding /Identity-H \
                 /DescendantFonts [7 0 R]{tu} >>"
            )
            .into_bytes(),
        );
        o.insert(
            7,
            format!(
                "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{base} \
                 /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
                 /FontDescriptor 9 0 R /DW 600 /CIDToGIDMap /Identity >>"
            )
            .into_bytes(),
        );
        let map: Box<dyn Fn(char) -> String> = match self.maps_to {
            Some(f) => Box::new(f),
            None => Box::new(|c| c.to_string()),
        };
        o.insert(8, packed("", &cmap(&self.cid_text, &*map)));
        o.insert(
            9,
            format!(
                "<< /Type /FontDescriptor /FontName /{base} /Flags 32 /FontBBox [0 -200 1000 900] \
                 /ItalicAngle 0 /Ascent 900 /Descent -200 /CapHeight 700 /StemV 80 >>"
            )
            .into_bytes(),
        );
        for (n, b) in &self.extra {
            o.insert(*n, b.clone());
        }
        o
    }

    pub fn build(&self) -> Bytes {
        file(&self.objs())
    }

    /// The same file with the optional-content group `/oc1` switched off. The content is drawn in
    /// a layer that no viewer shows.
    pub fn hidden(mut self, hidden_ops: &[u8]) -> Fx {
        let mut c = self.content.clone();
        c.extend_from_slice(b"/OC /oc1 BDC ");
        c.extend_from_slice(hidden_ops);
        c.extend_from_slice(b"EMC\n");
        self.content = c;
        self.resources_extra
            .push_str(" /Properties << /oc1 11 0 R >>");
        self.catalog_extra
            .push_str(" /OCProperties << /OCGs [11 0 R] /D << /OFF [11 0 R] >> >>");
        self.extra
            .insert(11, b"<< /Type /OCG /Name (Draft) >>".to_vec());
        self
    }

    /// A form XObject (object 10) with this content, using both fonts.
    pub fn form(content: &[u8]) -> Bytes {
        stream_obj(
            "/Type /XObject /Subtype /Form /BBox [0 0 612 792] \
             /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >>",
            content,
            true,
        )
    }

    /// An image XObject body of 8-bit RGB or gray samples.
    pub fn image(w: usize, h: usize, comps: usize, samples: &[u8]) -> Bytes {
        let cs = if comps == 1 {
            "/DeviceGray"
        } else {
            "/DeviceRGB"
        };
        stream_obj(
            &format!(
                "/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace {cs} \
                 /BitsPerComponent 8"
            ),
            samples,
            true,
        )
    }
}

/// The content that draws object 16 (an image) over 100 x 100 points at (72, 400).
pub const DRAW_IM1: &[u8] = b"q 100 0 0 100 72 400 cm /Im1 Do Q\n";

/// A page with the benign text and an image (object 16) drawn at (72, 400)-(172, 500).
pub fn with_image(image: Bytes) -> Fx {
    let mut fx = Fx::new(&[BENIGN, DRAW_IM1].concat());
    fx.resources_extra.push_str(" /XObject << /Im1 16 0 R >>");
    fx.extra.insert(16, image);
    fx
}
