//! Annotations: showing the ones a document has, and adding, changing and removing them.
//!
//! Every annotation is a real PDF annotation written through PDFium, with an appearance stream, so
//! that other readers draw and edit it. The window never says where markup goes: it names the
//! characters of the page text, and the engine finds their boxes. The window never sends a PDF
//! structure either; it sends numbers, colours and short text, which are checked here again.
//!
//! The text, the author and the box of an annotation that is already in the file are untrusted.
//! They are cut to the caps of the protocol, stripped of control characters, and sent as plain data.
//!
//! Annotations have ids for as long as the document is open. PDFium names an annotation by its
//! place in the page's list, which shifts when one is removed, so the engine keeps a list of ids
//! in step with each page's list (it is the only thing that changes the lists).

use std::collections::{HashMap, HashSet, VecDeque};
use std::os::raw::c_int;
use std::sync::Arc;

use partridge_protocol::{
    ability, AnnotInfo, AnnotKind, ErrorCode, SignatureMark, StampKind, MAX_ANNOTS_PER_PAGE,
    MAX_ANNOT_AUTHOR, MAX_ANNOT_TEXT, MAX_INK_POINTS, MAX_INK_STROKES, MAX_INK_WIDTH,
    MIN_INK_WIDTH,
};
use pdfium_render::prelude::*;

use crate::sigmark::{self, Seg};
use crate::text::{self, Mapper, TextChar};
use crate::{fail, Failure};

/// Most annotations of a page the engine looks at. The rest of a page that has more is ignored.
const MAX_SCANNED: usize = 5_000;

/// Most removed annotations the engine keeps so that they can be put back.
const MAX_REMEMBERED: usize = 1_000;

/// Most boxes (quadrilaterals) one markup is made of: one for each line of text it covers.
pub const MAX_MARKUP_BOXES: usize = 512;

/// Side of a sticky note's icon, in points.
const NOTE_SIDE: f32 = 20.0;

/// The print flag of an annotation (`/F`). PDFium writes no flags for a stamp, and a stamp without
/// this one is left out when another reader prints the page, so a signature picture sets it.
const FLAG_PRINT: i32 = 1 << 2;

/// The words a signature picture carries as its text.
pub const SIGNATURE_TEXT: &str = "Signature picture";

/// Most bytes of pictures of signatures the engine keeps to put them back after an undo, for one
/// document. A bigger sum is refused when a picture is added.
const MAX_MARK_BYTES: usize = 32 * 1024 * 1024;

/// Most bytes of pictures of signatures that may be made in PDFium over the life of one open
/// document, counting every time one is put on a page or put back after an undo. PDFium keeps what
/// it made for a picture until the document is closed, even after the picture is taken off, so
/// this is the bound on that memory (the bound above is on what is remembered). Past it the
/// document has to be saved and opened again.
const MAX_MARK_MADE_BYTES: usize = 256 * 1024 * 1024;

/// What making a picture is counted at, at least: a drawn or a typed one takes more of PDFium
/// than the bytes of its recipe.
const MARK_MADE_MIN: usize = 32 * 1024;

/// The flags of an annotation (`/F`) that lock it: read-only and locked. A third keeps its text.
const FLAG_READ_ONLY: i32 = 1 << 6;
const FLAG_LOCKED: i32 = 1 << 7;
const FLAG_LOCKED_CONTENTS: i32 = 1 << 9;

/// The number PDFium gives the popup subtype (`FPDF_ANNOT_POPUP`).
const SUBTYPE_POPUP: u32 = 16;
/// The numbers it gives the link and the widget subtypes (`FPDF_ANNOT_LINK`, `FPDF_ANNOT_WIDGET`).
const SUBTYPE_LINK: u32 = 2;
const SUBTYPE_WIDGET: u32 = 20;

/// Colour of a highlight, an underline or a strike-out that came without one.
const DEFAULT_COLOR: [u8; 4] = [255, 235, 59, 255];

/// How an annotation that the engine made itself is made again. Kept while the document is open,
/// so that an undo of a removal (and a redo of an addition) puts back exactly what was there.
#[derive(Clone, Debug)]
enum Recipe {
    Markup {
        kind: AnnotKind,
        color: [u8; 4],
        /// Four corners for each box, in page space: top left, top right, bottom left, bottom right.
        quads: Vec<[f32; 8]>,
        /// `[left, bottom, right, top]` in page space.
        bounds: [f32; 4],
    },
    Note {
        color: [u8; 4],
        bounds: [f32; 4],
    },
    Ink {
        color: [u8; 4],
        width: f32,
        /// Points in points from the top left of the page as it is drawn.
        strokes: Vec<Vec<[f32; 2]>>,
    },
    Stamp {
        stamp: StampKind,
        /// The top left corner on the page as it is drawn.
        x: f32,
        y: f32,
    },
    /// The picture of a signature.
    Signature {
        mark: Arc<SignatureMark>,
        /// The colour of a drawn or typed picture.
        color: [u8; 4],
        /// The box on the page as it is drawn: `[left, top, right, bottom]`.
        rect: [f32; 4],
        /// Width over height of the picture, which a move or a resize keeps.
        aspect: f32,
    },
}

/// The text of an annotation as the file has it.
#[derive(Clone, Debug)]
struct Original {
    /// The comment (`/Contents`).
    text: String,
    /// The rich text (`/RC`), when the annotation has any.
    rich: Option<String>,
    /// The date of the last change (`/M`), when the annotation has one.
    modified: Option<String>,
}

#[derive(Clone, Debug)]
struct Snapshot {
    recipe: Recipe,
    text: String,
    author: String,
}

/// What the engine knows about the annotations of one document. The page tools keep a copy of it
/// with each copy of the document they keep, so that taking a change of the pages back (or making
/// it again) puts back the ids, and what can be put back after a removal, that belong to that state.
#[derive(Default, Clone)]
pub struct DocAnnots {
    next_id: u32,
    /// The ids of the annotations of each page, in the order of the page's list.
    pages: HashMap<u32, Vec<u32>>,
    /// Annotations the engine made, by id. Only these can be put back after a removal: for an
    /// annotation that came with the file the engine would have to write a copy, and a copy
    /// loses what the engine does not read (rich text, dates, replies, layers).
    made: HashMap<u32, Recipe>,
    /// Pages whose annotations have been drawn once, so that each has an appearance stream.
    settled: HashSet<u32>,
    /// The comment and the rich text of an annotation as the file has them, kept before the first
    /// change of its text, so that a change that is taken back writes the file's own data again
    /// (line ends written as CR, formatting in the rich text).
    originals: HashMap<u32, Original>,
    /// The box (on the drawn page, not cut) an annotation had before it was first moved. A box a
    /// file puts partly outside its page may be moved back to where it was, never further out.
    reach: HashMap<u32, [f32; 4]>,
    /// Annotations that were removed, oldest first: `(page, id, what it was)`.
    removed: VecDeque<(u32, u32, Snapshot)>,
    /// Bytes of pictures of signatures made in PDFium since the document was opened (see
    /// [`MAX_MARK_MADE_BYTES`]).
    made_bytes: usize,
}

fn refused(_: PdfiumError) -> Failure {
    (
        ErrorCode::AnnotationRefused,
        "the annotation could not be written".to_string(),
    )
}

fn no_such() -> Failure {
    (
        ErrorCode::NoSuchAnnotation,
        "the annotation is not on the page any more".to_string(),
    )
}

fn pdf_color(c: [u8; 4]) -> PdfColor {
    PdfColor::new(c[0], c[1], c[2], c[3])
}

/// Text from a file made safe to hand on: line ends of every kind become a line feed, control
/// characters other than that are replaced by a space, and characters that change the direction of
/// the text around them or hide are dropped. The length is cut to `max` bytes on a character
/// boundary.
pub fn clean_text(text: &str, max: usize) -> String {
    let mut out = String::with_capacity(text.len().min(max));
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let c = match c {
            // A CR LF pair is one line end; a CR alone is one too.
            '\r' if chars.peek() == Some(&'\n') => continue,
            '\r' | '\n' | '\u{2028}' | '\u{2029}' => '\n',
            c if c.is_control() => ' ',
            // Characters that change the direction of the text around them, or hide: a note
            // must not be able to make the window show something other than what it holds.
            '\u{200E}'
            | '\u{200F}'
            | '\u{061C}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{200B}'
            | '\u{FEFF}' => continue,
            c => c,
        };
        if out.len() + c.len_utf8() > max {
            break;
        }
        out.push(c);
    }
    out
}

/// True when the text `clean_text` shows is the whole text of the file, apart from the line ends
/// (which are shown as line feeds). A text that was cut, or had characters taken out, cannot be
/// edited here: what the person saw is not what the file holds, and writing it back would lose
/// the rest.
fn text_is_whole(raw: &str) -> bool {
    let lines = raw.replace("\r\n", "\n").replace('\r', "\n");
    clean_text(raw, MAX_ANNOT_TEXT) == lines
}

fn kind_of(annotation: &PdfPageAnnotation) -> Option<AnnotKind> {
    Some(match annotation.annotation_type() {
        PdfPageAnnotationType::Highlight => AnnotKind::Highlight,
        PdfPageAnnotationType::Underline => AnnotKind::Underline,
        PdfPageAnnotationType::Strikeout => AnnotKind::Strikeout,
        PdfPageAnnotationType::Text => AnnotKind::Note,
        PdfPageAnnotationType::Ink => AnnotKind::Ink,
        PdfPageAnnotationType::Stamp => AnnotKind::Stamp,
        PdfPageAnnotationType::Squiggly
        | PdfPageAnnotationType::FreeText
        | PdfPageAnnotationType::Square
        | PdfPageAnnotationType::Circle => AnnotKind::Other,
        _ => return None,
    })
}

/// The corners of the boxes of a markup annotation, in page space, if the file has them.
fn quads_of(annotation: &PdfPageAnnotation) -> Vec<[f32; 8]> {
    let mut out = Vec::new();
    let points = annotation.attachment_points();
    for index in 0..points.len().min(MAX_MARKUP_BOXES) {
        if let Ok(q) = points.get(index) {
            let v = [
                q.x1().value,
                q.y1().value,
                q.x2().value,
                q.y2().value,
                q.x3().value,
                q.y3().value,
                q.x4().value,
                q.y4().value,
            ];
            if v.iter().all(|n| n.is_finite()) {
                out.push(v);
            }
        }
    }
    out
}

/// Most objects of an appearance that are looked at to find its colour, and the deepest level of
/// forms inside forms that is followed.
const COLOR_OBJECTS: usize = 64;
const COLOR_DEPTH: usize = 4;

