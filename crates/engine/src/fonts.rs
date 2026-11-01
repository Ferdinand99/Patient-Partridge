//! Fonts for an engine that cannot open files.
//!
//! PDFium finds fonts that a PDF does not carry (a PDF that says "Arial" and ships no Arial) by
//! scanning the system font folders and opening the files it needs when it draws. The sandboxed
//! engine cannot do that, so it does not let PDFium look. Instead the engine hands PDFium a font
//! source (see `sysfontinfo`) that answers from memory:
//!
//! - Fonts that sit in a `fonts` folder next to the PDFium library are read into memory at start,
//!   before the sandbox is applied, and served by family name. The app ships none yet.
//! - The app can hand over one system font file for each of seven scripts (Japanese, Korean, two
//!   Chinese, Arabic, Hebrew, Thai) as read-only handles; see `partridge_engine_client::system_fonts`.
//!   The file is read through its handle when PDFium first asks for that script, never before,
//!   checked, and kept once however many names ask for it. The engine does not choose or open a
//!   path, and a name in the PDF cannot reach a file: it only decides which of the seven handles
//!   answers. The bytes handed to PDFium are counted and stop at [`FONT_BUDGET_BYTES`] for the
//!   document.
//! - For everything else the source answers "no font", and PDFium falls back on the fonts that
//!   are built into the library itself: a sans serif, a serif and a monospace face with a
//!   multiple-master range of weights, and the Symbol and Dingbats faces. These cover Latin text of
//!   the 14 standard PDF fonts and anything named like them.
//!
//! What this does not give: text in one of those scripts in a PDF that does not embed its font,
//! on a computer where the app found no usable font for the script, or past the budget, shows as
//! blanks. The engine remembers which scripts it had to refuse (`missed`), and the app tells the
//! reader. A PDF that embeds its fonts, which is nearly every PDF made for the web or print, is not
//! affected.

use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use partridge_protocol::{check_font, FontScript, FONT_HEAD_BYTES};

/// Most font files read at start.
pub const MAX_FONT_FILES: usize = 64;
/// Most font data kept in memory from the `fonts` folder, in bytes.
pub const MAX_FONT_BYTES: u64 = 96 * 1024 * 1024;
/// Largest system font file the engine reads, in bytes.
pub const MAX_SYSTEM_FONT_BYTES: u64 = 128 * 1024 * 1024;
/// Most bytes of font data the engine hands to PDFium for one document. PDFium keeps its own copy
/// of a font for every name a PDF asks about, so a page with 150 font names would otherwise cost
/// 150 copies. Past this the source says "no font" and the text stays blank.
pub const FONT_BUDGET_BYTES: u64 = 512 * 1024 * 1024;

struct Font {
    /// Family name in the form used for matching: lower case letters and digits only, with the
    /// style words taken out.
    key: String,
    bold: bool,
    italic: bool,
    data: Arc<[u8]>,
}

/// A system font the app handed over as a handle to one file. Its bytes are read when PDFium first
/// asks for its script, not before: a document without such text never pays for it.
struct SystemFont {
    file: File,
    length: u64,
    state: SystemFontState,
}

enum SystemFontState {
    NotRead,
    Loaded(Arc<[u8]>),
    /// The file could not be read or did not pass the check; it is not tried again.
    Unusable,
}

impl SystemFont {
    /// The bytes of the file, read once and checked. The same bytes are shared by all scripts that
    /// use the same file (compared byte for byte), so one font file is held once.
    fn bytes(&mut self, shared: &mut Vec<Arc<[u8]>>) -> Option<Arc<[u8]>> {
        if matches!(self.state, SystemFontState::NotRead) {
            self.state = match self.read(shared) {
                Some(data) => SystemFontState::Loaded(data),
                None => SystemFontState::Unusable,
            };
        }
        match &self.state {
            SystemFontState::Loaded(data) => Some(data.clone()),
            _ => None,
        }
    }

