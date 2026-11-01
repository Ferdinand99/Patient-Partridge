//! Fonts for an engine that cannot open files.
//!
//! PDFium finds fonts that a PDF does not carry (a PDF that says "Arial" and ships no Arial) by
//! scanning the system font folders and opening the files it needs when it draws. The sandboxed
//! engine cannot do that, so it does not let PDFium look. Instead the engine hands PDFium a font
//! provider that answers from memory:
//!
//! - Fonts that sit in a `fonts` folder next to the PDFium library are read into memory at start,
//!   before the sandbox is applied, and served by family name. The app ships none yet.
//! - For everything else the provider answers "no font", and PDFium falls back on the fonts that
//!   are built into the library itself: a sans serif, a serif and a monospace face with a
//!   multiple-master range of weights, and the Symbol and Dingbats faces. These cover Latin text of
//!   the 14 standard PDF fonts and anything named like them.
//!
//! What this does not give: text in a script that needs a font the PDF does not embed (Chinese,
//! Japanese, Korean, Arabic, Hebrew, Thai) shows as blanks or boxes unless a font for it is in the
//! `fonts` folder. A PDF that embeds its fonts, which is nearly every PDF made for the web or
//! print, is not affected.

use std::path::Path;

use pdfium_render::prelude::*;

/// Most font files read at start.
pub const MAX_FONT_FILES: usize = 64;
/// Most font data kept in memory, in bytes.
pub const MAX_FONT_BYTES: u64 = 96 * 1024 * 1024;

struct Font {
    /// Family name in the form used for matching: lower case letters and digits only, with the
    /// style words taken out.
    key: String,
    bold: bool,
    italic: bool,
    data: Vec<u8>,
}

/// The fonts held in memory, and the answer to PDFium's questions about them.
#[derive(Default)]
pub struct MemoryFonts {
    fonts: Vec<Font>,
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
            data,
        });
    }

    pub fn len(&self) -> usize {
        self.fonts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty()
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
}

impl PdfiumCustomFontProvider for MemoryFonts {
    fn provide(
        &mut self,
        request: PdfiumCustomFontProviderRequest,
    ) -> Option<PdfiumCustomFontProviderResponse> {
        let bold = names_bold(&request.font_face);
        let italic = request.is_italic || names_italic(&request.font_face);
        let index = self.find(&request.font_face, bold, italic)?;
        Some(PdfiumCustomFontProviderResponse {
            // Handles must not be zero: PDFium reads zero as "no font".
            id: index as u64 + 1,
            font_face: request.font_face,
            character_set: PdfFontCharacterSet::Default,
            data: self.fonts[index].data.clone(),
        })
    }
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
    fn a_missing_folder_gives_no_fonts() {
        assert!(MemoryFonts::load_dir(Path::new("/definitely/not/here")).is_empty());
    }
}
