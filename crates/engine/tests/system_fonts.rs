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
    draw_named(script, kit::default_font_name(script), system_fonts)
}

fn draw_named(script: &str, font: &str, system_fonts: bool) -> usize {
    let file = kit::TempFile::new("script.pdf", &kit::script_pdf_with_font(script, font));
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

/// Scripts that this system must have a font for (`PARTRIDGE_REQUIRE_FONT_SCRIPTS`, names separated
/// by commas, or `all`). CI sets it for what a normal install of each runner has; anywhere else a
/// script without a font is reported and skipped.
fn required() -> Vec<FontScript> {
    let Ok(list) = std::env::var("PARTRIDGE_REQUIRE_FONT_SCRIPTS") else {
        return Vec::new();
    };
    if list.trim() == "all" {
        return FontScript::ALL.to_vec();
    }
    list.split(',')
        .filter(|n| !n.trim().is_empty())
        .map(|n| FontScript::from_name(n.trim()).unwrap_or_else(|| panic!("no script {n}")))
        .collect()
}

/// Adds a line to the summary of the CI run, if there is one.
fn summary(line: &str) {
    use std::io::Write;
    if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(path) {
            let _ = writeln!(file, "{line}");
        }
    }
}

#[test]
fn what_this_system_has() {
    let found = system_fonts::found();
    println!("system fonts found: {found:#?}");
    // Every file comes from the fixed table: an absolute path to a font file.
    for (_, path) in found {
        assert!(path.is_absolute());
    }
    for script in FontScript::ALL {
        match found.iter().find(|(s, _)| *s == script) {
            Some((_, path)) => println!("{}: {}", script.name(), path.display()),
            None => println!("{}: NO FONT ON THIS SYSTEM", script.name()),
        }
    }
    for script in required() {
        assert!(
            found.iter().any(|(s, _)| *s == script),
            "{}: this system must have a font for it, and the table finds none",
            script.name()
        );
    }
}