/// The colour of the first painted path in `object`, or in the form it is (an appearance often
/// draws through a form inside the form). A path paints with its fill, its stroke or both; the
/// side the kind of annotation normally uses is read when it is painted (the fill of a highlight,
/// the stroke of the other kinds), otherwise the other side. A path that paints neither is
/// passed over: PDFium answers black for a colour that was never set.
fn path_color(
    object: &PdfPageObject,
    kind: AnnotKind,
    depth: usize,
    budget: &mut usize,
) -> Option<[u8; 4]> {
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    if let Some(path) = object.as_path_object() {
        let filled = path
            .fill_mode()
            .is_ok_and(|mode| mode != PdfPathFillMode::None);
        let stroked = path.is_stroked().unwrap_or(false);
        let fill = || path.fill_color().ok();
        let stroke = || path.stroke_color().ok();
        let color = match (kind, filled, stroked) {
            (_, false, false) => None,
            (AnnotKind::Highlight, true, _) | (_, true, false) => fill(),
            _ => stroke(),
        };
        return color.map(|c| [c.red(), c.green(), c.blue(), 255]);
    }
    if depth < COLOR_DEPTH {
        if let Some(form) = object.as_x_object_form_object() {
            for index in 0..form.len().min(16) {
                if let Ok(inner) = form.get(index) {
                    if let Some(color) = path_color(&inner, kind, depth + 1, budget) {
                        return Some(color);
                    }
                }
            }
        }
    }
    None
}

/// The colour an annotation is drawn in, read from its appearance, or `None` when the appearance
/// has no path to read it from. The colour entry of the annotation itself is not used: PDFium
/// refuses to read it once an appearance stream exists, and the call that stands in for it in
/// `pdfium-render` would then read a page object where there is an annotation. The fill of a
/// highlight and the stroke of an underline or a strike-out come from the first path of the
/// appearance, also when it is drawn through forms.
fn color_of(annotation: &PdfPageAnnotation, kind: AnnotKind) -> Option<[u8; 4]> {
    let mut budget = COLOR_OBJECTS;
    for object in annotation.objects().iter().take(16) {
        if let Some(color) = path_color(&object, kind, 0, &mut budget) {
            return Some(color);
        }
    }
    None
}

fn bounds_of(annotation: &PdfPageAnnotation) -> Option<[f32; 4]> {
    let b = annotation.bounds().ok()?;
    let v = [
        b.left().value,
        b.bottom().value,
        b.right().value,
        b.top().value,
    ];
    v.iter().all(|n| n.is_finite()).then_some(v)
}

fn rect_of(bounds: [f32; 4]) -> PdfRect {
    PdfRect::new_from_values(bounds[1], bounds[0], bounds[3], bounds[2])
}

