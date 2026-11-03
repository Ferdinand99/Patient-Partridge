//! Test files, written byte by byte in code (no binary fixtures in the repository), and the
//! helpers to run the verifier on them. Nothing here uses PDFium or any PDF library: a mistake in
//! the code under test cannot make a fixture look clean.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use flate2::write::ZlibEncoder;
use flate2::Compression;
use redaction_verify::carve::sha256_hex;

pub type Bytes = Vec<u8>;

pub fn flate(data: &[u8]) -> Bytes {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

pub fn ascii_hex(data: &[u8]) -> Bytes {
    let mut s: String = data.iter().map(|b| format!("{b:02x}")).collect();
    s.push('>');
    s.into_bytes()
}

pub fn sha(data: &[u8]) -> String {
    sha256_hex(data)
}

/// `(text)` with the escapes that a literal string needs.
pub fn lit(s: &str) -> Bytes {
    let mut out = vec![b'('];
    for &b in s.as_bytes() {
        if matches!(b, b'(' | b')' | b'\\') {
            out.push(b'\\');
        }
        out.push(b);
    }
    out.push(b')');
    out
}

/// `<4c45..>`: the bytes of the text in a hex string.
pub fn hex(s: &str) -> Bytes {
    let mut out = vec![b'<'];
    out.extend(s.bytes().flat_map(|b| format!("{b:02X}").into_bytes()));
    out.push(b'>');
    out
}

/// `<FEFF..>`: the text as UTF-16BE with a byte order mark, in a hex string.
pub fn utf16_hex(s: &str) -> Bytes {
    let mut out = b"<FEFF".to_vec();
    for u in s.encode_utf16() {
        out.extend(format!("{u:04X}").into_bytes());
    }
    out.push(b'>');
    out
}

/// `(` FE FF 00 4C .. `)`: UTF-16BE bytes inside a literal string, with no escapes at all.
pub fn utf16_literal(s: &str) -> Bytes {
    let mut out = vec![b'(', 0xFE, 0xFF];
    for u in s.encode_utf16() {
        out.extend(u.to_be_bytes());
    }
    out.push(b')');
    out
}

/// A literal string where every third character is an octal escape.
pub fn escaped(s: &str) -> Bytes {
    let mut out = vec![b'('];
    for (i, b) in s.bytes().enumerate() {
        if i % 3 == 1 {
            out.extend(format!("\\{b:03o}").into_bytes());
        } else {
            out.push(b);
        }
    }
    out.push(b')');
    out
}

/// A name with the characters of `s` written as `#xx` where PDF needs it (the space).
pub fn name_escaped(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b == b' ' {
                "#20".to_owned()
            } else {
                (b as char).to_string()
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------

/// A PDF file being written: objects, cross-reference sections, incremental updates.
pub struct Pdf {
    pub buf: Bytes,
    offsets: BTreeMap<u32, usize>,
    pending: Vec<u32>,
    last_xref: Option<usize>,
}

impl Pdf {
    pub fn new(version: &str) -> Self {
        let mut buf = format!("%PDF-{version}\n").into_bytes();
        buf.extend_from_slice(b"%\xE2\xE3\xCF\xD3\n");
        Pdf {
            buf,
            offsets: BTreeMap::new(),
            pending: Vec::new(),
            last_xref: None,
        }
    }

    pub fn obj(&mut self, n: u32, body: &[u8]) {
        self.offsets.insert(n, self.buf.len());
        self.pending.push(n);
        self.buf.extend(format!("{n} 0 obj\n").into_bytes());
        self.buf.extend_from_slice(body);
        self.buf.extend_from_slice(b"\nendobj\n");
    }

    /// A stream object. `dict` is the content of the dictionary without `/Length`.
    pub fn stream(&mut self, n: u32, dict: &str, data: &[u8]) {
        let mut body = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        self.obj(n, &body);
    }

    pub fn pos_of(&self, n: u32) -> usize {
        self.offsets[&n]
    }

    /// Writes a classic cross-reference section for the objects written since the last one, and
    /// the trailer. `trailer` is the content of the trailer dictionary without `/Size` and
    /// `/Prev`.
    pub fn xref(&mut self, trailer: &str) {
        let first = self.last_xref.is_none();
        let size = self.offsets.keys().max().map_or(1, |m| m + 1);
        let mut nums = std::mem::take(&mut self.pending);
        nums.sort_unstable();
        nums.dedup();
        let start = self.buf.len();
        self.buf.extend_from_slice(b"xref\n");
        let mut entries: Vec<(u32, String)> = Vec::new();
        if first {
            entries.push((0, "0000000000 65535 f \n".to_owned()));
        }
        for n in nums {
            entries.push((n, format!("{:010} 00000 n \n", self.offsets[&n])));
        }
        let mut i = 0;
        while i < entries.len() {
            let mut j = i;
            while j + 1 < entries.len() && entries[j + 1].0 == entries[j].0 + 1 {
                j += 1;
            }
            self.buf
                .extend(format!("{} {}\n", entries[i].0, j - i + 1).into_bytes());
            for e in &entries[i..=j] {
                self.buf.extend_from_slice(e.1.as_bytes());
            }
            i = j + 1;
        }
        let prev = self
            .last_xref
            .map_or(String::new(), |p| format!(" /Prev {p}"));
        self.buf.extend(
            format!("trailer\n<< /Size {size} {trailer}{prev} >>\nstartxref\n{start}\n%%EOF\n")
                .into_bytes(),
        );
        self.last_xref = Some(start);
    }

    pub fn finish(self) -> Bytes {
        self.buf
    }
}

// ---------------------------------------------------------------------------------------------

/// An image XObject.
#[derive(Clone)]
pub struct Image {
    /// The dictionary entries, without `/Length`.
    pub dict: String,
    /// The stream data as stored (already filtered).
    pub data: Bytes,
}

impl Image {
    /// 2x2 RGB pixels stored with Flate.
    pub fn rgb_flate(samples: &[u8; 12]) -> Image {
        Image {
            dict: "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB \
                   /BitsPerComponent 8 /Filter /FlateDecode"
                .into(),
            data: flate(samples),
        }
    }

    /// 2x2 RGB pixels stored with ASCIIHex and then Flate (the chain is `[/ASCIIHexDecode
    /// /FlateDecode]`: the hex is undone first).
    pub fn rgb_hex_flate(samples: &[u8; 12]) -> Image {
        Image {
            dict: "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB \
                   /BitsPerComponent 8 /Filter [/ASCIIHexDecode /FlateDecode]"
                .into(),
            data: ascii_hex(&flate(samples)),
        }
    }

    /// 2x2 pixels as indexes into a 4-colour palette. The pixels as `pdfimages` draws them are
    /// the palette colours, not the indexes.
    pub fn indexed(indexes: &[u8; 4], palette: &[u8; 12]) -> Image {
        let pal: String = palette.iter().map(|b| format!("{b:02x}")).collect();
        Image {
            dict: format!(
                "/Type /XObject /Subtype /Image /Width 2 /Height 2 \
                 /ColorSpace [/Indexed /DeviceRGB 3 <{pal}>] /BitsPerComponent 8 \
                 /Filter /FlateDecode"
            ),
            data: flate(indexes),
        }
    }
}

/// The parts of a test document. Every field has a harmless default (`Doc::rich`): a leak
/// is made by changing one field.
#[derive(Clone)]
pub struct Doc {
    pub content: Bytes,
    pub content_flate: bool,
    pub font: String,
    /// Entries of the Info dictionary; `None` leaves the dictionary out.
    pub info: Option<Bytes>,
    /// The XMP packet and whether it is compressed.
    pub xmp: Option<(String, bool)>,
    pub outline_title: Option<Bytes>,
    /// `/Alt` and `/ActualText` of the structure element.
    pub structure: Option<(Bytes, Bytes)>,
    pub thumb: Option<Image>,
    pub piece_info: Option<Bytes>,
    /// `/V` and `/DV` of the form field.
    pub field: Option<(Bytes, Bytes)>,
    /// File name (a PDF string), description (a PDF string) and body of the attachment.
    pub attachment: Option<(Bytes, Bytes, Bytes)>,
    pub page_image: Option<Image>,
    /// An open action, a script, an additional action and an associated file.
    pub actions: bool,
    /// An optional-content group that is switched off: `/oc1` in the resources.
    pub hidden_layer: bool,
}

impl Doc {
    /// A document with only a page of text.
    pub fn bare() -> Doc {
        Doc {
            content: b"BT /F1 12 Tf 72 720 Td (Quarterly report) Tj ET".to_vec(),
            content_flate: false,
            font: "/Type /Font /Subtype /Type1 /BaseFont /Helvetica".into(),
            info: None,
            xmp: None,
            outline_title: None,
            structure: None,
            thumb: None,
            piece_info: None,
            field: None,
            attachment: None,
            page_image: None,
            actions: false,
            hidden_layer: false,
        }
    }

    /// A document that has every place where text can hide, all with harmless values.
    pub fn rich() -> Doc {
        Doc {
            info: Some(b"/Title (Quarterly report) /Author (Finance team)".to_vec()),
            xmp: Some((
                "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
                 xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description \
                 xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title>Quarterly \
                 report</dc:title></rdf:Description></rdf:RDF></x:xmpmeta>"
                    .into(),
                false,
            )),
            outline_title: Some(utf16_hex("Chapter one")),
            structure: Some((lit("A paragraph"), lit("A paragraph"))),
            thumb: Some(Image::rgb_flate(&[10; 12])),
            piece_info: Some(b"/Data (benign) /LastModified (D:20240101000000Z)".to_vec()),
            field: Some((lit("Jane Doe"), lit("Name"))),
            attachment: Some((
                lit("notes.txt"),
                lit("Meeting notes"),
                b"Agenda only.".to_vec(),
            )),
            page_image: Some(Image::rgb_flate(&[
                200, 30, 30, 30, 200, 30, 30, 30, 200, 90, 90, 90,
            ])),
            actions: true,
            ..Doc::bare()
        }
    }

    pub fn with_content(mut self, content: &[u8]) -> Doc {
        self.content = content.to_vec();
        self
    }

    /// Writes the file with a classic cross-reference table. The object numbers are fixed:
    /// 1 catalog, 2 pages, 3 page, 4 content, 5 font, 6 info, 7 XMP, 8 and 9 outline, 10 and
    /// 11 structure tree, 12 thumbnail, 13 piece info, 14 page image, 15 form field, 16 and 17
    /// attachment, 18 optional-content group, 19 script action.
    pub fn write(&self) -> Pdf {
        let mut pdf = Pdf::new("1.5");
        let mut catalog: Bytes = b"/Type /Catalog /Pages 2 0 R".to_vec();
        let mut page =
            String::from("/Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R");
        let mut res = String::from("/Font << /F1 5 0 R >>");
        if self.page_image.is_some() {
            res.push_str(" /XObject << /Im1 14 0 R >>");
        }
        if self.hidden_layer {
            res.push_str(" /Properties << /oc1 18 0 R >>");
            catalog
                .extend_from_slice(b" /OCProperties << /OCGs [18 0 R] /D << /OFF [18 0 R] >> >>");
        }
        page.push_str(&format!(" /Resources << {res} >>"));
        let mut trailer = String::from("/Root 1 0 R");
        if self.xmp.is_some() {
            catalog.extend_from_slice(b" /Metadata 7 0 R");
        }
        if self.outline_title.is_some() {
            catalog.extend_from_slice(b" /Outlines 8 0 R");
        }
        if self.structure.is_some() {
            catalog.extend_from_slice(b" /StructTreeRoot 10 0 R");
        }
        if self.field.is_some() {
            catalog.extend_from_slice(b" /AcroForm << /Fields [15 0 R] >>");
            page.push_str(" /Annots [15 0 R]");
        }
        if self.thumb.is_some() {
            page.push_str(" /Thumb 12 0 R");
        }
        if self.piece_info.is_some() {
            page.push_str(" /PieceInfo << /Partridge 13 0 R >>");
        }
        let mut names: Bytes = Vec::new();
        if let Some((name, ..)) = &self.attachment {
            names.extend_from_slice(b" /EmbeddedFiles << /Names [");
            names.extend_from_slice(name);
            names.extend_from_slice(b" 17 0 R] >>");
        }
        if self.actions {
            catalog.extend_from_slice(b" /OpenAction [3 0 R /Fit]");
            if self.attachment.is_some() {
                catalog.extend_from_slice(b" /AF [17 0 R]");
            }
            names.extend_from_slice(b" /JavaScript << /Names [(init) 19 0 R] >>");
            page.push_str(" /AA << /O 19 0 R >>");
        }
        if !names.is_empty() {
            catalog.extend_from_slice(b" /Names <<");
            catalog.extend_from_slice(&names);
            catalog.extend_from_slice(b" >>");
        }
        if self.info.is_some() {
            trailer.push_str(" /Info 6 0 R");
        }
        pdf.obj(1, &[b"<< ".as_slice(), &catalog, b" >>"].concat());
        pdf.obj(2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
        pdf.obj(3, format!("<< {page} >>").as_bytes());
        if self.content_flate {
            pdf.stream(4, "/Filter /FlateDecode", &flate(&self.content));
        } else {
            pdf.stream(4, "", &self.content);
        }
        pdf.obj(5, format!("<< {} >>", self.font).as_bytes());
        if let Some(info) = &self.info {
            pdf.obj(6, &[b"<< ".as_slice(), info, b" >>"].concat());
        }
        if let Some((xmp, compressed)) = &self.xmp {
            if *compressed {
                pdf.stream(
                    7,
                    "/Type /Metadata /Subtype /XML /Filter /FlateDecode",
                    &flate(xmp.as_bytes()),
                );
            } else {
                pdf.stream(7, "/Type /Metadata /Subtype /XML", xmp.as_bytes());
            }
        }
        if let Some(title) = &self.outline_title {
            pdf.obj(
                8,
                b"<< /Type /Outlines /First 9 0 R /Last 9 0 R /Count 1 >>",
            );
            pdf.obj(
                9,
                &[b"<< /Title ".as_slice(), title, b" /Parent 8 0 R >>"].concat(),
            );
        }
        if let Some((alt, actual)) = &self.structure {
            pdf.obj(10, b"<< /Type /StructTreeRoot /K 11 0 R >>");
            pdf.obj(
                11,
                &[
                    b"<< /Type /StructElem /S /P /P 10 0 R /Pg 3 0 R /Alt ".as_slice(),
                    alt,
                    b" /ActualText ",
                    actual,
                    b" >>",
                ]
                .concat(),
            );
        }
        if let Some(img) = &self.thumb {
            pdf.stream(12, &img.dict, &img.data);
        }
        if let Some(entries) = &self.piece_info {
            pdf.obj(13, &[b"<< ".as_slice(), entries, b" >>"].concat());
        }
        if let Some(img) = &self.page_image {
            pdf.stream(14, &img.dict, &img.data);
        }
        if let Some((v, dv)) = &self.field {
            pdf.obj(
                15,
                &[
                    b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (field1) /Rect [72 600 300 620] /P 3 0 R /V "
                        .as_slice(),
                    v,
                    b" /DV ",
                    dv,
                    b" >>",
                ]
                .concat(),
            );
        }
        if let Some((name, desc, body)) = &self.attachment {
            pdf.stream(16, "/Type /EmbeddedFile /Filter /FlateDecode", &flate(body));
            pdf.obj(
                17,
                &[
                    b"<< /Type /Filespec /F ".as_slice(),
                    name,
                    b" /UF ",
                    name,
                    b" /Desc ",
                    desc,
                    b" /EF << /F 16 0 R >> >>",
                ]
                .concat(),
            );
        }
        if self.hidden_layer {
            pdf.obj(18, b"<< /Type /OCG /Name (Hidden layer) >>");
        }
        if self.actions {
            pdf.obj(19, b"<< /S /JavaScript /JS (alert) >>");
        }
        pdf.xref(&trailer);
        pdf
    }

    /// The trailer entries of the file (without `/Size`).
    pub fn trailer(&self) -> String {
        if self.info.is_some() {
            "/Root 1 0 R /Info 6 0 R".into()
        } else {
            "/Root 1 0 R".into()
        }
    }

    /// The file with an incremental update that replaces the content stream (object 4). The old
    /// revision stays in the file, as an editor that saves incrementally leaves it.
    pub fn build_with_new_content(&self, new_content: &[u8], compress: bool) -> Bytes {
        let mut pdf = self.write();
        if compress {
            pdf.stream(4, "/Filter /FlateDecode", &flate(new_content));
        } else {
            pdf.stream(4, "", new_content);
        }
        pdf.xref(&self.trailer());
        pdf.finish()
    }

    /// The file with an incremental update that replaces the page image (object 14).
    pub fn build_with_new_image(&self, image: &Image) -> Bytes {
        let mut pdf = self.write();
        pdf.stream(14, &image.dict, &image.data);
        pdf.xref(&self.trailer());
        pdf.finish()
    }

    pub fn build(&self) -> Bytes {
        self.write().finish()
    }
}

/// A PDF 1.5 file with an object stream and a cross-reference stream. The catalog, pages, page,
/// font and Info dictionary are in the object stream (compressed). `info` is the content of the
/// Info dictionary.
pub fn object_stream_file(info: &[u8], content: &[u8]) -> Bytes {
    let mut pdf = Pdf::new("1.5");
    let objs: Vec<(u32, Bytes)> = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
        (
            3,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
              /Resources << /Font << /F1 5 0 R >> >> >>"
                .to_vec(),
        ),
        (
            5,
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        ),
        (7, [b"<< ".as_slice(), info, b" >>"].concat()),
    ];
    let mut header = String::new();
    let mut body: Bytes = Vec::new();
    for (n, o) in &objs {
        header.push_str(&format!("{n} {} ", body.len()));
        body.extend_from_slice(o);
        body.push(b'\n');
    }
    let first = header.len();
    let mut stm = header.into_bytes();
    stm.extend(body);
    pdf.stream(4, "", content);
    pdf.stream(
        6,
        &format!(
            "/Type /ObjStm /N {} /First {first} /Filter /FlateDecode",
            objs.len()
        ),
        &flate(&stm),
    );
    // The cross-reference stream is object 8. One entry per object 0..=8: type, field 2 (4
    // bytes), field 3 (2 bytes).
    let xref_pos = pdf.buf.len();
    let mut entries: Bytes = Vec::new();
    let mut e = |t: u8, a: u32, b: u16| {
        entries.push(t);
        entries.extend(a.to_be_bytes());
        entries.extend(b.to_be_bytes());
    };
    e(0, 0, 0xFFFF);
    // Objects 1, 2, 3 are the first three in the object stream, 5 the fourth, 7 the fifth.
    e(2, 6, 0);
    e(2, 6, 1);
    e(2, 6, 2);
    e(1, pdf.pos_of(4) as u32, 0);
    e(2, 6, 3);
    e(1, pdf.pos_of(6) as u32, 0);
    e(2, 6, 4);
    e(1, xref_pos as u32, 0);
    let dict = "/Type /XRef /Size 9 /W [1 4 2] /Root 1 0 R /Info 7 0 R";
    pdf.stream(8, dict, &entries);
    let mut out = pdf.finish();
    out.extend(format!("startxref\n{xref_pos}\n%%EOF\n").into_bytes());
    out
}

// ---------------------------------------------------------------------------------------------

/// Where the test files are written.
pub struct Dir(pub tempfile::TempDir);

impl Dir {
    pub fn new() -> Dir {
        Dir(tempfile::tempdir().unwrap())
    }

    pub fn put(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let p = self.0.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }
}

/// Encrypts a file with qpdf (an empty user password, so any reader opens it). The bytes of text
/// in it are not in the file any more; only a reader that decrypts can find them.
pub fn encrypt(dir: &Dir, bytes: &[u8]) -> Bytes {
    let input = dir.put("plain-for-encrypt.pdf", bytes);
    let out = dir.0.path().join("encrypted.pdf");
    let status = Command::new("qpdf")
        .args(["--encrypt", "", "owner-secret", "256", "--"])
        .arg(&input)
        .arg(&out)
        .status()
        .expect("qpdf runs");
    assert!(status.success(), "qpdf --encrypt failed");
    std::fs::read(out).unwrap()
}
