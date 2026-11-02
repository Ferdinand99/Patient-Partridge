//! Documents for the tests of the page tools. Each page carries things that a page tool must keep
//! (its text, bookmarks, links, a form field, a note, a tag) and, in the same places, secrets that
//! a deleted page must take with it. The streams are written with the ASCIIHex filter, which PDFium
//! copies as it finds it, so that the bytes of a saved file can be searched for them (a plain
//! stream would be compressed by the save and could not be found either way).

use std::fmt::Write as _;

use super::{assemble, stream_object};

/// The text drawn on page `i` (counted from 0).
pub fn page_text(i: usize) -> String {
    format!("PAGE-TEXT-{i}-SECRET")
}

/// Everything on page `i` that lives in the page only: its text, a picture, a note, a form field
/// value and the name of a font. A saved file that has none of them has lost the page. The names
/// and strings are plain in the file; the text and the picture are in streams, written in the
/// hexadecimal form of the ASCIIHex filter (see [`hex_form`]).
pub fn page_secrets(i: usize) -> Vec<Secret> {
    vec![
        Secret {
            what: "page text",
            plain: hex_form(&page_text(i)),
        },
        Secret {
            what: "picture",
            plain: hex_form(&format!("IMAGE-DATA-{i}-SECRET")),
        },
        Secret {
            what: "note",
            plain: format!("NOTE-{i}-SECRET"),
        },
        Secret {
            what: "form field value",
            plain: format!("VALUE-{i}-SECRET"),
        },
        Secret {
            what: "font name",
            plain: format!("FontSecret{i}"),
        },
    ]
}

/// A string to look for in the bytes of a file, and what it stands for.
pub struct Secret {
    pub what: &'static str,
    pub plain: String,
}

/// How the ASCIIHex filter writes a text: two hexadecimal digits for each byte.
pub fn hex_form(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02X}")).collect()
}

/// True when `needle` is somewhere in `hay`.
pub fn contains(hay: &[u8], needle: &str) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

fn hex_stream(dict: &str, text: &str) -> Vec<u8> {
    stream_object(
        &format!("{dict} /Filter /ASCIIHexDecode"),
        &format!("{}>", hex_form(text)),
    )
}

const PER_PAGE: usize = 9;
const FIRST: usize = 9;

/// The object number of the first object that belongs to page `i`.
fn base(i: usize) -> usize {
    FIRST + i * PER_PAGE
}