/// The page opened for changes: the page's own content is never rewritten, because annotations
/// live beside the content and the content of a page that is not ours to change stays as it is.
fn load(document: &PdfDocument<'static>, index: u32) -> Result<PdfPage<'static>, Failure> {
    let pages = document.pages();
    if index as usize >= pages.len() as usize {
        return fail(ErrorCode::PageOutOfRange, "the page does not exist");
    }
    let mut page = pages
        .get(index as PdfPageIndex)
        .map_err(|_| (ErrorCode::Corrupt, "the page could not be read".to_string()))?;
    page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
    Ok(page)
}

/// Draws the page into a tiny bitmap. PDFium makes the appearance stream of an annotation that
/// has none when it draws the page, and keeps it in the document; drawing here, right after a
/// change, means that the stream is there when the document is saved, whatever the window does next.
fn settle(page: &PdfPage) {
    let _ = page.render_with_config(&PdfRenderConfig::new().set_target_size(2, 2));
}

/// Draws a page once into a tiny bitmap so that PDFium makes the appearance streams of the
/// annotations and form fields that have none.
pub(crate) fn settle_page(document: &PdfDocument<'static>, index: u32) {
    if let Ok(page) = load(document, index) {
        settle(&page);
    }
}

impl DocAnnots {
    /// The list of annotations of some pages was changed by the form designer: widgets were added,
    /// taken away or put in another order. `maps` has, for each such page and for each entry of its
    /// new list, the place the entry had in the old list, or `None` for a new one. The ids of the
    /// entries that were there go with them; new entries get new ids. No page counts as drawn any
    /// more, because the document was loaded again.
    pub fn reconcile(&mut self, maps: &std::collections::BTreeMap<usize, Vec<Option<usize>>>) {
        for (page, map) in maps {
            let page = *page as u32;
            let old = self.pages.get(&page).cloned().unwrap_or_default();
            let mut fresh = Vec::with_capacity(map.len());
            for place in map {
                let id = match place.and_then(|p| old.get(p)) {
                    Some(id) => *id,
                    None => self.fresh_id(),
                };
                fresh.push(id);
            }
            self.pages.insert(page, fresh);
        }
        self.settled.clear();
    }

    /// The bytes of signature pictures made in PDFium so far (see [`MAX_MARK_MADE_BYTES`]).
    pub fn made_bytes(&self) -> usize {
        self.made_bytes
    }

    /// Sets that count, for a record that is made again after a change to the pages.
    pub fn set_made_bytes(&mut self, bytes: usize) {
        self.made_bytes = bytes;
    }

    /// The ids of the annotations of a page, brought in step with the page's list.
    fn ids(&mut self, page: u32, count: usize) -> &mut Vec<u32> {
        let next = &mut self.next_id;
        let ids = self.pages.entry(page).or_default();
        ids.truncate(count);
        while ids.len() < count {
            *next += 1;
            ids.push(*next);
        }
        ids
    }

    fn fresh_id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    fn remember(&mut self, page: u32, id: u32, snapshot: Snapshot) {
        self.removed.retain(|(_, old, _)| *old != id);
        self.removed.push_back((page, id, snapshot));
        while self.removed.len() > MAX_REMEMBERED {
            self.removed.pop_front();
        }
    }

    fn describe(
        &self,
        annotation: &PdfPageAnnotation,
        id: u32,
        page: &PdfPage,
        mapper: &Mapper,
        flags: i32,
        allowed: bool,
    ) -> Option<AnnotInfo> {
        let kind = kind_of(annotation)?;
        // A picture of a signature that was placed in this session is a stamp to the file, and a
        // signature to the window: it can be moved and resized.
        let kind = match (kind, self.made.get(&id)) {
            (AnnotKind::Stamp, Some(Recipe::Signature { .. })) => AnnotKind::Signature,
            (kind, _) => kind,
        };
        if annotation.is_hidden() {
            return None;
        }
        let made = self.made.contains_key(&id);
        let restore = if made { ability::RESTORE } else { 0 };
        let has_quads = !annotation.attachment_points().is_empty();
        let raw = annotation.contents().unwrap_or_default();
        let found = color_of(annotation, kind);
        let mut abilities = match kind {
            AnnotKind::Highlight | AnnotKind::Underline | AnnotKind::Strikeout => {
                let mut a = ability::EDIT_TEXT | ability::REMOVE | restore;
                // A colour is changed by making a plain picture in the new colour, and taken back
                // by making one in the old colour. That is only possible when the old colour is
                // known: an appearance that has no path to read it from is left as it is.
                if has_quads && found.is_some() {
                    a |= ability::RECOLOUR;
                }
                a
            }
            // PDFium draws every note the same, whatever its colour, so a note has no colour to pick.
            AnnotKind::Note => ability::EDIT_TEXT | ability::MOVE | ability::REMOVE | restore,
            AnnotKind::Ink => ability::EDIT_TEXT | ability::REMOVE | restore,
            AnnotKind::Stamp => ability::REMOVE | restore,
            AnnotKind::Signature => ability::MOVE | ability::RESIZE | ability::REMOVE | restore,
            AnnotKind::Other => ability::REMOVE,
        };
        // What the file or the document forbids comes first. Only an annotation the engine made
        // can be put back, and the text of one is changed only when it is shown whole.
        if !allowed || flags & (FLAG_READ_ONLY | FLAG_LOCKED) != 0 {
            abilities = ability::LOCKED;
        } else if flags & FLAG_LOCKED_CONTENTS != 0 || !text_is_whole(&raw) {
            abilities &= !ability::EDIT_TEXT;
        }
        let bounds = annotation.bounds().ok()?;
        // A box is cut to its page: the window never gets a box far outside it.
        let (width, height) = (page.width().value, page.height().value);
        let rect = mapper.rect(page, &bounds)?;
        let cut = |v: f32, limit: f32| {
            if v.is_finite() {
                v.clamp(0.0, limit)
            } else {
                0.0
            }
        };
        let rect = [
            cut(rect[0], width),
            cut(rect[1], height),
            cut(rect[2], width),
            cut(rect[3], height),
        ];
        let color = found.unwrap_or(DEFAULT_COLOR);
        Some(AnnotInfo {
            id,
            kind,
            abilities,
            rect,
            color,
            text: clean_text(&raw, MAX_ANNOT_TEXT),
            author: clean_text(&annotation.creator().unwrap_or_default(), MAX_ANNOT_AUTHOR),
        })
    }

    /// The annotations of a page.
    pub fn list(
        &mut self,
        document: &mut PdfDocument<'static>,
        page_index: u32,
    ) -> Result<Vec<AnnotInfo>, Failure> {
        let page = load(document, page_index)?;
        // An annotation that came without an appearance gets one when the page is drawn; the colour
        // of a highlight is read from it. Once is enough: every change draws the page itself.
        if self.settled.insert(page_index) {
            settle(&page);
        }
        let mapper = Mapper::new(&page);
        let count = page.annotations().len();
        let ids = self.ids(page_index, count).clone();
        let allowed = changes_allowed(document);
        let flags = flags_of(document, page_index, count);
        let mut out = Vec::new();
        #[allow(clippy::needless_range_loop)]
        for index in 0..count.min(MAX_SCANNED) {
            if out.len() >= MAX_ANNOTS_PER_PAGE {
                break;
            }
            let Ok(annotation) = page.annotations().get(index) else {
                continue;
            };
            let flag = flags.get(index).copied().unwrap_or(FLAG_LOCKED);
            if let Some(info) =
                self.describe(&annotation, ids[index], &page, &mapper, flag, allowed)
            {
                out.push(info);
            }
        }
        Ok(out)
    }

    /// The annotation with `id`: its place in the list of the page, and what it is.
    fn find(
        &mut self,
        document: &PdfDocument<'static>,
        page: &PdfPage,
        page_index: u32,
        id: u32,
    ) -> Result<(usize, AnnotInfo), Failure> {
        let mapper = Mapper::new(page);
        let count = page.annotations().len();
        let position = self
            .ids(page_index, count)
            .iter()
            .position(|i| *i == id)
            .ok_or_else(no_such)?;
        let annotation = page.annotations().get(position).map_err(|_| no_such())?;
        let flags = flags_at(document, page_index, position);
        let allowed = changes_allowed(document);
        let info = self
            .describe(&annotation, id, page, &mapper, flags, allowed)
            .ok_or_else(no_such)?;
        Ok((position, info))
    }

    /// The answer to an addition: the annotation that was added last, under the id it is given.
    fn finish_added(
        &mut self,
        page: &PdfPage,
        page_index: u32,
        new_id: u32,
    ) -> Result<AnnotInfo, Failure> {
        settle(page);
        let count = page.annotations().len();
        if count == 0 {
            return Err(no_such());
        }
        // The list had `count - 1` entries before the change (it was in step, or nearly): it is
        // brought to that length first, and then the new entry is added.
        let ids = self.ids(page_index, count - 1);
        ids.push(new_id);
        let mapper = Mapper::new(page);
        let annotation = page.annotations().get(count - 1).map_err(refused)?;
        self.describe(&annotation, new_id, page, &mapper, 0, true)
            .ok_or_else(|| {
                (
                    ErrorCode::AnnotationRefused,
                    "the annotation could not be read back".to_string(),
                )
            })
    }

    /// Removes the annotations that a failed change left behind: whatever is past `count`.
    fn roll_back(&mut self, page: &mut PdfPage<'static>, count: usize) {
        while page.annotations().len() > count {
            let last = page.annotations().len() - 1;
            let Ok(annotation) = page.annotations_mut().get(last) else {
                break;
            };
            if page
                .annotations_mut()
                .delete_annotation(annotation)
                .is_err()
            {
                break;
            }
        }
    }

    // ---- markup ------------------------------------------------------------------

    /// Marks the characters `start..end` of the page text.
    pub fn add_markup(
        &mut self,
        document: &mut PdfDocument<'static>,
        page_index: u32,
        kind: AnnotKind,
        color: [u8; 4],
        start: u32,
        end: u32,
    ) -> Result<AnnotInfo, Failure> {
        ensure_allowed(document)?;
        let mut page = load(document, page_index)?;
        let chars = text::page_chars(&page, true).map_err(|_| {
            (
                ErrorCode::Corrupt,
                "the text of the page could not be read".to_string(),
            )
        })?;
        let start = start as usize;
        let end = (end as usize).min(chars.len());
        if start >= end {
            return fail(ErrorCode::InvalidRequest, "there is no text in that range");
        }
        let mapper = Mapper::new(&page);
        let mut boxes = line_boxes(&chars[start..end]);
        boxes.truncate(MAX_MARKUP_BOXES);
        let (quads, bounds) = quads_for(&page, &mapper, &boxes);
        if quads.is_empty() {
            return fail(
                ErrorCode::AnnotationRefused,
                "the text in that range has no place on the page",
            );
        }
        let recipe = Recipe::Markup {
            kind,
            color,
            quads,
            bounds,
        };
        self.create_from(
            &mut page,
            document,
            page_index,
            &Snapshot {
                recipe,
                text: String::new(),
                author: String::new(),
            },
            None,
            true,
        )
    }

    // ---- notes -------------------------------------------------------------------

    pub fn add_note(
        &mut self,
        document: &mut PdfDocument<'static>,
        page_index: u32,
        x: f32,
        y: f32,
        color: [u8; 4],
        text: &str,
    ) -> Result<AnnotInfo, Failure> {
        ensure_allowed(document)?;
        let mut page = load(document, page_index)?;
        let mapper = Mapper::new(&page);
        let (width, height) = (page.width().value, page.height().value);
        let x = x.clamp(0.0, (width - NOTE_SIDE).max(0.0));
        let y = y.clamp(0.0, (height - NOTE_SIDE).max(0.0));
        let corners = [
            mapper.to_page(&page, x, y),
            mapper.to_page(&page, x + NOTE_SIDE, y + NOTE_SIDE),
        ];
        let (Some(a), Some(b)) = (corners[0], corners[1]) else {
            return fail(
                ErrorCode::AnnotationRefused,
                "that place is not on the page",
            );
        };
        let bounds = [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)];
        let snapshot = Snapshot {
            recipe: Recipe::Note { color, bounds },
            text: clean_text(text, MAX_ANNOT_TEXT),
            author: String::new(),
        };
        self.create_from(&mut page, document, page_index, &snapshot, None, true)
    }

    // ---- drawings ----------------------------------------------------------------

    pub fn add_ink(
        &mut self,
        document: &mut PdfDocument<'static>,
        page_index: u32,
        color: [u8; 4],
        width: f32,
        strokes: Vec<Vec<[f32; 2]>>,
    ) -> Result<AnnotInfo, Failure> {
        ensure_allowed(document)?;
        let mut page = load(document, page_index)?;
        let (w, h) = (page.width().value, page.height().value);
        // Points outside the page are pulled to its edge: a stroke that leaves the page still
        // ends at the border, and nothing outside the page is written.
        let mut total = 0usize;
        let mut kept: Vec<Vec<[f32; 2]>> = Vec::new();
        for stroke in strokes.into_iter().take(MAX_INK_STROKES) {
            if stroke.is_empty() || total + stroke.len() > MAX_INK_POINTS {
                break;
            }
            total += stroke.len();
            kept.push(
                stroke
                    .into_iter()
                    .map(|p| [p[0].clamp(0.0, w), p[1].clamp(0.0, h)])
                    .collect(),
            );
        }
        if kept.is_empty() {
            return fail(ErrorCode::InvalidRequest, "there is nothing to draw");
        }
        let recipe = Recipe::Ink {
            color,
            width: width.clamp(MIN_INK_WIDTH, MAX_INK_WIDTH),
            strokes: kept,
        };
        let snapshot = Snapshot {
            recipe,
            text: String::new(),
            author: String::new(),
        };
        self.create_from(&mut page, document, page_index, &snapshot, None, true)
    }

    // ---- stamps ------------------------------------------------------------------

    pub fn add_stamp(
        &mut self,
        document: &mut PdfDocument<'static>,
        page_index: u32,
        x: f32,
        y: f32,
        stamp: StampKind,
    ) -> Result<AnnotInfo, Failure> {
        ensure_allowed(document)?;
        let mut page = load(document, page_index)?;
        let (w, h) = (page.width().value, page.height().value);
        let (sw, sh) = stamp_size(stamp);
        let recipe = Recipe::Stamp {
            stamp,
            x: x.clamp(0.0, (w - sw).max(0.0)),
            y: y.clamp(0.0, (h - sh).max(0.0)),
        };
        let snapshot = Snapshot {
            recipe,
            text: stamp.label().to_string(),
            author: String::new(),
        };
        self.create_from(&mut page, document, page_index, &snapshot, None, true)
    }

    // ---- pictures of signatures --------------------------------------------------

    /// The bytes of the pictures of signatures that are kept for an undo: those on the pages and
    /// those taken off.
    fn mark_bytes(&self) -> usize {
        let weight = |r: &Recipe| match r {
            Recipe::Signature { mark, .. } => mark.weight(),
            _ => 0,
        };
        self.made.values().map(weight).sum::<usize>()
            + self
                .removed
                .iter()
                .filter(|(_, id, _)| !self.made.contains_key(id))
                .map(|(_, _, s)| weight(&s.recipe))
                .sum::<usize>()
    }

    /// Puts the picture of a signature on a page as a stamp. The picture is fitted whole into
    /// `rect` (`[left, top, right, bottom]` in points on the page as it is drawn). The stamp has
    /// the print flag, so that other readers print it too. It is a picture and nothing else: no
    /// signature field, no certificate, nothing that says who placed it.
    pub fn add_signature(
        &mut self,
        document: &mut PdfDocument<'static>,
        page_index: u32,
        rect: [f32; 4],
        color: [u8; 4],
        mark: SignatureMark,
    ) -> Result<AnnotInfo, Failure> {
        ensure_allowed(document)?;
        if !mark.within_caps() {
            return fail(
                ErrorCode::InvalidRequest,
                "that picture is outside the limits",
            );
        }
        let aspect = sigmark::aspect_of(&mark).map_err(|m| (ErrorCode::InvalidRequest, m))?;
        if self.mark_bytes() + mark.weight() > MAX_MARK_BYTES {
            return fail(
                ErrorCode::AnnotationRefused,
                "this document holds too many pictures of signatures",
            );
        }
        let mut page = load(document, page_index)?;
        let size = (page.width().value, page.height().value);
        let Some(placed) = sigmark::fit_box(aspect, rect, size) else {
            return fail(
                ErrorCode::InvalidRequest,
                "the picture does not fit in that box",
            );
        };
        let snapshot = Snapshot {
            recipe: Recipe::Signature {
                mark: Arc::new(mark),
                color,
                rect: placed,
                aspect,
            },
            text: SIGNATURE_TEXT.to_string(),
            author: String::new(),
        };
        self.create_from(&mut page, document, page_index, &snapshot, None, true)
    }

    /// Moves and resizes a picture of a signature that was placed in this session: it takes the
    /// top left corner and the width of `rect` and keeps its proportions.
    pub fn place_signature(
        &mut self,
        document: &mut PdfDocument<'static>,
        page_index: u32,
        id: u32,
        rect: [f32; 4],
    ) -> Result<AnnotInfo, Failure> {
        ensure_allowed(document)?;
        let page = load(document, page_index)?;
        let (position, info) = self.find(document, &page, page_index, id)?;
        let aspect = match self.made.get(&id) {
            Some(Recipe::Signature { aspect, .. }) if info.abilities & ability::RESIZE != 0 => {
                *aspect
            }
            _ => {
                return fail(
                    ErrorCode::AnnotationRefused,
                    "this annotation does not allow that change",
                )
            }
        };
        let size = (page.width().value, page.height().value);
        let Some(placed) = sigmark::place_box(aspect, rect, size) else {
            return fail(
                ErrorCode::AnnotationRefused,
                "the picture does not fit in that box",
            );
        };
        let mapper = Mapper::new(&page);
        let bounds = page_bounds(&page, &mapper, placed).ok_or_else(|| {
            (
                ErrorCode::AnnotationRefused,
                "that place is not on the page".to_string(),
            )
        })?;
        {
            let mut annotation = page.annotations().get(position).map_err(|_| no_such())?;
            annotation.set_bounds(rect_of(bounds)).map_err(refused)?;
        }
        settle(&page);
        let (_, info) = self.find(document, &page, page_index, id)?;
        if let Some(Recipe::Signature { rect, .. }) = self.made.get_mut(&id) {
            *rect = info.rect;
        }
        Ok(info)
    }

    /// Makes the annotation a snapshot describes, as the newest of the page. `id` is the id it
    /// is to have (when it is put back); without one it gets a new id. `remember` keeps its recipe
    /// for a later removal and restoring.
    fn create_from(
        &mut self,
        page: &mut PdfPage<'static>,
        document: &mut PdfDocument<'static>,
        page_index: u32,
        snapshot: &Snapshot,
        id: Option<u32>,
        remember: bool,
    ) -> Result<AnnotInfo, Failure> {
        let before = page.annotations().len();
        if let Recipe::Signature { mark, .. } = &snapshot.recipe {
            // Each picture that is made stays in PDFium's memory until the document is closed, also
            // when it is taken off again, so a document that is put through undo and redo of a big
            // picture many times is stopped before the engine runs out of memory.
            let cost = mark.weight().max(MARK_MADE_MIN);
            if self.made_bytes.saturating_add(cost) > MAX_MARK_MADE_BYTES {
                return fail(
                    ErrorCode::AnnotationRefused,
                    "pictures of signatures were put on and taken off too often in this document; save it and open it again to go on",
                );
            }
            self.made_bytes += cost;
        }
        // Brings the list of ids in step before something is added to it.
        self.ids(page_index, before);
        let new_id = match id {
            Some(id) => id,
            None => self.fresh_id(),
        };
        // A drawing or a stamp can be put back only if the engine knows how it was made, and the
        // answer says so, so the recipe is kept before the answer is made.
        let keeps = remember;
        if keeps {
            self.made.insert(new_id, snapshot.recipe.clone());
        }
        let result = write_annotation(page, document, page_index, snapshot)
            .and_then(|()| self.finish_added(page, page_index, new_id));
        if result.is_err() {
            self.roll_back(page, before);
            self.ids(page_index, before);
            if keeps && id.is_none() {
                self.made.remove(&new_id);
            }
        }
        result
    }

    // ---- changing ----------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        document: &mut PdfDocument<'static>,
        page_index: u32,
        id: u32,
        text: Option<&str>,
        color: Option<[u8; 4]>,
        dx: f32,
        dy: f32,
    ) -> Result<AnnotInfo, Failure> {
        ensure_allowed(document)?;
        let page = load(document, page_index)?;
        let (position, info) = self.find(document, &page, page_index, id)?;
        let wants_move = dx != 0.0 || dy != 0.0;
        if (text.is_some() && info.abilities & ability::EDIT_TEXT == 0)
            || (color.is_some() && info.abilities & ability::RECOLOUR == 0)
            || (wants_move && info.abilities & ability::MOVE == 0)
        {
            return fail(
                ErrorCode::AnnotationRefused,
                "this annotation does not allow that change",
            );
        }
        let mapper = Mapper::new(&page);
        // What the file holds of the text is kept before the first change of it.
        if text.is_some() && !self.originals.contains_key(&id) {
            let original = read_original(document, page_index, position)?;
            self.originals.insert(id, original);
        }
        // The text and the date that go with it, and the rich text to write next to the new text.
        let mut written: Option<(Option<String>, Option<String>)> = None;
        {
            let mut annotation = page.annotations().get(position).map_err(|_| no_such())?;
            if let Some(text) = text {
                // The text goes into the one key the person edits. When a change is taken back to
                // the text that was shown, the text of the file is written back as it was (line
                // ends written as CR, formatting in the rich text, the date of the last change).
                let wanted = clean_text(text, MAX_ANNOT_TEXT);
                let back = self
                    .originals
                    .get(&id)
                    .filter(|o| clean_text(&o.text, MAX_ANNOT_TEXT) == wanted)
                    .cloned();
                let value = back.as_ref().map_or(wanted, |o| o.text.clone());
                annotation.set_contents(&value).map_err(refused)?;
                written = Some(match back {
                    Some(o) => (o.rich, o.modified),
                    // A comment with rich text (`/RC`) is kept in step with its plain text.
                    None => (Some(rich_text(&value)), None),
                });
            }
            if wants_move {
                let from = mapper.to_page(&page, 0.0, 0.0);
                // The box stays on the page: a note cannot be pushed out of reach. A box that a
                // file put partly outside its page is measured as it is (the list shows it cut),
                // and may go back to where it was, so that a move can always be taken back.
                let (width, height) = (page.width().value, page.height().value);
                let shown = annotation
                    .bounds()
                    .ok()
                    .and_then(|b| mapper.rect(&page, &b))
                    .filter(|r| r.iter().all(|n| n.is_finite()));
                let Some(shown) = shown else {
                    return fail(
                        ErrorCode::AnnotationRefused,
                        "that place is not on the page",
                    );
                };
                let reach = *self.reach.entry(id).or_insert(shown);
                let dx = keep_within(
                    dx,
                    reach[0].min(0.0) - shown[0],
                    reach[2].max(width) - shown[2],
                );
                let dy = keep_within(
                    dy,
                    reach[1].min(0.0) - shown[1],
                    reach[3].max(height) - shown[3],
                );
                let to = mapper.to_page(&page, dx, dy);
                let (Some(from), Some(to), Some(bounds)) = (from, to, bounds_of(&annotation))
                else {
                    return fail(
                        ErrorCode::AnnotationRefused,
                        "that place is not on the page",
                    );
                };
                let moved = [
                    bounds[0] + to.0 - from.0,
                    bounds[1] + to.1 - from.1,
                    bounds[2] + to.0 - from.0,
                    bounds[3] + to.1 - from.1,
                ];
                annotation.set_bounds(rect_of(moved)).map_err(refused)?;
            }
        }
        if let Some((rich, modified)) = written {
            write_text_keys(
                document,
                page_index,
                position,
                rich.as_deref(),
                modified.as_deref(),
            )?;
        }
        if let Some(color) = color {
            // PDFium cannot change the colour of an annotation that has its appearance stream. The
            // appearance is taken off, the colour is set on the same annotation, and PDFium makes
            // the appearance again when the page is drawn: every other entry stays as it was.
            recolour_raw(document, page_index, position, color)?;
        }
        settle(&page);
        let (_, info) = self.find(document, &page, page_index, id)?;
        // A picture of a signature that is put back after a removal is put where it was moved to.
        if wants_move {
            if let Some(Recipe::Signature { rect, .. }) = self.made.get_mut(&id) {
                *rect = info.rect;
            }
        }
        Ok(info)
    }

    /// What it takes to make an annotation again, read from the annotation and from what the
    /// engine knows of it. `None` for one that cannot be made again.
    fn snapshot_of(
        &self,
        page: &PdfPage,
        position: usize,
        info: &AnnotInfo,
    ) -> Result<(Option<Snapshot>, Option<[f32; 4]>), Failure> {
        let annotation = page.annotations().get(position).map_err(|_| no_such())?;
        let text = clean_text(&annotation.contents().unwrap_or_default(), MAX_ANNOT_TEXT);
        let author = clean_text(&annotation.creator().unwrap_or_default(), MAX_ANNOT_AUTHOR);
        let bounds = bounds_of(&annotation);
        let recipe = match info.kind {
            AnnotKind::Highlight | AnnotKind::Underline | AnnotKind::Strikeout => {
                let quads = quads_of(&annotation);
                match (quads.is_empty(), bounds) {
                    (false, Some(bounds)) => Some(Recipe::Markup {
                        kind: info.kind,
                        color: info.color,
                        quads,
                        bounds,
                    }),
                    _ => None,
                }
            }
            AnnotKind::Note => bounds.map(|bounds| Recipe::Note {
                color: info.color,
                bounds,
            }),
            AnnotKind::Ink | AnnotKind::Stamp | AnnotKind::Signature => {
                self.made.get(&info.id).cloned()
            }
            AnnotKind::Other => None,
        };
        Ok((
            recipe.map(|recipe| Snapshot {
                recipe,
                text,
                author,
            }),
            bounds,
        ))
    }

    pub fn remove(
        &mut self,
        document: &mut PdfDocument<'static>,
        page_index: u32,
        id: u32,
    ) -> Result<(), Failure> {
        ensure_allowed(document)?;
        let page = load(document, page_index)?;
        let (position, info) = self.find(document, &page, page_index, id)?;
        if info.abilities & ability::REMOVE == 0 {
            return fail(
                ErrorCode::AnnotationRefused,
                "this annotation does not allow that change",
            );
        }
        // Only what the engine made is kept for a later restore: a copy of an annotation from the
        // file would lose what the engine does not read.
        let snapshot = if info.abilities & ability::RESTORE != 0 {
            self.snapshot_of(&page, position, &info)?.0
        } else {
            None
        };
        let before = page.annotations().len();
        // PDFium takes the annotation off the page but leaves its dictionary in the document. A
        // save leaves out the dictionary of a loaded file that nothing refers to, but writes one
        // made in this session as an object nobody refers to. What the person wrote is
        // overwritten first, with the rich text, the subject, the appearance and the popup of the
        // annotation, so that nothing readable is left in the second kind.
        let gone = remove_raw(document, page_index, position)?;
        let ids = self.ids(page_index, before);
        for place in gone.iter().rev() {
            if *place < ids.len() {
                ids.remove(*place);
            }
        }
        self.originals.remove(&id);
        self.reach.remove(&id);
        if let Some(snapshot) = snapshot {
            self.remember(page_index, id, snapshot);
        } else {
            self.made.remove(&id);
        }
        Ok(())
    }

    /// Puts back an annotation that was removed, under the id it had.
    pub fn restore(
        &mut self,
        document: &mut PdfDocument<'static>,
        page_index: u32,
        id: u32,
    ) -> Result<AnnotInfo, Failure> {
        ensure_allowed(document)?;
        let Some(at) = self
            .removed
            .iter()
            .position(|(p, i, _)| *p == page_index && *i == id)
        else {
            return fail(
                ErrorCode::AnnotationRefused,
                "the annotation cannot be put back any more",
            );
        };
        let (_, _, snapshot) = self.removed[at].clone();
        let mut page = load(document, page_index)?;
        let info = self.create_from(&mut page, document, page_index, &snapshot, Some(id), true)?;
        self.removed.remove(at);
        Ok(info)
    }

    /// Forgets what is kept about the document's annotations. Called when it is closed.
    pub fn clear(&mut self) {
        self.pages.clear();
        self.made.clear();
        self.removed.clear();
        self.settled.clear();
        self.originals.clear();
        self.reach.clear();
        self.made_bytes = 0;
    }
}

