//! Test support: small PDF files generated in code (so no binary fixtures live in the repo),
//! including a damaged one and a password-protected one, and a way to find the PDFium folder.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use md5::{Digest, Md5};

/// One page of a generated document.
pub struct PageSpec {
    pub width: u32,
    pub height: u32,
    /// Page content stream. `None` gives a blank page without a content stream.
    pub content: Option<String>,
    /// More entries for the page dictionary, such as `/Annots [..]` or `/Rotate 90`.
    pub extra: String,
}

/// Content of a page that has a title, a block of text and a coloured bar at the bottom.
pub fn demo_content(number: usize, width: u32, height: u32) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "BT /F1 40 Tf 48 {} Td (Page {number}) Tj ET",
        height - 90
    );
    let _ = writeln!(s, "BT /F1 11 Tf 48 {} Td 14 TL", height - 130);
    for line in 0..30 {
        let _ = writeln!(
            s,
            "(Line {line} of page {number}: the quick brown fox jumps over the lazy dog.) Tj T*"
        );
    }
    let _ = writeln!(s, "ET");
    let _ = writeln!(
        s,
        "0.1 0.3 0.8 rg 36 36 {} 24 re f",
        width.saturating_sub(72)
    );
    s
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Assembles objects 1.. into a file with a correct cross-reference table.
/// `objects[i]` is the body of object `i + 1`.
fn assemble(objects: &[Vec<u8>], trailer_extra: &str) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R {trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// Builds a document from page specs. `catalog_extra` goes into the catalog dictionary,
/// `trailer_extra` into the trailer, and `extra_objects` are appended after the pages.
fn build(
    pages: &[PageSpec],
    catalog_extra: &str,
    trailer_extra: &str,
    extra_objects: Vec<Vec<u8>>,
) -> Vec<u8> {
    // 1 catalog, 2 page tree, 3 font, then per page: the page and (optionally) its content.
    let mut objects: Vec<Vec<u8>> = Vec::new();
    let mut kids = String::new();
    let mut body_objects: Vec<Vec<u8>> = Vec::new();
    let mut next = 4;
    for spec in pages {
        let page_id = next;
        next += 1;
        let _ = write!(kids, "{page_id} 0 R ");
        let contents_ref = if spec.content.is_some() {
            let id = next;
            next += 1;
            format!("/Contents {id} 0 R ")
        } else {
            String::new()
        };
        body_objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] {contents_ref}/Resources << /Font << /F1 3 0 R >> >> {} >>",
                spec.width, spec.height, spec.extra
            )
            .into_bytes(),
        );
        if let Some(text) = &spec.content {
            let mut stream = format!("<< /Length {} >>\nstream\n", text.len()).into_bytes();
            stream.extend_from_slice(text.as_bytes());
            stream.extend_from_slice(b"\nendstream");
            body_objects.push(stream);
        }
    }

    objects.push(format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>").into_bytes());
    objects.push(format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()).into_bytes());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    objects.extend(body_objects);
    objects.extend(extra_objects);
    assemble(&objects, trailer_extra)
}

/// A valid document of `pages` Letter-size pages with text and a bar on each.
pub fn simple_pdf(pages: usize) -> Vec<u8> {
    let specs: Vec<PageSpec> = (1..=pages)
        .map(|n| PageSpec {
            width: 612,
            height: 792,
            content: Some(demo_content(n, 612, 792)),
            extra: String::new(),
        })
        .collect();
    build(&specs, "", "", Vec::new())
}

/// A valid document whose pages differ in size: Letter, A4 landscape, a small square, a tall strip.
pub fn mixed_size_pdf() -> Vec<u8> {
    let sizes = [(612, 792), (842, 595), (300, 300), (200, 1000)];
    let specs: Vec<PageSpec> = sizes
        .iter()
        .enumerate()
        .map(|(i, &(w, h))| PageSpec {
            width: w,
            height: h,
            content: Some(demo_content(i + 1, w, h)),
            extra: String::new(),
        })
        .collect();
    build(&specs, "", "", Vec::new())
}

