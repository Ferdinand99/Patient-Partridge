//! Deleting pages overwrites the words of the annotations on those pages, for every way a file can
//! hold an annotation (an object, an object in an object stream, a dictionary inside the page's
//! array, with a pop-up, changed in this session) and whether or not the page was drawn and listed
//! first: a save writes the objects made in the session, and drawing a page makes an object of
//! each annotation that the file holds as a dictionary inside the array. The tests check the saved
//! bytes. They also delete more pages with annotations than one group of the check for
//! annotations that a page which stays also reaches takes, and time a delete on a large document.
//!
//! These tests run the engine in this process, without the sandbox.

mod common;

use std::time::{Duration, Instant};

use common::*;
use partridge_protocol::{PagesRequest, RequestBody, ResponseBody};

type Eng = partridge_engine::Engine;

/// A document built object by object. Objects 1 (catalog) and 2 (page tree) are filled in last.
struct Builder {
    objects: Vec<String>,
    /// How many lines of text each page holds besides its label (a page that takes time to read).
    weight: usize,
}

impl Builder {
    fn new() -> Builder {
        let mut b = Builder {
            objects: Vec::new(),
            weight: 0,
        };
        b.add(String::new());
        b.add(String::new());
        // Object 3 is the font.
        b.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
        b
    }

    fn add(&mut self, body: String) -> u32 {
        self.objects.push(body);
        self.objects.len() as u32
    }

    /// A page with `annots` as the value of its `/Annots` entry (empty: none). Returns its number.
    fn page(&mut self, label: &str, annots: &str) -> u32 {
        let mut data = format!("BT /F1 24 Tf 72 700 Td ({label}) Tj ET");
        for line in 0..self.weight {
            data.push_str(&format!(
                "\nBT /F1 8 Tf 72 {} Td (line {line}) Tj ET",
                600 - line % 500
            ));
        }
        let contents = self.add(format!(
            "<< /Length {} >>\nstream\n{data}\nendstream",
            data.len()
        ));
        let annots = if annots.is_empty() {
            String::new()
        } else {
            format!(" /Annots {annots}")
        };
        self.add(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {contents} 0 R \
             /Resources << /Font << /F1 3 0 R >> >>{annots} >>"
        ))
    }

    /// A text note with the words in every place a person's words are kept.
    fn note(&mut self, tag: &str) -> u32 {
        self.add(note_dict(tag))
    }

    fn finish(mut self, kids: &[u32]) -> Vec<u8> {
        self.objects[0] = "<< /Type /Catalog /Pages 2 0 R >>".into();
        self.objects[1] = format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.iter()
                .map(|k| format!("{k} 0 R"))
                .collect::<Vec<_>>()
                .join(" "),
            kids.len()
        );
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in self.objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, body).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", self.objects.len() + 1).as_bytes(),
        );
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                self.objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }
}

/// The dictionary of a note that holds `tag` in its comment, author, rich text and subject.
fn note_dict(tag: &str) -> String {
    format!(
        "<< /Type /Annot /Subtype /Text /Rect [300 700 320 720] /F 4 /Contents ({tag}-CONTENTS) \
         /T ({tag}-AUTHOR) /Subj ({tag}-SUBJECT) \
         /RC (<body><p>{tag}-RICH</p></body>) >>"
    )
}

const PARTS: [&str; 4] = ["CONTENTS", "AUTHOR", "SUBJECT", "RICH"];

/// True when the bytes hold `text` in any form a save writes a string in: as it is, as UTF-16 with
/// or without the hex form.
fn holds(bytes: &[u8], text: &str) -> bool {
    let utf16: Vec<u8> = std::iter::once(0xFEFFu16)
        .chain(text.encode_utf16())
        .flat_map(|u| u.to_be_bytes())
        .collect();
    let hex: String = utf16.iter().map(|b| format!("{b:02X}")).collect();
    let hex_plain: String = text.bytes().map(|b| format!("{b:02X}")).collect();
    contains(bytes, text.as_bytes())
        || contains(bytes, &utf16[2..])
        || contains(bytes, hex.as_bytes())
        || contains(bytes, hex_plain.as_bytes())
}