// ---- what the file and PDFium's own calls are asked ----------------------------------------------

/// `delta` held between `low` and `high` (nothing moves when there is no room).
fn keep_within(delta: f32, low: f32, high: f32) -> f32 {
    if low > high || !delta.is_finite() {
        0.0
    } else {
        delta.clamp(low, high)
    }
}

/// True when the document allows annotations to be added and changed: it is not protected, or
/// the bit for annotating is set in the permissions it grants the user.
fn changes_allowed(document: &PdfDocument<'_>) -> bool {
    if let Ok(PdfSecurityHandlerRevision::Unprotected) =
        document.permissions().security_handler_revision()
    {
        return true;
    }
    let Some(handle) = raw_document(document) else {
        return false;
    };
    // SAFETY: the handle is the live document's. The call that answers for the owner is used, so
    // that a document opened with its owner password allows what the owner may do.
    let bits = unsafe { document.bindings().FPDF_GetDocPermissions(handle) };
    bits & 0x20 != 0
}

fn ensure_allowed(document: &PdfDocument<'_>) -> Result<(), Failure> {
    if changes_allowed(document) {
        Ok(())
    } else {
        fail(
            ErrorCode::AnnotationsNotAllowed,
            "the document does not allow annotations to be changed",
        )
    }
}

/// Runs `work` with PDFium's own handle of a page, closed again after it. `pdfium-render` keeps
/// the handles of annotations to itself, and PDFium's functions are the only way to some of what
/// an annotation holds. They are called only with handles that PDFium gave out here.
fn with_raw_page<R>(
    document: &PdfDocument<'static>,
    page_index: u32,
    work: impl FnOnce(&dyn PdfiumLibraryBindings, FPDF_PAGE) -> R,
) -> Result<R, Failure> {
    let Some(handle) = raw_document(document) else {
        return fail(
            ErrorCode::AnnotationRefused,
            "the document cannot be changed",
        );
    };
    let bindings = document.bindings();
    // SAFETY: `handle` is the live document's handle; the page is opened and closed here.
    unsafe {
        let page = bindings.FPDF_LoadPage(handle, page_index as c_int);
        if page.is_null() {
            return fail(ErrorCode::Corrupt, "the page could not be read");
        }
        let out = work(bindings, page);
        bindings.FPDF_ClosePage(page);
        Ok(out)
    }
}

