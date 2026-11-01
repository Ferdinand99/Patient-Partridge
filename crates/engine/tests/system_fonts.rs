//! Text in a script that the PDF does not carry a font for (Chinese, Japanese, Korean, Arabic,
//! Hebrew, Thai): the app picks a system font file from a fixed table and hands the engine a
//! read-only handle to it; the engine reads it when a page needs it.
//!
//! The pages here use fonts that are not embedded. Each test page is drawn twice: with the fonts the
//! app found on this system, and without (the control), and the first must show more ink than the
//! second. A script for which this system has no font in the table is reported and skipped; the
//! window then shows blanks and a notice (see `src/app`).

use partridge_engine_client::system_fonts;
use partridge_engine_client::{Bitmap, EngineClient, EngineConfig, SandboxPolicy};
use partridge_protocol::FontScript;
use partridge_testkit as kit;

fn config(system_fonts: bool) -> EngineConfig {
    let mut config = EngineConfig::new(env!("CARGO_BIN_EXE_partridge-engine"));
    config.args = vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
    config.sandbox = SandboxPolicy::Required;
    config.system_fonts = system_fonts;
    config
}

/// Dark pixels in the band of the page where the text is (700 points up, 48 points high).
fn ink(bitmap: &Bitmap) -> usize {
    let row = (bitmap.width * 4) as usize;
    let top = (792 - 760) as usize;
    let bottom = (792 - 690) as usize;
    bitmap.rgba[top * row..bottom * row]
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] < 110 && p[1] < 110 && p[2] < 110)
        .count()
}

fn draw(script: &str, system_fonts: bool) -> usize {
    let file = kit::TempFile::new("script.pdf", &kit::script_pdf(script));
    let client = EngineClient::new(config(system_fonts));
    let doc = client.open(&file.path_str(), None).unwrap();
    let bitmap = client.render(doc.doc, 0, 612, 792).unwrap();
    // The text is there to read, whether or not it can be drawn.
    let text = client.page_text(doc.doc, 0).unwrap();
    assert!(!text.text.trim().is_empty(), "{script}: no text");
    // The engine stays up.
    assert_eq!(client.spawn_count(), 1, "{script}: the engine restarted");
    ink(&bitmap)
}

#[test]
fn what_this_system_has() {
    let found = system_fonts::found();
    println!("system fonts found: {found:#?}");
    // Every file comes from the fixed table: an absolute path to a font file.
    for (_, path) in found {
        assert!(path.is_absolute());
    }
}

#[test]
fn a_page_in_each_script_is_drawn_with_the_font_the_app_found() {
    let available = system_fonts::available();
    let mut drawn = Vec::new();
    let mut skipped = Vec::new();
    for name in kit::SCRIPT_NAMES {
        let script = FontScript::from_name(name).unwrap();
        if !available.contains(&script) {
            skipped.push(name);
            continue;
        }
        let with = draw(name, true);
        let without = draw(name, false);
        println!("{name}: {with} dark pixels with the system font, {without} without");
        drawn.push((name, with, without));
    }
    println!("scripts drawn: {drawn:?}; no font on this system: {skipped:?}");
    // Every script that this system has a font for must be drawn from it, and must not be without
    // it (the control).
    for (name, with, without) in &drawn {
        assert!(
            with > &(without + 150),
            "{name}: {with} dark pixels with the font and {without} without"
        );
    }
}

#[test]
fn a_page_without_a_font_for_its_script_shows_blanks_and_the_engine_stays_up() {
    for name in kit::SCRIPT_NAMES {
        let _ = draw(name, false);
    }
}

/// The engine holds the document, one handle for each font, the three standard descriptors and the
/// copy of its output, and nothing else.
#[cfg(unix)]
#[test]
fn the_engine_holds_only_the_handles_it_was_given() {
    use std::io::Read;
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::Duration;

    let fonts = system_fonts::open_all();
    let document = kit::TempFile::new("handles.pdf", &kit::simple_pdf(1));
    let handle = std::fs::File::open(document.path()).unwrap();
    let selftest = {
        let exe = std::env::current_exe().unwrap();
        exe.parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("examples")
            .join("engine_selftest")
    };
    let args: Vec<std::ffi::OsString> = vec![
        "--pdfium-dir".into(),
        kit::pdfium_dir().into_os_string(),
        "descriptors".into(),
    ];
    let mut engine =
        partridge_engine_client::spawn_engine(&partridge_engine_client::SpawnRequest {
            program: &selftest,
            args: &args,
            document: Some(&handle),
            memory_limit: None,
            policy: SandboxPolicy::Required,
            fonts: &fonts,
        })
        .unwrap();
    drop(engine.stdin);
    let out = Arc::new(Mutex::new(String::new()));
    let (done_tx, done) = mpsc::channel();
    {
        let out = out.clone();
        let mut stdout = engine.stdout;
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stdout.read_to_string(&mut text);
            *out.lock().unwrap() = text;
            let _ = done_tx.send(());
        });
    }
    done.recv_timeout(Duration::from_secs(60))
        .expect("the program ended");
    let _ = engine.process.wait();
    let text = out.lock().unwrap().clone();
    let count: usize = text
        .lines()
        .find_map(|l| l.strip_prefix("descriptors: "))
        .unwrap_or_else(|| panic!("no count in {text}"))
        .trim()
        .parse()
        .unwrap();
    // 0, 1, 2; the document; one for each font; the copy of the output.
    assert_eq!(count, 3 + 1 + fonts.len() + 1, "{text}");
}