fn assert_gone(saved: &[u8], tag: &str) {
    for part in PARTS {
        let text = format!("{tag}-{part}");
        assert!(
            !holds(saved, &text),
            "{text} is still in the saved file after its page was deleted"
        );
    }
}

fn assert_kept(saved: &[u8], tag: &str) {
    for part in PARTS {
        let text = format!("{tag}-{part}");
        assert!(holds(saved, &text), "{text} is not in the saved file");
    }
}

fn delete(e: &mut Eng, doc: u32, pages: Vec<u32>) {
    match ask(e, RequestBody::Pages(PagesRequest::Delete { doc, pages })) {
        ResponseBody::Pages(_) => {}
        other => panic!("delete refused: {other:?}"),
    }
}

fn add_note(e: &mut Eng, doc: u32, page: u32, text: &str) {
    let made = annot(ask(
        e,
        RequestBody::AddNote {
            doc,
            page,
            x: 100.0,
            y: 100.0,
            color: YELLOW,
            text: text.into(),
        },
    ));
    assert_eq!(made.text, text);
}

// ---- the words of what the file holds go with the page ----------------------------------------

/// Eight pages. The annotations of pages 1 to 5 sit in different places of the file: in the
/// page's own array, in an array of its own object, as a direct dictionary, with a pop-up, and one
/// that is changed in this session. Pages 0 and 7 are controls: their words must be found.
fn kinds_of_holding() -> Vec<u8> {
    let mut b = Builder::new();
    let mut kids = Vec::new();
    // 0: a control.
    let n = b.note("LOADED-KEPT-0");
    kids.push(b.page("Page 0", &format!("[{n} 0 R]")));
    // 1: an indirect note in the page's own array.
    let n = b.note("LOADED-A1");
    kids.push(b.page("Page 1", &format!("[{n} 0 R]")));
    // 2: a highlight with a pop-up, in an array that is an object of its own.
    let popup = b.add(
        "<< /Type /Annot /Subtype /Popup /Rect [320 700 420 760] /Open true \
         /Contents (LOADED-A2-POPUP-CONTENTS) /T (LOADED-A2-POPUP-AUTHOR) \
         /Subj (LOADED-A2-POPUP-SUBJECT) /RC (LOADED-A2-POPUP-RICH) >>"
            .to_string(),
    );
    let high = b.add(format!(
        "<< /Type /Annot /Subtype /Highlight /Rect [72 690 200 720] /F 4 /C [1 1 0] \
         /QuadPoints [72 720 200 720 72 690 200 690] /Popup {popup} 0 R \
         /Contents (LOADED-A2-CONTENTS) /T (LOADED-A2-AUTHOR) /Subj (LOADED-A2-SUBJECT) \
         /RC (LOADED-A2-RICH) >>"
    ));
    let array = b.add(format!("[{high} 0 R {popup} 0 R]"));
    kids.push(b.page("Page 2", &format!("{array} 0 R")));
    // 3: direct dictionaries inside the array.
    kids.push(b.page("Page 3", &format!("[{}]", note_dict("LOADED-A3"))));
    // 4: a note that the session edits and moves.
    let n = b.note("LOADED-A4-OLD");
    kids.push(b.page("Page 4", &format!("[{n} 0 R]")));
    // 5: a note whose text is changed.
    let n = b.note("LOADED-A5");
    kids.push(b.page("Page 5", &format!("[{n} 0 R]")));
    // 6: no annotations.
    kids.push(b.page("Page 6", ""));
    // 7: a control.
    let n = b.note("LOADED-KEPT-7");
    kids.push(b.page("Page 7", &format!("[{n} 0 R]")));
    b.finish(&kids)
}

