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

/// A page tree with three pages whose list of kids names a thing that is not a page: `kid` is the
/// text put in the second place of the list (a reference to an object that is not there, to a null
/// object, to a number, to the catalog).
pub fn broken_page_list_pdf(kid: &str) -> Vec<u8> {
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [3 0 R {kid} 5 0 R] /Count 3 >>").into_bytes(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec(),
        b"null".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec(),
    ];
    assemble(&objects, "")
}

/// The things `broken_page_list_pdf` is tried with.
pub const BROKEN_KIDS: [&str; 5] = ["4 0 R", "99 0 R", "42", "1 0 R", "(text)"];

// ---- documents like the ones real programs write --------------------------------------------------
//
// The documents above are made for the tests of one tool each. These carry what real files carry and
// the page tools have to live with: notes with their pop-ups, a field shown on several pages, the
// look of typed text, tags with text of their own.

/// Three Letter pages, the second with a note and its pop-up (as notes are written by the programs
/// that make them: the pop-up names the note with `/Parent`, the note names the pop-up with
/// `/Popup`).
pub fn popup_note_pdf() -> Vec<u8> {
    let page = |extra: &str| {
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 6 0 R /Resources << /Font << /F1 9 0 R >> >> {extra} >>").into_bytes()
    };
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".to_vec(),
        page(""),
        page("/Annots [7 0 R 8 0 R]"),
        page(""),
        stream_object("", "BT /F1 12 Tf 72 700 Td (Hello) Tj ET"),
        b"<< /Type /Annot /Subtype /Text /Rect [100 700 120 720] /Contents (A note) /T (Ann) /Popup 8 0 R /P 4 0 R >>".to_vec(),
        b"<< /Type /Annot /Subtype /Popup /Rect [120 620 300 720] /Parent 7 0 R /Open false /P 4 0 R >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    assemble(&objects, "")
}

/// Two pages that show one form field: the field is the parent of two widgets, one on each page.
pub fn shared_field_pdf() -> Vec<u8> {
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [7 0 R] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [8 0 R] >>".to_vec(),
        b"<< /Fields [6 0 R] /DA (/Helv 0 Tf 0 g) >>".to_vec(),
        b"<< /FT /Tx /T (name) /V (Alice) /Kids [7 0 R 8 0 R] >>".to_vec(),
        b"<< /Type /Annot /Subtype /Widget /Parent 6 0 R /Rect [72 400 272 424] /F 4 /P 3 0 R >>"
            .to_vec(),
        b"<< /Type /Annot /Subtype /Widget /Parent 6 0 R /Rect [72 400 272 424] /F 4 /P 4 0 R >>"
            .to_vec(),
    ];
    assemble(&objects, "")
}

/// What `form_leftovers_pdf` holds on its second page, that a deleted page must take with it
/// (`(what, text in the file)`): the page text, the typed value of a field, its default value, its
/// tooltip and the look of the typed text (a drawing, so in the hexadecimal form of its stream).
pub fn form_leftovers_gone() -> Vec<(&'static str, String)> {
    vec![
        ("page text", hex_form("PAGE-ONE-SECRET")),
        ("typed value", "VALUE-SECRET".into()),
        ("default value", "DEFAULT-SECRET".into()),
        ("tooltip", "TOOLTIP-SECRET".into()),
        ("look of the typed text", hex_form("AP-VALUE-SECRET")),
    ]
}

/// What the same page holds that PDFium gives no way to remove: the alternative text and the actual
/// text of a tag. These stay in the file when the page is deleted, and the README says so.
pub fn form_leftovers_kept() -> Vec<(&'static str, String)> {
    vec![
        ("alternative text of a tag", "ALT-SECRET".into()),
        ("actual text of a tag", "ACTUAL-SECRET".into()),
    ]
}

