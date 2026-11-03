//! Deleting a page blanks the words of its annotations, but never those of an annotation that a
//! page which stays also reaches: a file may share one annotation dictionary between pages, share
//! one `/Annots` array, or list the same page twice.
//!
//! These tests run the engine in this process, without the sandbox.

mod common;

use common::*;
use partridge_protocol::{AnnotInfo, PagesRequest, RequestBody, ResponseBody};

type Eng = partridge_engine::Engine;

const KEPT: &str = "KEEP-THIS-NOTE-TEXT";

fn assemble(objects: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", i + 1, body).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

fn content(text: &str) -> String {
    let data = format!("BT /F1 24 Tf 72 700 Td ({text}) Tj ET");
    format!("<< /Length {} >>\nstream\n{data}\nendstream", data.len())
}

fn page(contents: u32, annots: &str) -> String {
    format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {contents} 0 R \
         /Resources << /Font << /F1 7 0 R >> >> /Annots {annots} >>"
    )
}

/// A document whose objects 3 and 5 are pages (with contents 4 and 6), and whose object 8 is a
/// note holding [`KEPT`]. `extra` adds the objects from number 9.
fn build(kids: &str, count: u32, pages: [String; 2], extra: &[&str]) -> Vec<u8> {
    let [first, second] = pages;
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {count} >>"),
        first,
        content("Page 1"),
        second,
        content("Page 2"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!(
            "<< /Type /Annot /Subtype /Text /Rect [300 700 320 720] /Contents ({KEPT}) /T (Bob) /F 4 >>"
        ),
    ];
    objects.extend(extra.iter().map(|e| e.to_string()));
    assemble(&objects)
}

/// Two pages whose `/Annots` entry is the same.
fn shared(annots: &str, extra: &[&str]) -> Vec<u8> {
    build("3 0 R 5 0 R", 2, [page(4, annots), page(6, annots)], extra)
}

fn delete(e: &mut Eng, doc: u32, pages: Vec<u32>) {
    match ask(e, RequestBody::Pages(PagesRequest::Delete { doc, pages })) {
        ResponseBody::Pages(_) => {}
        other => panic!("delete refused: {other:?}"),
    }
}

fn texts(items: &[AnnotInfo]) -> Vec<String> {
    items.iter().map(|a| a.text.clone()).collect()
}

/// Deletes `pages` of the document made from `bytes` and checks that the note of the page that
/// stays kept its text, in the list of the page and in the saved file.
fn the_note_of_the_page_that_stays_keeps_its_words(bytes: Vec<u8>, pages: Vec<u32>) {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &bytes, None).unwrap();
    assert_eq!(texts(&list(e, doc, 0)), [KEPT]);
    delete(e, doc, pages);
    assert_eq!(texts(&list(e, doc, 0)), [KEPT], "listed after the delete");
    let saved = save_all(e, doc);
    assert!(
        contains(&saved, KEPT.as_bytes()),
        "the note of a page that stays lost its words in the saved file"
    );
}

#[test]
fn an_annotation_that_two_pages_share_keeps_its_words_when_one_page_goes() {
    the_note_of_the_page_that_stays_keeps_its_words(shared("[8 0 R]", &[]), vec![1]);
}

#[test]
fn an_annots_array_that_two_pages_share_keeps_its_words_when_one_page_goes() {
    the_note_of_the_page_that_stays_keeps_its_words(shared("9 0 R", &["[8 0 R]"]), vec![1]);
}

#[test]
fn a_page_listed_twice_keeps_its_notes_when_one_listing_goes() {
    // The page tree names page object 3 twice and page object 5 once.
    let bytes = build(
        "3 0 R 3 0 R 5 0 R",
        3,
        [page(4, "[8 0 R]"), page(6, "[]")],
        &[],
    );
    the_note_of_the_page_that_stays_keeps_its_words(bytes, vec![1]);
}

#[test]
fn a_note_made_in_the_session_still_goes_with_its_page_next_to_a_shared_one() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &shared("[8 0 R]", &[]), None).unwrap();
    // A note that only page 2 holds, made in this session: a save writes its dictionary even when
    // nothing refers to it any more.
    let made = annot(ask(
        e,
        RequestBody::AddNote {
            doc,
            page: 1,
            x: 100.0,
            y: 100.0,
            color: YELLOW,
            text: "SECRET-NOTE-OF-PAGE-TWO".into(),
        },
    ));
    assert_eq!(made.text, "SECRET-NOTE-OF-PAGE-TWO");
    delete(e, doc, vec![1]);
    assert_eq!(texts(&list(e, doc, 0)), [KEPT]);
    let saved = save_all(e, doc);
    assert!(contains(&saved, KEPT.as_bytes()));
    assert!(
        !contains(&saved, b"SECRET-NOTE-OF-PAGE-TWO"),
        "the words of a note that belonged to the deleted page are in the saved file"
    );
}