/// A valid document with a document-level JavaScript open action. The engine must open and draw
/// it like any other file, and must not run the script.
pub fn javascript_pdf() -> Vec<u8> {
    let specs = [PageSpec {
        width: 612,
        height: 792,
        content: Some(demo_content(1, 612, 792)),
        extra: String::new(),
    }];
    build(
        &specs,
        "/OpenAction << /S /JavaScript /JS (app.alert\\(\"hello\"\\);) >> \
         /Names << /JavaScript << /Names [(init) << /S /JavaScript /JS (this.print\\(\\);) >>] >> >>",
        "",
        Vec::new(),
    )
}

/// Wraps a stream body into an object.
fn stream_object(dict: &str, data: &str) -> Vec<u8> {
    let mut out = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    out.extend_from_slice(data.as_bytes());
    out.extend_from_slice(b"\nendstream");
    out
}

/// A document with three pages that has everything the reader looks at: text to search and copy
/// (page 1), a rotated page (page 2), an outline, links of several kinds, and an attachment.
///
/// Page 1 holds "Hello Partridge" as a title and three lines: "A needle in a haystack.",
/// "Another NEEDLE here and quick" and "brown fox". Page 2 is turned a quarter turn. The outline
/// is "Chapter 1" (page 1) with "Section 1.1" (page 2) inside, "External" (a web address, no page)
/// and "Chapter 3" (page 3, reached with a GoTo action). The links on page 1 are, from the top:
/// a web link, a `javascript:` link, a link to page 3, and a link that launches a program. The
/// file has an embedded file and a file attachment annotation, which no code may ever open.
pub fn features_pdf() -> Vec<u8> {
    let page1 = "BT /F1 24 Tf 72 740 Td (Hello Partridge) Tj ET\n\
BT /F1 12 Tf 72 400 Td 16 TL (A needle in a haystack.) Tj T* (Another NEEDLE here and quick) Tj T* (brown fox) Tj ET\n"
        .to_string();
    let specs = [
        PageSpec {
            width: 612,
            height: 792,
            content: Some(page1),
            extra: "/Annots [15 0 R 16 0 R 17 0 R 18 0 R 19 0 R]".into(),
        },
        PageSpec {
            width: 612,
            height: 792,
            content: Some("BT /F1 24 Tf 72 740 Td (Rotated page) Tj ET\n".to_string()),
            extra: "/Rotate 90".into(),
        },
        PageSpec {
            width: 612,
            height: 792,
            content: Some(demo_content(3, 612, 792)),
            extra: String::new(),
        },
    ];
    // Object numbers: 1 catalog, 2 tree, 3 font, 4..9 the pages and their content.
    let extras: Vec<Vec<u8>> = vec![
        // 10: outline root
        b"<< /Type /Outlines /First 11 0 R /Last 14 0 R /Count 4 >>".to_vec(),
        // 11: Chapter 1, with a child
        b"<< /Title (Chapter 1) /Parent 10 0 R /Next 13 0 R /First 12 0 R /Last 12 0 R /Count 1 /Dest [4 0 R /Fit] >>".to_vec(),
        // 12: Section 1.1 on page 2
        b"<< /Title (Section 1.1) /Parent 11 0 R /Dest [6 0 R /Fit] >>".to_vec(),
        // 13: an outline entry that opens a web address: it must not get a page
        b"<< /Title (External) /Parent 10 0 R /Prev 11 0 R /Next 14 0 R /A << /S /URI /URI (https://example.com/outline) >> >>".to_vec(),
        // 14: Chapter 3, reached with a GoTo action
        b"<< /Title (Chapter 3) /Parent 10 0 R /Prev 13 0 R /A << /S /GoTo /D [8 0 R /Fit] >> >>".to_vec(),
        // 15..18: links
        b"<< /Type /Annot /Subtype /Link /Rect [72 700 172 720] /Border [0 0 0] /A << /S /URI /URI (https://example.com/page) >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /Link /Rect [72 650 172 670] /Border [0 0 0] /A << /S /URI /URI (javascript:alert\\(1\\)) >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /Link /Rect [72 600 172 620] /Border [0 0 0] /A << /S /GoTo /D [8 0 R /Fit] >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /Link /Rect [72 550 172 570] /Border [0 0 0] /A << /S /Launch /F (calc.exe) >> >>".to_vec(),
        // 19: a file attachment annotation
        b"<< /Type /Annot /Subtype /FileAttachment /Rect [300 700 320 720] /FS 20 0 R /Name /Paperclip >>".to_vec(),
        // 20: the file specification, 21: the embedded file
        b"<< /Type /Filespec /F (evil.exe) /UF (evil.exe) /EF << /F 21 0 R >> >>".to_vec(),
        stream_object("/Type /EmbeddedFile", "MZ not really a program"),
    ];
    build(
        &specs,
        "/Outlines 10 0 R /Names << /EmbeddedFiles << /Names [(evil.exe) 20 0 R] >> >>",
        "",
        extras,
    )
}

