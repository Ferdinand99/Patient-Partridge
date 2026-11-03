//! Fonts: how the bytes of a shown string become characters. This is what a reader does to show
//! text, and what the byte layers cannot do: a CID font with `/Identity-H` writes glyph codes,
//! and a simple font with `/Differences` or WinAnsi or MacRoman writes single bytes that are not
//! Latin-1. The decoders here read `ToUnicode` maps, the encodings of simple fonts, and glyph names.
//!
//! A byte that has no known character becomes U+FFFD. The search treats it as a wildcard. A font
//! that cannot be decoded at all (`Mapper::Unknown`) is reported to the caller, who must say that
//! the text was not checked.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::OnceLock;

use crate::pdfobj::{Doc, Obj};

pub const GAP: char = '\u{FFFD}';

// ---------------------------------------------------------------------------------------------
// Glyph names and the single-byte encodings.

const ASCII_NAMES: [&str; 33] = [
    "space",
    "exclam",
    "quotedbl",
    "numbersign",
    "dollar",
    "percent",
    "ampersand",
    "quotesingle",
    "parenleft",
    "parenright",
    "asterisk",
    "plus",
    "comma",
    "hyphen",
    "period",
    "slash",
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "colon",
    "semicolon",
    "less",
    "equal",
    "greater",
    "question",
    "at",
];

const ASCII_AFTER_Z: [&str; 6] = [
    "bracketleft",
    "backslash",
    "bracketright",
    "asciicircum",
    "underscore",
    "grave",
];

const ASCII_AFTER_LOWER_Z: [&str; 4] = ["braceleft", "bar", "braceright", "asciitilde"];

/// Glyph names of 0xA1 to 0xFF in Latin-1 order.
const LATIN1_NAMES: [&str; 95] = [
    "exclamdown",
    "cent",
    "sterling",
    "currency",
    "yen",
    "brokenbar",
    "section",
    "dieresis",
    "copyright",
    "ordfeminine",
    "guillemotleft",
    "logicalnot",
    "sfthyphen",
    "registered",
    "macron",
    "degree",
    "plusminus",
    "twosuperior",
    "threesuperior",
    "acute",
    "mu",
    "paragraph",
    "periodcentered",
    "cedilla",
    "onesuperior",
    "ordmasculine",
    "guillemotright",
    "onequarter",
    "onehalf",
    "threequarters",
    "questiondown",
    "Agrave",
    "Aacute",
    "Acircumflex",
    "Atilde",
    "Adieresis",
    "Aring",
    "AE",
    "Ccedilla",
    "Egrave",
    "Eacute",
    "Ecircumflex",
    "Edieresis",
    "Igrave",
    "Iacute",
    "Icircumflex",
    "Idieresis",
    "Eth",
    "Ntilde",
    "Ograve",
    "Oacute",
    "Ocircumflex",
    "Otilde",
    "Odieresis",
    "multiply",
    "Oslash",
    "Ugrave",
    "Uacute",
    "Ucircumflex",
    "Udieresis",
    "Yacute",
    "Thorn",
    "germandbls",
    "agrave",
    "aacute",
    "acircumflex",
    "atilde",
    "adieresis",
    "aring",
    "ae",
    "ccedilla",
    "egrave",
    "eacute",
    "ecircumflex",
    "edieresis",
    "igrave",
    "iacute",
    "icircumflex",
    "idieresis",
    "eth",
    "ntilde",
    "ograve",
    "oacute",
    "ocircumflex",
    "otilde",
    "odieresis",
    "divide",
    "oslash",
    "ugrave",
    "uacute",
    "ucircumflex",
    "udieresis",
    "yacute",
    "thorn",
    "ydieresis",
];