fn the_words_of_loaded_annotations_leave_with_their_pages(listed_first: bool) {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kinds_of_holding(), None).unwrap();
    if listed_first {
        // What the window does for the pages in view: it lists them, which draws each once.
        for page in 0..8 {
            let _ = list(e, doc, page);
        }
    }
    // A change of the text and a move of a note that came with the file.
    let id = list(e, doc, 4)[0].id;
    let changed = annot(ask(
        e,
        RequestBody::UpdateAnnot {
            doc,
            page: 4,
            id,
            text: Some("EDITED-A4-NEW-CONTENTS".into()),
            color: None,
            dx: 5.0,
            dy: 5.0,
        },
    ));
    assert_eq!(changed.text, "EDITED-A4-NEW-CONTENTS");
    // A change of the text of a note on page 5, which is not moved.
    let id = list(e, doc, 5)[0].id;
    ask(
        e,
        RequestBody::UpdateAnnot {
            doc,
            page: 5,
            id,
            text: Some("EDITED-A5-NEW-CONTENTS".into()),
            color: None,
            dx: 0.0,
            dy: 0.0,
        },
    );
    let before = save_all(e, doc);
    // The controls: the search finds words in the file as it is now.
    assert_kept(&before, "LOADED-A1");
    assert_kept(&before, "LOADED-A3");
    assert!(holds(&before, "LOADED-A2-CONTENTS"));
    assert!(holds(&before, "LOADED-A2-POPUP-CONTENTS"));

    delete(e, doc, vec![1, 2, 3, 4, 5]);
    let saved = save_all(e, doc);
    for tag in [
        "LOADED-A1",
        "LOADED-A2",
        "LOADED-A3",
        "LOADED-A4-OLD",
        "LOADED-A5",
    ] {
        assert_gone(&saved, tag);
    }
    for text in [
        "LOADED-A2-POPUP-CONTENTS",
        "LOADED-A2-POPUP-AUTHOR",
        "LOADED-A2-POPUP-SUBJECT",
        "LOADED-A2-POPUP-RICH",
        "EDITED-A4-NEW-CONTENTS",
        "EDITED-A5-NEW-CONTENTS",
    ] {
        assert!(!holds(&saved, text), "{text} is still in the saved file");
    }
    assert_kept(&saved, "LOADED-KEPT-0");
    assert_kept(&saved, "LOADED-KEPT-7");
    // The pages that stay still show their notes.
    assert_eq!(list(e, doc, 0)[0].text, "LOADED-KEPT-0-CONTENTS");
    assert_eq!(list(e, doc, 2)[0].text, "LOADED-KEPT-7-CONTENTS");
}

#[test]
fn the_words_of_loaded_annotations_leave_with_their_pages_when_the_pages_were_listed() {
    the_words_of_loaded_annotations_leave_with_their_pages(true);
}

#[test]
fn the_words_of_loaded_annotations_leave_with_their_pages_when_the_pages_were_never_listed() {
    the_words_of_loaded_annotations_leave_with_their_pages(false);
}

