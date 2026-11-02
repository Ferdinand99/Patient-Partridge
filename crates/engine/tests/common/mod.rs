#![allow(dead_code)]
//! Helpers of the engine tests: an engine shared by the tests of a file, a way to open a
//! document from bytes, to save it, and to look at a page drawn, and a file the engine reads that a
//! test can make fail, change or slow down while the document is open.

use std::fs::File;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime};

use partridge_engine::source::{SourceFile, BLOCK};
use partridge_engine::Engine;
use partridge_protocol::{AnnotInfo, ErrorCode, Request, RequestBody, ResponseBody};
use partridge_testkit as kit;
use pdfium_render::prelude::*;

/// The engine holds PDFium handles that are not `Send`. Only one test at a time touches it.
pub struct Shared(pub Engine);
unsafe impl Send for Shared {}

pub fn engine() -> MutexGuard<'static, Shared> {
    static ENGINE: OnceLock<Mutex<Shared>> = OnceLock::new();
    ENGINE
        .get_or_init(|| Mutex::new(Shared(Engine::new(&kit::pdfium_dir()))))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

pub fn ask(engine: &mut Engine, body: RequestBody) -> ResponseBody {
    engine.handle(Request { id: 1, body }).body
}

pub fn error_code(body: ResponseBody) -> ErrorCode {
    match body {
        ResponseBody::Error { code, .. } => code,
        other => panic!("expected an error, got {other:?}"),
    }
}

/// Opens `bytes` as a document; returns its id.
pub fn open(engine: &mut Engine, bytes: &[u8], password: Option<&str>) -> Result<u32, ErrorCode> {
    let file = kit::TempFile::new("annots.pdf", bytes);
    engine.set_document(File::open(file.path()).unwrap());
    match ask(
        engine,
        RequestBody::Open {
            password: password.map(str::to_string),
        },
    ) {
        ResponseBody::Opened { doc, .. } => Ok(doc),
        other => Err(error_code(other)),
    }
}

/// As `open`, with the size of every page as it is drawn, in whole points.
pub fn open_sized(
    engine: &mut Engine,
    bytes: &[u8],
    password: Option<&str>,
) -> Result<(u32, Vec<(u32, u32)>), ErrorCode> {
    let file = kit::TempFile::new("annots.pdf", bytes);
    engine.set_document(File::open(file.path()).unwrap());
    match ask(
        engine,
        RequestBody::Open {
            password: password.map(str::to_string),
        },
    ) {
        ResponseBody::Opened { doc, pages } => Ok((
            doc,
            pages
                .iter()
                .map(|p| (p.width.round() as u32, p.height.round() as u32))
                .collect(),
        )),
        other => Err(error_code(other)),
    }
}

pub fn save_all(engine: &mut Engine, doc: u32) -> Vec<u8> {
    let ResponseBody::SaveReady { size } = ask(engine, RequestBody::SaveBegin { doc }) else {
        panic!("no SaveReady");
    };
    let mut out = Vec::new();
    while (out.len() as u32) < size {
        let len = (1 << 20).min(size - out.len() as u32);
        match ask(
            engine,
            RequestBody::SaveChunk {
                doc,
                offset: out.len() as u32,
                len,
            },
        ) {
            ResponseBody::SaveData { data, .. } => out.extend_from_slice(&data),
            other => panic!("{other:?}"),
        }
    }
    assert!(matches!(
        ask(engine, RequestBody::SaveEnd { doc }),
        ResponseBody::SaveEnded
    ));
    out
}

pub fn list(engine: &mut Engine, doc: u32, page: u32) -> Vec<AnnotInfo> {
    match ask(engine, RequestBody::ListAnnots { doc, page }) {
        ResponseBody::Annots { items } => items,
        other => panic!("{other:?}"),
    }
}

pub fn annot(body: ResponseBody) -> AnnotInfo {
    match body {
        ResponseBody::Annot { item } => item,
        other => panic!("expected an annotation, got {other:?}"),
    }
}

pub fn page_text(engine: &mut Engine, doc: u32, page: u32) -> (String, Vec<[f32; 4]>) {
    match ask(engine, RequestBody::PageText { doc, page }) {
        ResponseBody::PageText { text, rects } => (text, rects),
        other => panic!("{other:?}"),
    }
}

/// The first match of `query` on a page: where it starts and how long it is, in characters.
pub fn find(engine: &mut Engine, doc: u32, page: u32, query: &str) -> (u32, u32) {
    match ask(
        engine,
        RequestBody::Search {
            doc,
            page,
            query: query.into(),
        },
    ) {
        ResponseBody::Matches { matches } => (matches[0].start, matches[0].len),
        other => panic!("{other:?}"),
    }
}

/// The box that surrounds the characters `start..start + len`, on the page as it is drawn.
pub fn text_box(engine: &mut Engine, doc: u32, page: u32, start: u32, len: u32) -> [f32; 4] {
    let (_, rects) = page_text(engine, doc, page);
    let used = &rects[start as usize..(start + len) as usize];
    let mut out = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for r in used.iter().filter(|r| r[2] > r[0]) {
        out = [
            out[0].min(r[0]),
            out[1].min(r[1]),
            out[2].max(r[2]),
            out[3].max(r[3]),
        ];
    }
    out
}