/// Letters of the right-to-left test page, in reading order: the first ten letters of the Hebrew
/// alphabet (U+05D0 to U+05D9).
pub const RTL_LINE: &str = "\u{5d0}\u{5d1}\u{5d2}\u{5d3}\u{5d4}\u{5d5}\u{5d6}\u{5d7}\u{5d8}\u{5d9}";

/// One page with two lines of right-to-left text, laid out the way such text is drawn: the glyphs
/// are placed from the left to the right of the line, so the last letter in reading order comes
/// first in the content. The font maps its codes to Hebrew letters with a `ToUnicode` table, so
/// PDFium has to put the letters back in reading order, with the first letter at the right end.
///
/// Line 1 (the top line) reads `RTL_LINE`, ten letters. Line 2 reads U+05D5 to U+05D9, the last
/// five letters of the first line.
pub fn rtl_pdf() -> Vec<u8> {
    // Code 0x41 + i is the i-th glyph from the left; it stands for the letter U+05D9 - i.
    let map: String = (0..10)
        .map(|i| format!("<{:02X}> <{:04X}>\n", 0x41 + i, 0x5d9 - i))
        .collect();
    let cmap = format!(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
1 begincodespacerange\n<00> <FF>\nendcodespacerange\n\
10 beginbfchar\n{map}endbfchar\nendcmap\nCMapName currentdict /CMap defineresource pop\nend\nend"
    );
    let content =
        "BT /F1 18 Tf 300 700 Td (ABCDEFGHIJ) Tj ET\nBT /F1 18 Tf 300 660 Td (ABCDE) Tj ET\n";
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /ToUnicode 6 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>".to_vec(),
        stream_object("", content),
        stream_object("", &cmap),
    ];
    assemble(&objects, "")
}

/// A page-free helper: three plain pages and an outline built by `outline_objects`, which returns
/// the objects from number 10 (the outline root) on. Page 1 is object 4, page 2 is 6, page 3 is 8.
fn plain_pdf_with_outline(outline_objects: Vec<Vec<u8>>, annots: &str) -> Vec<u8> {
    let specs: Vec<PageSpec> = (1..=3)
        .map(|n| PageSpec {
            width: 612,
            height: 792,
            content: Some(demo_content(n, 612, 792)),
            extra: if n == 1 {
                annots.to_string()
            } else {
                String::new()
            },
        })
        .collect();
    build(&specs, "/Outlines 10 0 R", "", outline_objects)
}

