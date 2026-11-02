#![no_main]
//! Arbitrary bytes as a PDF file, through the engine's own request handler: open, outline, then
//! for the first pages and the last one the text, the links, searches and a small render, and
//! close. PDFium is the pinned build from `pdfium.lock.json`, loaded from
//! `PARTRIDGE_PDFIUM_DIR` or `vendor/pdfium` (see docs/fuzzing.md).
//!
//! PDFium is a prebuilt library without coverage instrumentation, so the fuzzer only sees which
//! paths the engine code takes (the result of each step), not the paths inside PDFium. Bugs in
//! PDFium are found by mutating real PDFs from `fuzz/seeds/engine_open_render` blindly, and a
//! crash inside the library shows up as a signal. Memory errors inside PDFium are not reported
//! by AddressSanitizer unless they hit a guard page.
//!
//! What is checked, on the answer as the engine builds it (before the encoder cuts any list):
//! - Open: page sizes are finite, positive and not above 14 400 points.
//! - Outline, Close: the right kind of answer. An outline has depth at most 15, titles at most
//!   512 bytes, and an entry that has a page names a page that exists. Close answers `Closed`,
//!   and the document is gone afterwards. (The check for at most 5 000 entries is there, but no
//!   input reaches it: such an outline is far bigger than the `-max_len`.)
//! - PageText: the check for at most 100 000 characters is there, but no input reaches it (the
//!   test kit writes no compressed streams, so such a page does not fit in the `-max_len`).
//!   One box for each character cannot fail here: the engine builds both from one list.
//! - PageLinks: at most 500 (the seed `links_over_the_cap` has 501), addresses not empty and at
//!   most 2 048 bytes, and a link to a page names a page that exists (the seeds include
//!   documents whose link and outline destinations are page numbers far past the end or
//!   negative).
//! - Search: matches are not empty, in order, do not overlap, and lie inside the text of the page
//!   (counted in characters of the `PageText` answer), and the text they cover reads like the
//!   search text (checked with a second, separate way of folding case and white space). Besides
//!   fixed words, two searches are for pieces of the page itself, so matches do happen.
//! - Render: a bitmap of exactly the size asked for.
//! - Save: `DocInfo` names the pages the document has. `SaveBegin` gives a size within the cap, and
//!   the pieces asked for one after the other add up to a file that starts like a PDF. A piece past
//!   the end, or longer than a piece may be, is refused. The saved file opens again (with the
//!   password, for the encrypted seed) with the same number of pages and the same kind of
//!   protection. A document that PDFium cannot write fails only with `SaveFailed`.
//! - The page tools, on the same document: a turn, a crop, a blank page, a delete and the copy it
//!   leaves (restore), a move of every page and the insert of a copy (only with PARTRIDGE_FUZZ_STRUCTURE set), the
//!   copy of the first page into a new document (which opens with one page and the digest the engine gave) and its insert. The number
//!   of pages after each is the number it should be, a refusal is one of a short list of errors, and
//!   the document, saved after all of it, opens again with the same pages and the same digest.
//! - Every answer can be encoded and decoded by the app, and the encoder cuts nothing.
//! - A page that exists fails only with `Corrupt` or `RenderFailed`; a page past the end gives
//!   `PageOutOfRange`.

use std::cell::RefCell;
use std::path::PathBuf;

use libfuzzer_sys::fuzz_target;
use partridge_engine::Engine;
use partridge_fuzz::check_response_caps;
use partridge_protocol::{
    ErrorCode, LinkTarget, PageSize, PagesRequest, PagesResponse, Request, RequestBody, Response,
    ResponseBody, MAX_SAVE_BYTES, MAX_SAVE_CHUNK, PROTOCOL_VERSION,
};

/// Longest side of the bitmaps the target draws. Small, so that an iteration is fast.
const RENDER_SIDE: f32 = 256.0;

/// Pages looked at per file, from the start, plus the last one.
const PAGES_PER_FILE: u32 = 3;

/// The password of the encrypted seed file.
const PASSWORD: &str = "secret";