    fn read(&self, shared: &mut Vec<Arc<[u8]>>) -> Option<Arc<[u8]>> {
        let mut buffer = vec![0u8; usize::try_from(self.length).ok()?];
        read_exact_at(&self.file, &mut buffer, 0).ok()?;
        // The app checked the header already; a file that changed since, or a handle that is not
        // what it was said to be, is not handed to PDFium.
        check_font(&buffer[..buffer.len().min(FONT_HEAD_BYTES)], self.length).ok()?;
        if let Some(same) = shared.iter().find(|d| d[..] == buffer[..]) {
            return Some(same.clone());
        }
        let data: Arc<[u8]> = buffer.into();
        shared.push(data.clone());
        Some(data)
    }
}

#[cfg(unix)]
fn read_exact_at(file: &File, buffer: &mut [u8], offset: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(buffer, offset)
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut buffer: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buffer.is_empty() {
        let n = file.seek_read(buffer, offset)?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        buffer = &mut buffer[n..];
        offset += n as u64;
    }
    Ok(())
}

// The character sets PDFium asks for (FXFONT_*_CHARSET).
const CHARSET_DEFAULT: i32 = 1;
const CHARSET_SHIFTJIS: i32 = 128;
const CHARSET_HANGEUL: i32 = 129;
const CHARSET_GB2312: i32 = 134;
const CHARSET_BIG5: i32 = 136;
const CHARSET_HEBREW: i32 = 177;
const CHARSET_ARABIC: i32 = 178;
const CHARSET_THAI: i32 = 222;

/// The script a character set that PDFium asks a font for belongs to, if it is one of ours.
fn script_of_charset(charset: i32) -> Option<FontScript> {
    Some(match charset {
        CHARSET_SHIFTJIS => FontScript::Japanese,
        CHARSET_HANGEUL => FontScript::Korean,
        CHARSET_GB2312 => FontScript::ChineseSimplified,
        CHARSET_BIG5 => FontScript::ChineseTraditional,
        CHARSET_ARABIC => FontScript::Arabic,
        CHARSET_HEBREW => FontScript::Hebrew,
        CHARSET_THAI => FontScript::Thai,
        _ => return None,
    })
}

/// The character set to report for a font of a script (the one PDFium asked for).
fn charset_of(script: FontScript) -> i32 {
    match script {
        FontScript::Japanese => CHARSET_SHIFTJIS,
        FontScript::Korean => CHARSET_HANGEUL,
        FontScript::ChineseSimplified => CHARSET_GB2312,
        FontScript::ChineseTraditional => CHARSET_BIG5,
        FontScript::Arabic => CHARSET_ARABIC,
        FontScript::Hebrew => CHARSET_HEBREW,
        FontScript::Thai => CHARSET_THAI,
    }
}

/// Fonts of Windows and Macintosh documents that are made for one script, by name. PDFium does
/// not ask for the character set of these scripts when it draws ordinary text in a font it does
/// not have: it asks for the font by its name only. A name that is made for one script is enough
/// to say which of the app's fonts that is. (Names are compared without spaces and style words, by
/// their beginning, so that `Angsana New` and `AngsanaUPC` both match `angsana`.) A name that
/// several scripts use, such as Arial or Tahoma, is not in the list.
const NAMES_BY_SCRIPT: [(FontScript, &[&str]); 3] = [
    (
        FontScript::Arabic,
        &[
            "traditionalarabic",
            "simplifiedarabic",
            "arabictypesetting",
            "sakkalmajalla",
            "andalus",
            "geezapro",
            "scheherazade",
            "lateef",
            "notosansarabic",
            "notonaskharabic",
        ],
    ),
    (
        FontScript::Hebrew,
        &[
            "david",
            "narkisim",
            "miriam",
            "frankruehl",
            "frankruhl",
            "nehama",
            "arialhebrew",
            "notosanshebrew",
        ],
    ),
    (
        FontScript::Thai,
        &[
            "angsana",
            "browallia",
            "cordia",
            "dillenia",
            "eucrosia",
            "freesia",
            "iris",
            "jasmine",
            "kodchiang",
            "lilyupc",
            "thsarabun",
            "leelawadee",
            "garuda",
            "loma",
            "norasi",
            "waree",
            "notosansthai",
        ],
    ),
];

/// The script a font name belongs to, if it is a name made for one script.
fn script_of_name(face: &str) -> Option<FontScript> {
    let key = match_key(face);
    NAMES_BY_SCRIPT
        .iter()
        .find(|(_, names)| names.iter().any(|n| is_name_of(&key, n)))
        .map(|(script, _)| *script)
}