/// Runs `work` with the handle of the annotation at `position`, closed again after it.
///
/// # Safety
/// `page` must be a live page handle of `bindings`.
unsafe fn with_annot<R>(
    bindings: &dyn PdfiumLibraryBindings,
    page: FPDF_PAGE,
    position: usize,
    work: impl FnOnce(FPDF_ANNOTATION) -> R,
) -> Option<R> {
    let annotation = bindings.FPDFPage_GetAnnot(page, position as c_int);
    if annotation.is_null() {
        return None;
    }
    let out = work(annotation);
    bindings.FPDFPage_CloseAnnot(annotation);
    Some(out)
}

/// The flags (`/F`) of the first `count` annotations of a page. When they cannot be read, every
/// annotation counts as locked.
fn flags_of(document: &PdfDocument<'static>, page_index: u32, count: usize) -> Vec<i32> {
    let n = count.min(MAX_SCANNED);
    if n == 0 {
        return Vec::new();
    }
    with_raw_page(document, page_index, |bindings, page| {
        (0..n)
            .map(|i| {
                // SAFETY: `page` is live.
                unsafe { with_annot(bindings, page, i, |a| bindings.FPDFAnnot_GetFlags(a)) }
                    .unwrap_or(FLAG_LOCKED)
            })
            .collect()
    })
    .unwrap_or_else(|_| vec![FLAG_LOCKED; n])
}

fn flags_at(document: &PdfDocument<'static>, page_index: u32, position: usize) -> i32 {
    with_raw_page(document, page_index, |bindings, page| {
        // SAFETY: `page` is live.
        unsafe { with_annot(bindings, page, position, |a| bindings.FPDFAnnot_GetFlags(a)) }
    })
    .ok()
    .flatten()
    .unwrap_or(FLAG_LOCKED)
}

/// Changes the colour of the annotation at `position` where it is: the appearance stream is taken
/// off (PDFium refuses a colour while there is one), the colour is set, and PDFium makes a new
/// appearance when the page is drawn. The opacity (`/CA`) the annotation has is kept, and so is
/// every other entry of its dictionary.
fn recolour_raw(
    document: &PdfDocument<'static>,
    page_index: u32,
    position: usize,
    color: [u8; 4],
) -> Result<(), Failure> {
    let done = with_raw_page(document, page_index, |bindings, page| {
        // SAFETY: `page` is live, and so is the annotation inside the closure.
        unsafe {
            with_annot(bindings, page, position, |a| {
                let mut opacity = 0.0f32;
                let alpha = if bindings.FPDFAnnot_GetNumberValue(a, "CA", &mut opacity) != 0 {
                    (opacity.clamp(0.0, 1.0) * 255.0).round() as u32
                } else {
                    color[3] as u32
                };
                bindings.FPDFAnnot_SetAP(a, 0 as FPDF_ANNOT_APPEARANCEMODE, std::ptr::null()) != 0
                    && bindings.FPDFAnnot_SetColor(
                        a,
                        0 as FPDFANNOT_COLORTYPE,
                        color[0] as _,
                        color[1] as _,
                        color[2] as _,
                        alpha as _,
                    ) != 0
            })
        }
    })?;
    if done == Some(true) {
        Ok(())
    } else {
        fail(
            ErrorCode::AnnotationRefused,
            "the colour could not be changed",
        )
    }
}

/// The rich text of a comment, as the file's own program reads it: one paragraph for each line.
fn rich_text(text: &str) -> String {
    let mut out =
        String::from("<?xml version=\"1.0\"?><body xmlns=\"http://www.w3.org/1999/xhtml\">");
    for line in text.split('\n') {
        out.push_str("<p>");
        for c in line.chars() {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                c => out.push(c),
            }
        }
        out.push_str("</p>");
    }
    out.push_str("</body>");
    out
}

/// Most bytes of a text kept to be written back (the rich text of an annotation).
const MAX_KEPT_TEXT: usize = 1 << 20;

/// The text under `key`, as PDFium reads it: `Ok(None)` when the annotation has no such key, and an
/// error when the text is longer than `MAX_KEPT_TEXT`.
///
/// # Safety
/// `annotation` must be a live annotation handle of `bindings`.
unsafe fn read_key(
    bindings: &dyn PdfiumLibraryBindings,
    annotation: FPDF_ANNOTATION,
    key: &str,
) -> Result<Option<String>, ()> {
    if bindings.FPDFAnnot_HasKey(annotation, key) == 0 {
        return Ok(None);
    }
    let bytes =
        bindings.FPDFAnnot_GetStringValue(annotation, key, std::ptr::null_mut(), 0) as usize;
    if bytes > MAX_KEPT_TEXT {
        return Err(());
    }
    if bytes <= 2 {
        return Ok(Some(String::new()));
    }
    let mut units = vec![0u16; bytes / 2 + 1];
    let got = bindings.FPDFAnnot_GetStringValue(
        annotation,
        key,
        units.as_mut_ptr(),
        bytes as std::os::raw::c_ulong,
    ) as usize;
    // The text ends with a terminating zero.
    let count = (got.min(bytes) / 2).saturating_sub(1);
    Ok(Some(String::from_utf16_lossy(&units[..count])))
}

/// What the annotation at `position` holds of its text now: the comment, the rich text and the
/// date of the last change. Refused when the rich text is too long to be kept.
fn read_original(
    document: &PdfDocument<'static>,
    page_index: u32,
    position: usize,
) -> Result<Original, Failure> {
    let read = with_raw_page(document, page_index, |bindings, page| {
        // SAFETY: `page` is live, and so is the annotation inside the closure.
        unsafe {
            with_annot(bindings, page, position, |a| {
                Some(Original {
                    text: read_key(bindings, a, "Contents").ok()?.unwrap_or_default(),
                    rich: read_key(bindings, a, "RC").ok()?,
                    modified: read_key(bindings, a, "M").ok()?,
                })
            })
        }
    })?;
    read.flatten().ok_or_else(|| {
        (
            ErrorCode::AnnotationRefused,
            "the text of this annotation is too long to be changed here".to_string(),
        )
    })
}

/// Writes the rich text of the annotation, if it has any, so that the two texts do not tell
/// different things, and the date of the last change when one is given (a text that is taken back
/// has the date it had).
fn write_text_keys(
    document: &PdfDocument<'static>,
    page_index: u32,
    position: usize,
    rich: Option<&str>,
    modified: Option<&str>,
) -> Result<(), Failure> {
    let done = with_raw_page(document, page_index, |bindings, page| {
        // SAFETY: `page` is live, and so is the annotation inside the closure.
        unsafe {
            with_annot(bindings, page, position, |a| {
                let mut ok = true;
                if let Some(rich) = rich {
                    if bindings.FPDFAnnot_HasKey(a, "RC") != 0 {
                        ok &= bindings.FPDFAnnot_SetStringValue_str(a, "RC", rich) != 0;
                    }
                }
                if let Some(modified) = modified {
                    ok &= bindings.FPDFAnnot_SetStringValue_str(a, "M", modified) != 0;
                }
                ok
            })
        }
    })?;
    if done == Some(true) {
        Ok(())
    } else {
        fail(
            ErrorCode::AnnotationRefused,
            "the text could not be written",
        )
    }
}

/// Overwrites the text an annotation holds: comment, author, rich text, subject.
///
/// # Safety
/// `annotation` must be a live annotation handle of `bindings`.
unsafe fn blank(bindings: &dyn PdfiumLibraryBindings, annotation: FPDF_ANNOTATION) {
    for key in ["Contents", "T", "RC", "Subj"] {
        if bindings.FPDFAnnot_HasKey(annotation, key) != 0 {
            bindings.FPDFAnnot_SetStringValue_str(annotation, key, "");
        }
    }
}

/// Overwrites the text of every annotation of the pages that are about to be deleted: the comment,
/// the author, the rich text and the subject (also of pop-ups). A save leaves out the dictionaries
/// of a loaded file that nothing refers to any more, but writes those made in this session as
/// objects nobody refers to, so the words of a note written in this session would stay in the file
/// with a page that was deleted. That is not only the notes the engine made: when a page is drawn,
/// PDFium turns an annotation that the file holds as a dictionary inside the page's array into an
/// object of its own, which is one made in this session too, and nothing tells that one from an
/// annotation that came as an object. So every annotation of the pages that go is looked at. Form
/// fields are not touched (the form refers to them, and the page tools deal with them), and
/// neither are links.
///
/// An annotation that a page which stays also reaches is left alone, whether the two pages share
/// the dictionary, share the `/Annots` array, or are the same page listed twice: its words belong
/// to a page that remains. `total` is the number of pages the document has now; `deleted` holds
/// the indices that go. `tick` is called after every page that is looked at, so that a long
/// document does not go quiet. Returns how many annotations were overwritten.
pub fn scrub_pages(
    document: &PdfDocument<'_>,
    deleted: &[u32],
    total: u32,
    tick: &mut dyn FnMut(),
) -> usize {
    let Some(handle) = raw_document(document) else {
        return 0;
    };
    let bindings = document.bindings();
    let mut done = 0;
    // SAFETY: `handle` is the live document's; every page and annotation handle is opened and
    // closed inside `walk_groups`, and PDFium only gets handles it gave out.
    unsafe {
        walk_groups(
            bindings,
            handle,
            (deleted, total),
            Listed::Notes,
            tick,
            &mut |found| {
                for entry in found.iter().filter(|entry| !entry.shared) {
                    blank(bindings, entry.annotation);
                    done += 1;
                }
            },
        );
    }
    done
}

/// The widgets of the pages that go that a page which stays also lists, as the positions in the
/// list of annotations of the page that goes, for each page that goes. A widget like that is a
/// form field that a page which remains shows, with its value and its look, so whoever clears the
/// fields of the pages that go leaves these alone. `total` and `deleted` are as for
/// [`scrub_pages`], and so is `tick`.
pub fn shared_widgets(
    document: &PdfDocument<'_>,
    deleted: &[u32],
    total: u32,
    tick: &mut dyn FnMut(),
) -> HashMap<u32, HashSet<i32>> {
    let mut shared: HashMap<u32, HashSet<i32>> = HashMap::new();
    let Some(handle) = raw_document(document) else {
        return shared;
    };
    let bindings = document.bindings();
    // SAFETY: as in `scrub_pages`.
    unsafe {
        walk_groups(
            bindings,
            handle,
            (deleted, total),
            Listed::Widgets,
            tick,
            &mut |found| {
                for entry in found.iter().filter(|entry| entry.shared) {
                    shared.entry(entry.page).or_default().insert(entry.position);
                }
            },
        );
    }
    shared
}

/// How many pages with annotations of the pages that go are open at once in [`scrub_pages`]. Every
/// group looks at each page that stays once, so the pages that stay are read about
/// `ceil(annotated pages that go / SCRUB_GROUP)` times; a page that is open takes the memory of its
/// content, which is why the group is not every page that goes.
const SCRUB_GROUP: usize = 128;

/// Which annotations of the pages that go a walk looks at.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Listed {
    /// Everything but form fields and links.
    Notes,
    /// Form fields.
    Widgets,
}

/// An open annotation of a page that goes.
struct Found {
    annotation: FPDF_ANNOTATION,
    /// The index of the page that goes, and the place of the annotation in its list.
    page: u32,
    position: i32,
    /// True when a page that stays lists the same annotation.
    shared: bool,
}