/// A document of `pages` Letter pages that has, besides the pages: an information dictionary, an
/// outline with one entry for each page, a named destination for each page, a form with one text
/// field on each page, a tag for each page, and on each page its text, a picture, a note, a link
/// to the next page (the last page links to the first) and a font of its own.
///
/// Object numbers: 1 catalog, 2 page tree, 3 the font all pages share, 4 information, 5 outline
/// root, 6 named destinations, 7 form, 8 structure tree root, then nine for each page: the page,
/// its content, its picture, its note, its link, its form field, its font, its outline entry and
/// its tag.
pub fn marked_pdf(pages: usize) -> Vec<u8> {
    assert!(pages >= 1);
    let mut objects: Vec<Vec<u8>> = Vec::new();
    let page_ref = |i: usize| format!("{} 0 R", base(i));

    let mut kids = String::new();
    let mut fields = String::new();
    let mut dests = String::new();
    let mut tags = String::new();
    for i in 0..pages {
        let _ = write!(kids, "{} ", page_ref(i));
        let _ = write!(fields, "{} 0 R ", base(i) + 5);
        let _ = write!(dests, "(dest{i}) [{} /Fit] ", page_ref(i));
        let _ = write!(tags, "{} 0 R ", base(i) + 8);
    }
    objects.push(
        "<< /Type /Catalog /Pages 2 0 R /Outlines 5 0 R /Names << /Dests 6 0 R >> /AcroForm 7 0 R \
         /StructTreeRoot 8 0 R /ViewerPreferences << /HideToolbar true >> >>"
            .to_string()
            .into_bytes(),
    );
    objects.push(format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>").into_bytes());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    objects.push(b"<< /Title (INFO-TITLE-MARKER) /Author (INFO-AUTHOR-MARKER) >>".to_vec());
    objects.push(
        format!(
            "<< /Type /Outlines /First {} 0 R /Last {} 0 R /Count {pages} >>",
            base(0) + 7,
            base(pages - 1) + 7
        )
        .into_bytes(),
    );
    objects.push(format!("<< /Names [{dests}] >>").into_bytes());
    objects.push(
        format!("<< /Fields [{fields}] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >>")
            .into_bytes(),
    );
    objects.push(format!("<< /Type /StructTreeRoot /K [{tags}] >>").into_bytes());

    for i in 0..pages {
        let b = base(i);
        let next = (i + 1) % pages;
        // The page.
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R \
                 /Resources << /Font << /F1 3 0 R /F2 {} 0 R >> /XObject << /Im1 {} 0 R >> >> \
                 /Annots [{} 0 R {} 0 R {} 0 R] /StructParents {i} >>",
                b + 1,
                b + 6,
                b + 2,
                b + 3,
                b + 4,
                b + 5,
            )
            .into_bytes(),
        );
        // Its content: the text, in the font of its own, and the picture.
        objects.push(hex_stream(
            "",
            &format!(
                "BT /F2 18 Tf 72 700 Td ({}) Tj ET BT /F1 12 Tf 72 650 Td (second line of page {i}) Tj ET \
                 q 50 0 0 50 72 500 cm /Im1 Do Q",
                page_text(i)
            ),
        ));
        // Its picture.
        objects.push(hex_stream(
            "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8",
            &format!("IMAGE-DATA-{i}-SECRET"),
        ));
        // Its note.
        objects.push(
            format!(
                "<< /Type /Annot /Subtype /Text /Rect [400 700 420 720] /Contents (NOTE-{i}-SECRET) /P {} >>",
                page_ref(i)
            )
            .into_bytes(),
        );
        // Its link, to the next page.
        objects.push(
            format!(
                "<< /Type /Annot /Subtype /Link /Rect [72 600 172 620] /Border [0 0 0] /Dest [{} /Fit] >>",
                page_ref(next)
            )
            .into_bytes(),
        );
        // Its form field: one object that is the field and its widget.
        objects.push(
            format!(
                "<< /Type /Annot /Subtype /Widget /FT /Tx /T (field{i}) /V (VALUE-{i}-SECRET) \
                 /Rect [72 400 272 424] /F 4 /P {} /DA (/Helv 12 Tf 0 g) >>",
                page_ref(i)
            )
            .into_bytes(),
        );
        // Its font.
        objects.push(
            format!("<< /Type /Font /Subtype /Type1 /BaseFont /FontSecret{i} >>").into_bytes(),
        );
        // Its outline entry.
        let mut item = format!(
            "<< /Title (Chapter {i}) /Parent 5 0 R /Dest [{} /Fit]",
            page_ref(i)
        );
        if i > 0 {
            let _ = write!(item, " /Prev {} 0 R", base(i - 1) + 7);
        }
        if i + 1 < pages {
            let _ = write!(item, " /Next {} 0 R", base(i + 1) + 7);
        }
        item.push_str(" >>");
        objects.push(item.into_bytes());
        // Its tag.
        objects.push(
            format!(
                "<< /Type /StructElem /S /P /P 8 0 R /Pg {} /K {i} >>",
                page_ref(i)
            )
            .into_bytes(),
        );
    }
    assemble(&objects, "/Info 4 0 R")
}

/// `n` Letter pages with no content: the quickest way to a document with a given number of pages.
pub fn blank_pages_pdf(n: usize) -> Vec<u8> {
    let mut objects: Vec<Vec<u8>> = Vec::with_capacity(n + 2);
    let mut kids = String::with_capacity(n * 8);
    for i in 0..n {
        let _ = write!(kids, "{} 0 R ", 3 + i);
    }
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    objects.push(format!("<< /Type /Pages /Kids [{kids}] /Count {n} >>").into_bytes());
    for _ in 0..n {
        objects.push(b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec());
    }
    assemble(&objects, "")
}

/// A page tree that loops: the second node of the tree lists the first among its kids.
pub fn circular_tree_pdf() -> Vec<u8> {
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 3 >>".to_vec(),
        b"<< /Type /Pages /Parent 2 0 R /Kids [2 0 R 4 0 R 3 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 3 0 R /MediaBox [0 0 612 792] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec(),
    ];
    assemble(&objects, "")
}