struct State {
    engine: Engine,
    file: PathBuf,
    next_id: u32,
    /// A page tool failed half way and the engine closed the document (it answers `UnknownDocument`
    /// from then on). Only the page tools do that, and only for a document they could not put back.
    lost: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// The number of entries of a list answer (0 for answers without a list).
fn list_len(body: &ResponseBody) -> usize {
    match body {
        ResponseBody::Opened { pages, .. } => pages.len(),
        ResponseBody::Outline { items } => items.len(),
        ResponseBody::PageText { rects, .. } => rects.len(),
        ResponseBody::PageLinks { links } => links.len(),
        ResponseBody::Matches { matches } => matches.len(),
        _ => 0,
    }
}

/// Folds text the way a search is meant to read it: lower case, every run of white space is one
/// space, soft hyphens are dropped, spaces at both ends are cut. Written again here, apart from
/// the engine, to check what the engine reports as a match.
fn folded(chars: impl Iterator<Item = char>) -> String {
    let mut out = String::new();
    for c in chars {
        if c == '\u{AD}' {
            continue;
        }
        let f = if c.is_whitespace() {
            ' '
        } else {
            c.to_lowercase().next().unwrap_or(c)
        };
        if f == ' ' && out.ends_with(' ') {
            continue;
        }
        out.push(f);
    }
    out.trim_matches(' ').to_string()
}

impl State {
    fn new() -> State {
        let mut engine = Engine::new(&partridge_testkit::pdfium_dir());
        let hello = engine.handle(Request {
            id: 0,
            body: RequestBody::Hello {
                protocol_version: PROTOCOL_VERSION,
            },
        });
        // A target that cannot load PDFium would "pass" without testing anything: stop instead.
        if let ResponseBody::Error { code, message } = hello.body {
            panic!("PDFium is not usable ({code:?}): {message}");
        }
        State {
            engine,
            file: std::env::temp_dir().join(format!("partridge-fuzz-{}.pdf", std::process::id())),
            next_id: 1,
            lost: false,
        }
    }

    /// Sends one request and checks what every answer must satisfy.
    fn ask(&mut self, body: RequestBody) -> ResponseBody {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let response = self.engine.handle(Request { id, body });
        assert_eq!(response.id, id);
        // The caps, on the answer before the encoder cuts anything.
        check_response_caps(&response);
        // The app must be able to read the answer, and the encoder must not have cut a list.
        let decoded = Response::decode(response.encode()).expect("the app cannot decode an answer");
        assert_eq!(decoded.id, id);
        assert_eq!(
            list_len(&decoded.body),
            list_len(&response.body),
            "the encoder cut a list of the answer"
        );
        response.body
    }

    /// Gives the engine a read-only handle to the input file, as the app does when it starts an
    /// engine.
    fn hand_over(&mut self) {
        let file = std::fs::File::open(&self.file).expect("open the input file");
        self.engine.set_document(file);
    }

