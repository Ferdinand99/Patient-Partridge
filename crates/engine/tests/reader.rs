//! Tests of what the reader asks the engine besides drawing: the outline, the text of a page with
//! the place of every character, search and links. They run the real engine with the real PDFium
//! library, through the same client the app uses. Needs the PDFium library: run
//! `node scripts/fetch-pdfium.mjs` first, or set `PARTRIDGE_PDFIUM_DIR`.

use std::time::Duration;

use partridge_engine_client::{ClientError, EngineClient, EngineConfig, LinkTarget};
use partridge_protocol::ErrorCode;
use partridge_testkit as kit;

fn client() -> EngineClient {
    let mut config = EngineConfig::new(env!("CARGO_BIN_EXE_partridge-engine"));
    config.args = vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
    EngineClient::new(config)
}

fn code_of(err: ClientError) -> ErrorCode {
    match err {
        ClientError::Engine { code, .. } => code,
        other => panic!("expected an engine error, got {other:?}"),
    }
}

fn open_features(
    client: &EngineClient,
) -> (partridge_engine_client::OpenedDocument, kit::TempFile) {
    let file = kit::TempFile::new("features.pdf", &kit::features_pdf());
    let doc = client.open(&file.path_str(), None).unwrap();
    (doc, file)
}

#[test]
fn the_outline_lists_entries_with_their_pages() {
    let client = client();
    let (doc, _file) = open_features(&client);
    let items = client.outline(doc.doc).unwrap();
    let shown: Vec<(u8, Option<u32>, &str)> = items
        .iter()
        .map(|i| (i.depth, i.page, i.title.as_str()))
        .collect();
    assert_eq!(
        shown,
        vec![
            (0, Some(0), "Chapter 1"),
            (1, Some(1), "Section 1.1"),
            // An entry that opens a web address leads to no page, and is not followed.
            (0, None, "External"),
            // An entry with a GoTo action is found too.
            (0, Some(2), "Chapter 3"),
        ]
    );
}

#[test]
fn a_document_without_an_outline_gives_an_empty_list() {
    let file = kit::TempFile::new("plain.pdf", &kit::simple_pdf(2));
    let client = client();
    let doc = client.open(&file.path_str(), None).unwrap();
    assert!(client.outline(doc.doc).unwrap().is_empty());
}

#[test]
fn an_outline_that_loops_ends() {
    let file = kit::TempFile::new("loop.pdf", &kit::outline_loop_pdf());
    let client = client();
    let doc = client.open(&file.path_str(), None).unwrap();
    let started = std::time::Instant::now();
    let items = client.outline(doc.doc).unwrap();
    assert_eq!(items.len(), 1);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(client.spawn_count(), 1);
}

#[test]
fn a_very_long_outline_is_cut_at_the_limit() {
    let file = kit::TempFile::new("wide.pdf", &kit::outline_wide_pdf(6_000));
    let client = client();
    let doc = client.open(&file.path_str(), None).unwrap();
    let items = client.outline(doc.doc).unwrap();
    assert_eq!(items.len(), partridge_protocol::MAX_OUTLINE_ITEMS);
    assert_eq!(items[0].title, "Item 0");
    assert_eq!(items[1].page, Some(1));
}

#[test]
fn a_very_deep_outline_is_cut_at_the_depth_limit() {
    let file = kit::TempFile::new("deep.pdf", &kit::outline_deep_pdf(40));
    let client = client();
    let doc = client.open(&file.path_str(), None).unwrap();
    let items = client.outline(doc.doc).unwrap();
    assert_eq!(
        items.len(),
        partridge_protocol::MAX_OUTLINE_DEPTH as usize + 1
    );
    assert_eq!(
        items.last().unwrap().depth,
        partridge_protocol::MAX_OUTLINE_DEPTH
    );
}

#[test]
fn page_text_has_one_box_per_character_in_the_right_place() {
    let client = client();
    let (doc, _file) = open_features(&client);
    let page = client.page_text(doc.doc, 0).unwrap();
    assert_eq!(page.text.chars().count(), page.rects.len());
    assert!(page.text.contains("Hello Partridge"), "{:?}", page.text);
    assert!(page.text.contains("A needle in a haystack."));

    let chars: Vec<char> = page.text.chars().collect();
    let h = chars.iter().position(|c| *c == 'H').unwrap();
    let [left, top, right, bottom] = page.rects[h];
    // The title starts at x = 72 and its baseline is 52 points below the top edge.
    assert!((70.0..76.0).contains(&left), "left {left}");
    assert!(top > 20.0 && top < 60.0, "top {top}");
    assert!(bottom > top && bottom < 80.0, "bottom {bottom}");
    assert!(
        right > left && right - left < 30.0,
        "width {}",
        right - left
    );
    // The next letter is to the right of the first.
    assert!(page.rects[h + 1][0] >= right - 1.0);
    // Every box with a place is inside the page.
    for [l, t, r, b] in &page.rects {
        if (*l, *t, *r, *b) != (0.0, 0.0, 0.0, 0.0) {
            assert!(*l >= -1.0 && *r <= 613.0 && *t >= -1.0 && *b <= 793.0);
        }
    }
    // Lines further down the page come later in reading order.
    let needle = page.text.find("needle").unwrap();
    let needle_char = page.text[..needle].chars().count();
    assert!(page.rects[needle_char][1] > page.rects[h][1] + 200.0);
}