/// An outline that loops: entry 11 is its own next sibling and its own first child. A reader that
/// walks it without keeping track of what it has seen never finishes.
pub fn outline_loop_pdf() -> Vec<u8> {
    plain_pdf_with_outline(
        vec![
            b"<< /Type /Outlines /First 11 0 R /Last 11 0 R /Count 1 >>".to_vec(),
            b"<< /Title (Loop) /Parent 10 0 R /Next 11 0 R /First 11 0 R /Last 11 0 R /Count 1 /Dest [4 0 R /Fit] >>".to_vec(),
        ],
        "",
    )
}

/// An outline with `count` entries on the top level.
pub fn outline_wide_pdf(count: usize) -> Vec<u8> {
    let mut objects = vec![format!(
        "<< /Type /Outlines /First 11 0 R /Last {} 0 R /Count {count} >>",
        10 + count
    )
    .into_bytes()];
    for i in 0..count {
        let id = 11 + i;
        let mut dict = format!(
            "<< /Title (Item {i}) /Parent 10 0 R /Dest [{} 0 R /Fit]",
            4 + 2 * (i % 3)
        );
        if i > 0 {
            let _ = write!(dict, " /Prev {} 0 R", id - 1);
        }
        if i + 1 < count {
            let _ = write!(dict, " /Next {} 0 R", id + 1);
        }
        dict.push_str(" >>");
        objects.push(dict.into_bytes());
    }
    plain_pdf_with_outline(objects, "")
}

/// An outline where every entry is the only child of the one before, `depth` levels deep.
pub fn outline_deep_pdf(depth: usize) -> Vec<u8> {
    let mut objects =
        vec![
            format!("<< /Type /Outlines /First 11 0 R /Last 11 0 R /Count {depth} >>").into_bytes(),
        ];
    for i in 0..depth {
        let id = 11 + i;
        let parent = if i == 0 { 10 } else { id - 1 };
        let mut dict = format!("<< /Title (Level {i}) /Parent {parent} 0 R /Dest [4 0 R /Fit]");
        if i + 1 < depth {
            let _ = write!(dict, " /First {0} 0 R /Last {0} 0 R", id + 1);
        }
        dict.push_str(" >>");
        objects.push(dict.into_bytes());
    }
    plain_pdf_with_outline(objects, "")
}

/// A first page with `count` web links, to test the limit on links per page.
pub fn many_links_pdf(count: usize) -> Vec<u8> {
    // Object 10 is a placeholder outline root; the link annotations follow from 11.
    let mut objects = vec![b"<< /Type /Outlines /Count 0 >>".to_vec()];
    let mut refs = String::new();
    for i in 0..count {
        let _ = write!(refs, "{} 0 R ", 11 + i);
        objects.push(
            format!(
                "<< /Type /Annot /Subtype /Link /Rect [10 10 20 20] /Border [0 0 0] /A << /S /URI /URI (https://example.com/{i}) >> >>"
            )
            .into_bytes(),
        );
    }
    plain_pdf_with_outline(objects, &format!("/Annots [{refs}]"))
}

/// The fonts of `fonts_pdf`, one line of text each from the top of the page down: the name the PDF
/// gives (none of the fonts is embedded) and the kind of font object.
pub const FONT_LINES: &[(&str, &str)] = &[
    ("Helvetica", "Type1"),
    ("Helvetica-Bold", "Type1"),
    ("Times-Roman", "Type1"),
    ("Times-BoldItalic", "Type1"),
    ("Courier", "Type1"),
    ("Courier-Oblique", "Type1"),
    ("Arial", "TrueType"),
    ("Arial-BoldMT", "TrueType"),
    ("TimesNewRomanPSMT", "TrueType"),
    ("Calibri", "TrueType"),
    ("Verdana", "TrueType"),
    ("Georgia-Italic", "TrueType"),
    ("NoSuchFontAnywhere", "Type1"),
];

/// The text of every line of `fonts_pdf`.
pub const FONT_LINE_TEXT: &str = "Sphinx of black quartz judge my vow";

