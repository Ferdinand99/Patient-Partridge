//! System fonts for the scripts that PDFium's built-in fonts do not cover.
//!
//! A PDF that names a font it does not carry (a Japanese document that says "MS-Mincho" and has no
//! font file inside) needs a font from the computer to be readable. The engine cannot look for one:
//! it cannot open files. So the app chooses, and hands over what it chose.
//!
//! How the choice is made matters, and it is kept simple on purpose:
//!
//! - The files come from a **fixed table** below, by script and by system. The table names only
//!   files that the operating system or a common font package installs, in the folders of the
//!   system that only an administrator can write to (`/usr/share/fonts`, the Windows `Fonts`
//!   folder, `/System/Library/Fonts` and `/Library/Fonts`). Fonts that a user installed for
//!   themselves (their own font folder) are **not** looked at. The list is in
//!   `docs/system-fonts.md`, and a test keeps the two the same.
//! - **Nothing in a PDF influences a path**: the engine tells PDFium which script a font name needs
//!   only by using the handle that was made for that script; it cannot ask for another file.
//! - The app opens each file **read-only, one single file**, never a folder, at most one for each
//!   script (seven), and each must be an ordinary file of a size that a font can have and the
//!   start of an intact font (the table directory is there and every table lies inside the file).
//!   A candidate that fails is passed over for the next one in the table.
//! - The engine reads a font from its handle only when a page asks for that script.
//! - The table is read again every time an engine is started, so a font that was installed after
//!   the app started is used from the next document on.
//!
//! A script for which the system has none of the listed fonts has no handle, and the text shows as
//! blanks; the window tells the user once.

use std::fs::File;
use std::path::{Path, PathBuf};

use partridge_protocol::{check_font, FontScript, FONT_HEAD_BYTES};

/// Largest font file the app hands over. The biggest CJK fonts of the systems are below 128 MB.
pub const MAX_FONT_BYTES: u64 = 128 * 1024 * 1024;

/// The systems the table has lists for. A list for each is kept on every system so that the
/// documentation test can check all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum System {
    Windows,
    MacOs,
    Linux,
}

impl System {
    /// The system this program runs on.
    pub const THIS: System = if cfg!(windows) {
        System::Windows
    } else if cfg!(target_os = "macos") {
        System::MacOs
    } else {
        System::Linux
    };

    pub const ALL: [System; 3] = [System::Windows, System::MacOs, System::Linux];
}

/// The folders the table's file names are relative to, in the order they are tried.
pub fn folders(system: System) -> Vec<PathBuf> {
    match system {
        System::Windows => vec![std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
            .join("Fonts")],
        System::MacOs => [
            "/System/Library/Fonts",
            "/System/Library/Fonts/Supplemental",
            "/Library/Fonts",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect(),
        System::Linux => vec![PathBuf::from("/usr/share/fonts")],
    }
}

/// The file names (relative to [`folders`]) that serve a script on a system, in the order they are
/// tried. The first one that exists and is intact is used.
pub fn names(system: System, script: FontScript) -> Vec<&'static str> {
    match system {
        System::Windows => match script {
            FontScript::Japanese => {
                vec!["YuGothR.ttc", "YuGothM.ttc", "msgothic.ttc", "meiryo.ttc"]
            }
            FontScript::Korean => vec!["malgun.ttf", "gulim.ttc", "batang.ttc"],
            FontScript::ChineseSimplified => {
                vec!["msyh.ttc", "simsun.ttc", "simhei.ttf", "Deng.ttf"]
            }
            FontScript::ChineseTraditional => vec!["msjh.ttc", "mingliu.ttc"],
            FontScript::Arabic => vec!["segoeui.ttf", "arial.ttf", "tahoma.ttf"],
            FontScript::Hebrew => vec!["segoeui.ttf", "arial.ttf", "david.ttf", "tahoma.ttf"],
            FontScript::Thai => vec!["leelawui.ttf", "tahoma.ttf", "angsa.ttf"],
        },
        System::MacOs => match script {
            FontScript::Japanese => vec![
                "ヒラギノ角ゴシック W3.ttc",
                "Hiragino Sans GB.ttc",
                "PingFang.ttc",
                "Arial Unicode.ttf",
            ],
            FontScript::Korean => {
                vec![
                    "AppleSDGothicNeo.ttc",
                    "AppleGothic.ttf",
                    "Arial Unicode.ttf",
                ]
            }
            FontScript::ChineseSimplified => vec![
                "PingFang.ttc",
                "STHeiti Light.ttc",
                "Hiragino Sans GB.ttc",
                "Arial Unicode.ttf",
            ],
            FontScript::ChineseTraditional => {
                vec!["PingFang.ttc", "STHeiti Light.ttc", "Arial Unicode.ttf"]
            }
            FontScript::Arabic => vec!["GeezaPro.ttc", "Arial Unicode.ttf", "AlBayan.ttc"],
            FontScript::Hebrew => vec!["ArialHB.ttc", "Arial Hebrew.ttc", "Arial Unicode.ttf"],
            FontScript::Thai => vec![
                "Thonburi.ttc",
                "Thonburi.ttf",
                "Ayuthaya.ttf",
                "Arial Unicode.ttf",
            ],
        },
        // Where the Debian family, Ubuntu, Arch and Fedora's older packages put Noto and a few
        // older families. Only Ubuntu was run.
        System::Linux => {
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
            let mut all: Vec<&str> = Vec::new();
            if wide {
                all.extend(cjk);
            }
            all.extend(more);
            all
        }
    }
}

