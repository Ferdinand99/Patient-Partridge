//! Annotations with the real engine process, the real PDFium and the sandbox: the app asks for
//! annotations, a save writes them through the safe save path (a second sandboxed engine checks the
//! new file), and a fresh engine reads them back from the file on disk. The engine writes no file
//! of its own; the sandbox is as it was.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use partridge_engine_client::{
    AnnotKind, ClientError, EngineClient, EngineConfig, EnginePool, SandboxPolicy, SaveOptions,
    StampKind,
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
            "partridge-annotproc-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

const YELLOW: [u8; 4] = [255, 235, 59, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

fn pixel(bitmap: &partridge_engine_client::Bitmap, x: u32, y: u32) -> [u8; 3] {
    let at = ((y * bitmap.width + x) * 4) as usize;
    [bitmap.rgba[at], bitmap.rgba[at + 1], bitmap.rgba[at + 2]]
}

#[test]
fn every_kind_of_annotation_is_made_saved_and_read_back_with_the_sandbox_on() {
    let dir = Dir::new("all");
    let path = dir.write("a.pdf", &kit::features_pdf());
    let pool = EnginePool::new(config(), 4);
    let (handle, pages) = pool.open(&text(&path), None).unwrap();
    assert!(pages.len() >= 2);

    let page_text = pool.page_text(handle, 0).unwrap();
    let start = page_text.text.find("needle").unwrap();
    // Characters, not bytes: the text of this page is plain ASCII.
    let (start, end) = (start as u32, start as u32 + 6);
    let highlight = pool
        .add_markup(handle, 0, AnnotKind::Highlight, YELLOW, start, end)
        .unwrap();
    assert_eq!(highlight.kind, AnnotKind::Highlight);
    let note = pool
        .add_note(handle, 0, 400.0, 100.0, YELLOW, "A note \u{2713}")
        .unwrap();
    let ink = pool
        .add_ink(handle, 0, BLUE, 3.0, vec![vec![[300.0, 500.0], [400.0, 520.0], [450.0, 480.0]]])
        .unwrap();
    let stamp = pool
        .add_stamp(handle, 0, 100.0, 600.0, StampKind::Approved)
        .unwrap();
    // A change and a removal, then the same things put back.
    pool.update_annot(handle, 0, note.id, Some("A changed note"), None, 10.0, 10.0)
        .unwrap();
    pool.remove_annot(handle, 0, stamp.id).unwrap();
    pool.restore_annot(handle, 0, stamp.id).unwrap();
    assert_eq!(pool.list_annots(handle, 0).unwrap().len(), 4);

    // The save path checks the new file with a second sandboxed engine before it replaces the old.
    pool.save(handle, SaveOptions::default()).unwrap();

    let client = EngineClient::new(config());
    let opened = client.open(&text(&path), None).unwrap();
    let items = client.list_annots(opened.doc, 0).unwrap();
    let kinds: Vec<AnnotKind> = items.iter().map(|i| i.kind).collect();
    for kind in [
        AnnotKind::Highlight,
        AnnotKind::Note,
        AnnotKind::Ink,
        AnnotKind::Stamp,
    ] {
        assert!(kinds.contains(&kind), "{kind:?} is not in the saved file: {kinds:?}");
    }
    let saved_note = items.iter().find(|i| i.kind == AnnotKind::Note).unwrap();
    assert_eq!(saved_note.text, "A changed note");
    let _ = ink;

    // Drawn by the engine that read the file: the ink is blue where it was drawn.
    let width = opened.pages[0].width as u32;
    let height = opened.pages[0].height as u32;
    let bitmap = client.render(opened.doc, 0, width, height).unwrap();
    let mut blue_near = 0;
    for y in 475..525 {
        for x in 295..455 {
            let p = pixel(&bitmap, x, y);
            if p[2] > 200 && p[0] < 80 && p[1] < 80 {
                blue_near += 1;
            }
        }
    }
    assert!(blue_near > 150, "the drawing is not in the file ({blue_near})");
}

#[test]
fn a_protected_file_stays_protected_and_a_hostile_one_is_only_listed() {
    let dir = Dir::new("protected");
    let path = dir.write("p.pdf", &kit::encrypted_pdf("secret", 1));
    let pool = EnginePool::new(config(), 4);
    let (handle, _) = pool.open(&text(&path), Some("secret")).unwrap();
    pool.add_note(handle, 0, 50.0, 50.0, YELLOW, "ZZPROTECTEDZZ").unwrap();
    pool.save(handle, SaveOptions::default()).unwrap();
    let bytes = fs::read(&path).unwrap();
    assert!(!bytes.windows(13).any(|w| w == b"ZZPROTECTEDZZ"));
    let client = EngineClient::new(config());
    match client.open(&text(&path), None) {
        Err(ClientError::Engine {
            code: ErrorCode::PasswordRequired,
            ..
        }) => {}
        other => panic!("{other:?}"),
    }
    let opened = client.open(&text(&path), Some("secret")).unwrap();
    assert_eq!(client.list_annots(opened.doc, 0).unwrap()[0].text, "ZZPROTECTEDZZ");

    let hostile = dir.write("h.pdf", &kit::hostile_annots_pdf());
    let client = EngineClient::new(config());
    let opened = client.open(&text(&hostile), None).unwrap();
    let items = client.list_annots(opened.doc, 0).unwrap();
    assert_eq!(items.len(), 3);
    assert!(items.iter().all(|i| i.text.len() <= 4096));
    // The same engine is still fine afterwards.
    assert!(client.render(opened.doc, 0, 100, 130).is_ok());
}