/// Where the baseline of line `i` of `fonts_pdf` is, in points from the bottom of the page.
pub fn font_line_baseline(i: usize) -> f32 {
    740.0 - 44.0 * i as f32
}

/// One Letter page with a line of text in each of the fonts of `FONT_LINES`. No font is embedded,
/// so the reader has to find a face for each name itself: the case in which a reader looks at the
/// fonts of the system, which a sandboxed engine cannot do.
pub fn fonts_pdf() -> Vec<u8> {
    let mut content = String::new();
    for i in 0..FONT_LINES.len() {
        let _ = writeln!(
            content,
            "BT /F{} 20 Tf 40 {} Td ({FONT_LINE_TEXT}) Tj ET",
            i + 1,
            font_line_baseline(i)
        );
    }
    let mut fonts = String::new();
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
    ];
    let mut font_objects: Vec<Vec<u8>> = Vec::new();
    for (i, (name, kind)) in FONT_LINES.iter().enumerate() {
        let _ = write!(fonts, "/F{} {} 0 R ", i + 1, 5 + i);
        let encoding = if *kind == "TrueType" {
            " /Encoding /WinAnsiEncoding"
        } else {
            ""
        };
        font_objects.push(
            format!("<< /Type /Font /Subtype /{kind} /BaseFont /{name}{encoding} >>").into_bytes(),
        );
    }
    objects.push(
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << {fonts}>> >> >>"
        )
        .into_bytes(),
    );
    objects.push(stream_object("", &content));
    objects.extend(font_objects);
    assemble(&objects, "")
}

/// One page of Japanese text in a font that is not embedded (a Shift-JIS encoded CID font named
/// MS-Mincho) next to a line of Latin text. Without a CJK font on hand the Japanese shows as
/// blanks; the engine must neither crash nor hang.
pub fn cjk_pdf() -> Vec<u8> {
    let content = "BT /F1 28 Tf 50 700 Td <93FA967B8CEA> Tj ET\n\
BT /F2 20 Tf 50 640 Td (Latin text next to it) Tj ET\n";
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 7 0 R >> >> >>".to_vec(),
        stream_object("", content),
        b"<< /Type /Font /Subtype /Type0 /BaseFont /MS-Mincho /Encoding /90ms-RKSJ-H /DescendantFonts [6 0 R] >>".to_vec(),
        b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /MS-Mincho /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 2 >> /DW 1000 >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    assemble(&objects, "")
}

/// The scripts of `script_pdf`, by the names `partridge_protocol::FontScript` has.
pub const SCRIPT_NAMES: [&str; 7] = [
    "japanese",
    "korean",
    "chinese-simplified",
    "chinese-traditional",
    "arabic",
    "hebrew",
    "thai",
];

/// One Letter page with a line of large text in a script that needs a font the page does not carry,
/// in a font that is not embedded. The CJK pages name their font the way such files do and use
/// the predefined encodings of their collections (Japan1, GB1, CNS1, Korea1); the others use an
/// identity encoding with a table of the characters. The text is at the top of the page, at 48
/// points, from 700 points up from the bottom.
pub fn script_pdf(script: &str) -> Vec<u8> {
    script_pdf_with_font(script, default_font_name(script))
}

/// The font name `script_pdf` gives the text of a script.
pub fn default_font_name(script: &str) -> &'static str {
    match script {
        "japanese" => "MS-Mincho",
        "korean" => "Batang",
        "chinese-simplified" => "SimSun",
        "chinese-traditional" => "MingLiU",
        "arabic" => "TraditionalArabic",
        "hebrew" => "David",
        "thai" => "Angsana",
        other => panic!("no page for the script {other}"),
    }
}