/// The paths to try for a script on a system, in order.
fn candidates(system: System, script: FontScript) -> Vec<PathBuf> {
    let folders = folders(system);
    let names = names(system, script);
    // Folder by folder, the way a font lookup would go.
    folders
        .iter()
        .flat_map(|folder| names.iter().map(move |name| folder.join(name)))
        .collect()
}

#[cfg(unix)]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(buffer, offset)
}

#[cfg(windows)]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(buffer, offset)
}

/// The start of the file, as far as the check needs it. A positional read: the file offset that
/// the engine's copy of the handle shares is not moved.
fn read_head(file: &File, len: u64) -> Option<Vec<u8>> {
    let want = usize::try_from(len).ok()?.min(FONT_HEAD_BYTES);
    let mut head = vec![0u8; want];
    let mut filled = 0;
    while filled < want {
        match read_at(file, &mut head[filled..], filled as u64) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(_) => return None,
        }
    }
    head.truncate(filled);
    Some(head)
}

/// True when the file is something to hand over: an ordinary file that looks like a font, of a size
/// that a font can have, whose table directory is intact and lies inside the file.
fn is_usable(file: &File, path: &Path) -> bool {
    let font_name = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "ttc" | "otf"))
        .unwrap_or(false);
    if !font_name {
        return false;
    }
    let Ok(meta) = file.metadata() else {
        return false;
    };
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_FONT_BYTES {
        return false;
    }
    read_head(file, meta.len()).is_some_and(|head| check_font(&head, meta.len()).is_ok())
}

/// The first of the paths that can be opened and passes [`is_usable`], with the open file.
fn pick_first(paths: impl IntoIterator<Item = PathBuf>) -> Option<(PathBuf, File)> {
    paths.into_iter().find_map(|path| {
        let file = File::open(&path).ok()?;
        is_usable(&file, &path).then_some((path, file))
    })
}

/// The font of the table for a script that this system has, with the open file.
fn open_first(script: FontScript) -> Option<(PathBuf, File)> {
    pick_first(candidates(System::THIS, script))
}

/// The scripts for which this system has a font in the table, and the file for each. Looked up
/// anew at every call.
pub fn found() -> Vec<(FontScript, PathBuf)> {
    FontScript::ALL
        .into_iter()
        .filter_map(|s| open_first(s).map(|(path, _)| (s, path)))
        .collect()
}

/// The scripts for which this system has a font.
pub fn available() -> Vec<FontScript> {
    found().into_iter().map(|(s, _)| s).collect()
}