/// Two pages. The second has a filled-in text field with the look of its value, a default value and
/// a tooltip, and a tag (a structure element) with alternative text and actual text.
pub fn form_leftovers_pdf() -> Vec<u8> {
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm 3 0 R /StructTreeRoot 4 0 R /MarkInfo << /Marked true >> >>".to_vec(),
        b"<< /Type /Pages /Kids [5 0 R 6 0 R] /Count 2 >>".to_vec(),
        b"<< /Fields [9 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 8 0 R >> >> >>".to_vec(),
        b"<< /Type /StructTreeRoot /K [11 0 R] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 7 0 R /Resources << /Font << /F1 8 0 R >> >> >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 12 0 R /Resources << /Font << /F1 8 0 R >> >> /Annots [9 0 R] >>".to_vec(),
        hex_stream("", "BT /F1 12 Tf 72 700 Td (KEEP) Tj ET"),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (f1) /V (VALUE-SECRET) /DV (DEFAULT-SECRET) /TU (TOOLTIP-SECRET) /Rect [72 400 272 424] /F 4 /P 6 0 R /DA (/Helv 12 Tf 0 g) /AP << /N 10 0 R >> >>".to_vec(),
        hex_stream("/Type /XObject /Subtype /Form /BBox [0 0 200 24] /Resources << /Font << /Helv 8 0 R >> >>", "/Tx BMC BT /Helv 12 Tf 2 6 Td (AP-VALUE-SECRET) Tj ET EMC"),
        b"<< /Type /StructElem /S /Figure /P 4 0 R /Pg 6 0 R /Alt (ALT-SECRET) /ActualText (ACTUAL-SECRET) /K 0 >>".to_vec(),
        hex_stream("", "BT /F1 12 Tf 72 700 Td (PAGE-ONE-SECRET) Tj ET"),
    ];
    assemble(&objects, "")
}

/// Three pages where the first and the third are one page object that stands twice in the page tree.
pub fn repeated_page_pdf() -> Vec<u8> {
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R 3 0 R] /Count 3 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R /Resources << /Font << /F1 7 0 R >> >> >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> >> >>".to_vec(),
        stream_object("", "BT /F1 12 Tf 72 700 Td (REPEATED) Tj ET"),
        stream_object("", "BT /F1 12 Tf 72 700 Td (MIDDLE) Tj ET"),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    assemble(&objects, "")
}

/// One Letter page whose box lies far from the origin, so that the numbers of its boxes are as large
/// as a file can make them while the page itself is an ordinary size.
pub fn far_box_pdf() -> Vec<u8> {
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [20000000 20000000 20000612 20000792] >>".to_vec(),
    ];
    assemble(&objects, "")
}

/// Three pages, the last with a `/Contents` that is a dictionary holding a `/First` key. PDFium
/// copies a page with such an entry without repairing the reference under that key (its page copy
/// skips `/Parent`, `/Prev` and `/First`), the way it does for a pop-up note's `/Parent`.
pub fn first_key_pdf() -> Vec<u8> {
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 6 0 R >>".to_vec(),
        b"<< /First 7 0 R >>".to_vec(),
    ];
    assemble(&objects, "")
}

/// The password of [`page_count_lost_pdf`].
pub const PAGE_COUNT_LOST_PASSWORD: &str = "secret";

