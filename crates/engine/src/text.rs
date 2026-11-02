//! Read-only views of a document for the reader: the text of a page with the place of every
//! character, search, links and the table of contents.
//!
//! Everything here treats the document as untrusted. Every list has a limit (see the constants in
//! `partridge-protocol`), loops over document structures count their steps, and nothing here ever
//! acts on what it finds: a link is reported, never followed, and attachments are not looked at.

use std::collections::HashSet;

use partridge_protocol::{
    LinkTarget, OutlineItem, PageLink, TextMatch, MAX_LINKS_PER_PAGE, MAX_MATCHES_PER_PAGE,
    MAX_OUTLINE_DEPTH, MAX_OUTLINE_ITEMS, MAX_OUTLINE_TITLE, MAX_PAGE_CHARS, MAX_URI_LEN,
};
use pdfium_render::prelude::*;

/// One character of a page: the character itself and its box, `[left, top, right, bottom]` in
/// points from the top left of the page as it is drawn. Characters without a place (line breaks)
/// have a box of zeros.
#[derive(Debug, Clone, PartialEq)]
pub struct TextChar {
    pub ch: char,
    pub rect: [f32; 4],
}

/// The text of a page could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextUnavailable;

/// Precision of the page to device conversion: PDFium answers in whole pixels, so the conversion
/// asks for a bitmap that is this many times larger than the page and divides the answer.
const PRECISION: f32 = 16.0;

/// Turns page coordinates into points from the top left of the page as it is drawn, using the
/// same conversion PDFium uses when it draws the page, so rotated pages and pages whose box does
/// not start at zero come out right.
pub(crate) struct Mapper {
    config: PdfRenderConfig,
    sx: f32,
    sy: f32,
}

impl Mapper {
    pub(crate) fn new(page: &PdfPage) -> Mapper {
        let width = page.width().value;
        let height = page.height().value;
        let w = (width * PRECISION).round().clamp(PRECISION, 4.0e6) as i32;
        let h = (height * PRECISION).round().clamp(PRECISION, 4.0e6) as i32;
        Mapper {
            config: PdfRenderConfig::new().set_target_size(w, h),
            sx: if width > 0.0 { w as f32 / width } else { 1.0 },
            sy: if height > 0.0 { h as f32 / height } else { 1.0 },
        }
    }

    fn point(&self, page: &PdfPage, x: f32, y: f32) -> Option<(f32, f32)> {
        let (px, py) = page
            .points_to_pixels(PdfPoints::new(x), PdfPoints::new(y), &self.config)
            .ok()?;
        Some((px as f32 / self.sx, py as f32 / self.sy))
    }

    /// The other way: a point on the drawn page, in points from its top left, as a point in page
    /// coordinates. The answer is exact to a sixteenth of a point.
    pub(crate) fn to_page(&self, page: &PdfPage, x: f32, y: f32) -> Option<(f32, f32)> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let px = (x * self.sx).round().clamp(-1.0e9, 1.0e9) as i32;
        let py = (y * self.sy).round().clamp(-1.0e9, 1.0e9) as i32;
        let (u, v) = page.pixels_to_points(px, py, &self.config).ok()?;
        let (u, v) = (u.value, v.value);
        (u.is_finite() && v.is_finite()).then_some((u, v))
    }

    /// A box in page coordinates, as `[left, top, right, bottom]` on the drawn page.
    pub(crate) fn rect(&self, page: &PdfPage, r: &PdfRect) -> Option<[f32; 4]> {
        let (x1, y1) = self.point(page, r.left().value, r.top().value)?;
        let (x2, y2) = self.point(page, r.right().value, r.bottom().value)?;
        Some([x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2)])
    }
}

