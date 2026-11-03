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
//!   protection. Its fingerprint is the one of the session just before the save, where both can be
//!   read in full. Every page looked at that the saved file, a fresh open of the original and the
//!   session can all read, and on which the last two agree, has the same size and the same text in
//!   the saved file. (The original's fingerprint is not used: the fields and annotations added
//!   before the save change the objects and annotations it counts.) A document that PDFium cannot
//!   write fails only with `SaveFailed`, or with `SaveMetadataNotEncrypted` when it is protected
//!   and has an XMP metadata stream.
//! - Forms: `FormInfo` answers, and for a form the reader fills in, the fields of each page are
//!   listed (within the caps, with a place on the page) and the first three are set to a value of
//!   their kind. A change is answered with the list or refused with `FormFieldRefused`; a form
//!   that is not an AcroForm is refused with `FormUnsupported`. The save that follows writes the
//!   changed form, and the check that reopens it holds for it too.
//! - Annotations: the list is inside the caps with ids that differ; a highlight, a note, a drawing
//!   and a stamp can be added (or are refused with an error that means "not this one"), one is
//!   changed, all are removed and put back with the ids they had, and an id that was never given
//!   out is refused. All of it happens before the save, so the saved file holds them and is looked
//!   at in the same way as the session.
//! - Every answer can be encoded and decoded by the app, and the encoder cuts nothing.
//! - A page that exists fails only with `Corrupt` or `RenderFailed`; a page past the end gives
//!   `PageOutOfRange`.

use std::cell::RefCell;
use std::path::PathBuf;

use libfuzzer_sys::fuzz_target;
use partridge_engine::Engine;
use partridge_fuzz::check_response_caps;
use partridge_protocol::{
    AnnotKind, ErrorCode, FormFieldKind, FormKind, FormValue, LinkTarget, PageSize, Request,
    RequestBody, Response, ResponseBody, StampKind, MAX_ANNOTS_PER_PAGE, MAX_SAVE_BYTES,
    MAX_SAVE_CHUNK, PROTOCOL_VERSION,
};

/// Longest side of the bitmaps the target draws. Small, so that an iteration is fast.
const RENDER_SIDE: f32 = 256.0;

/// Pages looked at per file, from the start, plus the last one.
const PAGES_PER_FILE: u32 = 3;

/// The password of the encrypted seed file.
const PASSWORD: &str = "secret";

/// The pages looked at in a file: the first few and the last one. They are the pages the target
/// changes (fields and annotations), and the pages whose text the saved file is compared on.
fn looked_at(count: u32) -> Vec<u32> {
    let mut pages: Vec<u32> = (0..count.min(PAGES_PER_FILE)).collect();
    if count > PAGES_PER_FILE {
        pages.push(count - 1);
    }
    pages
}

/// What a reader sees of a page without any mark on it: its size and its text. The text is `None`
/// when the page cannot be read.
type Look = (PageSize, Option<String>);

struct State {
    engine: Engine,
    file: PathBuf,
    next_id: u32,
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
        ResponseBody::FormFields { fields } => fields.len(),
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

        // The window asks this once, before the first page is drawn.
        self.form_kind(doc);
        for page in looked_at(count) {
            self.page(doc, page, count, pages[page as usize]);
            self.form(doc, page);
        }
        // Past the end is an error, never a crash.
        self.expect_range_error(RequestBody::PageText { doc, page: count });
        self.expect_range_error(RequestBody::Render {
            doc,
            page: count,
            width: 10,
            height: 10,
        });

        self.annotate(doc, count, &pages);

        self.save(doc, count, &pages);

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