/// A damaged, password protected document of one page, found by the fuzz target, that PDFium reads
/// with one page but writes out as a document that reads back with none. Whatever the page tools
/// do with it, they must never put a document of no pages in place of the one that is open.
pub fn page_count_lost_pdf() -> Vec<u8> {
    b"\
        %PDF-1.4\n%\xe2\xe3\xcf\xd3\n1\x200\x20obj\n<<\x20/Type\x20/Catalog\x20/Pages\x202\x200\x20\
        R\x20\x20>>\nendobj\n2\x200\x20obj\n<<\x20/Type\x20/\n3\x200\x20obj\n<<\x20/Type\x20/Font\
        \x20/Subtype\x20/Type1\x20/BaseFont\x20/Helvetica\x20>>\nendobj\n4\x200\x20obj\n<<\x20/Ty\
        pe\x20/Page\x20/Parent\x202\x200\x20R\x20/MediaBox\x20[0\x200\x20612\x20792]\x20/Resourcd\
        s\x20<<\x20/Font\x20<<\x20/F1\x203\x200\x20R\x20>>\x20>>\x20\x20>>\nendobj\n5\x200\x20obj\
        \n<<\x20/Filter\x20/Standard\x20/V\x201\x20/R\x202\x20/P\x20-4\x20/O\x20<030a11\xd3\n1\x20\
        0\x20obj\n<<\x20/Type\x20/Catalog\x20/Pages\x202\x200\x20R\x20\x20>>\nendobj\n2\x200\x20o\
        bj\n<<\x20/Type\x20/\n3\x200\x20obj\n<<\x20/Type\x20/Font\x20/Subtype\x20/Type1\x20/BaseF\
        ont\x20/Helvetica\x20>>\nendobj\n4\x200\x20obj\n<<\x20/Type\x20/Page\x20/Parent\x202\x200\
        \x20R\x20/MediaBox\x20[0\x200\x20612\x20792]\x20/Resourcds\x20<<\x20/Font\x20<<\x20/F1\x20\
        3\x200\x20R\x20>>\x20>>\x20\x20>>\nendobj\n5\x200\x20obj\n<<\x20/Filter\x20/Standard\x20/\
        V\x201\x20/R\x202\x20/P\x20-4\x20/O\x20<030a11181f262d343b424950575e656c737a81888f969da4a\
        bb2b9c0c7ced5dc>\x20/U\x20<89a909fd5138dd7597952e9fc5b3094947139c79356b9fe1c9b7a0360655bb\
        c9>\x20>>\nendobj\nxref\n0\x206\n0000000000\x2065535\x20f\x20\n0000000015\x2000000\x20n\x20\
        \n0000000065\x2000000\x20n\x20\n0000000123\x2000000\x20n\x20\n0000181f262d343b424950575e6\
        56c737a81888f969da4abb2b9c0c7ced5dc>\x20/U\x20<89a909fd5138dd7597952e9fc5b3094947139c7935\
        6b9fe1c9b7a0360655bbc9>\x20>>\nendobj\nxref\n0\x206\n0000000000\x2065535\x20f\x20\n000000\
        0015\x2000000\x20n\x20\n0000000065\x2000000\x20n\x20\n0000000123\x2000000\x20n\x20\n00000\
        00193\x2000000\x20n\x20\n0000000304\x2000000\x20n\x20\ntrailer\n<<\x20/Size\x206\x20/Root\
        \x201\x200\x20R\x20/Encrypt\x205\x200\x20R\x20/ID\x20[<7061727472696467652d746573742d31>\x20\
        <7061727472696467652d746573742d31>]\x20>>\nstartxref\n499\n%%EOF\n"
        .to_vec()
}

/// A document protected like [`crate::encrypted_pdf`] (revision 2) with the permissions `p` (the
/// `/P` entry: bit 4 modify, 5 copy, 11 assemble; bits 1, 2, 7 and 8 stay as the standard has them).
pub fn encrypted_with_permissions_pdf(user_password: &str, pages: usize, p: u32) -> Vec<u8> {
    super::encrypted_pdf_with(user_password, pages, p as i32, false)
}

/// As [`encrypted_with_permissions_pdf`], with the 128 bit RC4 handler of revision 3, which has a
/// permission bit of its own for assembling the document.
pub fn encrypted_r3_with_permissions_pdf(user_password: &str, pages: usize, p: u32) -> Vec<u8> {
    super::encrypted_pdf_with(user_password, pages, p as i32, true)
}

/// Three pages in a page tree whose list of kids ends with an entry that is not there. PDFium finds
/// the three pages and every one of them is sound, so a look at the pages does not show it; its
/// move of pages stops the process on such a tree.
pub fn trailing_null_kid_pdf() -> Vec<u8> {
    let page = b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec();
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 99 0 R] /Count 3 >>".to_vec(),
        page.clone(),
        page.clone(),
        page,
    ];
    assemble(&objects, "")
}