/// The characters of a page in reading order. PDFium inserts line breaks and spaces of its own
/// where the page has a gap; those are kept, because they are what makes copied text readable.
/// With `with_rects` false the boxes are left as zeros, which is faster and enough for a search.
pub fn page_chars(page: &PdfPage, with_rects: bool) -> Result<Vec<TextChar>, TextUnavailable> {
    let text = page.text().map_err(|_| TextUnavailable)?;
    let chars = text.chars();
    // Bound the work by what the page claims to hold, not only by what is kept.
    let raw = chars.len().min(MAX_PAGE_CHARS * 2);
    let mapper = with_rects.then(|| Mapper::new(page));
    let mut out: Vec<TextChar> = Vec::new();
    let mut i = 0;
    while i < raw && out.len() < MAX_PAGE_CHARS {
        let c = chars.get(i).map_err(|_| TextUnavailable)?;
        let index = i;
        i += 1;
        let mut value = c.unicode_value();
        // PDFium builds that count in UTF-16 report a character outside the basic plane as two
        // halves. Join them, so one character is one entry.
        if (0xD800..0xDC00).contains(&value) && i < raw {
            if let Ok(next) = chars.get(i) {
                let low = next.unicode_value();
                if (0xDC00..0xE000).contains(&low) {
                    value = 0x10000 + ((value - 0xD800) << 10) + (low - 0xDC00);
                    i += 1;
                }
            }
        }
        let ch = match char::from_u32(value) {
            Some('\r') | Some('\u{0}') | Some('\u{FFFE}') | Some('\u{FEFF}') => continue,
            Some('\t') => ' ',
            Some(c) if c.is_control() && c != '\n' => continue,
            Some(c) => c,
            None => '\u{FFFD}',
        };
        let rect = match (&mapper, ch) {
            (_, '\n') | (None, _) => [0.0; 4],
            (Some(mapper), _) => chars
                .get(index)
                .ok()
                .and_then(|c| c.loose_bounds().or_else(|_| c.tight_bounds()).ok())
                .and_then(|b| mapper.rect(page, &b))
                .unwrap_or([0.0; 4]),
        };
        out.push(TextChar { ch, rect });
    }
    Ok(out)
}