    fn run(&mut self, data: &[u8]) {
        std::fs::write(&self.file, data).expect("write the input file");
        // The engine opens only the document it was handed (the app does that in real life).
        self.hand_over();
        let mut opened = self.ask(RequestBody::Open { password: None });
        if matches!(
            opened,
            ResponseBody::Error {
                code: ErrorCode::PasswordRequired,
                ..
            }
        ) {
            self.hand_over();
            opened = self.ask(RequestBody::Open {
                password: Some(PASSWORD.to_string()),
            });
        }
        let (doc, pages) = match opened {
            ResponseBody::Opened { doc, pages } => (doc, pages),
            ResponseBody::Error { code, .. } => {
                assert!(
                    matches!(
                        code,
                        ErrorCode::Corrupt
                            | ErrorCode::PasswordRequired
                            | ErrorCode::PasswordIncorrect
                            | ErrorCode::Unsupported
                    ),
                    "unexpected error {code:?} for a file that exists"
                );
                return;
            }
            other => panic!("unexpected answer to Open: {other:?}"),
        };
        assert_ne!(doc, 0);
        for size in &pages {
            assert!(size.width.is_finite() && size.width > 0.0 && size.width <= 14_400.0);
            assert!(size.height.is_finite() && size.height > 0.0 && size.height <= 14_400.0);
        }
        let count = pages.len() as u32;

        match self.ask(RequestBody::Outline { doc }) {
            ResponseBody::Outline { items } => {
                for item in &items {
                    if let Some(p) = item.page {
                        assert!(p < count, "outline entry for page {p} of {count}");
                    }
                }
            }
            other => panic!("unexpected answer to Outline: {other:?}"),
        }

        let mut wanted: Vec<u32> = (0..count.min(PAGES_PER_FILE)).collect();
        if count > PAGES_PER_FILE {
            wanted.push(count - 1);
        }
        for page in wanted {
            self.page(doc, page, count, pages[page as usize]);
        }
        // Past the end is an error, never a crash.
        self.expect_range_error(RequestBody::PageText { doc, page: count });
        self.expect_range_error(RequestBody::Render {
            doc,
            page: count,
            width: 10,
            height: 10,
        });

        // The page tools work on the document as it was saved and opened again (see `save`).
        self.save(doc, count, data, 0);

        match self.ask(RequestBody::Close { doc }) {
            ResponseBody::Closed => {}
            other => panic!("unexpected answer to Close: {other:?}"),
        }
        match self.ask(RequestBody::Outline { doc }) {
            ResponseBody::Error {
                code: ErrorCode::UnknownDocument,
                ..
            } => {}
            other => panic!("a closed document answered: {other:?}"),
        }
    }

    /// Asks a page tool. A refusal must be one of the errors a tool may give; any other answer than
    /// a page tool answer is a bug.
    fn tool(&mut self, op: PagesRequest) -> Option<PagesResponse> {
        if std::env::var_os("PARTRIDGE_FUZZ_TRACE").is_some() {
            eprintln!("page tool: {op:?}");
        }
        match self.ask(RequestBody::Pages(op)) {
            ResponseBody::Pages(answer) => Some(answer),
            ResponseBody::Error { code, .. } => {
                assert!(
                    matches!(
                        code,
                        ErrorCode::InvalidRequest
                            | ErrorCode::PageOutOfRange
                            | ErrorCode::PageChangeFailed
                            | ErrorCode::TooManyPages
                            | ErrorCode::SaveTooLarge
                            | ErrorCode::SourceChanged
                            | ErrorCode::SourceUnreadable
                            | ErrorCode::ImportFailed
                            | ErrorCode::SnapshotGone
                            | ErrorCode::SaveFailed
                            | ErrorCode::UnknownDocument
                    ),
                    "a page tool gave {code:?}"
                );
                if code == ErrorCode::UnknownDocument {
                    self.lost = true;
                }
                None
            }
            other => panic!("a page tool was answered with {other:?}"),
        }
    }

    /// How many pages the document has now, as the engine says.
    fn pages_now(&mut self, doc: u32) -> u32 {
        match self.ask(RequestBody::DocInfo { doc }) {
            ResponseBody::DocInfo { pages, .. } => pages,
            // A document the page tools had to close.
            ResponseBody::Error {
                code: ErrorCode::UnknownDocument,
                ..
            } => {
                self.lost = true;
                0
            }
            other => panic!("unexpected answer to DocInfo: {other:?}"),
        }
    }

    /// A page tool that changes the document: asks it, and checks that the answer names the pages
    /// the document really has (as DocInfo says), that the number is the number it should be
    /// (`expected`, when the change has one), and that every size is a size. Returns the number of
    /// the copy to go back to, or None when the tool refused.
    fn change(&mut self, doc: u32, op: PagesRequest, expected: Option<u32>) -> Option<u32> {
        match self.tool(op) {
            Some(PagesResponse::Changed {
                pages: sizes,
                snapshot,
                ..
            }) => {
                if let Some(expected) = expected {
                    assert_eq!(sizes.len() as u32, expected, "the number of pages after a change");
                }
                for s in &sizes {
                    assert!(s.width.is_finite() && s.width > 0.0 && s.width <= 14_400.0);
                    assert!(s.height.is_finite() && s.height > 0.0 && s.height <= 14_400.0);
                }
                assert_eq!(
                    sizes.len() as u32,
                    self.pages_now(doc),
                    "a change reports other pages than the document has"
                );
                Some(snapshot)
            }
            None => None,
            Some(other) => panic!("a change was answered with {other:?}"),
        }
    }

