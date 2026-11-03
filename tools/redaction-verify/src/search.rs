//! Searching a buffer for a text in the encodings that PDF files use.

use std::fmt;

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

impl NeedleSet {
    /// `texts` is a list of (needle index, text). An empty text would match everywhere and is
    /// refused by the caller.
    pub fn new(texts: &[(usize, &str)], features: Features) -> Self {
        NeedleSet {
            entries: texts
                .iter()
                .map(|&(index, text)| Entry {
                    index,
                    text: text.to_owned(),
                    patterns: patterns(text, &features),
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
        for obj in crate::content::text_objects(buf) {
            self.scan(&obj.joined, obj.offset, Via::Split, true, &mut out);
            self.scan(&obj.spaced, obj.offset, Via::Split, true, &mut out);
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
            if !want.is_empty() {
                for off in find_all(folded.as_bytes(), want.as_bytes()) {
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
        out
    }

    /// Searches a decoded value from the document structure. Text values are compared as text;
    /// binary strings as bytes.
    pub fn search_value(&self, value: &str) -> Vec<BufHit> {
        let mut out = Vec::new();
        for e in &self.entries {
            if let Some(off) = value.find(e.text.as_str()) {
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