/// Three pages whose resources hold dictionaries of their own with a `/Type` (a font and a graphics
/// state written in place), and a page tree with two levels. Everything is as it should be: the
/// check of the page tree must not take the `/Type` of an inner dictionary for the page's.
pub fn inline_dictionaries_pdf() -> Vec<u8> {
    let page = |parent: u32| {
        format!(
            "<< /Resources << /ExtGState << /G0 << /Type /ExtGState /CA 0.5 >> >> /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> /Parent {parent} 0 R /MediaBox [0 0 612 792] /Type /Page >>"
        )
        .into_bytes()
    };
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Kids [3 0 R 6 0 R] /Count 3 /Type /Pages >>".to_vec(),
        b"<< /Kids [4 0 R 5 0 R] /Count 2 /Type /Pages /Parent 2 0 R >>".to_vec(),
        page(3),
        page(3),
        page(2),
    ];
    // Object 3 is the inner node; the pages are 4, 5 and 6.
    let mut objects = objects;
    objects.swap(2, 2);
    assemble(&objects, "")
}

/// As [`trailing_null_kid_pdf`], with the entries after the pages in the list of kids a name and an
/// array instead of a reference to an object that is not there.
pub fn junk_after_the_kids_pdf() -> Vec<u8> {
    let page = b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_vec();
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R /MediaBox [0 0 0 0]] /Count 3 >>".to_vec(),
        page.clone(),
        page.clone(),
        page,
    ];
    assemble(&objects, "")
}

// ---- page trees as real programs write them ----------------------------------------------------------

/// A page with its own text, a coloured bar whose length depends on `n`, a picture and a form, all
/// drawn with the names `/F1`, `/Im1` and `/Fm1` of the resources the page gets from itself or from
/// a node above it.
fn drawn_page(n: usize) -> Vec<u8> {
    stream_object(
        "",
        &format!(
            "q 0.8 0.2 0.2 rg 20 20 {} 30 re f Q BT /F1 14 Tf 10 100 Td (PAGE {n}) Tj ET \
             q 40 0 0 40 60 50 cm /Im1 Do Q q 0.3 0 0 0.3 120 10 cm /Fm1 Do Q",
            20 + 15 * n
        ),
    )
}

/// The picture and the form that the pages of [`inherited_tree_pdf`] and [`root_inherited_pdf`] draw.
fn picture_and_form() -> [Vec<u8>; 2] {
    [
        stream_object(
            "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /ASCIIHexDecode",
            "00 FF FF 00>",
        ),
        stream_object(
            "/Type /XObject /Subtype /Form /BBox [0 0 100 100]",
            "0 0 1 rg 0 0 100 100 re f",
        ),
    ]
}