/// A page tree that is deeper than any reader follows: a chain of nodes with one page at the end.
pub fn deep_tree_pdf(depth: usize) -> Vec<u8> {
    let mut objects: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
    for level in 0..depth {
        let id = 2 + level;
        let parent = if level == 0 {
            String::new()
        } else {
            format!("/Parent {} 0 R", id - 1)
        };
        objects.push(
            format!(
                "<< /Type /Pages {parent} /Kids [{} 0 R] /Count 1 >>",
                id + 1
            )
            .into_bytes(),
        );
    }
    objects.push(
        format!(
            "<< /Type /Page /Parent {} 0 R /MediaBox [0 0 612 792] >>",
            depth + 1
        )
        .into_bytes(),
    );
    assemble(&objects, "")
}

/// Two real pages in a tree that says it has `claim` pages.
pub fn page_count_lie_pdf(claim: u64) -> Vec<u8> {
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [3 0 R 4 0 R] /Count {claim} >>").into_bytes(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec(),
    ];
    assemble(&objects, "")
}

/// Pages with odd `/Rotate` values: not a multiple of 90, negative, past 360, huge, and a real
/// number. Each page is 200 by 100 points and has the text "rotate N" where it can be read.
pub fn odd_rotation_pdf() -> Vec<u8> {
    let values = [
        "45",
        "-90",
        "450",
        "2147483647",
        "-2147483648",
        "90.5",
        "(x)",
        "null",
        "270",
    ];
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        Vec::new(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    let mut kids = String::new();
    for (i, v) in values.iter().enumerate() {
        let page = 4 + 2 * i;
        let _ = write!(kids, "{page} 0 R ");
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Rotate {v} /Contents {} 0 R \
                 /Resources << /Font << /F1 3 0 R >> >> >>",
                page + 1
            )
            .into_bytes(),
        );
        objects.push(stream_object(
            "",
            &format!("BT /F1 12 Tf 10 50 Td (rotate {i}) Tj ET"),
        ));
    }
    objects[1] = format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", values.len()).into_bytes();
    assemble(&objects, "")
}

/// Pages with odd boxes, as `(label, MediaBox and other entries)`.
pub fn odd_boxes() -> Vec<(&'static str, &'static str)> {
    vec![
        ("inverted media box", "/MediaBox [612 792 0 0]"),
        ("zero area", "/MediaBox [0 0 0 0]"),
        ("huge", "/MediaBox [0 0 1e12 1e12]"),
        ("negative origin", "/MediaBox [-300 -400 300 400]"),
        (
            "crop outside media",
            "/MediaBox [0 0 200 200] /CropBox [-500 -500 900 900]",
        ),
        (
            "crop apart from media",
            "/MediaBox [0 0 200 200] /CropBox [500 500 600 600]",
        ),
        (
            "crop inverted",
            "/MediaBox [0 0 200 200] /CropBox [150 150 50 50]",
        ),
        ("tiny", "/MediaBox [0 0 0.5 0.5]"),
        ("no media box", ""),
        ("media box of strings", "/MediaBox [(a) (b) (c) (d)]"),
        ("media box too short", "/MediaBox [0 0 100]"),
        ("huge numbers", "/MediaBox [-1e38 -1e38 1e38 1e38]"),
        ("thin", "/MediaBox [0 0 5000 1]"),
    ]
}

/// A document with one page for each of [`odd_boxes`].
pub fn odd_boxes_pdf() -> Vec<u8> {
    let cases = odd_boxes();
    let mut objects: Vec<Vec<u8>> = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(), Vec::new()];
    let mut kids = String::new();
    for (i, (_, boxes)) in cases.iter().enumerate() {
        let _ = write!(kids, "{} 0 R ", 3 + i);
        objects.push(format!("<< /Type /Page /Parent 2 0 R {boxes} >>").into_bytes());
    }
    objects[1] = format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", cases.len()).into_bytes();
    assemble(&objects, "")
}

/// A document made of the pages described, with a font named `/F1` (Helvetica) on each.
pub fn pdf_from_specs(specs: &[super::PageSpec]) -> Vec<u8> {
    super::build(specs, "", "", Vec::new())
}