/// As `script_pdf`, with the text in an unembedded font of another name (as documents made on
/// other systems and by other programs name them). A name with a space is written the way a PDF
/// writes it (`#20`).
pub fn script_pdf_with_font(script: &str, name: &str) -> Vec<u8> {
    let name = &name.replace(' ', "#20");
    let (font, descendant, content) = match script {
        "japanese" => cid_font(name, "90ms-RKSJ-H", "Japan1", 2, "93FA967B8CEA"),
        "korean" => cid_font(name, "KSCms-UHC-H", "Korea1", 1, "C7D1B1B9BEEE"),
        "chinese-simplified" => cid_font(name, "GBK-EUC-H", "GB1", 2, "D6D0CEC4"),
        "chinese-traditional" => cid_font(name, "ETen-B5-H", "CNS1", 0, "A4A4A4E5"),
        "arabic" => identity_font(name, &[0x645, 0x631, 0x62D, 0x628, 0x627]),
        "hebrew" => identity_font(name, &[0x5E9, 0x5DC, 0x5D5, 0x5DD]),
        "thai" => identity_font(name, &[0xE2A, 0xE27, 0xE31, 0xE2A, 0xE14, 0xE35]),
        other => panic!("no page for the script {other}"),
    };
    let page_content = format!(
        "BT /F1 48 Tf 60 700 Td <{content}> Tj ET
"
    );
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream_object("", &page_content),
        font.into_bytes(),
        descendant.into_bytes(),
    ];
    if script == "arabic" || script == "hebrew" || script == "thai" {
        objects.push(to_unicode(script).into_bytes());
    }
    assemble(&objects, "")
}

/// One Letter page with `fonts` unembedded Japanese fonts of different names (`CJKFont0001` ...),
/// each with a line of text in the page. A reader that serves a system font for each name holds a
/// copy of that font for each of them, in a file of a few hundred bytes per font.
pub fn many_fonts_pdf(fonts: usize) -> Vec<u8> {
    let mut resources = String::new();
    let mut content = String::new();
    for i in 0..fonts {
        resources.push_str(&format!("/F{i} {} 0 R ", 5 + 2 * i));
        content.push_str(&format!(
            "BT /F{i} 4 Tf 20 {} Td <93FA967B8CEA> Tj ET
",
            780 - 5 * i as i32
        ));
    }
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << {resources}>> >> >>").into_bytes(),
        stream_object("", &content),
    ];
    for i in 0..fonts {
        let (font, descendant, _) = cid_font(
            &format!("CJKFont{:04}", i + 1),
            "90ms-RKSJ-H",
            "Japan1",
            2,
            "",
        );
        // The font object is object 5 + 2i and its descendant 6 + 2i.
        objects.push(font.replace("[6 0 R]", &format!("[{} 0 R]", 6 + 2 * i)).into_bytes());
        objects.push(descendant.into_bytes());
    }
    assemble(&objects, "")
}

/// A Type 0 font with a predefined encoding of an Adobe collection, not embedded: the font object,
/// its descendant, and the hex string of the text in that encoding.
fn cid_font(
    name: &str,
    encoding: &str,
    ordering: &str,
    supplement: u32,
    hex: &str,
) -> (String, String, String) {
    (
        format!("<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /{encoding} /DescendantFonts [6 0 R] >>"),
        format!("<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{name} /CIDSystemInfo << /Registry (Adobe) /Ordering ({ordering}) /Supplement {supplement} >> /DW 1000 >>"),
        hex.to_string(),
    )
}

/// A Type 0 font with identity encoding, not embedded, whose character codes 1, 2, 3 ... are the
/// letters of the text (a `ToUnicode` table says which). The text is the codes in hex.
fn identity_font(name: &str, letters: &[u32]) -> (String, String, String) {
    let hex: String = (1..=letters.len()).map(|i| format!("{i:04X}")).collect();
    (
        format!("<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /Identity-H /DescendantFonts [6 0 R] /ToUnicode 7 0 R >>"),
        format!("<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{name} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /DW 1000 >>"),
        hex,
    )
}

