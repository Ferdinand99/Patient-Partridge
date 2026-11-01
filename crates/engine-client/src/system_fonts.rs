//! System fonts for the scripts that PDFium's built-in fonts do not cover.
//!
//! A PDF that names a font it does not carry (a Japanese document that says "MS-Mincho" and has no
//! font file inside) needs a font from the computer to be readable. The engine cannot look for one:
//! it cannot open files. So the app chooses, and hands over what it chose.
//!
//! How the choice is made matters, and it is kept simple on purpose:
//!
//! - The files come from a **fixed table** below, by script and by system. The table names only
//!   files that the operating system or a common font package installs. **Nothing in a PDF
//!   influences a path**: the engine tells PDFium which script a font name needs only by using the
//!   handle that was made for that script; it cannot ask for another file.
//! - The app opens each file **read-only, one single file**, never a folder, at most one for each
//!   script (seven), and each must be an ordinary file of a size that a font can have.
//! - The engine reads a font from its handle only when a page asks for that script.
//!
//! A script for which the system has none of the listed fonts has no handle, and the text shows as
//! blanks; the window tells the user once.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use partridge_protocol::FontScript;

/// Largest font file the app hands over. The biggest CJK fonts of the systems are below 40 MB.
pub const MAX_FONT_BYTES: u64 = 128 * 1024 * 1024;

#[cfg(target_os = "windows")]
fn candidates(script: FontScript) -> Vec<PathBuf> {
    let root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts");
    let names: &[&str] = match script {
        FontScript::Japanese => &["YuGothR.ttc", "YuGothM.ttc", "msgothic.ttc", "meiryo.ttc"],
        FontScript::Korean => &["malgun.ttf", "gulim.ttc", "batang.ttc"],
        FontScript::ChineseSimplified => &["msyh.ttc", "simsun.ttc", "simhei.ttf", "Deng.ttf"],
        FontScript::ChineseTraditional => &["msjh.ttc", "mingliu.ttc"],
        FontScript::Arabic => &["segoeui.ttf", "arial.ttf", "tahoma.ttf"],
        FontScript::Hebrew => &["segoeui.ttf", "arial.ttf", "david.ttf", "tahoma.ttf"],
        FontScript::Thai => &["leelawui.ttf", "tahoma.ttf", "angsa.ttf"],
    };
    names.iter().map(|n| root.join(n)).collect()
}