/// Names outside ASCII and Latin-1, as `name:HEX`.
const EXTRA_NAMES: &str =
    "Euro:20AC quotesinglbase:201A florin:192 quotedblbase:201E ellipsis:2026 \
dagger:2020 daggerdbl:2021 circumflex:2C6 perthousand:2030 Scaron:160 guilsinglleft:2039 OE:152 \
Zcaron:17D quoteleft:2018 quoteright:2019 quotedblleft:201C quotedblright:201D bullet:2022 \
endash:2013 emdash:2014 tilde:2DC trademark:2122 scaron:161 guilsinglright:203A oe:153 zcaron:17E \
Ydieresis:178 fi:FB01 fl:FB02 ff:FB00 ffi:FB03 ffl:FB04 f_i:FB01 f_l:FB02 f_f:FB00 f_f_i:FB03 \
f_f_l:FB04 fraction:2044 dotlessi:131 Lslash:141 lslash:142 breve:2D8 dotaccent:2D9 ring:2DA \
hungarumlaut:2DD ogonek:2DB caron:2C7 nbspace:A0 nonbreakingspace:A0 Delta:2206 Omega:2126 \
notequal:2260 infinity:221E lessequal:2264 greaterequal:2265 partialdiff:2202 summation:2211 \
product:220F pi:3C0 integral:222B radical:221A approxequal:2248 lozenge:25CA apple:F8FF \
minus:2212 middot:B7 hyphenminus:2D uni00A0:A0 Abreve:102 abreve:103 Aogonek:104 aogonek:105 \
Cacute:106 cacute:107 Ccaron:10C ccaron:10D Dcaron:10E dcaron:10F Dcroat:110 dcroat:111 \
Eogonek:118 eogonek:119 Ecaron:11A ecaron:11B Gbreve:11E gbreve:11F Idotaccent:130 Nacute:143 \
nacute:144 Ncaron:147 ncaron:148 Odblacute:150 odblacute:151 Racute:154 racute:155 Rcaron:158 \
rcaron:159 Sacute:15A sacute:15B Scedilla:15E scedilla:15F Tcaron:164 tcaron:165 Uring:16E \
uring:16F Udblacute:170 udblacute:171 Zacute:179 zacute:17A Zdotaccent:17B zdotaccent:17C \
afii10017:410 afii61352:2116";

fn glyph_map() -> &'static HashMap<String, char> {
    static MAP: OnceLock<HashMap<String, char>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut m = HashMap::new();
        for (i, n) in ASCII_NAMES.iter().enumerate() {
            m.insert((*n).to_owned(), char::from(0x20 + i as u8));
        }
        for c in 'A'..='Z' {
            m.insert(c.to_string(), c);
        }
        for (i, n) in ASCII_AFTER_Z.iter().enumerate() {
            m.insert((*n).to_owned(), char::from(0x5B + i as u8));
        }
        for c in 'a'..='z' {
            m.insert(c.to_string(), c);
        }
        for (i, n) in ASCII_AFTER_LOWER_Z.iter().enumerate() {
            m.insert((*n).to_owned(), char::from(0x7B + i as u8));
        }
        for (i, n) in LATIN1_NAMES.iter().enumerate() {
            m.insert((*n).to_owned(), char::from(0xA1 + i as u8));
        }
        for item in EXTRA_NAMES.split_whitespace() {
            if let Some((n, h)) = item.split_once(':') {
                if let Some(c) = u32::from_str_radix(h, 16).ok().and_then(char::from_u32) {
                    m.insert(n.to_owned(), c);
                }
            }
        }
        m
    })
}

/// The character of a glyph name: the Adobe glyph names of the Latin encodings, `uniXXXX`,
/// `uXXXX`. Names like `g12` or `cid34` have none.
pub fn glyph_char(name: &str) -> Option<char> {
    let base = name.split('.').next().unwrap_or(name);
    if base.is_empty() {
        return None;
    }
    if let Some(c) = glyph_map().get(base) {
        return Some(*c);
    }
    if let Some(h) = base.strip_prefix("uni") {
        if h.len() >= 4 && h.is_char_boundary(4) {
            return u32::from_str_radix(&h[..4], 16)
                .ok()
                .and_then(char::from_u32);
        }
    }
    if let Some(h) = base.strip_prefix('u') {
        if (4..=6).contains(&h.len()) {
            return u32::from_str_radix(h, 16).ok().and_then(char::from_u32);
        }
    }
    None
}