/// A document whose notes and their array live inside an object stream, with a cross-reference
/// stream: the packed form most programs write.
fn packed() -> Vec<u8> {
    // Objects: 1 catalog, 2 tree, 3 font, 4 and 5 and 6 (page 0: contents, page, note),
    // 7 and 8 and 9 (page 1), 10 object stream, 11 cross-reference stream. The pages' notes and the
    // arrays are in the object stream; the notes are objects 12 to 15, the arrays 16 and 17.
    let content = |label: &str| {
        let data = format!("BT /F1 24 Tf 72 700 Td ({label}) Tj ET");
        format!("<< /Length {} >>\nstream\n{data}\nendstream", data.len())
    };
    let page = |contents: u32, annots: &str| {
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {contents} 0 R \
             /Resources << /Font << /F1 3 0 R >> >> /Annots {annots} >>"
        )
    };
    let plain: Vec<(u32, String)> = vec![
        (1, "<< /Type /Catalog /Pages 2 0 R >>".into()),
        (2, "<< /Type /Pages /Kids [5 0 R 8 0 R] /Count 2 >>".into()),
        (
            3,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        ),
        (4, content("Page 0")),
        (5, page(4, "16 0 R")),
        (7, content("Page 1")),
        (8, page(7, "17 0 R")),
    ];
    let packed: Vec<(u32, String)> = vec![
        (12, note_dict("PACKED-KEPT")),
        (13, note_dict("PACKED-GONE")),
        (16, "[12 0 R]".into()),
        (17, "[13 0 R]".into()),
    ];
    // The object stream: pairs of "number offset", then the objects.
    let mut head = String::new();
    let mut body = String::new();
    for (number, text) in &packed {
        head.push_str(&format!("{number} {} ", body.len()));
        body.push_str(text);
        body.push('\n');
    }
    let stream = format!("{head}{body}");
    let objstm = format!(
        "<< /Type /ObjStm /N {} /First {} /Length {} >>\nstream\n{stream}\nendstream",
        packed.len(),
        head.len(),
        stream.len()
    );
    let mut out = b"%PDF-1.5\n".to_vec();
    let mut offsets = std::collections::BTreeMap::new();
    for (number, text) in &plain {
        offsets.insert(*number, out.len());
        out.extend_from_slice(format!("{number} 0 obj\n{text}\nendobj\n").as_bytes());
    }
    offsets.insert(10, out.len());
    out.extend_from_slice(format!("10 0 obj\n{objstm}\nendobj\n").as_bytes());
    // The cross-reference stream: type, field 2, field 3 (one, four and two bytes wide).
    let size = 18u32;
    let xref_at = out.len();
    offsets.insert(11, xref_at);
    let mut rows = Vec::new();
    for number in 0..size {
        if let Some(off) = offsets.get(&number) {
            rows.push(1u8);
            rows.extend_from_slice(&(*off as u32).to_be_bytes());
            rows.extend_from_slice(&0u16.to_be_bytes());
        } else if let Some(index) = packed.iter().position(|(n, _)| *n == number) {
            rows.push(2u8);
            rows.extend_from_slice(&10u32.to_be_bytes());
            rows.extend_from_slice(&(index as u16).to_be_bytes());
        } else {
            rows.push(0u8);
            rows.extend_from_slice(&0u32.to_be_bytes());
            rows.extend_from_slice(&0u16.to_be_bytes());
        }
    }
    out.extend_from_slice(
        format!(
            "11 0 obj\n<< /Type /XRef /Size {size} /W [1 4 2] /Root 1 0 R /Length {} >>\nstream\n",
            rows.len()
        )
        .as_bytes(),
    );
    out.extend_from_slice(&rows);
    out.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{xref_at}\n%%EOF\n").as_bytes());
    out
}

#[test]
fn the_words_of_a_loaded_annotation_in_an_object_stream_leave_with_its_page() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &packed(), None).unwrap();
    assert_eq!(list(e, doc, 0)[0].text, "PACKED-KEPT-CONTENTS");
    assert_eq!(list(e, doc, 1)[0].text, "PACKED-GONE-CONTENTS");
    delete(e, doc, vec![1]);
    let saved = save_all(e, doc);
    assert_gone(&saved, "PACKED-GONE");
    assert_kept(&saved, "PACKED-KEPT");
}

// ---- more than one group of pages --------------------------------------------------------------

/// `count` pages, each with a note that came with the file, tagged `LOADED-<n>`.
fn many_pages(count: u32, extra_listing_of: Option<u32>, weight: usize) -> Vec<u8> {
    let mut b = Builder::new();
    b.weight = weight;
    let mut kids = Vec::new();
    for n in 0..count {
        let note = b.note(&format!("LOADED-{n}"));
        kids.push(b.page(&format!("Page {n}"), &format!("[{note} 0 R]")));
    }
    if let Some(n) = extra_listing_of {
        // The page tree names the page of number `n` a second time, at the end.
        let again = kids[n as usize];
        kids.push(again);
    }
    b.finish(&kids)
}