/// Endings that a family name takes without being another family (`AngsanaUPC`, `THSarabunPSK`,
/// `David CLM`, `LeelawadeeUI`).
const FAMILY_ENDINGS: [&str; 7] = ["", "upc", "new", "psk", "clm", "ui", "mt"];

/// True when the name (as `match_key` makes it) is the family `name` or one of its usual variants.
/// A plain prefix would also take `Cordial` for `Cordia` and `Lomax` for `Loma`.
fn is_name_of(key: &str, name: &str) -> bool {
    key.strip_prefix(name)
        .is_some_and(|rest| FAMILY_ENDINGS.contains(&rest))
}

/// What the source hands to PDFium for one request.
pub struct Served {
    pub data: Arc<[u8]>,
    /// The character set to report for the font.
    pub charset: i32,
}

/// The fonts held by the engine, and the answer to PDFium's questions about them.
pub struct MemoryFonts {
    fonts: Vec<Font>,
    system: HashMap<FontScript, SystemFont>,
    /// The distinct system font files already read, so that scripts sharing a file share its bytes.
    shared: Vec<Arc<[u8]>>,
    /// Bytes of system font still allowed to go to PDFium.
    budget_left: u64,
    /// The scripts that were asked for and refused, one bit each (bit n is `FontScript::ALL[n]`).
    missed: Arc<AtomicU8>,
}

impl Default for MemoryFonts {
    fn default() -> Self {
        MemoryFonts {
            fonts: Vec::new(),
            system: HashMap::new(),
            shared: Vec::new(),
            budget_left: FONT_BUDGET_BYTES,
            missed: Arc::new(AtomicU8::new(0)),
        }
    }
}

const STYLE_WORDS: [&str; 7] = [
    "bold", "italic", "oblique", "regular", "roman", "normal", "book",
];

/// Lower case letters and digits of a font name, style words removed. `Arial-BoldMT` becomes
/// `arialmt`.
fn match_key(name: &str) -> String {
    let mut s: String = name
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    for word in STYLE_WORDS {
        s = s.replace(word, "");
    }
    s
}

fn names_bold(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("bold") || n.contains("black") || n.contains("heavy")
}

fn names_italic(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("italic") || n.contains("oblique")
}