const WIN_80: [Option<char>; 32] = [
    Some('\u{20AC}'),
    Some('\u{2022}'),
    Some('\u{201A}'),
    Some('\u{0192}'),
    Some('\u{201E}'),
    Some('\u{2026}'),
    Some('\u{2020}'),
    Some('\u{2021}'),
    Some('\u{02C6}'),
    Some('\u{2030}'),
    Some('\u{0160}'),
    Some('\u{2039}'),
    Some('\u{0152}'),
    Some('\u{2022}'),
    Some('\u{017D}'),
    Some('\u{2022}'),
    Some('\u{2022}'),
    Some('\u{2018}'),
    Some('\u{2019}'),
    Some('\u{201C}'),
    Some('\u{201D}'),
    Some('\u{2022}'),
    Some('\u{2013}'),
    Some('\u{2014}'),
    Some('\u{02DC}'),
    Some('\u{2122}'),
    Some('\u{0161}'),
    Some('\u{203A}'),
    Some('\u{0153}'),
    Some('\u{2022}'),
    Some('\u{017E}'),
    Some('\u{0178}'),
];

/// MacRomanEncoding, 0x80 to 0xFF (as the PDF reference defines it).
const MAC_HIGH: &str = "ÄÅÇÉÑÖÜáàâäãåçéèêëíìîïñóòôöõúùûü†°¢£§•¶ß®©™´¨≠ÆØ∞±≤≥¥µ∂∑∏π∫ªºΩæø¿¡¬√ƒ≈∆«»… ÀÃÕŒœ–—“”‘’÷◊ÿŸ⁄¤‹›ﬁﬂ‡·‚„‰ÂÊÁËÈÍÎÏÌÓÔ\u{F8FF}ÒÚÛÙıˆ˜¯˘˙˚¸˝˛ˇ";

/// StandardEncoding above ASCII, as (code, character).
const STANDARD_HIGH: &[(u8, char)] = &[
    (0xA1, '¡'),
    (0xA2, '¢'),
    (0xA3, '£'),
    (0xA4, '\u{2044}'),
    (0xA5, '¥'),
    (0xA6, 'ƒ'),
    (0xA7, '§'),
    (0xA8, '¤'),
    (0xA9, '\''),
    (0xAA, '“'),
    (0xAB, '«'),
    (0xAC, '‹'),
    (0xAD, '›'),
    (0xAE, 'ﬁ'),
    (0xAF, 'ﬂ'),
    (0xB1, '–'),
    (0xB2, '†'),
    (0xB3, '‡'),
    (0xB4, '·'),
    (0xB6, '¶'),
    (0xB7, '•'),
    (0xB8, '‚'),
    (0xB9, '„'),
    (0xBA, '”'),
    (0xBB, '»'),
    (0xBC, '…'),
    (0xBD, '‰'),
    (0xBF, '¿'),
    (0xC1, '`'),
    (0xC2, '´'),
    (0xC3, 'ˆ'),
    (0xC4, '˜'),
    (0xC5, '¯'),
    (0xC6, '˘'),
    (0xC7, '˙'),
    (0xC8, '¨'),
    (0xCA, '˚'),
    (0xCB, '¸'),
    (0xCD, '˝'),
    (0xCE, '˛'),
    (0xCF, 'ˇ'),
    (0xD0, '—'),
    (0xE1, 'Æ'),
    (0xE3, 'ª'),
    (0xE8, 'Ł'),
    (0xE9, 'Ø'),
    (0xEA, 'Œ'),
    (0xEB, 'º'),
    (0xF1, 'æ'),
    (0xF5, 'ı'),
    (0xF8, 'ł'),
    (0xF9, 'ø'),
    (0xFA, 'œ'),
    (0xFB, 'ß'),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    WinAnsi,
    MacRoman,
    Standard,
}

fn ascii(code: u8) -> Option<char> {
    (0x20..=0x7E).contains(&code).then(|| char::from(code))
}