/// A page drawn at one pixel to the point, as RGBA.
pub struct Shot {
    pub width: usize,
    pub rgba: Vec<u8>,
}

impl Shot {
    pub fn take(engine: &mut Engine, doc: u32, page: u32, width: u32, height: u32) -> Shot {
        match ask(
            engine,
            RequestBody::Render {
                doc,
                page,
                width,
                height,
            },
        ) {
            ResponseBody::Rendered { rgba, width, .. } => Shot {
                width: width as usize,
                rgba,
            },
            other => panic!("{other:?}"),
        }
    }

    pub fn pixel(&self, x: usize, y: usize) -> [u8; 3] {
        let at = (y * self.width + x) * 4;
        [self.rgba[at], self.rgba[at + 1], self.rgba[at + 2]]
    }

    /// How many pixels of the box are not white.
    pub fn inked(&self, b: [f32; 4]) -> usize {
        let mut n = 0;
        for y in b[1].floor() as usize..b[3].ceil() as usize {
            for x in b[0].floor() as usize..b[2].ceil() as usize {
                if self.pixel(x, y).iter().any(|c| *c < 240) {
                    n += 1;
                }
            }
        }
        n
    }

    /// How many pixels of the box are clearly of this colour: each channel within `tolerance`.
    pub fn near(&self, b: [f32; 4], color: [u8; 3], tolerance: i32) -> usize {
        let mut n = 0;
        for y in b[1].floor() as usize..b[3].ceil() as usize {
            for x in b[0].floor() as usize..b[2].ceil() as usize {
                let p = self.pixel(x, y);
                if (0..3).all(|i| (p[i] as i32 - color[i] as i32).abs() <= tolerance) {
                    n += 1;
                }
            }
        }
        n
    }
}

pub fn close(a: [f32; 4], b: [f32; 4], tolerance: f32) -> bool {
    a.iter()
        .zip(b.iter())
        .all(|(x, y)| (x - y).abs() <= tolerance)
}

/// Looks at the annotations of a page the way another program would: through PDFium, with the
/// appearance stream. Returns, for each annotation of the type, how many objects its appearance
/// holds.
pub fn appearance_objects(
    engine: &mut Engine,
    doc: u32,
    page: u32,
    kind: PdfPageAnnotationType,
) -> Vec<usize> {
    engine
        .edit_for_tests(doc, |document| {
            let page = document.pages().get(page as PdfPageIndex).unwrap();
            let mut out = Vec::new();
            for annotation in page.annotations().iter() {
                if annotation.annotation_type() == kind {
                    out.push(annotation.objects().len());
                }
            }
            out
        })
        .expect("the document is open")
}

pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

pub const YELLOW: [u8; 4] = [255, 235, 59, 255];

/// No block fails.
pub const NO_BLOCK: u64 = u64::MAX;

/// What a test does to the file while the engine has it open.
#[derive(Clone)]
pub struct Controls {
    /// The bytes of the file; a test may change them.
    pub data: Arc<Mutex<Vec<u8>>>,
    /// The block (of 32 KiB) whose reads fail, or [`NO_BLOCK`].
    pub fail_block: Arc<AtomicU64>,
    /// Every read takes a tenth of a second.
    pub slow: Arc<AtomicBool>,
}

impl Controls {
    pub fn fail_reads_of_block(&self, block: u64) {
        self.fail_block.store(block, Ordering::SeqCst);
    }

    pub fn reads_work_again(&self) {
        self.fail_block.store(NO_BLOCK, Ordering::SeqCst);
    }
}

pub struct ControlledFile {
    controls: Controls,
}

impl ControlledFile {
    pub fn new(bytes: &[u8]) -> (ControlledFile, Controls) {
        let controls = Controls {
            data: Arc::new(Mutex::new(bytes.to_vec())),
            fail_block: Arc::new(AtomicU64::new(NO_BLOCK)),
            slow: Arc::new(AtomicBool::new(false)),
        };
        (
            ControlledFile {
                controls: controls.clone(),
            },
            controls,
        )
    }
}

impl SourceFile for ControlledFile {
    fn read_at(&mut self, pos: u64, buf: &mut [u8]) -> io::Result<usize> {
        if self.controls.slow.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }
        if pos / BLOCK == self.controls.fail_block.load(Ordering::SeqCst) {
            return Err(io::Error::other("the share dropped this read"));
        }
        let data = self.controls.data.lock().unwrap();
        let pos = (pos as usize).min(data.len());
        let n = buf.len().min(data.len() - pos);
        buf[..n].copy_from_slice(&data[pos..pos + n]);
        Ok(n)
    }

    fn state(&mut self) -> io::Result<(u64, Option<SystemTime>)> {
        Ok((self.controls.data.lock().unwrap().len() as u64, None))
    }

    fn is_plain_file(&mut self) -> bool {
        true
    }
}