/// Looks at the annotations of the pages that go, a few pages at a time, finds out which of them
/// a page that stays also lists, and hands each group to `each`. The annotations and the pages are
/// closed when `each` has returned.
///
/// # Safety
/// `handle` must be the live document of `bindings`.
unsafe fn walk_groups(
    bindings: &dyn PdfiumLibraryBindings,
    handle: FPDF_DOCUMENT,
    (deleted, total): (&[u32], u32),
    kind: Listed,
    tick: &mut dyn FnMut(),
    each: &mut dyn FnMut(&[Found]),
) {
    // The pages with annotations to look at are kept open a few at a time: a page that has none is
    // closed at once (a page with a lot on it takes memory while it is open).
    let mut pages: Vec<FPDF_PAGE> = Vec::new();
    let mut found: Vec<Found> = Vec::new();
    for &index in deleted {
        let page = bindings.FPDF_LoadPage(handle, index as c_int);
        if page.is_null() {
            continue;
        }
        let before = found.len();
        let count = bindings.FPDFPage_GetAnnotCount(page).max(0);
        for position in 0..count {
            let annotation = bindings.FPDFPage_GetAnnot(page, position);
            if annotation.is_null() {
                continue;
            }
            let subtype = bindings.FPDFAnnot_GetSubtype(annotation) as u32;
            let wanted = match kind {
                Listed::Notes => subtype != SUBTYPE_WIDGET && subtype != SUBTYPE_LINK,
                Listed::Widgets => subtype == SUBTYPE_WIDGET,
            };
            if wanted {
                found.push(Found {
                    annotation,
                    page: index,
                    position,
                    shared: false,
                });
            } else {
                bindings.FPDFPage_CloseAnnot(annotation);
            }
        }
        if found.len() > before {
            pages.push(page);
        } else {
            bindings.FPDF_ClosePage(page);
        }
        tick();
        if pages.len() >= SCRUB_GROUP {
            finish_group(
                bindings,
                handle,
                (deleted, total),
                (std::mem::take(&mut pages), std::mem::take(&mut found)),
                tick,
                each,
            );
        }
    }
    finish_group(
        bindings,
        handle,
        (deleted, total),
        (pages, found),
        tick,
        each,
    );
}

/// Marks the open annotations of some open pages that go that a page which stays lists too, hands
/// them to `each`, and closes the annotations and the pages.
///
/// # Safety
/// `handle` must be the live document of `bindings`, `pages` open pages of it and `found` open
/// annotations that belong to those pages.
unsafe fn finish_group(
    bindings: &dyn PdfiumLibraryBindings,
    handle: FPDF_DOCUMENT,
    (deleted, total): (&[u32], u32),
    (pages, mut found): (Vec<FPDF_PAGE>, Vec<Found>),
    tick: &mut dyn FnMut(),
    each: &mut dyn FnMut(&[Found]),
) {
    // A page that stays and lists the same dictionary keeps it.
    if !found.is_empty() {
        for index in 0..total {
            if deleted.contains(&index) {
                continue;
            }
            let page = bindings.FPDF_LoadPage(handle, index as c_int);
            if page.is_null() {
                // A page that cannot be read cannot be shown to leave an annotation alone.
                for entry in found.iter_mut() {
                    entry.shared = true;
                }
                break;
            }
            for entry in found.iter_mut().filter(|entry| !entry.shared) {
                entry.shared = bindings.FPDFPage_GetAnnotIndex(page, entry.annotation) >= 0;
            }
            bindings.FPDF_ClosePage(page);
            tick();
            if found.iter().all(|entry| entry.shared) {
                break;
            }
        }
    }
    each(&found);
    for entry in found {
        bindings.FPDFPage_CloseAnnot(entry.annotation);
    }
    for page in pages {
        bindings.FPDF_ClosePage(page);
    }
}

/// Takes the annotation at `position` off the page, with the popup it owns, after overwriting
/// what the person wrote and detaching the appearance. Returns the places that were taken off,
/// lowest first. A save writes the dictionaries of annotations made in the session (and any
/// appearance stream PDFium made, which no call can overwrite, and the pictures of a signature)
/// as objects that nothing refers to; those of a loaded file are left out. The save therefore
/// writes the document twice (see `save::serialise_clean`), and the text is overwritten here as
/// well, so that it is not left behind even where a document is written only once.
fn remove_raw(
    document: &PdfDocument<'static>,
    page_index: u32,
    position: usize,
) -> Result<Vec<usize>, Failure> {
    let gone = with_raw_page(document, page_index, |bindings, page| {
        // SAFETY: `page` is live; every annotation handle is closed before the page is.
        unsafe {
            let annotation = bindings.FPDFPage_GetAnnot(page, position as c_int);
            if annotation.is_null() {
                return None;
            }
            let mut places = vec![position];
            let popup = bindings.FPDFAnnot_GetLinkedAnnot(annotation, "Popup");
            if !popup.is_null() {
                // Only a popup goes with its owner: a file can name any annotation there.
                if bindings.FPDFAnnot_GetSubtype(popup) as u32 == SUBTYPE_POPUP {
                    let at = bindings.FPDFPage_GetAnnotIndex(page, popup);
                    blank(bindings, popup);
                    if at >= 0 && at as usize != position {
                        places.push(at as usize);
                    }
                }
                bindings.FPDFPage_CloseAnnot(popup);
            }
            blank(bindings, annotation);
            bindings.FPDFAnnot_SetAP(annotation, 0 as FPDF_ANNOT_APPEARANCEMODE, std::ptr::null());
            bindings.FPDFPage_CloseAnnot(annotation);
            // The higher place first, so that the lower one does not move.
            places.sort_unstable_by(|a, b| b.cmp(a));
            for place in &places {
                if bindings.FPDFPage_RemoveAnnot(page, *place as c_int) == 0 {
                    return None;
                }
            }
            places.reverse();
            Some(places)
        }
    })?;
    gone.ok_or_else(|| {
        (
            ErrorCode::AnnotationRefused,
            "the annotation could not be taken off the page".to_string(),
        )
    })
}

// ---- writing an annotation -----------------------------------------------------

/// Writes the annotation of a snapshot as the newest of the page.
fn write_annotation(
    page: &mut PdfPage<'static>,
    document: &mut PdfDocument<'static>,
    page_index: u32,
    snapshot: &Snapshot,
) -> Result<(), Failure> {
    match &snapshot.recipe {
        Recipe::Markup {
            kind,
            color,
            quads,
            bounds,
        } => {
            macro_rules! markup {
                ($create:ident) => {{
                    let mut annotation = page.annotations_mut().$create().map_err(refused)?;
                    annotation
                        .set_stroke_color(pdf_color(*color))
                        .map_err(refused)?;
                    annotation.set_bounds(rect_of(*bounds)).map_err(refused)?;
                    for q in quads {
                        annotation
                            .attachment_points_mut()
                            .create_attachment_point_at_end(PdfQuadPoints::new_from_values(
                                q[0], q[1], q[2], q[3], q[4], q[5], q[6], q[7],
                            ))
                            .map_err(refused)?;
                    }
                    if !snapshot.text.is_empty() {
                        annotation.set_contents(&snapshot.text).map_err(refused)?;
                    }
                    if !snapshot.author.is_empty() {
                        annotation.set_creator(&snapshot.author).map_err(refused)?;
                    }
                }};
            }
            match kind {
                AnnotKind::Highlight => markup!(create_highlight_annotation),
                AnnotKind::Underline => markup!(create_underline_annotation),
                AnnotKind::Strikeout => markup!(create_strikeout_annotation),
                _ => return fail(ErrorCode::InvalidRequest, "that is not markup"),
            }
            Ok(())
        }
        Recipe::Note { color, bounds } => {
            let mut annotation = page
                .annotations_mut()
                .create_text_annotation(&snapshot.text)
                .map_err(refused)?;
            annotation
                .set_stroke_color(pdf_color(*color))
                .map_err(refused)?;
            annotation.set_bounds(rect_of(*bounds)).map_err(refused)?;
            if !snapshot.author.is_empty() {
                annotation.set_creator(&snapshot.author).map_err(refused)?;
            }
            Ok(())
        }
        Recipe::Ink {
            color,
            width,
            strokes,
        } => write_ink(
            page, document, page_index, snapshot, *color, *width, strokes,
        ),
        Recipe::Stamp { stamp, x, y } => write_stamp(page, document, snapshot, *stamp, *x, *y),
        Recipe::Signature {
            mark, color, rect, ..
        } => write_signature(page, document, page_index, mark, *color, *rect),
    }
}