pub fn table(base: Base) -> Vec<Option<char>> {
    let mut t: Vec<Option<char>> = (0..=255u8).map(ascii).collect();
    match base {
        Base::WinAnsi => {
            for (i, c) in WIN_80.iter().enumerate() {
                t[0x80 + i] = *c;
            }
            for code in 0xA0..=0xFFu32 {
                t[code as usize] = char::from_u32(code);
            }
            t[0xA0] = Some(' ');
            t[0xAD] = Some('-');
        }
        Base::MacRoman => {
            for (i, c) in MAC_HIGH.chars().enumerate() {
                if let Some(slot) = t.get_mut(0x80 + i) {
                    *slot = Some(if c == ' ' { '\u{A0}' } else { c });
                }
            }
        }
        Base::Standard => {
            t[0x27] = Some('\u{2019}');
            t[0x60] = Some('\u{2018}');
            for (code, c) in STANDARD_HIGH {
                t[usize::from(*code)] = Some(*c);
            }
        }
    }
    t
}

// ---------------------------------------------------------------------------------------------
// CMaps (ToUnicode).

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Hex(Vec<u8>),
    Open,
    Close,
    Word(String),
}

fn cmap_tokens(data: &[u8]) -> Vec<Tok> {
    let mut out = Vec::new();
    let n = data.len();
    let mut i = 0;
    while i < n {
        let c = data[i];
        match c {
            b'%' => {
                while i < n && data[i] != b'\n' && data[i] != b'\r' {
                    i += 1;
                }
            }
            b'<' if data.get(i + 1) == Some(&b'<') => i += 2,
            b'>' if data.get(i + 1) == Some(&b'>') => i += 2,
            b'<' => match crate::strings::parse_hex(data, i) {
                Some((b, e)) => {
                    out.push(Tok::Hex(b));
                    i = e;
                }
                None => i += 1,
            },
            b'[' => {
                out.push(Tok::Open);
                i += 1;
            }
            b']' => {
                out.push(Tok::Close);
                i += 1;
            }
            b'(' => match crate::strings::parse_literal(data, i) {
                Some((_, e, _)) => i = e,
                None => i += 1,
            },
            _ if crate::strings::is_space(c) => i += 1,
            _ => {
                let s = i;
                while i < n
                    && !crate::strings::is_space(data[i])
                    && !matches!(data[i], b'<' | b'>' | b'[' | b']' | b'(' | b'%')
                {
                    i += 1;
                }
                if i == s {
                    i += 1;
                } else {
                    out.push(Tok::Word(String::from_utf8_lossy(&data[s..i]).into_owned()));
                }
            }
        }
    }
    out
}

fn be(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .take(4)
        .fold(0u32, |a, b| a << 8 | u32::from(*b))
}

fn units(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks(2)
        .map(|p| u16::from(p[0]) << 8 | u16::from(*p.get(1).unwrap_or(&0)))
        .collect()
}

/// A `ToUnicode` map: code space ranges and character mappings.
#[derive(Debug, Default)]
pub struct CMap {
    spaces: Vec<(usize, u32, u32)>,
    /// (code length, first code) -> (last code, UTF-16 of the first code)
    map: BTreeMap<(usize, u32), (u32, Vec<u16>)>,
}