/// Seven pages in a page tree of three levels in which the inner nodes hold what their pages
/// inherit: `/Resources` (written in place on one node, as a reference on another), `/MediaBox`,
/// `/CropBox` and `/Rotate`, and the root holds a `/MediaBox` too. Some pages have entries of
/// their own that win over the ones above them. Every page draws text, a bar, a picture and a form
/// through the names in the resources, so a page that loses what it inherits is drawn differently.
///
/// Object numbers: 2 root, 3 and 4 its nodes, 8 a node below 3, 5 a page under the root; the pages
/// in order of the document are 6, 7, 9, 10, 11, 12, 5.
pub fn inherited_tree_pdf() -> Vec<u8> {
    let resources = |font: u32| {
        format!("<< /Font << /F1 {font} 0 R >> /XObject << /Im1 16 0 R /Fm1 17 0 R >> >>")
    };
    let [image, form] = picture_and_form();
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 7 /MediaBox [0 0 400 300] >>".to_vec(),
        format!(
            "<< /Type /Pages /Parent 2 0 R /Kids [6 0 R 7 0 R 8 0 R] /Count 4 /Resources {} /MediaBox [0 0 200 150] >>",
            resources(13)
        )
        .into_bytes(),
        b"<< /Type /Pages /Parent 2 0 R /Kids [11 0 R 12 0 R] /Count 2 /Resources 25 0 R /MediaBox [0 0 250 250] >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /Contents 24 0 R /Resources {} >>",
            resources(15)
        )
        .into_bytes(),
        b"<< /Type /Page /Parent 3 0 R /Contents 18 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 3 0 R /Contents 19 0 R /Rotate 90 >>".to_vec(),
        b"<< /Type /Pages /Parent 3 0 R /Kids [9 0 R 10 0 R] /Count 2 /Rotate 180 /CropBox [10 10 190 140] >>".to_vec(),
        b"<< /Type /Page /Parent 8 0 R /Contents 20 0 R >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 8 0 R /Contents 21 0 R /CropBox [0 0 150 100] /Resources {} >>",
            resources(14)
        )
        .into_bytes(),
        b"<< /Type /Page /Parent 4 0 R /Contents 22 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 4 0 R /Contents 23 0 R /MediaBox [0 0 120 160] >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman >>".to_vec(),
        image,
        form,
        drawn_page(0),
        drawn_page(1),
        drawn_page(2),
        drawn_page(3),
        drawn_page(4),
        drawn_page(5),
        drawn_page(6),
        resources(15).into_bytes(),
    ];
    assemble(&objects, "")
}

/// Three pages under a root that holds all four of the entries a page can inherit, as a document
/// from most programs does: nothing below the root holds one, so a page can go anywhere in the
/// tree and keep what it has.
pub fn root_inherited_pdf() -> Vec<u8> {
    let [image, form] = picture_and_form();
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 200] /CropBox [5 5 295 195] /Rotate 90 /Resources << /Font << /F1 6 0 R >> /XObject << /Im1 7 0 R /Fm1 8 0 R >> >> >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 9 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 10 0 R /Rotate 0 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 11 0 R >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        image,
        form,
        drawn_page(0),
        drawn_page(1),
        drawn_page(2),
    ];
    assemble(&objects, "")
}

fn plain_page(parent: u32, width: u32) -> Vec<u8> {
    format!("<< /Type /Page /Parent {parent} 0 R /MediaBox [0 0 {width} 200] >>").into_bytes()
}

/// Three pages (of the widths 100, 200 and 300) whose list of kids is an object of its own, which
/// is as valid as writing it in the node.
pub fn indirect_kids_pdf() -> Vec<u8> {
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids 6 0 R /Count 7 0 R >>".to_vec(),
        plain_page(2, 100),
        plain_page(2, 200),
        plain_page(2, 300),
        b"[3 0 R 4 0 R 5 0 R]".to_vec(),
        b"3".to_vec(),
    ];
    assemble(&objects, "")
}

/// Three pages (of the widths 100, 200 and 300) under a node that has words in strings that a
/// search for words in the bytes of the node would take for the syntax of the file: `stream` and
/// `endobj` and a closing `>>`. With `before_kids` the strings stand before the list of kids, else
/// after it.
pub fn words_in_node_pdf(before_kids: bool) -> Vec<u8> {
    let words = "/Comment (a stream of pages) /Note (endobj >> stream) /Other << /A (stream) >>";
    let node = if before_kids {
        format!("<< /Type /Pages {words} /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>")
    } else {
        format!("<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 {words} >>")
    };
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        node.into_bytes(),
        plain_page(2, 100),
        plain_page(2, 200),
        plain_page(2, 300),
    ];
    assemble(&objects, "")
}

/// As [`trailing_null_kid_pdf`], with the entry that is not there an object with a very big number
/// (2,000,000) that is `null`, in a table of objects that has a part of its own for it.
pub fn trailing_far_kid_pdf() -> Vec<u8> {
    let objects: Vec<(u32, Vec<u8>)> = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (
            2,
            b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 2000000 0 R] /Count 3 >>".to_vec(),
        ),
        (3, plain_page(2, 100)),
        (4, plain_page(2, 200)),
        (5, plain_page(2, 300)),
        (2_000_000, b"null".to_vec()),
    ];
    assemble_sparse(&objects)
}