#[test]
fn a_page_in_each_script_is_drawn_with_the_font_the_app_found() {
    let found = system_fonts::found();
    let mut drawn = Vec::new();
    let mut skipped = Vec::new();
    summary("| script | font handed to the engine | dark pixels with it | dark pixels without |");
    summary("|---|---|---|---|");
    for name in kit::SCRIPT_NAMES {
        let script = FontScript::from_name(name).unwrap();
        let Some((_, path)) = found.iter().find(|(s, _)| *s == script) else {
            skipped.push(name);
            let without = draw(name, false);
            println!("{name}: no font on this system; blanks ({without} dark pixels)");
            println!(
                "::notice title=No font for {name}::This runner has none of the fonts the app looks for, so the {name} page is drawn as blanks."
            );
            summary(&format!("| {name} | none on this system | - | {without} |"));
            continue;
        };
        let with = draw(name, true);
        let without = draw(name, false);
        println!(
            "{name}: {} gives {with} dark pixels, {without} without",
            path.display()
        );
        summary(&format!(
            "| {name} | `{}` | {with} | {without} |",
            path.display()
        ));
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
    for script in required() {
        assert!(
            !skipped.contains(&script.name()),
            "{}: this system must draw it",
            script.name()
        );
    }
}

/// Names that documents use for their unembedded fonts, by script. CJK fonts are asked for by
/// character set, so any name gets the font of the script. The other three are asked for by name
/// only, and the engine knows the names that were made for one script.
const NAMED: [(&str, &[&str]); 7] = [
    (
        "japanese",
        &[
            "MS-Gothic",
            "MS-PMincho",
            "HeiseiKakuGo-W5",
            "KozMinPro-Regular",
            "Meiryo",
        ],
    ),
    (
        "korean",
        &["Gulim", "Malgun Gothic", "HYSMyeongJo-Medium", "Dotum"],
    ),
    (
        "chinese-simplified",
        &["SimHei", "Microsoft YaHei", "STSong-Light", "FangSong"],
    ),
    (
        "chinese-traditional",
        &[
            "PMingLiU",
            "Microsoft JhengHei",
            "MSung-Light",
            "MHei-Medium",
        ],
    ),
    (
        "arabic",
        &[
            "SimplifiedArabic",
            "ArabicTypesetting",
            "Sakkal Majalla",
            "Andalus",
        ],
    ),
    (
        "hebrew",
        &["David-Bold", "Miriam", "FrankRuehl", "Narkisim"],
    ),
    (
        "thai",
        &["AngsanaNew", "Cordia New", "TH SarabunPSK", "BrowalliaUPC"],
    ),
];

#[test]
fn the_names_documents_use_find_the_font_of_their_script() {
    let found = system_fonts::found();
    let mut failures = Vec::new();
    summary("| script | font name in the PDF | dark pixels |");
    summary("|---|---|---|");
    for (name, fonts) in NAMED {
        let script = FontScript::from_name(name).unwrap();
        if !found.iter().any(|(s, _)| *s == script) {
            println!("{name}: no font on this system, names not tried");
            continue;
        }
        for font in fonts {
            let ink = draw_named(name, font, true);
            println!("{name} as {font}: {ink} dark pixels");
            summary(&format!("| {name} | {font} | {ink} |"));
            if ink < 150 {
                failures.push(format!("{name} as {font}: {ink}"));
            }
        }
    }
    assert!(failures.is_empty(), "drawn as blanks: {failures:?}");
}

/// Names of the Asian Windows and Macintosh fonts in the code page of their language, as such PDFs
/// write them in `/BaseFont` (a name with `#` and two digits for each byte). They are not UTF-8.
/// PDFium asks for a font by character set and these bytes, and the engine must still serve the
/// font of the script.
const LEGACY_NAMES: [(&str, &[&str]); 4] = [
    // MS Gothic and MS Mincho in Shift-JIS
    (
        "japanese",
        &[
            "#82#6C#82#72#20#83#53#83#56#83#62#83#4E",
            "#82#6C#82#72#20#96#BE#92#A9",
        ],
    ),
    // Batang and Gulim in EUC-KR
    ("korean", &["#B9#D9#C5#C1", "#B1#BC#B8#B2"]),
    // SimSun and SimHei in GBK
    ("chinese-simplified", &["#CB#CE#CC#E5", "#BA#DA#CC#E5"]),
    // MingLiU and DFKai-SB in Big5
    (
        "chinese-traditional",
        &["#B7#73#B2#D3#A9#FA#C5#E9", "#BC#D0#B7#C7#C5#E9"],
    ),
];

#[test]
fn font_names_in_a_code_page_find_the_font_of_their_script() {
    let found = system_fonts::found();
    let mut failures = Vec::new();
    for (name, fonts) in LEGACY_NAMES {
        let script = FontScript::from_name(name).unwrap();
        if !found.iter().any(|(s, _)| *s == script) {
            println!("{name}: no font on this system, names not tried");
            continue;
        }
        for font in fonts {
            let ink = draw_named(name, font, true);
            println!("{name} as {font}: {ink} dark pixels");
            summary(&format!("| {name} | `{font}` (code page) | {ink} |"));
            if ink < 150 {
                failures.push(format!("{name} as {font}: {ink}"));
            }
        }
    }
    assert!(failures.is_empty(), "drawn as blanks: {failures:?}");
}

/// One page with 150 unembedded fonts of different names would, with a font file of 20 MB or more
/// behind each name, make PDFium hold 3 GB. The engine hands PDFium a bounded number of bytes and
/// the rest of the page stays blank; the engine stays up and keeps working.
#[test]
fn a_page_with_150_font_names_stays_under_the_memory_limit_and_keeps_working() {
    if !system_fonts::available().contains(&FontScript::Japanese) {
        println!("japanese: no font on this system, nothing to hold");
        return;
    }
    let file = kit::TempFile::new("many.pdf", &kit::many_fonts_pdf(150));
    let mut config = config(true);
    // One gigabyte, a third of what the engine may use: the fonts must not reach it.
    config.memory_limit = Some(1024 * 1024 * 1024);
    let client = EngineClient::new(config);
    let doc = client.open(&file.path_str(), None).unwrap();
    let bitmap = client.render(doc.doc, 0, 612, 792).unwrap();
    assert!(ink_anywhere(&bitmap) > 0, "the first names are drawn");
    // The page is read and drawn again, and the engine did not restart.
    let again = client.render(doc.doc, 0, 306, 396).unwrap();
    assert!(again.width == 306);
    let text = client.page_text(doc.doc, 0).unwrap();
    assert!(!text.text.trim().is_empty());
    assert_eq!(client.spawn_count(), 1, "the engine restarted");
}

fn ink_anywhere(bitmap: &Bitmap) -> usize {
    bitmap
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] < 110 && p[1] < 110 && p[2] < 110)
        .count()
}

