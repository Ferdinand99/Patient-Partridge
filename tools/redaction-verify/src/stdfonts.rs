//! The widths of the standard fonts that a file may use without embedding them: Helvetica and
//! Arial, Times, and Courier, in their four styles. A viewer places the glyphs of such a font with
//! these widths. They are in thousandths of an em, for the codes 32 to 126. Two codes differ with
//! the encoding: 39 and 96 are the quotesingle and the grave in `WinAnsiEncoding` and
//! `MacRomanEncoding`, and the quoteright and the quoteleft in `StandardEncoding`, which is the
//! encoding of a font that has none. The region check uses the tables to place text, with a small
//! tolerance (see [`crate::profile::STD_TOLERANCE`]); a code outside the table, and the fonts that
//! are not in it (Symbol, ZapfDingbats), have no width here.

/// The vertical extent of the cell of a glyph of a standard font, in em: below the baseline and
/// above it. It holds the font bounding boxes of all 14 standard fonts.
pub const CELL_Y: [f64; 2] = [-0.25, 0.97];

const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

const HELVETICA_BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, 975, 722, 722, 722, 722, 667,
    611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 333, 278, 333, 584, 556, 333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556,
    278, 889, 611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];

const TIMES: [u16; 95] = [
    250, 333, 408, 500, 500, 833, 778, 180, 333, 333, 500, 564, 250, 333, 250, 278, 500, 500, 500,
    500, 500, 500, 500, 500, 500, 500, 278, 278, 564, 564, 564, 444, 921, 722, 667, 667, 722, 611,
    556, 722, 722, 333, 389, 722, 611, 889, 722, 722, 556, 722, 667, 556, 611, 722, 722, 944, 722,
    722, 611, 333, 278, 333, 469, 500, 333, 444, 500, 444, 500, 444, 333, 500, 500, 278, 278, 500,
    278, 778, 500, 500, 500, 500, 333, 389, 278, 500, 500, 722, 500, 500, 444, 480, 200, 480, 541,
];

const TIMES_BOLD: [u16; 95] = [
    250, 333, 555, 500, 500, 1000, 833, 278, 333, 333, 500, 570, 250, 333, 250, 278, 500, 500, 500,
    500, 500, 500, 500, 500, 500, 500, 333, 333, 570, 570, 570, 500, 930, 722, 667, 722, 722, 667,
    611, 778, 778, 389, 500, 778, 667, 944, 722, 778, 611, 778, 722, 556, 667, 722, 722, 1000, 722,
    722, 667, 333, 278, 333, 581, 500, 333, 500, 556, 444, 556, 444, 333, 500, 556, 278, 333, 556,
    278, 833, 556, 500, 556, 556, 444, 389, 333, 556, 500, 722, 500, 500, 444, 394, 220, 394, 520,
];

const TIMES_ITALIC: [u16; 95] = [
    250, 333, 420, 500, 500, 833, 778, 214, 333, 333, 500, 675, 250, 333, 250, 278, 500, 500, 500,
    500, 500, 500, 500, 500, 500, 500, 333, 333, 675, 675, 675, 500, 920, 611, 611, 667, 722, 611,
    611, 722, 722, 333, 444, 667, 556, 833, 667, 722, 611, 722, 611, 500, 556, 722, 611, 833, 611,
    556, 556, 389, 278, 389, 422, 500, 333, 500, 500, 444, 500, 444, 278, 500, 500, 278, 278, 444,
    278, 722, 500, 500, 500, 500, 389, 389, 278, 500, 444, 667, 444, 444, 389, 400, 275, 400, 541,
];

const TIMES_BOLD_ITALIC: [u16; 95] = [
    250, 389, 555, 500, 500, 833, 778, 278, 333, 333, 500, 570, 250, 333, 250, 278, 500, 500, 500,
    500, 500, 500, 500, 500, 500, 500, 333, 333, 570, 570, 570, 500, 832, 667, 667, 667, 722, 667,
    667, 722, 778, 389, 500, 667, 611, 889, 722, 722, 611, 722, 667, 556, 611, 722, 667, 889, 667,
    611, 611, 333, 278, 333, 570, 500, 333, 500, 500, 444, 500, 444, 333, 500, 556, 278, 278, 500,
    278, 778, 556, 500, 500, 500, 389, 389, 278, 556, 444, 667, 500, 444, 389, 348, 220, 348, 570,
];

/// The widths of a standard font.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Std {
    /// Courier in any style: every glyph is 600 wide.
    Courier,
    /// The widths of the codes 32 to 126, and the width of the quoteright and the quoteleft.
    Table(&'static [u16; 95], u16),
}

impl Std {
    /// The width of the glyph for a code, in em, if the table has it. `standard` is whether the
    /// encoding is `StandardEncoding` (a font with no `/Encoding`).
    pub fn width(self, code: u32, standard: bool) -> Option<f64> {
        match self {
            Std::Courier => (32..=255).contains(&code).then_some(0.6),
            Std::Table(t, quote) => {
                if standard && (code == 39 || code == 96) {
                    return Some(f64::from(quote) / 1000.0);
                }
                let i = usize::try_from(code.checked_sub(32)?).ok()?;
                t.get(i).map(|w| f64::from(*w) / 1000.0)
            }
        }
    }
}