#[test]
fn text_of_a_rotated_page_is_placed_on_the_page_as_it_is_drawn() {
    let client = client();
    let (doc, _file) = open_features(&client);
    // The page is 612 x 792 and turned a quarter turn: it is drawn 792 wide and 612 high.
    assert_eq!((doc.pages[1].width, doc.pages[1].height), (792.0, 612.0));
    let page = client.page_text(doc.doc, 1).unwrap();
    assert!(page.text.starts_with("Rotated page"), "{:?}", page.text);
    // The text now runs downwards: the boxes of the first letters go down, and stay in one column.
    let r = &page.rects;
    assert!(
        r[1][1] > r[0][1] && r[2][1] > r[1][1] && r[3][1] > r[2][1],
        "{r:?}"
    );
    assert!((r[0][0] - r[3][0]).abs() < 4.0, "{r:?}");
    assert!(r[0][0] > 700.0 && r[0][2] < 792.0, "{r:?}");
}

#[test]
fn a_page_without_text_gives_empty_text() {
    let file = kit::TempFile::new("blank.pdf", &kit::encrypted_pdf("x", 1));
    let client = client();
    let doc = client.open(&file.path_str(), Some("x")).unwrap();
    let page = client.page_text(doc.doc, 0).unwrap();
    assert!(page.text.is_empty());
    assert!(page.rects.is_empty());
}

#[test]
fn search_finds_text_on_a_page_ignoring_case_and_line_breaks() {
    let client = client();
    let (doc, _file) = open_features(&client);
    let page = client.page_text(doc.doc, 0).unwrap();
    let chars: Vec<char> = page.text.chars().collect();
    let squash = |from: u32, len: u32| -> String {
        let slice: String = chars[from as usize..(from + len) as usize].iter().collect();
        slice
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };

    let found = client.search_page(doc.doc, 0, "needle").unwrap();
    assert_eq!(found.len(), 2);
    for m in &found {
        assert_eq!(squash(m.start, m.len), "needle");
    }
    // A search text that spans the end of a line.
    let found = client.search_page(doc.doc, 0, "QUICK   brown").unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(squash(found[0].start, found[0].len), "quick brown");

    assert_eq!(
        client
            .search_page(doc.doc, 0, "Hello PARTRIDGE")
            .unwrap()
            .len(),
        1
    );
    assert!(client.search_page(doc.doc, 0, "zebra").unwrap().is_empty());
    assert!(client.search_page(doc.doc, 0, "").unwrap().is_empty());
    // The demo text on page 3 has the same phrase on 30 lines.
    assert_eq!(
        client.search_page(doc.doc, 2, "lazy dog").unwrap().len(),
        30
    );
}

#[test]
fn a_search_text_that_is_too_long_is_refused() {
    let client = client();
    let (doc, _file) = open_features(&client);
    let long = "x".repeat(partridge_protocol::MAX_QUERY_LEN + 1);
    assert_eq!(
        code_of(client.search_page(doc.doc, 0, &long).unwrap_err()),
        ErrorCode::InvalidRequest
    );
    assert_eq!(client.spawn_count(), 1);
}

#[test]
fn links_are_reported_but_only_web_and_page_links() {
    let client = client();
    let (doc, _file) = open_features(&client);
    let links = client.page_links(doc.doc, 0).unwrap();
    let targets: Vec<LinkTarget> = links.iter().map(|l| l.target.clone()).collect();
    assert_eq!(
        targets,
        vec![
            LinkTarget::Uri("https://example.com/page".into()),
            // The reader decides what to do with a scheme like this: the engine only reports it.
            LinkTarget::Uri("javascript:alert(1)".into()),
            LinkTarget::Page(2),
            // The link that launches a program is not reported, and neither is the attachment.
        ]
    );
    // The first link is at x 72..172 and 700..720 up from the bottom, so 72..92 from the top.
    let [l, t, r, b] = links[0].rect;
    assert!(
        (l - 72.0).abs() < 1.0 && (r - 172.0).abs() < 1.0,
        "{:?}",
        links[0].rect
    );
    assert!(
        (t - 72.0).abs() < 1.0 && (b - 92.0).abs() < 1.0,
        "{:?}",
        links[0].rect
    );
    // The other pages have none.
    assert!(client.page_links(doc.doc, 1).unwrap().is_empty());
}