/// Opens the font file of every script that has one, read-only, for handing to an engine. Looked
/// up anew, so a font installed since the last engine is found.
pub fn open_all() -> Vec<(FontScript, File)> {
    FontScript::ALL
        .into_iter()
        .filter_map(|s| open_first(s).map(|(_, file)| (s, file)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A font header that passes the check: a `cmap` and a `glyf` table, both at the end of the
    /// file after `filler` bytes of other data (so that a file cut short loses them).
    fn font(filler: usize) -> Vec<u8> {
        let mut out = vec![0, 1, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0];
        let end = 44 + filler as u32;
        for (tag, offset) in [(b"cmap", end), (b"glyf", end + 4)] {
            out.extend_from_slice(tag);
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&offset.to_be_bytes());
            out.extend_from_slice(&4u32.to_be_bytes());
        }
        out.extend_from_slice(&vec![9u8; filler + 8]);
        out
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("partridge-sysfonts-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn every_candidate_is_a_fixed_absolute_path_to_a_font_file() {
        for system in System::ALL {
            for script in FontScript::ALL {
                let list = names(system, script);
                assert!(!list.is_empty(), "{system:?} {script:?}");
                for name in list {
                    let ext = Path::new(name)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("");
                    assert!(
                        matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "ttc" | "otf"),
                        "{name}"
                    );
                    assert!(!name.contains(".."), "{name}");
                }
            }
        }
        for script in FontScript::ALL {
            for path in candidates(System::THIS, script) {
                assert!(path.is_absolute(), "{path:?}");
            }
        }
    }

    #[test]
    fn only_ordinary_font_files_are_usable() {
        let dir = temp_dir("ordinary");
        // A folder is not a font.
        if let Ok(folder) = File::open(&dir) {
            assert!(!is_usable(&folder, &dir.join("x.ttf")));
        }
        // A program is not a font: the extension decides first.
        let exe = std::env::current_exe().unwrap();
        assert!(!is_usable(&File::open(&exe).unwrap(), &exe));
        // An intact font is.
        let good = dir.join("good.ttf");
        std::fs::write(&good, font(100)).unwrap();
        assert!(is_usable(&File::open(&good).unwrap(), &good));
        // Text with the name of a font is not.
        let text = dir.join("text.ttf");
        std::fs::write(&text, b"this is not a font, but its name says it is one").unwrap();
        assert!(!is_usable(&File::open(&text).unwrap(), &text));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_first_choice_does_not_block_the_next() {
        let dir = temp_dir("damaged");
        let whole = font(5_000);
        let first = dir.join("first.ttf");
        let second = dir.join("second.ttf");
        let third = dir.join("third.ttf");
        // The first choice was cut short by a download that stopped.
        std::fs::write(&first, &whole[..whole.len() - 3_000]).unwrap();
        // The second is not a font at all.
        std::fs::write(&second, vec![0u8; 4_000]).unwrap();
        std::fs::write(&third, &whole).unwrap();
        let picked = pick_first([
            first.clone(),
            dir.join("missing.ttf"),
            second,
            third.clone(),
        ]);
        assert_eq!(picked.map(|(p, _)| p), Some(third));
        // Nothing intact: no font, and no panic.
        assert!(pick_first([first]).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_is_found_is_opened_once_for_each_script() {
        let scripts = available();
        let opened = open_all();
        assert_eq!(scripts.len(), opened.len());
        println!("system fonts found: {:?}", found());
    }

    /// The list in the documentation is the list in the table: every file name of every system is
    /// in it.
    #[test]
    fn the_documentation_lists_every_font_of_the_table() {
        let docs = include_str!("../../../docs/system-fonts.md");
        for system in System::ALL {
            for script in FontScript::ALL {
                for name in names(system, script) {
                    assert!(
                        docs.contains(&format!("`{name}`")),
                        "docs/system-fonts.md does not list {name} ({system:?}, {script:?})"
                    );
                }
            }
        }
    }

    /// Prints the lists in the form of `docs/system-fonts.md` (run it with `--ignored --nocapture`
    /// when the table changes).
    #[test]
    #[ignore]
    fn print_the_lists_for_the_documentation() {
        for system in System::ALL {
            println!("\n### {system:?}\n");
            println!("| Script | Files, in the order they are tried |");
            println!("|---|---|");
            for script in FontScript::ALL {
                let files: Vec<String> = names(system, script)
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect();
                println!("| {} | {} |", script.name(), files.join(", "));
            }
        }
    }
}