/// The raw handle PDFium has for a document. `pdfium-render` keeps it to itself; the text it gives
/// for `Debug` names it, and a test (`the_raw_handle_of_a_document_can_be_read`) fails if that
/// ever stops being so.
pub fn raw_document(document: &PdfDocument<'_>) -> Option<FPDF_DOCUMENT> {
    let text = format!("{document:?}");
    let start = text.find("0x")? + 2;
    let hex: String = text[start..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    let value = usize::from_str_radix(&hex, 16).ok()?;
    if value == 0 || value % std::mem::align_of::<usize>() != 0 {
        return None;
    }
    // The handle must be the one of this document: it has as many pages as the document.
    let handle = value as FPDF_DOCUMENT;
    // SAFETY: the value was printed by the document for its own handle; PDFium only reads it.
    let pages = unsafe { document.bindings().FPDF_GetPageCount(handle) };
    (pages >= 0 && pages as usize == document.pages().len() as usize).then_some(handle)
}

/// A drawing is an ink annotation with a list of strokes (`InkList`), a colour and a line width, as
/// other readers expect it; PDFium makes the appearance from those when the page is drawn.
/// `pdfium-render` has no way to add a stroke to the list, so this goes through PDFium's own
/// functions, on a second handle for the same page of the same document (changes made through one
/// handle are in the document, which the other sees). The functions are called only with handles
/// that PDFium gave out here and with data that was checked.
fn write_ink(
    page: &mut PdfPage<'static>,
    document: &mut PdfDocument<'static>,
    page_index: u32,
    snapshot: &Snapshot,
    color: [u8; 4],
    width: f32,
    strokes: &[Vec<[f32; 2]>],
) -> Result<(), Failure> {
    let mapper = Mapper::new(page);
    // The strokes in page space, and their bounding box.
    let mut paths: Vec<Vec<FS_POINTF>> = Vec::new();
    let (mut left, mut bottom) = (f32::MAX, f32::MAX);
    let (mut right, mut top) = (f32::MIN, f32::MIN);
    for stroke in strokes {
        let mut points: Vec<(f32, f32)> = stroke
            .iter()
            .filter_map(|p| mapper.to_page(page, p[0], p[1]))
            .collect();
        if points.len() == 1 {
            // A dot: a ring of points so small that the line, drawn at its width, is a dot.
            let (x, y) = points[0];
            let r = (width * 0.2).max(0.05);
            points = (0..=8)
                .map(|i| {
                    let a = i as f32 * std::f32::consts::FRAC_PI_4;
                    (x + r * a.cos(), y + r * a.sin())
                })
                .collect();
        }
        for (x, y) in &points {
            left = left.min(*x);
            right = right.max(*x);
            bottom = bottom.min(*y);
            top = top.max(*y);
        }
        if !points.is_empty() {
            paths.push(
                points
                    .into_iter()
                    .map(|(x, y)| FS_POINTF { x, y })
                    .collect(),
            );
        }
    }
    if paths.is_empty() {
        return fail(
            ErrorCode::AnnotationRefused,
            "that place is not on the page",
        );
    }
    let pad = width / 2.0 + 1.0;
    let rect = FS_RECTF {
        left: left - pad,
        top: top + pad,
        right: right + pad,
        bottom: bottom - pad,
    };
    let Some(handle) = raw_document(document) else {
        return fail(
            ErrorCode::AnnotationRefused,
            "the document cannot be changed",
        );
    };
    let bindings = document.bindings();
    let text = snapshot.text.as_str();
    // SAFETY: `handle` is the live document's handle. The page, and the annotation, are opened
    // here and closed here, and are used between. The points outlive the calls that read them.
    unsafe {
        let raw_page = bindings.FPDF_LoadPage(handle, page_index as std::os::raw::c_int);
        if raw_page.is_null() {
            return fail(ErrorCode::Corrupt, "the page could not be read");
        }
        let annotation = bindings.FPDFPage_CreateAnnot(raw_page, 15 as FPDF_ANNOTATION_SUBTYPE);
        let done = if annotation.is_null() {
            false
        } else {
            let mut ok = bindings.FPDFAnnot_SetRect(annotation, &rect) != 0
                && bindings.FPDFAnnot_SetColor(
                    annotation,
                    0 as FPDFANNOT_COLORTYPE,
                    color[0] as _,
                    color[1] as _,
                    color[2] as _,
                    color[3] as _,
                ) != 0
                && bindings.FPDFAnnot_SetBorder(annotation, 0.0, 0.0, width) != 0;
            for points in &paths {
                ok = ok
                    && bindings.FPDFAnnot_AddInkStroke(annotation, points.as_ptr(), points.len())
                        >= 0;
            }
            if ok && !text.is_empty() {
                ok = bindings.FPDFAnnot_SetStringValue_str(annotation, "Contents", text) != 0;
            }
            bindings.FPDFPage_CloseAnnot(annotation);
            ok
        };
        bindings.FPDF_ClosePage(raw_page);
        if done {
            Ok(())
        } else {
            fail(
                ErrorCode::AnnotationRefused,
                "the drawing could not be written",
            )
        }
    }
}

/// Size of a stamp in points on the page as it is drawn.
fn stamp_size(stamp: StampKind) -> (f32, f32) {
    // Bold sans-serif capitals are about 0.72 of the font size wide.
    let width = stamp.label().len() as f32 * STAMP_FONT * 0.72 + 2.0 * STAMP_PAD;
    (width, STAMP_FONT + 2.0 * STAMP_PAD)
}

const STAMP_FONT: f32 = 26.0;
const STAMP_PAD: f32 = 9.0;

fn stamp_color(stamp: StampKind) -> [u8; 4] {
    match stamp {
        StampKind::Approved => [26, 127, 55, 255],
        StampKind::Rejected => [194, 24, 24, 255],
        StampKind::Draft => [90, 90, 90, 255],
        StampKind::Final => [24, 90, 190, 255],
        StampKind::Confidential => [194, 24, 24, 255],
        StampKind::Reviewed => [100, 50, 170, 255],
    }
}

fn write_stamp(
    page: &mut PdfPage<'static>,
    document: &mut PdfDocument<'static>,
    snapshot: &Snapshot,
    stamp: StampKind,
    x: f32,
    y: f32,
) -> Result<(), Failure> {
    let mapper = Mapper::new(page);
    let (width, height) = stamp_size(stamp);
    // The stamp is drawn upright on the page as it is seen: its own frame has the origin at the
    // bottom left of the stamp as drawn, and goes to page space through three points.
    let corner = |dx: f32, dy: f32| mapper.to_page(page, x + dx, y + height - dy);
    let (Some(o), Some(u), Some(v)) = (corner(0.0, 0.0), corner(width, 0.0), corner(0.0, height))
    else {
        return fail(
            ErrorCode::AnnotationRefused,
            "that place is not on the page",
        );
    };
    // The axes are measured over the whole width and height, where the rounding of the points to
    // 1/16 of a point matters least.
    let matrix = PdfMatrix::new(
        (u.0 - o.0) / width,
        (u.1 - o.1) / width,
        (v.0 - o.0) / height,
        (v.1 - o.1) / height,
        o.0,
        o.1,
    );
    let corners: Vec<(f32, f32)> = [(0.0, 0.0), (width, 0.0), (0.0, height), (width, height)]
        .iter()
        .filter_map(|(dx, dy)| corner(*dx, *dy))
        .collect();
    if corners.len() != 4 {
        return fail(
            ErrorCode::AnnotationRefused,
            "that place is not on the page",
        );
    }
    let bounds = [
        corners.iter().map(|c| c.0).fold(f32::MAX, f32::min),
        corners.iter().map(|c| c.1).fold(f32::MAX, f32::min),
        corners.iter().map(|c| c.0).fold(f32::MIN, f32::max),
        corners.iter().map(|c| c.1).fold(f32::MIN, f32::max),
    ];
    let color = pdf_color(stamp_color(stamp));
    let mut annotation = page
        .annotations_mut()
        .create_stamp_annotation()
        .map_err(refused)?;
    annotation.set_bounds(rect_of(bounds)).map_err(refused)?;

    let border = 3.0;
    let mut frame = PdfPagePathObject::new_rect(
        document,
        PdfRect::new_from_values(
            border / 2.0,
            border / 2.0,
            height - border / 2.0,
            width - border / 2.0,
        ),
        Some(color),
        Some(PdfPoints::new(border)),
        None,
    )
    .map_err(refused)?;
    frame.apply_matrix(matrix).map_err(refused)?;
    annotation
        .objects_mut()
        .add_object(PdfPageObject::from(frame))
        .map_err(refused)?;

    let font = document.fonts_mut().helvetica_bold();
    let mut label =
        PdfPageTextObject::new(document, stamp.label(), font, PdfPoints::new(STAMP_FONT))
            .map_err(refused)?;
    label.set_fill_color(color).map_err(refused)?;
    label
        .apply_matrix(PdfMatrix::new(
            1.0,
            0.0,
            0.0,
            1.0,
            STAMP_PAD,
            STAMP_PAD + 0.2 * STAMP_FONT,
        ))
        .map_err(refused)?;
    label.apply_matrix(matrix).map_err(refused)?;
    annotation
        .objects_mut()
        .add_object(PdfPageObject::from(label))
        .map_err(refused)?;
    annotation.set_contents(stamp.label()).map_err(refused)?;
    let _ = snapshot;
    Ok(())
}

/// A box on the page as it is drawn (`[left, top, right, bottom]`) as the box that holds it in
/// page space (`[left, bottom, right, top]`).
fn page_bounds(page: &PdfPage, mapper: &Mapper, r: [f32; 4]) -> Option<[f32; 4]> {
    let corners = [
        mapper.to_page(page, r[0], r[1])?,
        mapper.to_page(page, r[2], r[1])?,
        mapper.to_page(page, r[0], r[3])?,
        mapper.to_page(page, r[2], r[3])?,
    ];
    let fold = |pick: fn(&(f32, f32)) -> f32, f: fn(f32, f32) -> f32, from: f32| {
        corners.iter().map(pick).fold(from, f)
    };
    Some([
        fold(|c| c.0, f32::min, f32::MAX),
        fold(|c| c.1, f32::min, f32::MAX),
        fold(|c| c.0, f32::max, f32::MIN),
        fold(|c| c.1, f32::max, f32::MIN),
    ])
}

/// A picture of a signature: a stamp annotation with an appearance made of the paths (or the
/// image) of the picture, and the print flag. The appearance is drawn upright on the page as it is
/// seen, in a frame with the origin at the bottom left of the box, and goes to page space through
/// three points, as the fixed stamps do.
fn write_signature(
    page: &mut PdfPage<'static>,
    document: &mut PdfDocument<'static>,
    page_index: u32,
    mark: &SignatureMark,
    color: [u8; 4],
    rect: [f32; 4],
) -> Result<(), Failure> {
    let mapper = Mapper::new(page);
    let (x, y) = (rect[0], rect[1]);
    let (width, height) = (rect[2] - rect[0], rect[3] - rect[1]);
    let corner = |dx: f32, dy: f32| mapper.to_page(page, x + dx, y + height - dy);
    let (Some(o), Some(u), Some(v)) = (corner(0.0, 0.0), corner(width, 0.0), corner(0.0, height))
    else {
        return fail(
            ErrorCode::AnnotationRefused,
            "that place is not on the page",
        );
    };
    let matrix = PdfMatrix::new(
        (u.0 - o.0) / width,
        (u.1 - o.1) / width,
        (v.0 - o.0) / height,
        (v.1 - o.1) / height,
        o.0,
        o.1,
    );
    let Some(bounds) = page_bounds(page, &mapper, rect) else {
        return fail(
            ErrorCode::AnnotationRefused,
            "that place is not on the page",
        );
    };
    let color = pdf_color(color);
    let before = page.annotations().len();
    let mut annotation = page
        .annotations_mut()
        .create_stamp_annotation()
        .map_err(refused)?;
    annotation.set_bounds(rect_of(bounds)).map_err(refused)?;

    match mark {
        SignatureMark::Strokes {
            width: box_w,
            height: box_h,
            line,
            strokes,
        } => {
            let (sx, sy) = (width / box_w, height / box_h);
            let thickness = (line * (sx + sy) / 2.0).max(0.05);
            for stroke in strokes {
                let mut points: Vec<(f32, f32)> = stroke
                    .iter()
                    .map(|p| {
                        (
                            p[0].clamp(0.0, *box_w) * sx,
                            height - p[1].clamp(0.0, *box_h) * sy,
                        )
                    })
                    .collect();
                if points.len() == 1 {
                    // A dot: a line so short that its round ends are all there is.
                    points.push((points[0].0 + 0.01, points[0].1));
                }
                let Some(first) = points.first().copied() else {
                    continue;
                };
                let mut path = PdfPagePathObject::new(
                    document,
                    PdfPoints::new(first.0),
                    PdfPoints::new(first.1),
                    Some(color),
                    Some(PdfPoints::new(thickness)),
                    None,
                )
                .map_err(refused)?;
                for p in &points[1..] {
                    path.line_to(PdfPoints::new(p.0), PdfPoints::new(p.1))
                        .map_err(refused)?;
                }
                path.set_line_cap(PdfPageObjectLineCap::Round)
                    .map_err(refused)?;
                path.set_line_join(PdfPageObjectLineJoin::Round)
                    .map_err(refused)?;
                path.apply_matrix(matrix).map_err(refused)?;
                annotation
                    .objects_mut()
                    .add_object(PdfPageObject::from(path))
                    .map_err(refused)?;
            }
        }
        SignatureMark::Typed { font, text } => {
            let outline = sigmark::typed_outline(*font, text).map_err(|_| {
                (
                    ErrorCode::InvalidRequest,
                    "that text cannot be set in this font".to_string(),
                )
            })?;
            let b = outline.bounds;
            let (sx, sy) = (width / (b[2] - b[0]), height / (b[3] - b[1]));
            let at = |gx: f32, gy: f32| {
                (
                    PdfPoints::new((gx - b[0]) * sx),
                    PdfPoints::new((gy - b[1]) * sy),
                )
            };
            for segs in &outline.paths {
                let Some(Seg::Move(mx, my)) = segs.first().copied() else {
                    continue;
                };
                let (px, py) = at(mx, my);
                let mut path = PdfPagePathObject::new(document, px, py, None, None, Some(color))
                    .map_err(refused)?;
                for seg in &segs[1..] {
                    match *seg {
                        Seg::Move(gx, gy) => {
                            let (px, py) = at(gx, gy);
                            path.move_to(px, py).map_err(refused)?;
                        }
                        Seg::Line(gx, gy) => {
                            let (px, py) = at(gx, gy);
                            path.line_to(px, py).map_err(refused)?;
                        }
                        Seg::Cubic(c1x, c1y, c2x, c2y, gx, gy) => {
                            let (px, py) = at(gx, gy);
                            let (a, b) = at(c1x, c1y);
                            let (c, d) = at(c2x, c2y);
                            path.bezier_to(px, py, a, b, c, d).map_err(refused)?;
                        }
                        Seg::Close => path.close_path().map_err(refused)?,
                    }
                }
                path.apply_matrix(matrix).map_err(refused)?;
                annotation
                    .objects_mut()
                    .add_object(PdfPageObject::from(path))
                    .map_err(refused)?;
            }
        }
        SignatureMark::Image {
            width: px_w,
            height: px_h,
            rgba,
        } => {
            // PDFium takes blue, green, red, alpha.
            let mut bgra = Vec::with_capacity(rgba.len());
            for px in rgba.as_chunks::<4>().0 {
                bgra.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
            let bitmap =
                PdfBitmap::from_bytes(*px_w as i32, *px_h as i32, PdfBitmapFormat::BGRA, &mut bgra)
                    .map_err(refused)?;
            let mut image = PdfPageImageObject::new(document).map_err(refused)?;
            image.set_bitmap(&bitmap).map_err(refused)?;
            image
                .apply_matrix(PdfMatrix::new(width, 0.0, 0.0, height, 0.0, 0.0))
                .map_err(refused)?;
            image.apply_matrix(matrix).map_err(refused)?;
            annotation
                .objects_mut()
                .add_object(PdfPageObject::from(image))
                .map_err(refused)?;
        }
    }
    annotation.set_contents(SIGNATURE_TEXT).map_err(refused)?;

    // The print flag, so that the picture is on the paper when another reader prints the page.
    let flagged = with_raw_page(document, page_index, |bindings, raw| {
        // SAFETY: `raw` is live, and so is the annotation inside the closure.
        unsafe {
            with_annot(bindings, raw, before, |a| {
                bindings.FPDFAnnot_SetFlags(a, FLAG_PRINT) != 0
            })
        }
    })?;
    if flagged == Some(true) {
        Ok(())
    } else {
        fail(
            ErrorCode::AnnotationRefused,
            "the picture could not be written",
        )
    }
}

// ---- boxes of text -------------------------------------------------------------

fn overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    a1.min(b1) - a0.max(b0)
}

/// True when the box of the next character carries on the run the union `u` stands for: it
/// shares most of its height with the run and is close to it (text that runs along a line), or
/// shares most of its width and is close (text that runs down a column). Which end it joins does not
/// matter, so the run may go left to right or right to left.
fn joins(u: &[f32; 4], r: &[f32; 4]) -> bool {
    let (uw, uh) = (u[2] - u[0], u[3] - u[1]);
    let (rw, rh) = (r[2] - r[0], r[3] - r[1]);
    let along_line = overlap(u[1], u[3], r[1], r[3]) > 0.5 * uh.min(rh)
        && overlap(u[0], u[2], r[0], r[2]) > -0.6 * uh.max(rh);
    // A column is a stack of characters of the same width, one under the other, with hardly a gap.
    // The run so far is as tall as it is wide or taller, and the new character is as wide as it.
    // Two lines of text one under the other are not a column: their boxes differ in width.
    let down_column = uh >= uw * 0.95
        && (rw - uw).abs() <= 0.3 * uw.max(rw)
        && overlap(u[0], u[2], r[0], r[2]) > 0.8 * uw.min(rw)
        && overlap(u[1], u[3], r[1], r[3]) > -0.3 * uw.max(rw);
    along_line || down_column
}

/// The boxes that cover a run of characters, one for each stretch that belongs together (a line,
/// or part of a line when the direction changes). Boxes are `[left, top, right, bottom]` on the page
/// as it is drawn. A line break ends a stretch; a character without a place is skipped.
pub fn line_boxes(chars: &[TextChar]) -> Vec<[f32; 4]> {
    let mut out: Vec<[f32; 4]> = Vec::new();
    let mut current: Option<[f32; 4]> = None;
    for c in chars {
        let r = c.rect;
        let usable = r[2] - r[0] > 0.0 && r[3] - r[1] > 0.0 && r.iter().all(|v| v.is_finite());
        if c.ch == '\n' || c.ch == '\r' {
            out.extend(current.take());
            continue;
        }
        if !usable {
            continue;
        }
        current = match current {
            None => Some(r),
            Some(u) if joins(&u, &r) => Some([
                u[0].min(r[0]),
                u[1].min(r[1]),
                u[2].max(r[2]),
                u[3].max(r[3]),
            ]),
            Some(u) => {
                out.push(u);
                Some(r)
            }
        };
    }
    out.extend(current);
    out
}

/// The boxes as quadrilaterals in page space (top left, top right, bottom left, bottom right of
/// the box in page space, the order other PDF tools write) and the box around all of them as
/// `[left, bottom, right, top]`.
fn quads_for(page: &PdfPage, mapper: &Mapper, boxes: &[[f32; 4]]) -> (Vec<[f32; 8]>, [f32; 4]) {
    let mut quads = Vec::new();
    let (mut left, mut bottom) = (f32::MAX, f32::MAX);
    let (mut right, mut top) = (f32::MIN, f32::MIN);
    for b in boxes {
        let corners = [
            mapper.to_page(page, b[0], b[1]),
            mapper.to_page(page, b[2], b[1]),
            mapper.to_page(page, b[0], b[3]),
            mapper.to_page(page, b[2], b[3]),
        ];
        let Some(points) = corners.into_iter().collect::<Option<Vec<(f32, f32)>>>() else {
            continue;
        };
        let l = points.iter().map(|p| p.0).fold(f32::MAX, f32::min);
        let r = points.iter().map(|p| p.0).fold(f32::MIN, f32::max);
        let bt = points.iter().map(|p| p.1).fold(f32::MAX, f32::min);
        let tp = points.iter().map(|p| p.1).fold(f32::MIN, f32::max);
        quads.push([l, tp, r, tp, l, bt, r, bt]);
        left = left.min(l);
        right = right.max(r);
        bottom = bottom.min(bt);
        top = top.max(tp);
    }
    (quads, [left, bottom, right, top])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(c: char, rect: [f32; 4]) -> TextChar {
        TextChar { ch: c, rect }
    }

    #[test]
    fn text_from_a_file_loses_its_controls_and_its_length() {
        assert_eq!(clean_text("a\u{0}b\tc\r\nd", 100), "a b c\nd");
        assert_eq!(clean_text("x\u{202E}y\u{200B}z", 100), "xyz");
        // Every kind of line end is a line feed; marks that steer the direction are dropped.
        assert_eq!(
            clean_text("a\rb\r\nc\u{2028}d\u{2029}e", 100),
            "a\nb\nc\nd\ne"
        );
        assert_eq!(clean_text("a\u{200E}b\u{200F}c\u{061C}d", 100), "abcd");
        let cut = clean_text(&"é".repeat(50), 11);
        assert_eq!(cut.len(), 10);
        assert!(cut.chars().all(|c| c == 'é'));
        assert_eq!(clean_text("", 10), "");
    }

    #[test]
    fn a_line_left_to_right_is_one_box() {
        let chars: Vec<TextChar> = (0..5)
            .map(|i| {
                ch(
                    'a',
                    [10.0 + i as f32 * 6.0, 100.0, 16.0 + i as f32 * 6.0, 112.0],
                )
            })
            .collect();
        assert_eq!(line_boxes(&chars), vec![[10.0, 100.0, 40.0, 112.0]]);
    }

    #[test]
    fn a_line_right_to_left_is_one_box() {
        // The first character in reading order is at the right end.
        let chars: Vec<TextChar> = (0..5)
            .map(|i| {
                let right = 100.0 - i as f32 * 6.0;
                ch('\u{5d0}', [right - 6.0, 100.0, right, 112.0])
            })
            .collect();
        assert_eq!(line_boxes(&chars), vec![[70.0, 100.0, 100.0, 112.0]]);
    }

    #[test]
    fn two_lines_are_two_boxes_and_a_break_ends_a_line() {
        let mut chars = vec![
            ch('a', [10.0, 100.0, 16.0, 112.0]),
            ch('b', [16.0, 100.0, 22.0, 112.0]),
            ch('\n', [0.0; 4]),
            ch('c', [10.0, 116.0, 16.0, 128.0]),
            ch('d', [16.0, 116.0, 22.0, 128.0]),
        ];
        assert_eq!(
            line_boxes(&chars),
            vec![[10.0, 100.0, 22.0, 112.0], [10.0, 116.0, 22.0, 128.0]]
        );
        // Without the break the lines are still apart: they do not share their height.
        chars.remove(2);
        assert_eq!(line_boxes(&chars).len(), 2);
    }

    #[test]
    fn a_run_that_jumps_across_the_line_is_cut() {
        // Left to right text, then a run that starts far to the right (a change of direction).
        let chars = vec![
            ch('a', [10.0, 100.0, 16.0, 112.0]),
            ch('b', [16.0, 100.0, 22.0, 112.0]),
            ch('\u{5d0}', [200.0, 100.0, 206.0, 112.0]),
            ch('\u{5d1}', [194.0, 100.0, 200.0, 112.0]),
        ];
        assert_eq!(
            line_boxes(&chars),
            vec![[10.0, 100.0, 22.0, 112.0], [194.0, 100.0, 206.0, 112.0]]
        );
    }

    #[test]
    fn a_column_of_vertical_text_is_one_box() {
        let chars: Vec<TextChar> = (0..4)
            .map(|i| {
                ch(
                    'x',
                    [50.0, 10.0 + i as f32 * 12.0, 62.0, 22.0 + i as f32 * 12.0],
                )
            })
            .collect();
        assert_eq!(line_boxes(&chars), vec![[50.0, 10.0, 62.0, 58.0]]);
    }

    #[test]
    fn characters_without_a_place_are_skipped() {
        let chars = vec![
            ch('a', [10.0, 100.0, 16.0, 112.0]),
            ch(' ', [0.0; 4]),
            ch('b', [16.0, 100.0, 22.0, 112.0]),
            ch('c', [f32::NAN, 0.0, 1.0, 1.0]),
        ];
        assert_eq!(line_boxes(&chars), vec![[10.0, 100.0, 22.0, 112.0]]);
        assert!(line_boxes(&[]).is_empty());
        assert!(line_boxes(&[ch('a', [0.0; 4])]).is_empty());
    }
}