    /// The page tools, one after the other. Returns the number of pages the document has at the
    /// end (0 when it was closed). The pages and the places are taken from the bytes of the input,
    /// so that the same input does the same. The number of pages is read from the engine before
    /// each tool: a document whose copies PDFium cannot read again (a damaged one) may refuse to go
    /// back, and the tools must still report what the document has.
    fn tools(&mut self, doc: u32, count: u32, data: &[u8]) -> u32 {
        let seed = data
            .iter()
            .fold(0x9E37_79B9u32, |a, b| a.wrapping_mul(31).wrapping_add(u32::from(*b)));
        let pick = |n: u32, salt: u32| {
            if n == 0 {
                0
            } else {
                (seed ^ salt.wrapping_mul(0x85EB_CA6B)) % n
            }
        };
        if count == 0 {
            return 0;
        }

        // Turn one page; crop one page; add a blank page.
        let pages = self.pages_now(doc);
        let turned = self.change(
            doc,
            PagesRequest::Rotate {
                doc,
                pages: vec![pick(pages, 1)],
                turns: 1 + (pick(3, 2) as u8),
            },
            Some(pages),
        );
        let _ = turned;
        let pages = self.pages_now(doc);
        match self.tool(PagesRequest::Crop {
            doc,
            pages: vec![pick(pages, 3)],
            margins: [1.0, 2.0, 3.0, 4.0],
        }) {
            Some(PagesResponse::Cropped {
                pages: sizes,
                before,
                after,
            }) => {
                assert_eq!(sizes.len() as u32, pages);
                assert_eq!(before.len(), 1);
                assert_eq!(after.len(), 1);
            }
            None => {}
            Some(other) => panic!("a crop was answered with {other:?}"),
        }
        let pages = self.pages_now(doc);
        self.change(
            doc,
            PagesRequest::InsertBlank {
                doc,
                at: pick(pages + 1, 4),
                width: 200.0,
                height: 300.0,
            },
            Some(pages + 1),
        );

        // Delete a page and take it back; move every page and take it back. (Going back may be
        // refused for a document that PDFium cannot read again; then it stays as it is.)
        let pages = self.pages_now(doc);
        if pages >= 2 {
            let deleted = self.change(
                doc,
                PagesRequest::Delete {
                    doc,
                    pages: vec![pick(pages, 5)],
                },
                Some(pages - 1),
            );
            if let Some(snapshot) = deleted.filter(|s| *s != 0) {
                self.change(doc, PagesRequest::Restore { doc, snapshot }, None);
            }
        }
        // A move, and pages put into this document, are tried only when asked for
        // (PARTRIDGE_FUZZ_STRUCTURE): on a file damaged in the right way PDFium crashes in the work that
        // follows (a SEGV inside the library, see docs/fuzzing.md), which the sandbox contains, and a
        // target that stops on it every few minutes finds nothing else. They are tested on made-up
        // documents in the engine tests.
        let structure = std::env::var_os("PARTRIDGE_FUZZ_STRUCTURE").is_some();
        let pages = self.pages_now(doc);
        if (2..=40).contains(&pages) && structure {
            let moved = self.change(
                doc,
                PagesRequest::Reorder {
                    doc,
                    order: (0..pages).rev().collect(),
                },
                Some(pages),
            );
            if let Some(snapshot) = moved.filter(|s| *s != 0) {
                self.change(doc, PagesRequest::Restore { doc, snapshot }, None);
            }
        }

        // Copy the first page into a new document, and look at it.
        if let Some(PagesResponse::Exported {
            size,
            pages: copied,
            digest,
            ..
        }) = self.tool(PagesRequest::ExportBegin {
            doc,
            pages: vec![0],
        }) {
            assert_eq!(copied, 1);
            let mut bytes: Vec<u8> = Vec::new();
            while bytes.len() < size as usize {
                let len = (size as usize - bytes.len()).min(MAX_SAVE_CHUNK);
                match self.ask(RequestBody::SaveChunk {
                    doc,
                    offset: bytes.len() as u32,
                    len: len as u32,
                }) {
                    ResponseBody::SaveData { data, .. } => bytes.extend_from_slice(&data),
                    other => panic!("unexpected answer to SaveChunk: {other:?}"),
                }
            }
            self.ask(RequestBody::SaveEnd { doc });
            assert!(bytes.starts_with(b"%PDF-"), "a copy that is no PDF");
            let copy = self.file.with_extension("copy.pdf");
            std::fs::write(&copy, &bytes).expect("write the copy");
            self.engine
                .set_document(std::fs::File::open(&copy).expect("open the copy"));
            if let ResponseBody::Opened { doc: other, pages: n } =
                self.ask(RequestBody::Open { password: None })
            {
                assert_eq!(n.len(), 1, "the copy of one page has other pages");
                if let ResponseBody::Fingerprint { failed: 0, digest: d, .. } =
                    self.ask(RequestBody::Fingerprint { doc: other })
                {
                    assert_eq!(d, digest, "the copy is not the document the engine described");
                }
                self.ask(RequestBody::Close { doc: other });
            }
            // And put it in the document, in pieces of an odd size.
            let pages = self.pages_now(doc);
            if !structure {
                // Not into this document (see above); the bytes are handed over and dropped, to try the
                // pieces and the end of an insert that is given up.
                if let Some(PagesResponse::ImportReady) = self.tool(PagesRequest::ImportBegin {
                    doc,
                    size: bytes.len() as u32,
                }) {
                    self.tool(PagesRequest::ImportChunk {
                        doc,
                        offset: 0,
                        data: bytes[..bytes.len().min(3001)].to_vec(),
                    });
                    self.tool(PagesRequest::ImportAbort { doc });
                    // A piece after the abort has nothing to belong to.
                    assert!(self
                        .tool(PagesRequest::ImportChunk {
                            doc,
                            offset: 0,
                            data: vec![1],
                        })
                        .is_none());
                }
            } else if let Some(PagesResponse::ImportReady) = self.tool(PagesRequest::ImportBegin {
                doc,
                size: bytes.len() as u32,
            }) {
                let mut sent = 0usize;
                while sent < bytes.len() {
                    let end = (sent + 3001).min(bytes.len());
                    match self.tool(PagesRequest::ImportChunk {
                        doc,
                        offset: sent as u32,
                        data: bytes[sent..end].to_vec(),
                    }) {
                        Some(PagesResponse::ImportAck { received }) => {
                            assert_eq!(received as usize, end);
                        }
                        other => panic!("a piece was answered with {other:?}"),
                    }
                    sent = end;
                }
                self.change(doc, PagesRequest::ImportEnd { doc, at: 0 }, Some(pages + 1));
            }
            let _ = std::fs::remove_file(&copy);
        }
        self.pages_now(doc)
    }