#[test]
fn notes_on_more_than_one_group_of_deleted_pages_are_overwritten_and_those_of_kept_pages_stay() {
    // 150 pages with notes go: more than one group.
    const PAGES: u32 = 300;
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &many_pages(PAGES, None, 0), None).unwrap();
    for page in 0..PAGES {
        add_note(e, doc, page, &format!("SESSION-{page}-CONTENTS"));
    }
    // Every odd page goes.
    let deleted: Vec<u32> = (0..PAGES).filter(|p| p % 2 == 1).collect();
    delete(e, doc, deleted);
    let saved = save_all(e, doc);
    for page in (0..PAGES).filter(|p| p % 2 == 1) {
        assert!(
            !holds(&saved, &format!("SESSION-{page}-CONTENTS")),
            "the note of deleted page {page} is in the saved file"
        );
        assert_gone(&saved, &format!("LOADED-{page}"));
    }
    for (new, page) in (0..PAGES).filter(|p| p % 2 == 0).enumerate() {
        assert!(
            holds(&saved, &format!("SESSION-{page}-CONTENTS")),
            "the note of kept page {page} is not in the saved file"
        );
        assert_kept(&saved, &format!("LOADED-{page}"));
        let texts: Vec<String> = list(e, doc, new as u32)
            .iter()
            .map(|a| a.text.clone())
            .collect();
        assert_eq!(
            texts,
            [
                format!("LOADED-{page}-CONTENTS"),
                format!("SESSION-{page}-CONTENTS")
            ],
            "the notes of kept page {page}"
        );
    }
}

#[test]
fn a_session_note_of_a_page_listed_twice_stays_when_more_than_one_group_goes() {
    const PAGES: u32 = 300;
    let mut guard = engine();
    let e = &mut guard.0;
    // Page 1 is listed again as the last page, which stays.
    let doc = open(e, &many_pages(PAGES, Some(1), 0), None).unwrap();
    for page in 0..PAGES {
        add_note(e, doc, page, &format!("SESSION-{page}-CONTENTS"));
    }
    // The odd pages and page 0 go. The page that is listed twice is deleted at its first place, in
    // the first group, and the same page stays as the last in the list.
    let mut deleted: Vec<u32> = (0..PAGES).filter(|p| p % 2 == 1).collect();
    deleted.push(0);
    delete(e, doc, deleted);
    let saved = save_all(e, doc);
    // The note made on page 1 belongs to a page that remains: it is not overwritten.
    assert!(holds(&saved, "SESSION-1-CONTENTS"));
    assert_kept(&saved, "LOADED-1");
    // The others went with their pages.
    assert!(!holds(&saved, "SESSION-0-CONTENTS"));
    assert!(!holds(&saved, "SESSION-3-CONTENTS"));
    assert!(!holds(&saved, "SESSION-299-CONTENTS"));
    assert_gone(&saved, "LOADED-3");
}

// ---- a large document -------------------------------------------------------------------------

/// How long a delete may take on a large document: the same in every run. It takes about five
/// seconds on the machine the bound was set on (a debug build), and the bound leaves room for a
/// slower one. It stops a delete from growing with the number of pages that go times the number
/// that stay, which is what reading every page that stays for each group of fewer pages would do.
const DELETE_BOUND: Duration = Duration::from_secs(30);

#[test]
fn a_delete_on_a_large_document_stays_within_a_fixed_time() {
    const PAGES: u32 = 600;
    let mut guard = engine();
    let e = &mut guard.0;
    // Pages with 1,500 lines of text each, so that reading a page takes some time.
    let doc = open(e, &many_pages(PAGES, None, 1500), None).unwrap();
    // A note of this session on some of the pages that go, which all have a note of the file.
    let deleted: Vec<u32> = (0..PAGES).filter(|p| p % 2 == 1).collect();
    for &page in deleted.iter().step_by(7).take(60) {
        add_note(e, doc, page, &format!("SESSION-{page}-CONTENTS"));
    }
    let started = Instant::now();
    delete(e, doc, deleted.clone());
    let took = started.elapsed();
    println!("deleting {} of {PAGES} pages took {took:?}", deleted.len());
    assert!(
        took < DELETE_BOUND,
        "deleting {} of {PAGES} pages took {took:?}",
        deleted.len()
    );
    let saved = save_all(e, doc);
    for &page in deleted.iter().step_by(7).take(60) {
        assert!(!holds(&saved, &format!("SESSION-{page}-CONTENTS")));
    }
    assert_gone(&saved, "LOADED-599");
    assert_kept(&saved, "LOADED-598");
    assert_kept(&saved, "LOADED-0");
}
