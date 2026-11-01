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

/// The smallest of three counts: a handle that the system or a library holds for a moment may be
/// counted once, and the ones the engine keeps are in every count.
fn engine_handles_least(fonts: &[(FontScript, std::fs::File)]) -> (usize, Vec<usize>) {
    let counts: Vec<usize> = (0..3).map(|_| engine_handles(fonts)).collect();
    (*counts.iter().min().unwrap(), counts)
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
    let (none, none_all) = engine_handles_least(&[]);
    let (all, all_all) = engine_handles_least(&seven);
    let (one, one_all) = engine_handles_least(&seven[..1]);
    println!("handles in the engine: {none_all:?} with no font, {one_all:?} with one, {all_all:?} with seven");
    assert_eq!(one, none + 1, "{none_all:?} {one_all:?} {all_all:?}");
    assert_eq!(all, none + 7, "{none_all:?} {one_all:?} {all_all:?}");

    // The fonts this system really has.
    let real = system_fonts::open_all();
    let (with_real, with_real_all) = engine_handles_least(&real);
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