    /// The three requests of a save, and a look at the file that comes out.
    ///
    /// `level` 0 is the document as it was opened; the page tools are then tried on the saved file
    /// opened again, which is how a document has been written out by PDFium (and so is in a form
    /// PDFium can be expected to handle; a file as damaged as the fuzzer makes it can crash PDFium in
    /// a change of the pages, which the engine's sandbox contains, but which is no finding of this
    /// target). What the tools leave is saved and checked again, at `level` 1.
    fn save(&mut self, doc: u32, count: u32, data: &[u8], level: u8) {
        let (pages, security) = match self.ask(RequestBody::DocInfo { doc }) {
            ResponseBody::DocInfo {
                pages, security, ..
            } => (pages, security),
            other => panic!("unexpected answer to DocInfo: {other:?}"),
        };
        let print = match self.ask(RequestBody::Fingerprint { doc }) {
            ResponseBody::Fingerprint {
                pages: seen,
                failed,
                digest,
            } => {
                assert_eq!(seen, count, "the fingerprint names another number of pages");
                (failed, digest)
            }
            other => panic!("unexpected answer to Fingerprint: {other:?}"),
        };
        assert_eq!(pages, count, "DocInfo names another number of pages");
        let size = match self.ask(RequestBody::SaveBegin { doc }) {
            ResponseBody::SaveReady { size } => size as usize,
            ResponseBody::Error {
                code: ErrorCode::SaveFailed,
                ..
            } => return,
            other => panic!("unexpected answer to SaveBegin: {other:?}"),
        };
        assert!(size <= MAX_SAVE_BYTES);
        // Pieces of a size that is not a divisor of anything.
        let mut bytes: Vec<u8> = Vec::with_capacity(size);
        while bytes.len() < size {
            let len = (size - bytes.len()).min(4099);
            match self.ask(RequestBody::SaveChunk {
                doc,
                offset: bytes.len() as u32,
                len: len as u32,
            }) {
                ResponseBody::SaveData { offset, data } => {
                    assert_eq!(offset as usize, bytes.len());
                    assert_eq!(data.len(), len);
                    bytes.extend_from_slice(&data);
                }
                other => panic!("unexpected answer to SaveChunk: {other:?}"),
            }
        }
        assert!(bytes.starts_with(b"%PDF-"), "a saved file that is no PDF");
        // The end of the file, a piece past it, and a piece longer than a piece may be.
        for body in [
            RequestBody::SaveChunk {
                doc,
                offset: size as u32,
                len: 1,
            },
            RequestBody::SaveChunk {
                doc,
                offset: u32::MAX,
                len: 2,
            },
            RequestBody::SaveChunk {
                doc,
                offset: 0,
                len: MAX_SAVE_CHUNK as u32 + 1,
            },
        ] {
            match self.ask(body) {
                ResponseBody::Error {
                    code: ErrorCode::InvalidRequest,
                    ..
                } => {}
                other => panic!("a piece that cannot exist gave {other:?}"),
            }
        }
        match self.ask(RequestBody::SaveEnd { doc }) {
            ResponseBody::SaveEnded => {}
            other => panic!("unexpected answer to SaveEnd: {other:?}"),
        }
        match self.ask(RequestBody::SaveChunk {
            doc,
            offset: 0,
            len: 1,
        }) {
            ResponseBody::Error {
                code: ErrorCode::InvalidRequest,
                ..
            } => {}
            other => panic!("the buffer outlived SaveEnd: {other:?}"),
        }

        // The saved file opens again. (A file that PDFium had to repair may not; then the app's
        // check would refuse the save. One that opens must be the same document.)
        let saved = self.file.with_extension(format!("saved{level}.pdf"));
        std::fs::write(&saved, &bytes).expect("write the saved file");
        for password in [None, Some(PASSWORD.to_string())] {
            self.engine
                .set_document(std::fs::File::open(&saved).expect("open the saved file"));
            if let ResponseBody::Opened { doc: again, pages } = self.ask(RequestBody::Open { password }) {
                assert_eq!(pages.len() as u32, count, "the saved file has other pages");
                match self.ask(RequestBody::DocInfo { doc: again }) {
                    ResponseBody::DocInfo {
                        security: now, ..
                    } => assert_eq!(now, security, "the saved file is protected differently"),
                    other => panic!("unexpected answer to DocInfo: {other:?}"),
                }
                // What a reader sees of the saved file is what it saw of the document. (A page that
                // could not be read before cannot be expected to read now.)
                if print.0 == 0 {
                    match self.ask(RequestBody::Fingerprint { doc: again }) {
                        ResponseBody::Fingerprint { failed, digest, .. } => {
                            assert_eq!(failed, 0, "pages of the saved file cannot be read");
                            assert_eq!(digest, print.1, "the saved file is another document");
                        }
                        other => panic!("unexpected answer to Fingerprint: {other:?}"),
                    }
                }
                if level == 0 {
                    self.lost = false;
                    let now = self.tools(again, count, data);
                    if !self.lost && now != count {
                        self.save(again, now, data, 1);
                    }
                }
                self.ask(RequestBody::Close { doc: again });
                break;
            }
        }
        let _ = std::fs::remove_file(&saved);
    }