    /// Annotations on the first pages: the list the file already has, then every kind added, one
    /// changed, removed and put back. Whatever the file, the answers are annotations inside the
    /// caps or one of the errors that mean "not this one", and the document stays usable.
    fn annotate(&mut self, doc: u32, count: u32, sizes: &[PageSize]) {
        for page in 0..count.min(2) {
            let size = sizes[page as usize];
            let listed = match self.ask(RequestBody::ListAnnots { doc, page }) {
                ResponseBody::Annots { items } => items,
                // A page that cannot be read has no annotations to work with.
                ResponseBody::Error {
                    code: ErrorCode::Corrupt,
                    ..
                } => continue,
                other => panic!("unexpected answer to ListAnnots: {other:?}"),
            };
            check_response_caps(&Response {
                id: 0,
                body: ResponseBody::Annots {
                    items: listed.clone(),
                },
            });
            let mut ids: Vec<u32> = listed.iter().map(|i| i.id).collect();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), listed.len(), "two annotations share an id");
            let text_len = match self.ask(RequestBody::PageText { doc, page }) {
                ResponseBody::PageText { rects, .. } => rects.len() as u32,
                _ => 0,
            };
            let mut made: Vec<(AnnotKind, u32)> = Vec::new();
            let requests = [
                (
                    AnnotKind::Highlight,
                    RequestBody::AddMarkup {
                        doc,
                        page,
                        kind: AnnotKind::Highlight,
                        color: [255, 235, 59, 255],
                        start: 0,
                        end: text_len.clamp(1, 40),
                    },
                ),
                (
                    AnnotKind::Note,
                    RequestBody::AddNote {
                        doc,
                        page,
                        x: size.width / 3.0,
                        y: size.height / 3.0,
                        color: [255, 235, 59, 255],
                        text: "fuzz".into(),
                    },
                ),
                (
                    AnnotKind::Ink,
                    RequestBody::AddInk {
                        doc,
                        page,
                        color: [0, 0, 255, 255],
                        width: 3.0,
                        strokes: vec![
                            vec![[10.0, 10.0], [size.width / 2.0, size.height / 2.0]],
                            vec![[size.width, size.height]],
                        ],
                    },
                ),
                (
                    AnnotKind::Stamp,
                    RequestBody::AddStamp {
                        doc,
                        page,
                        x: size.width / 2.0,
                        y: size.height / 2.0,
                        stamp: StampKind::Draft,
                    },
                ),
            ];
            for (kind, body) in requests {
                match self.ask(body) {
                    ResponseBody::Annot { item } => {
                        assert_eq!(item.kind, kind);
                        assert!(!ids.contains(&item.id), "a new annotation took an id in use");
                        ids.push(item.id);
                        check_response_caps(&Response {
                            id: 0,
                            body: ResponseBody::Annot { item: item.clone() },
                        });
                        made.push((kind, item.id));
                    }
                    ResponseBody::Error {
                        code:
                            ErrorCode::InvalidRequest
                            | ErrorCode::AnnotationRefused
                            | ErrorCode::AnnotationsNotAllowed
                            | ErrorCode::Corrupt,
                        ..
                    } => {}
                    other => panic!("unexpected answer to adding {kind:?}: {other:?}"),
                }
            }
            // The ones made are listed, and the others did not go away.
            let after = match self.ask(RequestBody::ListAnnots { doc, page }) {
                ResponseBody::Annots { items } => items,
                // A page of a damaged file that PDFium could load the first time may not load again
                // (a page tree that does not match its own count reads once and then not): the
                // changes to that page are over.
                ResponseBody::Error {
                    code: ErrorCode::Corrupt,
                    ..
                } => continue,
                other => panic!("unexpected answer to ListAnnots: {other:?}"),
            };
            if after.len() < MAX_ANNOTS_PER_PAGE {
                for (_, id) in &made {
                    assert!(after.iter().any(|i| i.id == *id), "annotation {id} is not listed");
                }
                for item in &listed {
                    assert!(after.iter().any(|i| i.id == item.id), "annotation {} was lost", item.id);
                }
            }
            if let Some((_, id)) = made.iter().find(|(k, _)| *k == AnnotKind::Note) {
                match self.ask(RequestBody::UpdateAnnot {
                    doc,
                    page,
                    id: *id,
                    text: Some("fuzz changed".into()),
                    color: None,
                    dx: 5.0,
                    dy: 5.0,
                }) {
                    ResponseBody::Annot { item } => assert_eq!(item.id, *id),
                    other => panic!("unexpected answer to a change: {other:?}"),
                }
            }
            // Remove each one made, then put each back: the same ids come back.
            for (_, id) in &made {
                match self.ask(RequestBody::RemoveAnnot { doc, page, id: *id }) {
                    ResponseBody::AnnotRemoved => {}
                    other => panic!("unexpected answer to a removal: {other:?}"),
                }
            }
            for (_, id) in &made {
                match self.ask(RequestBody::RestoreAnnot { doc, page, id: *id }) {
                    ResponseBody::Annot { item } => assert_eq!(item.id, *id),
                    ResponseBody::Error {
                        code: ErrorCode::AnnotationRefused,
                        ..
                    } => {}
                    other => panic!("unexpected answer to a restore: {other:?}"),
                }
            }
            // An id that was never given out, and a page past the end.
            for body in [
                RequestBody::RemoveAnnot {
                    doc,
                    page,
                    id: u32::MAX,
                },
                RequestBody::UpdateAnnot {
                    doc,
                    page,
                    id: u32::MAX,
                    text: None,
                    color: None,
                    dx: 1.0,
                    dy: 1.0,
                },
            ] {
                match self.ask(body) {
                    ResponseBody::Error {
                        code: ErrorCode::NoSuchAnnotation,
                        ..
                    } => {}
                    other => panic!("an annotation that does not exist gave {other:?}"),
                }
            }
        }
        self.expect_range_error(RequestBody::ListAnnots { doc, page: count });
    }

    /// The three requests of a save, and a look at the file that comes out.
    fn save(&mut self, doc: u32, count: u32, sizes: &[PageSize]) {
        let (pages, security) = match self.ask(RequestBody::DocInfo { doc }) {
            ResponseBody::DocInfo {
                pages, security, ..
            } => (pages, security),
            other => panic!("unexpected answer to DocInfo: {other:?}"),
        };
        // What the app looks at before a save and compares with the saved file afterwards.
        let session_print = match self.ask(RequestBody::Fingerprint { doc }) {
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
                code: ErrorCode::SaveFailed | ErrorCode::SaveMetadataNotEncrypted,
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

        // The original, read afresh as the saved file will be.
        let fresh = self.fresh_look(count);
        // And the pages of the document in this session, to tell the pages that read the same in a
        // session and in a fresh read from those that do not.
        let live = self.looks(doc, sizes, count);

        // The saved file opens again. (A file that PDFium had to repair may not; then the app's
        // check would refuse the save. One that opens must be the same document.)
        let saved = self.file.with_extension("saved.pdf");
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
                // Like for like: the session as it was just before the save, against the saved
                // file. Where both can be read in full the two must agree, which is what the app
                // checks before it accepts the file (`crates/engine-client/src/save.rs`). Both
                // hold the form changes and the annotations of this run.
                if session_print.0 == 0 {
                    if let ResponseBody::Fingerprint {
                        failed: 0, digest, ..
                    } = self.ask(RequestBody::Fingerprint { doc: again })
                    {
                        assert_eq!(
                            digest, session_print.1,
                            "the saved file does not have the pages of the session"
                        );
                    }
                }
                // A page the saved file shows must read as it does in the original: the same size
                // and the same text. Only that, because the fields and the annotations this target
                // adds change what the file holds besides (objects, annotations), and the pages
                // they sit on keep their size and their text. The original is read afresh for
                // this, not taken from the document the changes were made in: reading the outline
                // fills PDFium's list of pages, so a page that a fresh read cannot load (a page
                // tree that does not match its own count) reads in that session and not in the next
                // one, and the two are not like for like. For the same reason a page is only
                // compared when this session and the fresh read of the original agree about it
                // (a page tree with a kid that is not an object makes PDFium number the pages
                // differently in the two, and then it writes the tree it numbered in the session).
                // A page that any of the three cannot read is not compared either: PDFium repairs a
                // damaged file when it opens it and can write the repaired tree in a form it cannot
                // read back, and the app's check of a saved file refuses exactly that
                // (`crates/engine-client/src/save.rs`).
                if let Some(before) = fresh {
                    let after = self.looks(again, &pages, count);
                    for (((page, was), (_, now)), (_, here)) in
                        before.iter().zip(after.iter()).zip(live.iter())
                    {
                        let (Some(was_text), Some(now_text), Some(here_text)) =
                            (&was.1, &now.1, &here.1)
                        else {
                            continue;
                        };
                        if was_text != here_text || was.0 != here.0 {
                            continue;
                        }
                        assert_eq!(was_text, now_text, "page {page} of the saved file reads another text");
                        assert_eq!(was.0, now.0, "page {page} of the saved file has another size");
                    }
                }
                self.ask(RequestBody::Close { doc: again });
                break;
            }
        }
        let _ = std::fs::remove_file(&saved);
    }

    /// The size and the text of each page in `looked_at`, as an open document gives them. `sizes`
    /// are the page sizes the `Open` answer named.
    fn looks(&mut self, doc: u32, sizes: &[PageSize], count: u32) -> Vec<(u32, Look)> {
        looked_at(count)
            .into_iter()
            .map(|page| {
                let text = match self.ask(RequestBody::PageText { doc, page }) {
                    ResponseBody::PageText { text, .. } => Some(text),
                    _ => None,
                };
                (page, (sizes[page as usize], text))
            })
            .collect()
    }

    /// The pages of the input file, looked at in a document opened from nothing. `None` when it
    /// does not open (it did once, so that is rare).
    fn fresh_look(&mut self, count: u32) -> Option<Vec<(u32, Look)>> {
        for password in [None, Some(PASSWORD.to_string())] {
            self.hand_over();
            if let ResponseBody::Opened { doc, pages } = self.ask(RequestBody::Open { password }) {
                let looks = (pages.len() as u32 == count).then(|| self.looks(doc, &pages, count));
                self.ask(RequestBody::Close { doc });
                return looks;
            }
        }
        None
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

    /// What kind of form the document has now. It can change while the pages are drawn: PDFium adds
    /// an `/AcroForm` with no `/Fields` to a document when it makes the picture of a text box that has none.
    fn form_kind(&mut self, doc: u32) -> FormKind {
        match self.ask(RequestBody::FormInfo { doc }) {
            ResponseBody::FormInfo { kind, .. } => kind,
            // A form whose handle cannot be reached is told, not taken for a document without a form.
            ResponseBody::Error {
                code: ErrorCode::FormUnsupported,
                ..
            } => FormKind::None,
            other => panic!("unexpected answer to FormInfo: {other:?}"),
        }
    }

    /// The fields of a page, and a change to the first three of them.
    fn form(&mut self, doc: u32, page: u32) {
        let kind = self.form_kind(doc);
        let listed = self.ask(RequestBody::ListFormFields { doc, page });
        if kind != FormKind::Acro {
            match listed {
                ResponseBody::Error {
                    code: ErrorCode::FormUnsupported,
                    ..
                } => return,
                other => panic!("fields of a document that has no form the reader fills in: {other:?}"),
            }
        }
        let fields = match listed {
            ResponseBody::FormFields { fields } => fields,
            // A page that cannot be loaded.
            ResponseBody::Error {
                code: ErrorCode::PageOutOfRange | ErrorCode::Corrupt,
                ..
            } => return,
            other => panic!("unexpected answer to ListFormFields: {other:?}"),
        };
        for field in &fields {
            let [l, t, r, b] = field.rect;
            assert!(r - l >= 1.0 && b - t >= 1.0, "a field without a place: {:?}", field.rect);
            assert!(field.options.len() <= partridge_protocol::MAX_FORM_OPTIONS);
        }
        for field in fields.iter().take(3) {
            let value = match field.kind {
                FormFieldKind::Text => FormValue::Text("fuzz\nvalue (\\) \u{e5}".into()),
                FormFieldKind::Checkbox | FormFieldKind::Radio => FormValue::Checked(true),
                FormFieldKind::Combo | FormFieldKind::List => FormValue::Choice(vec![0]),
            };
            match self.ask(RequestBody::SetFormField {
                doc,
                page,
                index: field.index,
                name: field.name.clone(),
                value,
            }) {
                ResponseBody::FormFields { fields: after } => {
                    assert!(after.iter().any(|f| f.index == field.index));
                }
                ResponseBody::Error {
                    code: ErrorCode::FormFieldRefused,
                    ..
                } => {}
                other => panic!("unexpected answer to SetFormField: {other:?}"),
            }
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