/// The `ToUnicode` table of `identity_font` for the text of a script.
fn to_unicode(script: &str) -> String {
    let letters: &[u32] = match script {
        "arabic" => &[0x645, 0x631, 0x62D, 0x628, 0x627],
        "hebrew" => &[0x5E9, 0x5DC, 0x5D5, 0x5DD],
        _ => &[0xE2A, 0xE27, 0xE31, 0xE2A, 0xE14, 0xE35],
    };
    let map: String = letters
        .iter()
        .enumerate()
        .map(|(i, u)| {
            format!(
                "<{:04X}> <{:04X}>
",
                i + 1,
                u
            )
        })
        .collect();
    let cmap = format!(
        "/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def
/CMapName /Adobe-Identity-UCS def
/CMapType 2 def
1 begincodespacerange
<0000> <FFFF>
endcodespacerange
{} beginbfchar
{map}endbfchar
endcmap
CMapName currentdict /CMap defineresource pop
end
end",
        letters.len()
    );
    String::from_utf8(stream_object("", &cmap)).unwrap()
}

/// Starts like a PDF but is not one: the structure is cut off in the middle of the catalog.
pub fn corrupt_pdf() -> Vec<u8> {
    b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R\n".to_vec()
}

/// Not a PDF at all.
pub fn not_a_pdf() -> Vec<u8> {
    b"This is a plain text file, not a PDF.\n".repeat(20)
}

/// A real document with the second half cut off. Readers may repair it or refuse it, but they
/// must not crash.
pub fn truncated_pdf() -> Vec<u8> {
    let full = simple_pdf(5);
    full[..full.len() * 6 / 10].to_vec()
}

const PADDING: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s: Vec<u8> = (0..=255).collect();
    let mut j = 0usize;
    for i in 0..256 {
        j = (j + s[i] as usize + key[i % key.len()] as usize) % 256;
        s.swap(i, j);
    }
    let (mut i, mut j) = (0usize, 0usize);
    data.iter()
        .map(|b| {
            i = (i + 1) % 256;
            j = (j + s[i] as usize) % 256;
            s.swap(i, j);
            b ^ s[(s[i] as usize + s[j] as usize) % 256]
        })
        .collect()
}

/// A document protected with the standard security handler (RC4, 40 bit, revision 2) that needs
/// `user_password` to open. The pages are blank so nothing has to be encrypted besides the
/// handler's own entries.
pub fn encrypted_pdf(user_password: &str, pages: usize) -> Vec<u8> {
    let mut padded = [0u8; 32];
    let pw = user_password.as_bytes();
    let n = pw.len().min(32);
    padded[..n].copy_from_slice(&pw[..n]);
    padded[n..].copy_from_slice(&PADDING[..32 - n]);

    // The owner entry only matters for the owner password, which these files do not use.
    let owner: Vec<u8> = (0u8..32)
        .map(|i| i.wrapping_mul(7).wrapping_add(3))
        .collect();
    let permissions: i32 = -4;
    let id: [u8; 16] = *b"partridge-test-1";

    // Algorithm 2 for revision 2: the file key.
    let mut md5 = Md5::new();
    md5.update(padded);
    md5.update(&owner);
    md5.update(permissions.to_le_bytes());
    md5.update(id);
    let key = md5.finalize()[..5].to_vec();
    // Algorithm 4: the user entry is the padding string encrypted with the file key.
    let user = rc4(&key, &PADDING);

    let encrypt_object = format!(
        "<< /Filter /Standard /V 1 /R 2 /P {permissions} /O <{}> /U <{}> >>",
        hex(&owner),
        hex(&user)
    )
    .into_bytes();
    // Object numbers: 1 catalog, 2 tree, 3 font, one per page from 4, then the encrypt dictionary.
    let encrypt_id = 4 + pages;
    let specs: Vec<PageSpec> = (0..pages)
        .map(|_| PageSpec {
            width: 612,
            height: 792,
            content: None,
            extra: String::new(),
        })
        .collect();
    build(
        &specs,
        "",
        &format!("/Encrypt {encrypt_id} 0 R /ID [<{0}> <{0}>]", hex(&id)),
        vec![encrypt_object],
    )
}