    fn expect_range_error(&mut self, body: RequestBody) {
        match self.ask(body) {
            ResponseBody::Error {
                code: ErrorCode::PageOutOfRange,
                ..
            } => {}
            other => panic!("a page past the end gave {other:?}"),
        }
    }

    fn page(&mut self, doc: u32, page: u32, count: u32, size: PageSize) {
        // Which fonts the engine had to refuse: at most the seven scripts, each once, in order.
        match self.ask(RequestBody::FontReport) {
            ResponseBody::FontReport { missed } => {
                assert!(
                    missed.windows(2).all(|w| w[0].code() < w[1].code()),
                    "{missed:?}"
                );
            }
            other => panic!("a font report was answered with {other:?}"),
        }
        // The text of the page when it could be read, and two pieces of it to search for.
        let mut text: Option<Vec<char>> = None;
        let mut from_text: Vec<String> = Vec::new();
        match self.ask(RequestBody::PageText { doc, page }) {
            ResponseBody::PageText { text: t, .. } => {
                let chars: Vec<char> = t.chars().collect();
                let middle = chars.len() / 2;
                for start in [0, middle] {
                    from_text.push(chars.iter().skip(start).take(8).collect());
                }
                text = Some(chars);
            }
            other => self.expect_page_error(&other),
        }

        match self.ask(RequestBody::PageLinks { doc, page }) {
            ResponseBody::PageLinks { links } => {
                for link in &links {
                    match &link.target {
                        LinkTarget::Page(p) => {
                            assert!(*p < count, "link to page {p} of {count}");
                        }
                        LinkTarget::Uri(u) => assert!(!u.is_empty()),
                    }
                }
            }
            other => self.expect_page_error(&other),
        }

        let mut queries = vec!["the".to_string(), "needle".to_string(), "a b".to_string()];
        queries.extend(from_text);
        for query in queries {
            let found = self.ask(RequestBody::Search {
                doc,
                page,
                query: query.clone(),
            });
            match found {
                ResponseBody::Matches { matches } => {
                    let mut end = 0u64;
                    for m in &matches {
                        assert!(m.len > 0, "an empty match");
                        assert!(m.start as u64 >= end, "matches overlap or are out of order");
                        end = m.start as u64 + m.len as u64;
                        if let Some(chars) = &text {
                            assert!(
                                end <= chars.len() as u64,
                                "match up to {end} on a page of {} characters",
                                chars.len()
                            );
                            // What the match covers must read like the search text.
                            let covered = &chars[m.start as usize..end as usize];
                            assert_eq!(
                                folded(covered.iter().copied()),
                                folded(query.chars()),
                                "a match that does not read like {query:?}"
                            );
                        }
                    }
                }
                other => self.expect_page_error(&other),
            }
        }

        let scale = RENDER_SIDE / size.width.max(size.height);
        let width = ((size.width * scale).round() as u32).clamp(1, RENDER_SIDE as u32);
        let height = ((size.height * scale).round() as u32).clamp(1, RENDER_SIDE as u32);
        match self.ask(RequestBody::Render {
            doc,
            page,
            width,
            height,
        }) {
            ResponseBody::Rendered {
                width: w,
                height: h,
                rgba,
            } => {
                assert_eq!((w, h), (width, height));
                assert_eq!(rgba.len(), (w * h * 4) as usize);
            }
            other => self.expect_page_error(&other),
        }
    }

    /// A page that exists may fail to read or draw, but only with these errors.
    fn expect_page_error(&self, answer: &ResponseBody) {
        match answer {
            ResponseBody::Error {
                code: ErrorCode::Corrupt | ErrorCode::RenderFailed,
                ..
            } => {}
            other => panic!("unexpected answer for a page that exists: {other:?}"),
        }
    }
}

fuzz_target!(|data: &[u8]| {
    STATE.with(|cell| {
        let mut slot = cell.borrow_mut();
        slot.get_or_insert_with(State::new).run(data);
    });
});