/// A valid document of three pages (the widths 100, 200 and 300) with a very big object number
/// (2,000,000) for the last page.
pub fn far_object_number_pdf() -> Vec<u8> {
    let objects: Vec<(u32, Vec<u8>)> = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (
            2,
            b"<< /Type /Pages /Kids [3 0 R 4 0 R 2000000 0 R] /Count 3 >>".to_vec(),
        ),
        (3, plain_page(2, 100)),
        (4, plain_page(2, 200)),
        (2_000_000, plain_page(2, 300)),
    ];
    assemble_sparse(&objects)
}

/// A file whose table of objects has a part for each object, with the numbers as given.
fn assemble_sparse(objects: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut at = Vec::with_capacity(objects.len());
    for (number, body) in objects {
        at.push((*number, out.len()));
        out.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n");
    for (number, offset) in &at {
        out.extend_from_slice(format!("{number} 1\n{offset:010} 00000 n \n").as_bytes());
    }
    let size = objects.iter().map(|(n, _)| *n).max().unwrap_or(0) + 1;
    out.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// Three pages (of the widths 100, 200 and 300) in a file written the way most current programs
/// write them: the catalog, the nodes and the pages are in an object stream, and the table of
/// objects is a stream too.
pub fn object_streams_pdf() -> Vec<u8> {
    let inner: [(u32, &str); 5] = [
        (1, "<< /Type /Catalog /Pages 2 0 R >>"),
        (2, "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>"),
        (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] >>"),
        (4, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>"),
        (5, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >>"),
    ];
    let mut header = String::new();
    let mut body = String::new();
    for (number, text) in inner {
        let _ = write!(header, "{number} {} ", body.len());
        body.push_str(text);
        body.push(' ');
    }
    let first = header.len();
    let data = format!("{header}{body}");
    let mut out = b"%PDF-1.5\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let stream_at = out.len();
    out.extend_from_slice(
        format!(
            "6 0 obj\n<< /Type /ObjStm /N 5 /First {first} /Length {} >>\nstream\n{data}\nendstream\nendobj\n",
            data.len()
        )
        .as_bytes(),
    );
    let table_at = out.len();
    // Entries of one byte for the kind, two for the place and one for the index.
    let mut table: Vec<u8> = vec![0, 0, 0, 255];
    for index in 0..5u8 {
        table.extend_from_slice(&[2, 0, 6, index]);
    }
    table.extend_from_slice(&[1, (stream_at >> 8) as u8, stream_at as u8, 0]);
    table.extend_from_slice(&[1, (table_at >> 8) as u8, table_at as u8, 0]);
    out.extend_from_slice(
        format!(
            "7 0 obj\n<< /Type /XRef /Size 8 /W [1 2 1] /Root 1 0 R /Length {} >>\nstream\n",
            table.len()
        )
        .as_bytes(),
    );
    out.extend_from_slice(&table);
    out.extend_from_slice(
        format!("\nendstream\nendobj\nstartxref\n{table_at}\n%%EOF\n").as_bytes(),
    );
    out
}

/// `pages` Letter pages that all draw the same long content of `segments` line segments, which
/// every page takes a while to read, in a file that stays small.
pub fn heavy_pages_pdf(pages: usize, segments: usize) -> Vec<u8> {
    let mut content = String::with_capacity(segments * 16 + 16);
    content.push_str("0.5 w\n");
    for i in 0..segments {
        let _ = writeln!(content, "{} 10 m {} 700 l", 10 + i % 500, 20 + i % 400);
    }
    content.push_str("S\n");
    let mut kids = String::new();
    for i in 0..pages {
        let _ = write!(kids, "{} 0 R ", 4 + i);
    }
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>").into_bytes(),
        stream_object("", &content),
    ];
    for _ in 0..pages {
        objects.push(
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 3 0 R >>".to_vec(),
        );
    }
    assemble(&objects, "")
}