/// The names (lower case, letters and digits only) of the standard fonts and the names that a
/// viewer maps to them. A name that is not here (`Arial-Black`, `Helvetica-Narrow`, `ArialNarrow`,
/// `HelveticaNeue`) is another font, with other widths.
const HELVETICA_NAMES: [&str; 3] = ["helvetica", "arial", "arialmt"];
const HELVETICA_OBLIQUE_NAMES: [&str; 4] = [
    "helveticaoblique",
    "arialitalic",
    "arialitalicmt",
    "helveticaitalic",
];
const HELVETICA_BOLD_NAMES: [&str; 3] = ["helveticabold", "arialbold", "arialboldmt"];
const HELVETICA_BOLD_OBLIQUE_NAMES: [&str; 4] = [
    "helveticaboldoblique",
    "arialbolditalic",
    "arialbolditalicmt",
    "helveticabolditalic",
];
const TIMES_NAMES: [&str; 5] = [
    "times",
    "timesroman",
    "timesnewroman",
    "timesnewromanps",
    "timesnewromanpsmt",
];
const TIMES_BOLD_NAMES: [&str; 4] = [
    "timesbold",
    "timesnewromanbold",
    "timesnewromanpsbold",
    "timesnewromanpsboldmt",
];
const TIMES_ITALIC_NAMES: [&str; 4] = [
    "timesitalic",
    "timesnewromanitalic",
    "timesnewromanpsitalic",
    "timesnewromanpsitalicmt",
];
const TIMES_BOLD_ITALIC_NAMES: [&str; 4] = [
    "timesbolditalic",
    "timesnewromanbolditalic",
    "timesnewromanpsbolditalic",
    "timesnewromanpsbolditalicmt",
];
const COURIER_NAMES: [&str; 12] = [
    "courier",
    "courierbold",
    "courieroblique",
    "courierboldoblique",
    "couriernew",
    "couriernewbold",
    "couriernewitalic",
    "couriernewbolditalic",
    "couriernewpsmt",
    "couriernewpsboldmt",
    "couriernewpsitalicmt",
    "couriernewpsbolditalicmt",
];

/// The standard font that a `/BaseFont` name stands for (a subset tag is ignored, and so are
/// spaces, commas and hyphens: `Arial,Bold`, `TimesNewRomanPS-BoldItalicMT`). Only the names of
/// the 14 standard fonts and the ones a viewer maps to them: any other name is another font.
pub fn for_name(base_font: &str) -> Option<Std> {
    let name = match base_font.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()) => rest,
        _ => base_font,
    };
    let n: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    let n = n.as_str();
    let is = |names: &[&str]| names.contains(&n);
    if is(&COURIER_NAMES) {
        Some(Std::Courier)
    } else if is(&HELVETICA_NAMES) || is(&HELVETICA_OBLIQUE_NAMES) {
        Some(Std::Table(&HELVETICA, 222))
    } else if is(&HELVETICA_BOLD_NAMES) || is(&HELVETICA_BOLD_OBLIQUE_NAMES) {
        Some(Std::Table(&HELVETICA_BOLD, 278))
    } else if is(&TIMES_NAMES) {
        Some(Std::Table(&TIMES, 333))
    } else if is(&TIMES_BOLD_NAMES) {
        Some(Std::Table(&TIMES_BOLD, 333))
    } else if is(&TIMES_ITALIC_NAMES) {
        Some(Std::Table(&TIMES_ITALIC, 333))
    } else if is(&TIMES_BOLD_ITALIC_NAMES) {
        Some(Std::Table(&TIMES_BOLD_ITALIC, 333))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stand_for_their_tables() {
        assert_eq!(for_name("Helvetica").unwrap().width(65, false), Some(0.667));
        assert_eq!(
            for_name("ABCDEF+Arial-BoldMT").unwrap().width(65, false),
            Some(0.722)
        );
        assert_eq!(
            for_name("Times-Roman").unwrap().width(109, false),
            Some(0.778)
        );
        assert_eq!(
            for_name("TimesNewRomanPS-ItalicMT")
                .unwrap()
                .width(65, false),
            Some(0.611)
        );
        assert_eq!(
            for_name("Courier-Oblique").unwrap().width(200, false),
            Some(0.6)
        );
        assert_eq!(for_name("Symbol"), None);
        assert_eq!(for_name("ZapfDingbats"), None);
        // Other fonts that look like a standard font have other widths.
        for other in [
            "Arial-Black",
            "ArialNarrow",
            "Helvetica-Narrow",
            "HelveticaNeue",
            "Arial-BoldItalicMTX",
            "TimesTen-Roman",
            "CourierStd",
            "NotHelvetica",
        ] {
            assert_eq!(for_name(other), None, "{other}");
        }
        assert!(for_name("Arial,BoldItalic").is_some());
        assert!(for_name("TimesNewRoman,Bold").is_some());
        // Outside the table.
        assert_eq!(for_name("Helvetica").unwrap().width(200, false), None);
        // The quote codes follow the encoding.
        let h = for_name("Helvetica").unwrap();
        assert_eq!(h.width(39, false), Some(0.191));
        assert_eq!(h.width(39, true), Some(0.222));
        assert_eq!(h.width(96, false), Some(0.333));
        assert_eq!(h.width(96, true), Some(0.222));
    }

    #[test]
    fn the_tables_have_a_width_for_the_space_and_the_letters() {
        for t in [
            &HELVETICA,
            &HELVETICA_BOLD,
            &TIMES,
            &TIMES_BOLD,
            &TIMES_ITALIC,
            &TIMES_BOLD_ITALIC,
        ] {
            assert_eq!(t.len(), 95);
            assert!(t[0] == 250 || t[0] == 278, "space");
            // Every width is a plausible width of a glyph, and the digits are all one width.
            assert!(t.iter().all(|w| (180..=1100).contains(w)));
            assert!(t[16..26].iter().all(|w| *w == t[16]));
        }
    }
}