/// Folder name used for a platform in `vendor/pdfium` and in the pinned PDFium list.
pub fn pdfium_platform_id() -> &'static str {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "win-x64"
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        "win-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "mac-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "mac-x64"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "linux-arm64"
    } else {
        "linux-x64"
    }
}

/// The folder with the PDFium library for tests: `PARTRIDGE_PDFIUM_DIR`, or the folder that
/// `node scripts/fetch-pdfium.mjs` fills.
pub fn pdfium_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("PARTRIDGE_PDFIUM_DIR") {
        return PathBuf::from(dir);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/pdfium")
        .join(pdfium_platform_id())
}

/// A file in the temp folder that is removed again when this value is dropped.
pub struct TempFile {
    path: PathBuf,
}

impl TempFile {
    pub fn new(name: &str, bytes: &[u8]) -> TempFile {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = format!(
            "partridge-test-{}-{}-{name}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::write(&path, bytes).expect("write temp file");
        TempFile { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn path_str(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn simple_pdf_has_the_right_structure() {
        let pdf = simple_pdf(3);
        let t = text(&pdf);
        assert!(t.starts_with("%PDF-1.4"));
        assert!(t.contains("/Count 3"));
        assert!(t.trim_end().ends_with("%%EOF"));
        // The startxref offset must point at the xref keyword.
        let start: usize = t
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(pdf[start..].starts_with(b"xref"));
    }

    #[test]
    fn xref_offsets_point_at_their_objects() {
        let pdf = simple_pdf(2);
        let t = text(&pdf);
        let xref = t.find("xref\n").unwrap();
        for (i, line) in t[xref..].lines().skip(3).take(7).enumerate() {
            let off: usize = line[..10].parse().unwrap();
            let expect = format!("{} 0 obj", i + 1);
            assert!(text(&pdf[off..]).starts_with(&expect), "object {}", i + 1);
        }
    }

    #[test]
    fn rc4_matches_a_known_vector() {
        // RFC 6229 style check: key "Key", plaintext "Plaintext".
        let out = rc4(b"Key", b"Plaintext");
        assert_eq!(hex(&out), "bbf316e8d940af0ad3");
    }

    #[test]
    fn the_damaged_files_are_not_valid() {
        assert!(!text(&corrupt_pdf()).contains("%%EOF"));
        assert!(!text(&not_a_pdf()).starts_with("%PDF"));
        assert!(truncated_pdf().len() < simple_pdf(5).len());
    }

    #[test]
    fn the_feature_documents_have_the_right_structure() {
        for pdf in [
            features_pdf(),
            outline_loop_pdf(),
            outline_wide_pdf(30),
            outline_deep_pdf(30),
            many_links_pdf(30),
        ] {
            let t = text(&pdf);
            assert!(t.starts_with("%PDF-1.4"));
            assert!(t.trim_end().ends_with("%%EOF"));
            let start: usize = t
                .rsplit("startxref\n")
                .next()
                .unwrap()
                .lines()
                .next()
                .unwrap()
                .parse()
                .unwrap();
            assert!(pdf[start..].starts_with(b"xref"));
        }
        let t = text(&features_pdf());
        assert!(t.contains("/Outlines 10 0 R"));
        assert!(t.contains("/Subtype /FileAttachment"));
        assert!(t.contains("/Rotate 90"));
        assert_eq!(
            text(&many_links_pdf(7)).matches("/Subtype /Link").count(),
            7
        );
    }

    #[test]
    fn temp_files_are_removed() {
        let path;
        {
            let f = TempFile::new("x.pdf", b"x");
            path = f.path().to_path_buf();
            assert!(path.exists());
        }
        assert!(!path.exists());
    }
}
