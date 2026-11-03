//! Searching a buffer for a text in the encodings that PDF files use.

use std::fmt;

use unicode_normalization::UnicodeNormalization;

use crate::strings::{self, StringKind};

/// Which switches of the search are on. A test turns one off to show that the hit disappears
/// with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Features {
    /// Search UTF-16 (big and little endian) forms of every text.
    pub utf16: bool,
    /// Decode hex strings and escaped literal strings before searching.
    pub decode_strings: bool,
    /// Join the strings of one text object (`[(SEC) -20 (RET)] TJ`, `(SEC) Tj (RET) Tj`).
    pub split_text: bool,
}

impl Default for Features {
    fn default() -> Self {
        Features {
            utf16: true,
            decode_strings: true,
            split_text: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Encoding {
    Utf8,
    Latin1,
    Utf16Be,
    Utf16Le,
}

/// How the text was stored around its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Via {
    /// The bytes are in the buffer as they are.
    Direct,
    /// Inside a hex string `<..>`.
    Hex,
    /// Inside a literal string that uses backslash escapes.
    Escaped,
    /// Spread over several strings of one text object.
    Split,
    /// In text read back by a tool, with line breaks and spaces folded.
    Folded,
    /// A decoded value in the document structure (string, name or key).
    Structure,
    /// Text decoded from a content stream through the fonts of the file.
    Decoded,
    /// Rich text or XML with the markup taken out and the entities decoded.
    Markup,
    /// A trace of the text in a font that stays in the file.
    Font,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Form {
    pub encoding: Encoding,
    pub via: Via,
}

impl fmt::Display for Form {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let enc = match self.encoding {
            Encoding::Utf8 => "plain",
            Encoding::Latin1 => "latin1",
            Encoding::Utf16Be => "utf16be",
            Encoding::Utf16Le => "utf16le",
        };
        let via = match self.via {
            Via::Direct => "direct",
            Via::Hex => "hex-string",
            Via::Escaped => "escaped-string",
            Via::Split => "split-text",
            Via::Folded => "folded-text",
            Via::Structure => "structure-value",
            Via::Decoded => "decoded-text",
            Via::Markup => "markup-stripped",
            Via::Font => "font-trace",
        };
        write!(f, "{enc}/{via}")
    }
}

/// One text in one encoding.
#[derive(Debug, Clone)]
pub struct Pattern {
    pub encoding: Encoding,
    pub bytes: Vec<u8>,
}

/// A hit in a buffer.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct BufHit {
    pub needle: usize,
    pub offset: usize,
    pub form: Form,
}

/// The texts to look for, ready in every encoding.
#[derive(Debug, Clone)]
pub struct NeedleSet {
    entries: Vec<Entry>,
    features: Features,
}

#[derive(Debug, Clone)]
struct Entry {
    index: usize,
    text: String,
    patterns: Vec<Pattern>,
    /// The patterns of the text with all white space taken out (empty when that is the same).
    stripped: Vec<Pattern>,
    /// The text normalised (NFKC) with white space folded, and with it taken out.
    folded_chars: Vec<char>,
    stripped_chars: Vec<char>,
}

pub fn patterns(text: &str, features: &Features) -> Vec<Pattern> {
    let mut out = Vec::new();
    let utf8 = text.as_bytes().to_vec();
    out.push(Pattern {
        encoding: Encoding::Utf8,
        bytes: utf8.clone(),
    });
    if text.chars().all(|c| (c as u32) < 256) {
        let latin1: Vec<u8> = text.chars().map(|c| c as u8).collect();
        if latin1 != utf8 {
            out.push(Pattern {
                encoding: Encoding::Latin1,
                bytes: latin1,
            });
        }
    }
    if features.utf16 {
        let units: Vec<u16> = text.encode_utf16().collect();
        out.push(Pattern {
            encoding: Encoding::Utf16Be,
            bytes: units.iter().flat_map(|u| u.to_be_bytes()).collect(),
        });
        out.push(Pattern {
            encoding: Encoding::Utf16Le,
            bytes: units.iter().flat_map(|u| u.to_le_bytes()).collect(),
        });
    }
    out
}

/// Offsets of every (also overlapping) occurrence of `pat` in `hay`.
pub fn find_all(hay: &[u8], pat: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    if pat.is_empty() || hay.len() < pat.len() {
        return out;
    }
    let first = pat[0];
    let last_start = hay.len() - pat.len();
    let mut i = 0;
    while i <= last_start {
        match hay[i..=last_start].iter().position(|&b| b == first) {
            Some(p) => {
                i += p;
                if &hay[i..i + pat.len()] == pat {
                    out.push(i);
                }
                i += 1;
            }
            None => break,
        }
    }
    out
}

/// All white space folded to one space, and the ends trimmed.
pub fn fold_space(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The text in NFKC form: ligatures and compatibility forms become plain letters, and a letter
/// with a combining mark becomes one character, so that NFC and NFD forms compare equal.
pub fn norm(text: &str) -> String {
    text.nfkc().collect()
}

/// White space, soft hyphens and zero-width characters: what a reader does not see between letters.
fn is_blank(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '\u{AD}' | '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}'
        )
}

/// Normalised, with all white space folded to one space.
pub fn fold_norm(text: &str) -> String {
    let n: String = norm(text)
        .chars()
        .map(|c| if is_blank(c) { ' ' } else { c })
        .collect();
    fold_space(&n)
}

/// Normalised, with all white space taken out.
pub fn strip_norm(text: &str) -> String {
    norm(text).chars().filter(|c| !is_blank(*c)).collect()
}

/// Offsets where `pat` is in `hay`; a [`crate::fonts::GAP`] in `hay` stands for any one
/// character. At least half of the characters of `pat` must be matched by real characters, so
/// that text of which the font tells nothing is not a match for everything.
pub fn find_wild(hay: &[char], pat: &[char]) -> Vec<usize> {
    let mut out = Vec::new();
    if pat.is_empty() || hay.len() < pat.len() {
        return out;
    }
    for i in 0..=hay.len() - pat.len() {
        let mut real = 0usize;
        let mut ok = true;
        for (h, p) in hay[i..i + pat.len()].iter().zip(pat) {
            if h == p {
                real += 1;
            } else if *h != crate::fonts::GAP {
                ok = false;
                break;
            }
        }
        if ok && real * 2 >= pat.len() && real > 0 {
            out.push(i);
            if out.len() >= 16 {
                break;
            }
        }
    }
    out
}

/// The text of rich text or XML: tags taken out (replaced by `gap`), entities decoded.
pub fn strip_markup(s: &str, gap: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if let Some(body) = rest.strip_prefix("<![CDATA[") {
            let end = body.find("]]>").unwrap_or(body.len());
            out.push_str(&body[..end]);
            rest = body.get(end + 3..).unwrap_or("");
        } else if c == '<' {
            match rest.find('>') {
                Some(e) => {
                    out.push_str(gap);
                    rest = &rest[e + 1..];
                }
                None => {
                    out.push_str(rest);
                    break;
                }
            }
        } else if c == '&' {
            let semi = rest
                .char_indices()
                .take(12)
                .find(|(_, ch)| *ch == ';')
                .map(|(i, _)| i);
            match semi.and_then(|e| entity(&rest[1..e]).map(|d| (d, e))) {
                Some((d, e)) => {
                    out.push(d);
                    rest = &rest[e + 1..];
                }
                None => {
                    out.push('&');
                    rest = &rest[1..];
                }
            }
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{A0}'),
        _ => {
            let n = name.strip_prefix('#')?;
            let v = match n.strip_prefix(['x', 'X']) {
                Some(h) => u32::from_str_radix(h, 16).ok()?,
                None => n.parse().ok()?,
            };
            char::from_u32(v)
        }
    }
}

impl NeedleSet {
    /// `texts` is a list of (needle index, text). An empty text would match everywhere and is
    /// refused by the caller.
    pub fn new(texts: &[(usize, &str)], features: Features) -> Self {
        NeedleSet {
            entries: texts
                .iter()
                .map(|&(index, text)| {
                    let stripped_text: String =
                        text.chars().filter(|c| !c.is_whitespace()).collect();
                    Entry {
                        index,
                        text: text.to_owned(),
                        patterns: patterns(text, &features),
                        stripped: if stripped_text.is_empty() || stripped_text == text {
                            Vec::new()
                        } else {
                            patterns(&stripped_text, &features)
                        },
                        folded_chars: fold_norm(text).chars().collect(),
                        stripped_chars: strip_norm(text).chars().collect(),
                    }
                })
                .collect(),
            features,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn scan(&self, hay: &[u8], offset_base: usize, via: Via, fixed: bool, out: &mut Vec<BufHit>) {
        for e in &self.entries {
            for p in &e.patterns {
                for off in find_all(hay, &p.bytes) {
                    out.push(BufHit {
                        needle: e.index,
                        offset: if fixed {
                            offset_base
                        } else {
                            offset_base + off
                        },
                        form: Form {
                            encoding: p.encoding,
                            via,
                        },
                    });
                }
            }
        }
    }

    /// Searches a buffer: the bytes as they are, and (if on) the decoded bytes of every hex
    /// string and of every literal string that has escapes. `masked` ranges are not scanned for
    /// strings (compressed streams, which have their own layer). Splitting is separate:
    /// [`NeedleSet::search_split`].
    pub fn search(&self, buf: &[u8], masked: &[(usize, usize)]) -> Vec<BufHit> {
        let mut out = Vec::new();
        self.scan(buf, 0, Via::Direct, false, &mut out);
        if self.features.decode_strings {
            for s in strings::extract(buf, masked) {
                let via = match s.kind {
                    StringKind::Literal => continue,
                    StringKind::Escaped => Via::Escaped,
                    StringKind::Hex => Via::Hex,
                };
                self.scan(&s.bytes, s.start, via, true, &mut out);
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Searches the strings of each text object joined into one, with nothing between them and
    /// with a space between them.
    pub fn search_split(&self, buf: &[u8]) -> Vec<BufHit> {
        let mut out = Vec::new();
        if !self.features.split_text {
            return out;
        }
        for obj in crate::content::text_chains(buf) {
            self.scan(&obj.joined, obj.offset, Via::Split, true, &mut out);
            self.scan(&obj.spaced, obj.offset, Via::Split, true, &mut out);
            // The same with the white space of the needle taken out: a text that is set letter by
            // letter or word by word without a space between.
            for e in &self.entries {
                for p in &e.stripped {
                    for _ in find_all(&obj.joined, &p.bytes) {
                        out.push(BufHit {
                            needle: e.index,
                            offset: obj.offset,
                            form: Form {
                                encoding: p.encoding,
                                via: Via::Split,
                            },
                        });
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Searches text that a tool printed (Poppler, qpdf): as it is, with white space folded on both
    /// sides (a phrase that the tool broke over two lines). Offsets are byte offsets in `text`;
    /// the folded form reports the offset in the folded text.
    pub fn search_text(&self, text: &str) -> Vec<BufHit> {
        let mut out = Vec::new();
        let folded = fold_space(text);
        let nfolded = fold_norm(text);
        let nstripped = strip_norm(text);
        for e in &self.entries {
            for off in find_all(text.as_bytes(), e.text.as_bytes()) {
                out.push(BufHit {
                    needle: e.index,
                    offset: off,
                    form: Form {
                        encoding: Encoding::Utf8,
                        via: Via::Direct,
                    },
                });
            }
            let want = fold_space(&e.text);
            for (hay, want) in [
                (&folded, want),
                (&nfolded, fold_norm(&e.text)),
                (&nstripped, strip_norm(&e.text)),
            ] {
                if !want.is_empty() {
                    for off in find_all(hay.as_bytes(), want.as_bytes()) {
                        out.push(BufHit {
                            needle: e.index,
                            offset: off,
                            form: Form {
                                encoding: Encoding::Utf8,
                                via: Via::Folded,
                            },
                        });
                    }
                }
            }
        }
        // The folded form repeats the direct one, so only the most exact form of a needle is
        // reported.
        out.sort();
        out.dedup();
        let best: Vec<(usize, Via)> = out
            .iter()
            .map(|h| h.needle)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter_map(|n| {
                out.iter()
                    .filter(|h| h.needle == n)
                    .map(|h| h.form.via)
                    .min()
                    .map(|v| (n, v))
            })
            .collect();
        out.retain(|h| best.contains(&(h.needle, h.form.via)));
        // The folded forms find the same text: one hit for a needle is enough.
        let mut seen = std::collections::BTreeSet::new();
        out.retain(|h| h.form.via != Via::Folded || seen.insert(h.needle));
        out
    }

    /// Searches text that was read from a content stream through fonts. A [`crate::fonts::GAP`]
    /// is a character the font could not tell. Compared with white space folded and with it
    /// taken out, both sides normalised.
    pub fn search_decoded(&self, text: &[char]) -> Vec<BufHit> {
        let marker = '\u{E000}';
        let joined: String = text
            .iter()
            .map(|c| if *c == crate::fonts::GAP { marker } else { *c })
            .collect();
        let folded: Vec<char> = fold_norm(&joined)
            .chars()
            .map(|c| if c == marker { crate::fonts::GAP } else { c })
            .collect();
        let stripped: Vec<char> = strip_norm(&joined.replace(marker, "")).chars().collect();
        let mut out = Vec::new();
        for e in &self.entries {
            let hits = [
                find_wild(&folded, &e.folded_chars),
                find_wild(&stripped, &e.stripped_chars),
            ];
            for off in hits.into_iter().flatten() {
                out.push(BufHit {
                    needle: e.index,
                    offset: off,
                    form: Form {
                        encoding: Encoding::Utf8,
                        via: Via::Decoded,
                    },
                });
            }
        }
        out.sort();
        out.dedup_by_key(|h| h.needle);
        out
    }

    /// Searches rich text or XML (`/RC`, XMP): the text with the markup taken out and the
    /// entities decoded, with a tag read as nothing and as a space.
    pub fn search_markup(&self, text: &str) -> Vec<BufHit> {
        let mut out = Vec::new();
        for gap in ["", " "] {
            for h in self.search_text(&strip_markup(text, gap)) {
                out.push(BufHit {
                    form: Form {
                        encoding: Encoding::Utf8,
                        via: Via::Markup,
                    },
                    ..h
                });
            }
        }
        out.sort();
        out.dedup_by_key(|h| h.needle);
        out
    }

    /// Searches a decoded value from the document structure. Text values are compared as text;
    /// binary strings as bytes.
    pub fn search_value(&self, value: &str) -> Vec<BufHit> {
        let mut out = Vec::new();
        for e in &self.entries {
            let at = value.find(e.text.as_str()).or_else(|| {
                // Another Unicode form of the same text (NFC against NFD, ligatures).
                let hay = fold_norm(value);
                hay.find(fold_norm(&e.text).as_str())
            });
            if let Some(off) = at {
                out.push(BufHit {
                    needle: e.index,
                    offset: off,
                    form: Form {
                        encoding: Encoding::Utf8,
                        via: Via::Structure,
                    },
                });
            }
        }
        out
    }

    pub fn search_value_bytes(&self, value: &[u8]) -> Vec<BufHit> {
        let mut out = Vec::new();
        self.scan(value, 0, Via::Structure, false, &mut out);
        out.sort();
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_overlapping_occurrences() {
        assert_eq!(find_all(b"aaaa", b"aa"), vec![0, 1, 2]);
        assert!(find_all(b"abc", b"abcd").is_empty());
    }

    #[test]
    fn utf16_forms_are_there_only_when_asked() {
        let on = patterns("Aé", &Features::default());
        assert!(on.iter().any(|p| p.encoding == Encoding::Utf16Be));
        assert!(on.iter().any(|p| p.encoding == Encoding::Latin1));
        let off = patterns(
            "Aé",
            &Features {
                utf16: false,
                ..Features::default()
            },
        );
        assert!(!off.iter().any(|p| p.encoding == Encoding::Utf16Be));
    }

    #[test]
    fn hex_and_escaped_strings_are_decoded() {
        let set = NeedleSet::new(&[(0, "SECRET")], Features::default());
        let hits = set.search(b"<< /T <53454352 4554> /U (SEC\\122ET) >>", &[]);
        let vias: Vec<Via> = hits.iter().map(|h| h.form.via).collect();
        assert!(vias.contains(&Via::Hex));
        assert!(vias.contains(&Via::Escaped));
        assert!(!vias.contains(&Via::Direct));
        let off = NeedleSet::new(
            &[(0, "SECRET")],
            Features {
                decode_strings: false,
                ..Features::default()
            },
        );
        assert!(off
            .search(b"<< /T <53454352 4554> /U (SEC\\122ET) >>", &[])
            .is_empty());
    }

    #[test]
    fn folded_text_finds_a_phrase_over_a_line_break() {
        let set = NeedleSet::new(&[(0, "John Smith")], Features::default());
        assert!(set.search_text("ask John\nSmith now").len() == 1);
        assert!(set.search_text("ask John Smyth now").is_empty());
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;
    use crate::fonts::GAP;

    #[test]
    fn markup_is_taken_out_and_entities_are_decoded() {
        let s = "<p>Smith &amp; <b>Sons</b> &#x45;&#82; &lt;x&gt;</p><![CDATA[<raw>]]>";
        assert_eq!(strip_markup(s, ""), "Smith & Sons ER <x><raw>");
        assert_eq!(strip_markup("a<br/>b", " "), "a b");
        assert_eq!(strip_markup("a & b &unknown; c", ""), "a & b &unknown; c");
    }

    #[test]
    fn a_marked_up_phrase_is_found() {
        let set = NeedleSet::new(&[(0, "Ola Nordmann")], Features::default());
        assert_eq!(
            set.search_markup("<p>Ola <span>Nordmann</span></p>").len(),
            1
        );
        assert_eq!(set.search_markup("<p>Ola</p><p>Nordmann</p>").len(), 1);
        assert!(set.search_markup("<p>Ola</p><p>Nils</p>").is_empty());
    }

    #[test]
    fn a_gap_matches_one_character_but_not_most_of_the_phrase() {
        let hay: Vec<char> = "xx Ola\u{FFFD}Nordmann xx".chars().collect();
        let pat: Vec<char> = "Ola Nordmann".chars().collect();
        assert_eq!(find_wild(&hay, &pat), vec![3]);
        let blind = vec![GAP; 12];
        assert!(find_wild(&blind, &pat).is_empty());
    }

    #[test]
    fn decomposed_and_ligature_forms_compare_equal() {
        let set = NeedleSet::new(&[(0, "\u{e5}fi")], Features::default());
        assert_eq!(set.search_decoded(&['a', '\u{30A}', '\u{FB01}']).len(), 1);
        // White space between letters does not hide a phrase.
        let set = NeedleSet::new(&[(0, "SECRET")], Features::default());
        assert_eq!(
            set.search_decoded(&"S E C R E T".chars().collect::<Vec<_>>())
                .len(),
            1
        );
    }
}