impl MemoryFonts {
    /// Reads the font files of a folder (at most [`MAX_FONT_FILES`] files and [`MAX_FONT_BYTES`]
    /// bytes). A missing folder gives no fonts. Call before the sandbox is applied.
    pub fn load_dir(dir: &Path) -> MemoryFonts {
        let mut set = MemoryFonts::default();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return set;
        };
        let mut paths: Vec<_> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        paths.sort();
        let mut total = 0u64;
        for path in paths {
            let is_font = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "otf" | "ttc"))
                .unwrap_or(false);
            if !is_font || set.fonts.len() >= MAX_FONT_FILES {
                continue;
            }
            let Ok(data) = std::fs::read(&path) else {
                continue;
            };
            total += data.len() as u64;
            if total > MAX_FONT_BYTES {
                break;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            set.add(&stem, data);
        }
        set
    }

    /// Adds one font, named by its file name without the extension.
    pub fn add(&mut self, name: &str, data: Vec<u8>) {
        self.fonts.push(Font {
            key: match_key(name),
            bold: names_bold(name),
            italic: names_italic(name),
            data: data.into(),
        });
    }

    /// Adds the system font the app handed over for a script. The length is taken now, while
    /// files can still be asked about; the bytes are read later through the handle.
    pub fn add_system(&mut self, script: FontScript, file: File) {
        let Ok(length) = file.metadata().map(|m| m.len()) else {
            return;
        };
        // A handle that is not a file of a sensible size is not used.
        if length == 0 || length > MAX_SYSTEM_FONT_BYTES {
            return;
        }
        self.system.insert(
            script,
            SystemFont {
                file,
                length,
                state: SystemFontState::NotRead,
            },
        );
    }

    pub fn len(&self) -> usize {
        self.fonts.len() + self.system.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty() && self.system.is_empty()
    }

    /// The scripts that were asked for and refused so far (a bit for each), shared with the engine
    /// that reports them.
    pub fn missed_scripts(&self) -> Arc<AtomicU8> {
        self.missed.clone()
    }

    /// Sets the budget of font data for the document (tests).
    #[cfg(test)]
    fn set_budget(&mut self, bytes: u64) {
        self.budget_left = bytes;
    }

    /// The index of the font that fits a request best, if any family matches.
    fn find(&self, face: &str, bold: bool, italic: bool) -> Option<usize> {
        let key = match_key(face);
        if key.is_empty() {
            return None;
        }
        self.fonts
            .iter()
            .enumerate()
            .filter(|(_, f)| f.key == key)
            .max_by_key(|(_, f)| u8::from(f.bold == bold) * 2 + u8::from(f.italic == italic))
            .map(|(i, _)| i)
    }

    /// Answers PDFium's question for a font. `face` is the name as PDFium has it, with the bytes
    /// that are not UTF-8 replaced (it is only used to tell a script by name, and for the fonts of
    /// the `fonts` folder: a Chinese font written in GBK has the character set to go by).
    /// `charset` is the character set PDFium asks for. Returns what to hand over, or `None` for
    /// "no font".
    pub fn serve(&mut self, face: &str, italic: bool, charset: i32) -> Option<Served> {
        // A script the app has a system font for: PDFium asked for the character set (or for a
        // name made for the script), and the name chooses nothing else. The name never chooses a
        // file.
        let script = script_of_charset(charset).or_else(|| script_of_name(face));
        let mut refused = false;
        if let Some(script) = script {
            if let Some(system) = self.system.get_mut(&script) {
                match system.bytes(&mut self.shared) {
                    Some(data) if (data.len() as u64) <= self.budget_left => {
                        self.budget_left -= data.len() as u64;
                        return Some(Served {
                            data,
                            charset: charset_of(script),
                        });
                    }
                    _ => refused = true,
                }
            } else {
                refused = true;
            }
        }
        let bold = names_bold(face);
        let italic = italic || names_italic(face);
        if let Some(index) = self.find(face, bold, italic) {
            return Some(Served {
                data: self.fonts[index].data.clone(),
                charset: CHARSET_DEFAULT,
            });
        }
        if refused {
            if let Some(script) = script {
                self.missed.fetch_or(1 << script.code(), Ordering::Relaxed);
            }
        }
        None
    }
}