#[test]
fn the_number_of_links_on_a_page_is_limited() {
    let file = kit::TempFile::new("links.pdf", &kit::many_links_pdf(600));
    let client = client();
    let doc = client.open(&file.path_str(), None).unwrap();
    assert_eq!(
        client.page_links(doc.doc, 0).unwrap().len(),
        partridge_protocol::MAX_LINKS_PER_PAGE
    );
}

#[test]
fn a_document_with_an_attachment_can_be_read_without_a_problem() {
    // This only shows that nothing crashes. It cannot see whether an attachment was opened: that is
    // kept from happening by the check in scripts/check-engine-deps.sh, not by this test.
    // The file holds an embedded file and a file attachment annotation. Opening, drawing and
    // reading everything the reader reads must work, and nothing may start a program: the engine
    // has no code that opens attachments (scripts/check-engine-deps.sh keeps it that way).
    let client = client();
    let (doc, _file) = open_features(&client);
    for page in 0..3 {
        assert!(client.render(doc.doc, page, 200, 260).is_ok());
        assert!(client.page_text(doc.doc, page).is_ok());
        assert!(client.page_links(doc.doc, page).is_ok());
    }
    assert!(client.outline(doc.doc).is_ok());
    assert_eq!(client.spawn_count(), 1);
}

#[test]
fn the_new_requests_check_document_and_page() {
    let client = client();
    let (doc, _file) = open_features(&client);
    assert_eq!(
        code_of(client.page_text(doc.doc, 3).unwrap_err()),
        ErrorCode::PageOutOfRange
    );
    assert_eq!(
        code_of(client.page_links(doc.doc, u32::MAX).unwrap_err()),
        ErrorCode::PageOutOfRange
    );
    assert_eq!(
        code_of(client.search_page(doc.doc, 9, "a").unwrap_err()),
        ErrorCode::PageOutOfRange
    );
    // A handle of this engine that was never given out reaches the engine, which refuses it.
    assert_eq!(
        code_of(client.outline(doc.doc + 100).unwrap_err()),
        ErrorCode::UnknownDocument
    );
    client.kill();
    for err in [
        client.outline(doc.doc).unwrap_err(),
        client.page_text(doc.doc, 0).unwrap_err(),
        client.page_links(doc.doc, 0).unwrap_err(),
        client.search_page(doc.doc, 0, "a").unwrap_err(),
    ] {
        assert_eq!(code_of(err), ErrorCode::UnknownDocument);
    }
    assert_eq!(client.spawn_count(), 1);
}

/// Anything the engine's libraries print must not end up in the protocol.
#[test]
fn stray_output_does_not_reach_the_protocol() {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let exe = std::env::current_exe().unwrap();
    let probe = exe
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join(format!("stdout_probe{}", std::env::consts::EXE_SUFFIX));
    let mut child = Command::new(probe)
        .stdout(Stdio::piped())
        .spawn()
        .expect("the probe is built with the tests");
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(child.wait().unwrap().success());
    let mut cursor = std::io::Cursor::new(bytes);
    let frame = partridge_protocol::read_frame(&mut cursor)
        .unwrap()
        .unwrap();
    let response = partridge_protocol::Response::decode(frame).unwrap();
    assert_eq!(response.id, 7);
    // Nothing else came out: no garbage before the frame, none after it.
    assert!(partridge_protocol::read_frame(&mut cursor)
        .unwrap()
        .is_none());
}

#[test]
fn right_to_left_text_comes_in_reading_order_with_the_first_letter_at_the_right_end() {
    let file = kit::TempFile::new("rtl.pdf", &kit::rtl_pdf());
    let client = client();
    let doc = client.open(&file.path_str(), None).unwrap();
    let page = client.page_text(doc.doc, 0).unwrap();
    // The letters are drawn from the left to the right of the line, but PDFium gives them in
    // reading order, so the text is the way it is written and the boxes run from right to left.
    let lines: Vec<&str> = page.text.split('\n').collect();
    assert_eq!(
        lines,
        vec![kit::RTL_LINE, "\u{5d5}\u{5d6}\u{5d7}\u{5d8}\u{5d9}"]
    );
    let chars: Vec<char> = page.text.chars().collect();
    assert_eq!(chars.len(), page.rects.len());
    for line in [0..10, 11..16] {
        let boxes = &page.rects[line];
        for pair in boxes.windows(2) {
            // Each letter sits to the left of the one before it, on the same line.
            assert!(pair[1][2] <= pair[0][0] + 0.5, "{pair:?}");
            assert_eq!(pair[1][1], pair[0][1], "{pair:?}");
        }
    }
    // The first letter is the rightmost one, near x = 405 (the line starts at x = 300).
    assert!(
        page.rects[0][0] > 400.0 && page.rects[0][2] < 420.0,
        "{:?}",
        page.rects[0]
    );
    assert!(page.rects[9][0] < 305.0, "{:?}", page.rects[9]);
    // The line break has no box, and the second line is lower on the page.
    assert_eq!(page.rects[10], [0.0, 0.0, 0.0, 0.0]);
    assert!(page.rects[11][1] > page.rects[0][1] + 30.0);
}