impl CMap {
    pub fn parse(data: &[u8]) -> CMap {
        let toks = cmap_tokens(data);
        let mut cm = CMap::default();
        let mut i = 0;
        while i < toks.len() {
            let Tok::Word(w) = &toks[i] else {
                i += 1;
                continue;
            };
            i += 1;
            match w.as_str() {
                "begincodespacerange" => {
                    while let (Some(Tok::Hex(lo)), Some(Tok::Hex(hi))) =
                        (toks.get(i), toks.get(i + 1))
                    {
                        cm.spaces.push((lo.len(), be(lo), be(hi)));
                        i += 2;
                    }
                }
                "beginbfchar" => {
                    while let (Some(Tok::Hex(src)), Some(Tok::Hex(dst))) =
                        (toks.get(i), toks.get(i + 1))
                    {
                        let code = be(src);
                        cm.map.insert((src.len(), code), (code, units(dst)));
                        i += 2;
                    }
                }
                "beginbfrange" => {
                    while let (Some(Tok::Hex(lo)), Some(Tok::Hex(hi))) =
                        (toks.get(i), toks.get(i + 1))
                    {
                        let (l, a, b) = (lo.len(), be(lo), be(hi));
                        i += 2;
                        match toks.get(i) {
                            Some(Tok::Hex(dst)) => {
                                if b >= a {
                                    cm.map.insert((l, a), (b, units(dst)));
                                }
                                i += 1;
                            }
                            Some(Tok::Open) => {
                                i += 1;
                                let mut code = a;
                                while let Some(Tok::Hex(dst)) = toks.get(i) {
                                    cm.map.insert((l, code), (code, units(dst)));
                                    code = code.wrapping_add(1);
                                    i += 1;
                                }
                                if toks.get(i) == Some(&Tok::Close) {
                                    i += 1;
                                }
                            }
                            _ => break,
                        }
                    }
                }
                _ => {}
            }
        }
        cm
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    fn lookup(&self, len: usize, code: u32) -> Option<Vec<u16>> {
        let (&(l, lo), (hi, base)) = self.map.range(..=(len, code)).next_back()?;
        if l != len || code > *hi {
            return None;
        }
        let mut u = base.clone();
        if let Some(last) = u.last_mut() {
            *last = last.wrapping_add((code - lo) as u16);
        }
        Some(u)
    }

    pub fn decode(&self, bytes: &[u8], default_len: usize) -> Vec<char> {
        let mut lens: Vec<usize> = self.spaces.iter().map(|s| s.0).collect();
        lens.sort_unstable();
        lens.dedup();
        if lens.is_empty() {
            lens.push(default_len);
        }
        let mut out = Vec::new();
        let mut pos = 0;
        while pos < bytes.len() {
            let mut used = None;
            for &l in &lens {
                if l == 0 || pos + l > bytes.len() {
                    continue;
                }
                let code = be(&bytes[pos..pos + l]);
                if self.spaces.is_empty()
                    || self
                        .spaces
                        .iter()
                        .any(|(sl, lo, hi)| *sl == l && code >= *lo && code <= *hi)
                {
                    used = Some((l, code));
                    break;
                }
            }
            let (l, code) = used.unwrap_or_else(|| {
                let l = lens[0].max(1).min(bytes.len() - pos);
                (l, be(&bytes[pos..pos + l]))
            });
            match self.lookup(l, code) {
                Some(u) => out.extend(char::decode_utf16(u).map(|r| r.unwrap_or(GAP))),
                None => out.push(GAP),
            }
            pos += l;
        }
        out
    }

    /// Every (code, character) the map defines, one character per code (the first of a
    /// sequence). Ranges are cut at 70000 codes.
    pub fn entries(&self) -> Vec<(u32, char)> {
        let mut out = Vec::new();
        for (&(len, lo), (hi, base)) in &self.map {
            for code in lo..=(*hi).min(lo.saturating_add(70_000)) {
                if let Some(u) = self.lookup(len, code) {
                    if let Some(Ok(c)) = char::decode_utf16(u).next() {
                        out.push((code, c));
                    }
                }
            }
            let _ = base;
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------
// Mappers.

/// A way to read the bytes of shown strings.
#[derive(Clone)]
pub enum Mapper {
    Cmap(Rc<CMap>, usize),
    Table(Rc<Vec<Option<char>>>),
    /// Two-byte codes that are UTF-16BE (the `Uni*-UCS2-*` and `Uni*-UTF16-*` CMaps).
    Ucs2,
    /// The bytes cannot be turned into characters.
    Unknown(String),
}

impl Mapper {
    pub fn decode(&self, bytes: &[u8]) -> Vec<char> {
        match self {
            Mapper::Cmap(cm, len) => cm.decode(bytes, *len),
            Mapper::Table(t) => bytes
                .iter()
                .map(|b| t[usize::from(*b)].unwrap_or(GAP))
                .collect(),
            Mapper::Ucs2 => char::decode_utf16(units(bytes))
                .map(|r| r.unwrap_or(GAP))
                .collect(),
            Mapper::Unknown(_) => vec![GAP; bytes.len()],
        }
    }

    pub fn unknown(&self) -> Option<&str> {
        match self {
            Mapper::Unknown(why) => Some(why),
            _ => None,
        }
    }
}

fn stream_bytes(doc: &Doc, o: &Obj) -> Option<Vec<u8>> {
    let Obj::Ref(n) = o else { return None };
    doc.all(*n)
        .iter()
        .rev()
        .find_map(|e| doc.stream_data(e))
        .map(|d| d.data)
}

fn to_unicode(doc: &Doc, font: &Obj, default_len: usize) -> Option<Mapper> {
    let tu = font.get("ToUnicode")?;
    match tu {
        Obj::Name(n) if n == "Identity-H" => None,
        _ => {
            let data = stream_bytes(doc, tu)?;
            let cm = CMap::parse(&data);
            if cm.is_empty() {
                None
            } else {
                Some(Mapper::Cmap(Rc::new(cm), default_len))
            }
        }
    }
}

fn base_by_name(n: &str) -> Option<Base> {
    match n {
        "WinAnsiEncoding" => Some(Base::WinAnsi),
        "MacRomanEncoding" => Some(Base::MacRoman),
        "StandardEncoding" => Some(Base::Standard),
        _ => None,
    }
}

/// The ways to read the strings shown with this font dictionary.
pub fn mappers(doc: &Doc, font: &Obj) -> Vec<Mapper> {
    let subtype = doc.dget(font, "Subtype").name().unwrap_or("");
    let mut out = Vec::new();
    if subtype == "Type0" {
        out.extend(to_unicode(doc, font, 2));
        if let Obj::Name(n) = doc.dget(font, "Encoding") {
            if n.starts_with("Uni") && (n.contains("UCS2") || n.contains("UTF16")) {
                out.push(Mapper::Ucs2);
            }
        }
        if out.is_empty() {
            let enc = match doc.dget(font, "Encoding") {
                Obj::Name(n) => n.clone(),
                Obj::Dict(_) => "an embedded CMap".to_owned(),
                _ => "none".to_owned(),
            };
            out.push(Mapper::Unknown(format!(
                "composite font (encoding {enc}) has no ToUnicode map: its codes are glyph numbers"
            )));
        }
        return out;
    }
    out.extend(to_unicode(doc, font, 1));
    let desc = doc.dget(font, "FontDescriptor");
    let flags = doc.dget(desc, "Flags").int().unwrap_or(0);
    let symbolic = flags & 4 != 0 && flags & 32 == 0;
    let embedded = desc.get("FontFile").is_some()
        || desc.get("FontFile2").is_some()
        || desc.get("FontFile3").is_some();
    let enc = doc.dget(font, "Encoding");
    let mut base: Option<Base> = None;
    let mut explicit = false;
    let mut diffs: Vec<(u8, String)> = Vec::new();
    match enc {
        Obj::Name(n) => {
            base = base_by_name(n);
            explicit = true;
            if base.is_none() {
                out.push(Mapper::Unknown(format!(
                    "encoding {n} is not one of the Latin encodings"
                )));
            }
        }
        Obj::Dict(_) => {
            if let Obj::Name(n) = doc.dget(enc, "BaseEncoding") {
                base = base_by_name(n);
                explicit = base.is_some();
            }
            if let Obj::Array(a) = doc.dget(enc, "Differences") {
                let mut code = 0i64;
                for x in a {
                    match doc.resolve(x) {
                        Obj::Int(i) => code = *i,
                        Obj::Name(n) => {
                            if let Ok(c) = u8::try_from(code) {
                                diffs.push((c, n.clone()));
                            }
                            code += 1;
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }
    let builtin = base.is_none() && symbolic && embedded;
    if builtin && diffs.is_empty() && out.is_empty() {
        out.push(Mapper::Unknown(
            "an embedded symbolic font without an encoding uses the codes of its font program"
                .into(),
        ));
        return out;
    }
    let assumed = if subtype == "TrueType" {
        Base::WinAnsi
    } else {
        Base::Standard
    };
    let mut bases: Vec<Option<Base>> = Vec::new();
    if builtin || subtype == "Type3" {
        bases.push(None);
    } else if let Some(b) = base {
        bases.push(Some(b));
    } else {
        bases.push(Some(assumed));
        // The assumption can be wrong: also read it the other way.
        bases.push(Some(if assumed == Base::WinAnsi {
            Base::Standard
        } else {
            Base::WinAnsi
        }));
    }
    let _ = explicit;
    for b in bases {
        let mut t: Vec<Option<char>> = b.map_or_else(|| vec![None; 256], table);
        for (code, name) in &diffs {
            t[usize::from(*code)] = glyph_char(name);
        }
        out.push(Mapper::Table(Rc::new(t)));
    }
    out
}

/// What a font dictionary keeps: the characters its `ToUnicode` map and `/Differences` can show,
/// with the glyph numbers (codes) that show them.
pub struct FontTraces {
    pub subset: bool,
    pub chars: BTreeMap<char, Vec<u32>>,
    /// The `/CIDSet` bit string of a composite font, if there is one.
    pub cid_set: Option<Vec<u8>>,
}

pub fn traces(doc: &Doc, font: &Obj) -> FontTraces {
    let base = doc.dget(font, "BaseFont").name().unwrap_or("");
    let subset = base.len() > 7
        && base.as_bytes()[6] == b'+'
        && base[..6].bytes().all(|b| b.is_ascii_uppercase());
    let mut chars: BTreeMap<char, Vec<u32>> = BTreeMap::new();
    if let Some(Mapper::Cmap(cm, _)) = to_unicode(doc, font, 1) {
        for (code, c) in cm.entries() {
            chars.entry(c).or_default().push(code);
        }
    }
    if let Obj::Dict(_) = doc.dget(font, "Encoding") {
        if let Obj::Array(a) = doc.dget(doc.dget(font, "Encoding"), "Differences") {
            let mut code = 0i64;
            for x in a {
                match doc.resolve(x) {
                    Obj::Int(i) => code = *i,
                    Obj::Name(n) => {
                        if let Some(c) = glyph_char(n) {
                            chars.entry(c).or_default().push(code as u32);
                        }
                        code += 1;
                    }
                    _ => {}
                }
            }
        }
    }
    // A composite font keeps its descriptor in the descendant font.
    let mut desc = doc.dget(font, "FontDescriptor");
    if let Obj::Array(d) = doc.dget(font, "DescendantFonts") {
        if let Some(first) = d.first() {
            desc = doc.dget(doc.resolve(first), "FontDescriptor");
        }
    }
    let cid_set = stream_bytes(doc, desc.get("CIDSet").unwrap_or(&Obj::Null));
    FontTraces {
        subset,
        chars,
        cid_set,
    }
}

/// All characters that can be told apart in a set of strings, for tests and reports.
pub fn char_set(text: &str) -> BTreeSet<char> {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winansi_and_macroman_differ_from_latin1() {
        let w = table(Base::WinAnsi);
        assert_eq!(w[0x92], Some('\u{2019}'));
        assert_eq!(w[0xE5], Some('å'));
        let m = table(Base::MacRoman);
        assert_eq!(m[0x8C], Some('å'));
        assert_eq!(m[0xBE], Some('æ'));
        assert_eq!(m[0xBF], Some('ø'));
        assert_eq!(table(Base::Standard)[0x27], Some('\u{2019}'));
        assert_eq!(table(Base::Standard)[0xFB], Some('ß'));
    }

    #[test]
    fn the_macroman_table_has_128_high_characters() {
        assert_eq!(MAC_HIGH.chars().count(), 128);
    }

    #[test]
    fn glyph_names_read() {
        assert_eq!(glyph_char("A"), Some('A'));
        assert_eq!(glyph_char("hyphen"), Some('-'));
        assert_eq!(glyph_char("aring"), Some('å'));
        assert_eq!(glyph_char("uni2019"), Some('\u{2019}'));
        assert_eq!(glyph_char("Euro"), Some('€'));
        assert_eq!(glyph_char("a.alt"), Some('a'));
        assert_eq!(glyph_char("g23"), None);
    }

    #[test]
    fn a_tounicode_map_with_chars_and_ranges() {
        let cm = CMap::parse(
            b"1 begincodespacerange <0000> <FFFF> endcodespacerange \
              2 beginbfchar <0101> <004F> <0102> <006C> endbfchar \
              1 beginbfrange <0110> <0112> <0061> endbfrange",
        );
        assert_eq!(
            cm.decode(&[1, 1, 1, 2, 1, 0x11, 9, 9], 2),
            ['O', 'l', 'b', GAP]
        );
    }
}