/// The scripts in a bit set as the engine reports them, in the order of [`FontScript::ALL`].
pub fn scripts_of_bits(bits: u8) -> Vec<FontScript> {
    FontScript::ALL
        .into_iter()
        .filter(|s| bits & (1 << s.code()) != 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_compared_without_style_words_or_punctuation() {
        assert_eq!(match_key("Arial-BoldMT"), "arialmt");
        assert_eq!(match_key("Noto Sans Regular"), "notosans");
        assert_eq!(match_key("NotoSans-Italic"), "notosans");
    }

    #[test]
    fn the_closest_style_of_a_family_wins() {
        let mut set = MemoryFonts::default();
        set.add("NotoSans-Regular", vec![1]);
        set.add("NotoSans-Bold", vec![2]);
        set.add("NotoSerif-Regular", vec![3]);
        assert_eq!(set.find("Noto Sans", false, false), Some(0));
        assert_eq!(set.find("NotoSans-Bold", true, false), Some(1));
        assert_eq!(set.find("NotoSerif", false, false), Some(2));
        assert_eq!(set.find("Helvetica", false, false), None);
        assert_eq!(set.find("", false, false), None);
    }

    #[test]
    fn names_made_for_one_script_find_that_script() {
        assert_eq!(
            script_of_name("TraditionalArabic"),
            Some(FontScript::Arabic)
        );
        assert_eq!(script_of_name("David-Bold"), Some(FontScript::Hebrew));
        assert_eq!(script_of_name("AngsanaUPC"), Some(FontScript::Thai));
        assert_eq!(script_of_name("Angsana New"), Some(FontScript::Thai));
        assert_eq!(script_of_name("THSarabunPSK"), Some(FontScript::Thai));
        assert_eq!(script_of_name("David CLM"), Some(FontScript::Hebrew));
        // A Latin name that begins like a Thai one is not that font.
        for name in ["Cordial", "Lomax", "Irish Grover", "Davidson", "Wareeka"] {
            assert_eq!(script_of_name(name), None, "{name}");
        }
        // Names that every script uses are no help.
        for name in ["Arial", "Tahoma", "Times New Roman", "Helvetica", ""] {
            assert_eq!(script_of_name(name), None, "{name}");
        }
    }

    #[test]
    fn a_missing_folder_gives_no_fonts() {
        assert!(MemoryFonts::load_dir(Path::new("/definitely/not/here")).is_empty());
    }

    /// A tiny file that passes the check: a `cmap` and a `glyf` table.
    fn small_font(fill: u8) -> Vec<u8> {
        let mut out = vec![0, 1, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0];
        for (tag, offset) in [(b"cmap", 44u32), (b"glyf", 48u32)] {
            out.extend_from_slice(tag);
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&offset.to_be_bytes());
            out.extend_from_slice(&4u32.to_be_bytes());
        }
        out.extend_from_slice(&[fill; 8]);
        out
    }

    fn temp_file(name: &str, bytes: &[u8]) -> File {
        let path =
            std::env::temp_dir().join(format!("partridge-fonts-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let file = File::open(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        file
    }

    #[test]
    fn a_script_is_served_by_its_character_set_whatever_the_name_is() {
        let mut set = MemoryFonts::default();
        set.add_system(
            FontScript::ChineseSimplified,
            temp_file("a", &small_font(1)),
        );
        // The bytes of a name in GBK, decoded lossily, say nothing; the character set does.
        let served = set.serve("\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}", false, CHARSET_GB2312);
        assert_eq!(served.map(|s| s.charset), Some(CHARSET_GB2312));
        // Another script has no font: refused, and remembered.
        assert!(set.serve("x", false, CHARSET_HANGEUL).is_none());
        assert_eq!(
            scripts_of_bits(set.missed.load(Ordering::Relaxed)),
            vec![FontScript::Korean]
        );
        // Latin text is nobody's business.
        assert!(set.serve("Arial", false, 0).is_none());
        assert_eq!(
            set.missed.load(Ordering::Relaxed),
            1 << FontScript::Korean.code()
        );
    }

    #[test]
    fn scripts_that_share_a_file_share_its_bytes() {
        let mut set = MemoryFonts::default();
        let font = small_font(7);
        set.add_system(FontScript::Japanese, temp_file("j", &font));
        set.add_system(FontScript::Korean, temp_file("k", &font));
        let a = set.serve("a", false, CHARSET_SHIFTJIS).unwrap();
        let b = set.serve("b", false, CHARSET_HANGEUL).unwrap();
        assert!(Arc::ptr_eq(&a.data, &b.data));
    }

    #[test]
    fn past_the_budget_the_answer_is_no_font_and_the_script_is_reported() {
        let mut set = MemoryFonts::default();
        set.add_system(FontScript::Japanese, temp_file("j", &small_font(2)));
        let size = small_font(2).len() as u64;
        set.set_budget(size * 3);
        for i in 0..3 {
            assert!(set.serve("name", false, CHARSET_SHIFTJIS).is_some(), "{i}");
        }
        assert_eq!(set.missed.load(Ordering::Relaxed), 0);
        assert!(set.serve("name", false, CHARSET_SHIFTJIS).is_none());
        assert_eq!(
            scripts_of_bits(set.missed.load(Ordering::Relaxed)),
            vec![FontScript::Japanese]
        );
    }

    #[test]
    fn a_file_that_is_not_a_font_is_never_handed_over() {
        let mut set = MemoryFonts::default();
        set.add_system(
            FontScript::Thai,
            temp_file("t", b"%PDF-1.4 not a font, 64 bytes of text......"),
        );
        assert!(set.serve("Angsana", false, 0).is_none());
        assert_eq!(
            scripts_of_bits(set.missed.load(Ordering::Relaxed)),
            vec![FontScript::Thai]
        );
        // A truncated font: the directory says more than the file has.
        let mut cut = small_font(3);
        cut.truncate(cut.len() - 6);
        let mut set = MemoryFonts::default();
        set.add_system(FontScript::Thai, temp_file("c", &cut));
        assert!(set.serve("Angsana", false, 0).is_none());
    }
}
