//! The picture of a signature with the real engine process, the real PDFium and the sandbox: the
//! app asks for a picture three ways (lines, typed text, pixels) over the pipe, moves and resizes
//! it, a save writes it through the safe save path (a second sandboxed engine checks the new file),
//! and a fresh engine reads it back from the file on disk. The picture is a stamp and nothing more:
//! the saved file has no signature in it.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use partridge_engine_client::{
    AnnotKind, Bitmap, ClientError, EngineClient, EngineConfig, EnginePool, SandboxPolicy,
    SaveOptions, SignatureFont, SignatureMark,
};
use partridge_protocol::ErrorCode;
use partridge_testkit as kit;

fn config() -> EngineConfig {
    let mut config = EngineConfig::new(env!("CARGO_BIN_EXE_partridge-engine"));
    config.args = vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
    // The tests insist on the sandbox: an engine that cannot lock itself in fails them.
    config.sandbox = SandboxPolicy::Required;
    config
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "partridge-sigproc-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const INK: [u8; 4] = [10, 40, 120, 255];

fn scribble() -> SignatureMark {
    let line: Vec<[f32; 2]> = (0..=40)
        .map(|i| {
            let t = i as f32 / 40.0;
            [10.0 + t * 200.0, 50.0 + 30.0 * (t * 13.0).sin()]
        })
        .collect();
    SignatureMark::Strokes {
        width: 240.0,
        height: 100.0,
        line: 5.0,
        strokes: vec![line],
    }
}

fn pixels() -> SignatureMark {
    let mut rgba = Vec::new();
    for y in 0..30u32 {
        for x in 0..90u32 {
            if (x / 10 + y / 10) % 2 == 0 {
                rgba.extend_from_slice(&[200, 30, 30, 255]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    SignatureMark::Image {
        width: 90,
        height: 30,
        rgba,
    }
}

/// How many pixels of the box are not white.
fn inked(bitmap: &Bitmap, b: [f32; 4]) -> usize {
    let mut n = 0;
    for y in (b[1].floor() as u32)..(b[3].ceil() as u32).min(bitmap.height) {
        for x in (b[0].floor() as u32)..(b[2].ceil() as u32).min(bitmap.width) {
            let at = ((y * bitmap.width + x) * 4) as usize;
            if bitmap.rgba[at..at + 3].iter().any(|c| *c < 240) {
                n += 1;
            }
        }
    }
    n
}

fn grown(r: [f32; 4]) -> [f32; 4] {
    [r[0] - 2.0, r[1] - 2.0, r[2] + 2.0, r[3] + 2.0]
}

#[test]
fn a_picture_is_made_three_ways_moved_resized_saved_and_read_back_with_the_sandbox_on() {
    let dir = Dir::new("all");
    let path = dir.0.join("a.pdf");
    fs::write(&path, kit::blank_pages_pdf(1)).unwrap();
    let text = path.to_string_lossy().into_owned();
    let pool = EnginePool::new(config(), 4);
    let (handle, pages) = pool.open(&text, None).unwrap();
    assert_eq!(pages.len(), 1);

    let drawn = pool
        .add_signature(handle, 0, [60.0, 80.0, 300.0, 180.0], INK, scribble())
        .unwrap();
    let typed = pool
        .add_signature(
            handle,
            0,
            [60.0, 260.0, 360.0, 360.0],
            INK,
            SignatureMark::Typed {
                font: SignatureFont::GreatVibes,
                text: "Anna Alvorlig".into(),
            },
        )
        .unwrap();
    let image = pool
        .add_signature(handle, 0, [60.0, 440.0, 300.0, 540.0], INK, pixels())
        .unwrap();
    for item in [&drawn, &typed, &image] {
        assert_eq!(item.kind, AnnotKind::Signature);
    }
    // Moved and resized, taken off and put back.
    let moved = pool
        .update_annot(handle, 0, drawn.id, None, None, 200.0, 20.0)
        .unwrap();
    assert!((moved.rect[0] - drawn.rect[0] - 200.0).abs() < 0.2);
    let wider = pool
        .place_signature(
            handle,
            0,
            image.id,
            [image.rect[0], image.rect[1], image.rect[0] + 120.0, 0.0],
        )
        .unwrap();
    assert!((wider.rect[2] - wider.rect[0] - 120.0).abs() < 0.3);
    pool.remove_annot(handle, 0, typed.id).unwrap();
    pool.restore_annot(handle, 0, typed.id).unwrap();
    assert_eq!(pool.list_annots(handle, 0).unwrap().len(), 3);

    pool.save(handle, SaveOptions::default()).unwrap();

    // A fresh engine reads the file: three stamps, and nothing that makes a digital signature.
    let client = EngineClient::new(config());
    let opened = client.open(&text, None).unwrap();
    let items = client.list_annots(opened.doc, 0).unwrap();
    assert_eq!(items.len(), 3);
    assert!(items.iter().all(|i| i.kind == AnnotKind::Stamp));
    let bytes = fs::read(&path).unwrap();
    for forbidden in [&b"/ByteRange"[..], b"/AcroForm", b"/PKCS7", b"/FontFile"] {
        assert!(
            !bytes.windows(forbidden.len()).any(|w| w == forbidden),
            "{} is in the file",
            String::from_utf8_lossy(forbidden)
        );
    }
    let report = client.signatures(opened.doc, 1_700_000_000).unwrap();
    assert_eq!(report.items.len(), 0, "a picture is not a signature");

    // It is drawn where the engine put it, and nowhere else.
    let bitmap = client.render(opened.doc, 0, 612, 792).unwrap();
    let total = inked(&bitmap, [0.0, 0.0, 612.0, 792.0]);
    let mut sum = 0;
    for item in [&moved, &typed, &wider] {
        let n = inked(&bitmap, grown(item.rect));
        assert!(n > 100, "{:?} drew only {n} pixels", item.rect);
        sum += n;
    }
    assert_eq!(sum, total, "ink outside the pictures");
}

#[test]
fn a_picture_over_a_cap_is_refused_by_the_client_before_the_engine_is_asked() {
    let dir = Dir::new("caps");
    let path = dir.0.join("a.pdf");
    fs::write(&path, kit::blank_pages_pdf(1)).unwrap();
    let pool = EnginePool::new(config(), 4);
    let (handle, _) = pool.open(&path.to_string_lossy(), None).unwrap();
    let rect = [60.0, 80.0, 300.0, 180.0];
    let too_many = SignatureMark::Strokes {
        width: 100.0,
        height: 50.0,
        line: 2.0,
        strokes: vec![vec![[1.0, 1.0]]; 65],
    };
    let long_text = SignatureMark::Typed {
        font: SignatureFont::Allura,
        text: "a".repeat(41),
    };
    let big_image = SignatureMark::Image {
        width: 1_025,
        height: 1,
        rgba: vec![0; 1_025 * 4],
    };
    for (what, mark) in [
        ("65 strokes", too_many),
        ("41 characters", long_text),
        ("1025 pixels wide", big_image),
    ] {
        match pool.add_signature(handle, 0, rect, INK, mark) {
            Err(ClientError::Engine {
                code: ErrorCode::InvalidRequest,
                ..
            }) => {}
            other => panic!("{what}: {other:?}"),
        }
    }
    assert!(pool.list_annots(handle, 0).unwrap().is_empty());
    // A letter the font does not have is refused by the engine, which stays fine.
    match pool.add_signature(
        handle,
        0,
        rect,
        INK,
        SignatureMark::Typed {
            font: SignatureFont::Kristi,
            text: "\u{65e5}\u{672c}".into(),
        },
    ) {
        Err(ClientError::Engine {
            code: ErrorCode::InvalidRequest,
            ..
        }) => {}
        other => panic!("{other:?}"),
    }
    assert!(pool.add_signature(handle, 0, rect, INK, scribble()).is_ok());
}