/// A font that is damaged inside (the table directory is fine, the tables are not) is read by
/// PDFium and must cost at most the one engine. The next document is drawn by a new engine.
#[test]
fn a_font_damaged_inside_costs_at_most_one_engine() {
    let Some((script, path)) = system_fonts::found().first().cloned() else {
        println!("no system font here, nothing to damage");
        return;
    };
    let name = script.name();
    let mut bytes = std::fs::read(&path).unwrap();
    // Garbage over the middle of the file: the header and the table directory stay.
    let (from, to) = (bytes.len() / 3, bytes.len() * 2 / 3);
    for (i, b) in bytes[from..to].iter_mut().enumerate() {
        *b = (i as u8).wrapping_mul(131) ^ 0xA5;
    }
    let damaged = kit::TempFile::new("damaged.ttf", &bytes);
    let file = kit::TempFile::new("script.pdf", &kit::script_pdf(name));
    let mut cfg = config(true);
    cfg.system_font_files = Some(vec![(script, damaged.path().to_path_buf())]);
    let client = EngineClient::new(cfg);
    let drawn = client
        .open(&file.path_str(), None)
        .and_then(|doc| client.render(doc.doc, 0, 612, 792));
    println!(
        "{name}: damaged font, page {}; engines started: {}",
        if drawn.is_ok() { "drawn" } else { "failed" },
        client.spawn_count()
    );
    assert!(client.spawn_count() <= 2, "more than one restart");
    // A new document goes on as if nothing had happened.
    let plain = kit::TempFile::new("plain.pdf", &kit::simple_pdf(1));
    let doc = client.open(&plain.path_str(), None).unwrap();
    let bitmap = client.render(doc.doc, 0, 612, 792).unwrap();
    assert!(ink_anywhere(&bitmap) > 0);
}

/// Names that every script uses are no help to the engine: PDFium asks for "Arial" with no hint of
/// the script. This only reports what happens (the README says it is not drawn), so that the
/// claim is checked on each system.
#[test]
fn names_that_every_script_uses_are_reported() {
    for name in ["arabic", "hebrew", "thai"] {
        for font in ["Arial", "Tahoma", "TimesNewRoman"] {
            let ink = draw_named(name, font, true);
            println!("{name} as {font}: {ink} dark pixels (not required)");
        }
    }
}

#[test]
fn a_page_without_a_font_for_its_script_shows_blanks_and_the_engine_stays_up() {
    for name in kit::SCRIPT_NAMES {
        let _ = draw(name, false);
    }
}

/// How many descriptors (handles on Windows) a locked-in engine holds when it was given these
/// fonts, counted by the engine itself after the lock-in.
fn engine_handles(fonts: &[(FontScript, std::fs::File)]) -> usize {
    use std::io::Read;
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::Duration;

    let document = kit::TempFile::new("handles.pdf", &kit::simple_pdf(1));
    let handle = std::fs::File::open(document.path()).unwrap();
    let selftest = {
        let exe = std::env::current_exe().unwrap();
        exe.parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("examples")
            .join(format!("engine_selftest{}", std::env::consts::EXE_SUFFIX))
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
            fonts,
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
    text.lines()
        .find_map(|l| l.strip_prefix("descriptors: "))
        .unwrap_or_else(|| panic!("no count in {text}"))
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("no count in {text}"))
}

/// The largest of five counts. On Windows a count can be short by a handle or two that the system
/// or a library has not made yet or has already closed (an engine without fonts was seen at 101,
/// 101 and 103); the handles that the engine keeps are in every count, so the largest is the one
/// with nothing missing.
fn engine_handles_most(fonts: &[(FontScript, std::fs::File)]) -> (usize, Vec<usize>) {
    let counts: Vec<usize> = (0..5).map(|_| engine_handles(fonts)).collect();
    (*counts.iter().max().unwrap(), counts)
}

/// The engine holds one handle for each font it was given, and no other: with all seven scripts it
/// holds seven more than with none. (On Linux the whole count is known: the standard descriptors,
/// the document, the fonts and the copy of the output.) Whatever fonts the system has, stand-in
/// files are used for the count, so the same test runs on every system.
#[test]
fn the_engine_holds_only_the_handles_it_was_given() {
    let stand_in = kit::TempFile::new("font-stand-in.ttf", &kit::simple_pdf(1));
    let seven: Vec<_> = FontScript::ALL
        .into_iter()
        .map(|s| (s, std::fs::File::open(stand_in.path()).unwrap()))
        .collect();
    let (none, none_all) = engine_handles_most(&[]);
    let (all, all_all) = engine_handles_most(&seven);
    let (one, one_all) = engine_handles_most(&seven[..1]);
    println!("handles in the engine: {none_all:?} with no font, {one_all:?} with one, {all_all:?} with seven");
    assert_eq!(one, none + 1, "{none_all:?} {one_all:?} {all_all:?}");
    assert_eq!(all, none + 7, "{none_all:?} {one_all:?} {all_all:?}");

    // The fonts this system really has.
    let real = system_fonts::open_all();
    let (with_real, with_real_all) = engine_handles_most(&real);
    println!(
        "handles in the engine: {with_real} with the {} fonts of this system",
        real.len()
    );
    assert_eq!(
        with_real,
        none + real.len(),
        "{none_all:?} {with_real_all:?}"
    );

    #[cfg(target_os = "linux")]
    {
        // 0, 1, 2; the document; the copy of the output.
        assert_eq!(none, 3 + 1 + 1);
    }
}