/// Lower case for matching. Only the first character of a multi-character lower case is kept, so
/// the text keeps its length and matches can be mapped back to the original characters.
fn fold(c: char) -> char {
    if c.is_whitespace() {
        ' '
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

/// Folds `chars` for matching: lower case, every run of white space becomes one space, and soft
/// hyphens disappear. Returns the folded text and, for each folded character, the index of the
/// original character it came from.
fn normalise(chars: impl Iterator<Item = char>) -> (Vec<char>, Vec<usize>) {
    let mut out: Vec<char> = Vec::new();
    let mut map: Vec<usize> = Vec::new();
    for (i, c) in chars.enumerate() {
        if c == '\u{AD}' {
            continue;
        }
        let f = fold(c);
        if f == ' ' && out.last() == Some(&' ') {
            continue;
        }
        out.push(f);
        map.push(i);
    }
    (out, map)
}

/// Finds `query` in `text`, ignoring case and runs of white space. Matches do not overlap. The
/// answer counts characters of `text`. An empty query finds nothing.
pub fn find_matches(text: &[char], query: &str) -> Vec<TextMatch> {
    let (needle, _) = normalise(query.chars());
    // Spaces at the ends of the search text would only make it fail for no reason.
    let start = needle
        .iter()
        .position(|c| *c != ' ')
        .unwrap_or(needle.len());
    let end = needle
        .iter()
        .rposition(|c| *c != ' ')
        .map_or(start, |p| p + 1);
    let needle = &needle[start..end];
    if needle.is_empty() {
        return Vec::new();
    }
    let (hay, map) = normalise(text.iter().copied());
    let mut found = Vec::new();
    let mut i = 0;
    while i + needle.len() <= hay.len() && found.len() < MAX_MATCHES_PER_PAGE {
        if hay[i] == needle[0] && hay[i..i + needle.len()] == *needle {
            let first = map[i];
            let last = map[i + needle.len() - 1];
            found.push(TextMatch {
                start: first as u32,
                len: (last - first + 1) as u32,
            });
            i += needle.len();
        } else {
            i += 1;
        }
    }
    found
}

/// Cuts a title to what is allowed and replaces control characters by spaces.
fn clean_title(title: String) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut out = cleaned.trim().to_string();
    if out.len() > MAX_OUTLINE_TITLE {
        let mut end = MAX_OUTLINE_TITLE;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    out
}

fn destination_page(dest: Option<PdfDestination>, page_count: usize) -> Option<u32> {
    let index = dest?.page_index().ok()?;
    let index = index as i64;
    (index >= 0 && index < page_count as i64).then_some(index as u32)
}

/// The table of contents. Only entries that lead to a page of this document get a page; entries
/// that do anything else (open a file, a web address) are listed without a target and are not
/// followed.
pub fn outline(document: &PdfDocument) -> Vec<OutlineItem> {
    let page_count = document.pages().len() as usize;
    let mut items: Vec<OutlineItem> = Vec::new();
    let mut seen: HashSet<PdfBookmark> = HashSet::new();
    let mut pending: Vec<(PdfBookmark, u8)> = Vec::new();
    if let Some(root) = document.bookmarks().root() {
        pending.push((root, 0));
    }
    // Entries are visited at most once each, so a loop in a damaged outline ends by itself, and
    // the number of entries is limited.
    while let Some((node, depth)) = pending.pop() {
        if items.len() >= MAX_OUTLINE_ITEMS {
            break;
        }
        if !seen.insert(node.clone()) {
            continue;
        }
        if let Some(next) = node.next_sibling() {
            pending.push((next, depth));
        }
        if depth < MAX_OUTLINE_DEPTH {
            if let Some(child) = node.first_child() {
                pending.push((child, depth + 1));
            }
        }
        let page = destination_page(node.destination(), page_count).or_else(|| {
            let action = node.action()?;
            let local = action.as_local_destination_action()?;
            destination_page(local.destination().ok(), page_count)
        });
        items.push(OutlineItem {
            depth,
            page,
            title: clean_title(node.title().unwrap_or_default()),
        });
    }
    items
}

/// The links of a page that lead to a web address or to a page of this document. Links that open
/// a file, run a program or go to another document are not reported at all.
pub fn page_links(page: &PdfPage, page_count: usize) -> Vec<PageLink> {
    let mapper = Mapper::new(page);
    let mut out = Vec::new();
    for link in page.links().iter() {
        if out.len() >= MAX_LINKS_PER_PAGE {
            break;
        }
        let target = match link.action() {
            Some(action) => {
                if let Some(uri) = action.as_uri_action() {
                    match uri.uri() {
                        Ok(u) if !u.is_empty() && u.len() <= MAX_URI_LEN => {
                            Some(LinkTarget::Uri(u))
                        }
                        _ => None,
                    }
                } else if let Some(local) = action.as_local_destination_action() {
                    destination_page(local.destination().ok(), page_count).map(LinkTarget::Page)
                } else {
                    None
                }
            }
            None => destination_page(link.destination(), page_count).map(LinkTarget::Page),
        };
        let Some(target) = target else { continue };
        let Some(rect) = link.rect().ok().and_then(|r| mapper.rect(page, &r)) else {
            continue;
        };
        out.push(PageLink { rect, target });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn spans(found: &[TextMatch]) -> Vec<(u32, u32)> {
        found.iter().map(|m| (m.start, m.len)).collect()
    }

    #[test]
    fn finds_text_ignoring_case() {
        let found = find_matches(&chars("The Fox and the fox"), "FOX");
        assert_eq!(spans(&found), vec![(4, 3), (16, 3)]);
    }

    #[test]
    fn white_space_runs_and_line_breaks_match_a_single_space() {
        let text = chars("quick\n  brown\u{a0}fox");
        let found = find_matches(&text, "quick brown fox");
        assert_eq!(spans(&found), vec![(0, text.len() as u32)]);
    }

    #[test]
    fn matches_do_not_overlap_and_an_empty_query_finds_nothing() {
        assert_eq!(
            spans(&find_matches(&chars("aaaaa"), "aa")),
            vec![(0, 2), (2, 2)]
        );
        assert!(find_matches(&chars("abc"), "").is_empty());
        assert!(find_matches(&chars("abc"), "   ").is_empty());
        assert!(find_matches(&chars(""), "a").is_empty());
        assert!(find_matches(&chars("abc"), "abcd").is_empty());
    }

    #[test]
    fn soft_hyphens_are_skipped_and_the_range_covers_them() {
        let text = chars("co\u{AD}operate");
        let found = find_matches(&text, "cooperate");
        assert_eq!(spans(&found), vec![(0, 10)]);
    }

    #[test]
    fn non_ascii_letters_fold_too() {
        let found = find_matches(&chars("Blåbær og ÆØÅ"), "æøå");
        assert_eq!(spans(&found), vec![(10, 3)]);
    }

    #[test]
    fn the_number_of_matches_is_limited() {
        let text = chars(&"a ".repeat(MAX_MATCHES_PER_PAGE + 50));
        assert_eq!(find_matches(&text, "a").len(), MAX_MATCHES_PER_PAGE);
    }

    #[test]
    fn titles_are_cleaned_and_cut() {
        assert_eq!(clean_title("  Chapter\u{0}1\n".into()), "Chapter 1");
        let long = "é".repeat(MAX_OUTLINE_TITLE);
        let cut = clean_title(long);
        assert!(cut.len() <= MAX_OUTLINE_TITLE);
        assert!(cut.chars().all(|c| c == 'é'));
    }
}