#[cfg(target_os = "macos")]
fn candidates(script: FontScript) -> Vec<PathBuf> {
    let roots = [
        "/System/Library/Fonts",
        "/System/Library/Fonts/Supplemental",
        "/Library/Fonts",
    ];
    let names: &[&str] = match script {
        FontScript::Japanese => &[
            "ヒラギノ角ゴシック W3.ttc",
            "Hiragino Sans GB.ttc",
            "PingFang.ttc",
            "Arial Unicode.ttf",
        ],
        FontScript::Korean => &[
            "AppleSDGothicNeo.ttc",
            "AppleGothic.ttf",
            "Arial Unicode.ttf",
        ],
        FontScript::ChineseSimplified => &[
            "PingFang.ttc",
            "STHeiti Light.ttc",
            "Hiragino Sans GB.ttc",
            "Arial Unicode.ttf",
        ],
        FontScript::ChineseTraditional => {
            &["PingFang.ttc", "STHeiti Light.ttc", "Arial Unicode.ttf"]
        }
        FontScript::Arabic => &["GeezaPro.ttc", "Arial Unicode.ttf", "AlBayan.ttc"],
        FontScript::Hebrew => &["ArialHB.ttc", "Arial Hebrew.ttc", "Arial Unicode.ttf"],
        FontScript::Thai => &[
            "Thonburi.ttc",
            "Thonburi.ttf",
            "Ayuthaya.ttf",
            "Arial Unicode.ttf",
        ],
    };
    roots
        .iter()
        .flat_map(|root| names.iter().map(move |n| Path::new(root).join(n)))
        .collect()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn candidates(script: FontScript) -> Vec<PathBuf> {
    // Where the Debian family, Fedora and Arch put Noto and a few older families. The first file
    // that exists wins.
    let cjk = [
        "opentype/noto/NotoSansCJK-Regular.ttc",
        "noto-cjk/NotoSansCJK-Regular.ttc",
        "google-noto-cjk/NotoSansCJK-Regular.ttc",
        "truetype/noto/NotoSansCJK-Regular.ttc",
        "OTF/NotoSansCJK-Regular.ttc",
    ];
    let more: &[&str] = match script {
        FontScript::Japanese => &[
            "truetype/takao-gothic/TakaoPGothic.ttf",
            "opentype/ipafont-gothic/ipag.ttf",
            "truetype/vlgothic/VL-Gothic-Regular.ttf",
        ],
        FontScript::Korean => &[
            "truetype/nanum/NanumGothic.ttf",
            "truetype/unfonts-core/UnDotum.ttf",
        ],
        FontScript::ChineseSimplified | FontScript::ChineseTraditional => &[
            "truetype/wqy/wqy-microhei.ttc",
            "wqy-microhei/wqy-microhei.ttc",
            "truetype/wqy/wqy-zenhei.ttc",
            "truetype/arphic/uming.ttc",
        ],
        FontScript::Arabic => &[
            "truetype/noto/NotoSansArabic-Regular.ttf",
            "noto/NotoSansArabic-Regular.ttf",
            "truetype/dejavu/DejaVuSans.ttf",
            "dejavu/DejaVuSans.ttf",
            "TTF/DejaVuSans.ttf",
        ],
        FontScript::Hebrew => &[
            "truetype/noto/NotoSansHebrew-Regular.ttf",
            "noto/NotoSansHebrew-Regular.ttf",
            "truetype/dejavu/DejaVuSans.ttf",
            "dejavu/DejaVuSans.ttf",
            "TTF/DejaVuSans.ttf",
        ],
        FontScript::Thai => &[
            "truetype/noto/NotoSansThai-Regular.ttf",
            "noto/NotoSansThai-Regular.ttf",
            "truetype/tlwg/Garuda.ttf",
            "truetype/thai/Garuda.ttf",
            "TTF/Garuda.ttf",
        ],
    };
    let wide = matches!(
        script,
        FontScript::Japanese
            | FontScript::Korean
            | FontScript::ChineseSimplified
            | FontScript::ChineseTraditional
    );
    let mut names: Vec<&str> = Vec::new();
    if wide {
        names.extend(cjk);
    }
    names.extend(more);
    names
        .into_iter()
        .map(|n| Path::new("/usr/share/fonts").join(n))
        .collect()
}

#[cfg(not(any(unix, windows)))]
fn candidates(_script: FontScript) -> Vec<PathBuf> {
    Vec::new()
}

/// True when the file is something to hand over: an ordinary file that looks like a font, of a size
/// that a font can have.
fn is_usable(file: &File, path: &Path) -> bool {
    let font_name = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "ttc" | "otf"))
        .unwrap_or(false);
    font_name
        && file
            .metadata()
            .map(|m| m.is_file() && m.len() > 0 && m.len() <= MAX_FONT_BYTES)
            .unwrap_or(false)
}

/// The font of the table for a script that this system has, with the open file.
fn open_first(script: FontScript) -> Option<(PathBuf, File)> {
    candidates(script).into_iter().find_map(|path| {
        let file = File::open(&path).ok()?;
        is_usable(&file, &path).then_some((path, file))
    })
}

/// The scripts for which this system has a font in the table, and the file for each. Looked up
/// once.
pub fn found() -> &'static [(FontScript, PathBuf)] {
    static FOUND: OnceLock<Vec<(FontScript, PathBuf)>> = OnceLock::new();
    FOUND.get_or_init(|| {
        FontScript::ALL
            .into_iter()
            .filter_map(|s| open_first(s).map(|(path, _)| (s, path)))
            .collect()
    })
}

/// The scripts for which this system has a font.
pub fn available() -> Vec<FontScript> {
    found().iter().map(|(s, _)| *s).collect()
}

/// Opens the font file of every script that has one, read-only, for handing to an engine. A file
/// that has gone since the lookup is left out.
pub fn open_all() -> Vec<(FontScript, File)> {
    found()
        .iter()
        .filter_map(|(script, path)| {
            let file = File::open(path).ok()?;
            is_usable(&file, path).then_some((*script, file))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_candidate_is_a_fixed_absolute_path_to_a_font_file() {
        for script in FontScript::ALL {
            let list = candidates(script);
            assert!(!list.is_empty(), "{script:?}");
            for path in list {
                assert!(path.is_absolute(), "{path:?}");
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                assert!(
                    matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "ttc" | "otf"),
                    "{path:?}"
                );
            }
        }
    }

    #[test]
    fn only_ordinary_font_files_are_usable() {
        let dir = std::env::temp_dir();
        let folder = File::open(&dir);
        if let Ok(folder) = folder {
            assert!(!is_usable(&folder, &dir.join("x.ttf")));
        }
        let not_a_font = std::env::current_exe().unwrap();
        let file = File::open(&not_a_font).unwrap();
        // The extension decides first: a program is not a font.
        assert!(!is_usable(&file, &not_a_font));
    }

    #[test]
    fn what_is_found_is_opened_once_for_each_script() {
        let scripts = available();
        let opened = open_all();
        assert_eq!(scripts.len(), opened.len());
        println!("system fonts found: {:?}", found());
    }
}
