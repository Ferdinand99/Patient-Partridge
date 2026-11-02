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

use std::collections::{HashMap, VecDeque};

use partridge_protocol::{
    ability, AnnotInfo, AnnotKind, ErrorCode, StampKind, MAX_ANNOTS_PER_PAGE, MAX_ANNOT_AUTHOR,
    MAX_ANNOT_TEXT, MAX_INK_POINTS, MAX_INK_STROKES, MAX_INK_WIDTH, MIN_INK_WIDTH,
};
use pdfium_render::prelude::*;

use crate::text::{self, Mapper, TextChar};
use crate::{fail, Failure};

/// Most annotations of a page the engine looks at. The rest of a page that has more is ignored.
const MAX_SCANNED: usize = 5_000;

/// Most removed annotations the engine keeps so that they can be put back.
const MAX_REMEMBERED: usize = 200;

/// Most boxes (quadrilaterals) one markup is made of: one for each line of text it covers.
pub const MAX_MARKUP_BOXES: usize = 512;

/// Side of a sticky note's icon, in points.
const NOTE_SIDE: f32 = 20.0;

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
}

#[derive(Clone, Debug)]
struct Snapshot {
    recipe: Recipe,
    text: String,
    author: String,
}

/// What the engine knows about the annotations of one document.
#[derive(Default)]
pub struct DocAnnots {
    next_id: u32,
    /// The ids of the annotations of each page, in the order of the page's list.
    pages: HashMap<u32, Vec<u32>>,
    /// Annotations the engine made, by id.
    made: HashMap<u32, Recipe>,
    /// Annotations that were removed, oldest first: `(page, id, what it was)`.
    removed: VecDeque<(u32, u32, Snapshot)>,
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

/// Text from a file made safe to hand on: control characters other than a line break are replaced
/// by a space, and the length is cut to `max` bytes on a character boundary.
pub fn clean_text(text: &str, max: usize) -> String {
    let mut out = String::with_capacity(text.len().min(max));
    for c in text.chars() {
        let c = match c {
            '\n' => '\n',
            '\r' => continue,
            c if c.is_control() => ' ',
            // Characters that change the direction of the text around them, or hide: a note
            // must not be able to make the window show something other than what it holds.
            '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200B}' | '\u{FEFF}' => continue,
            c => c,
        };
        if out.len() + c.len_utf8() > max {
            break;
        }
        out.push(c);
    }
    out
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

/// The colour an annotation is drawn in, read from its appearance. The colour entry of the
/// annotation itself is not used: PDFium refuses to read it once an appearance stream exists, and
/// the call that stands in for it in `pdfium-render` would then read a page object where there is
/// an annotation. The fill of a highlight and the stroke of an underline or a strike-out come
/// from the first path of the appearance.
fn color_of(annotation: &PdfPageAnnotation, kind: AnnotKind) -> [u8; 4] {
    for object in annotation.objects().iter().take(16) {
        if let Some(path) = object.as_path_object() {
            let color = match kind {
                AnnotKind::Highlight => path.fill_color(),
                _ => path.stroke_color(),
            };
            if let Ok(c) = color {
                return [c.red(), c.green(), c.blue(), 255];
            }
        }
    }
    DEFAULT_COLOR
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

impl DocAnnots {
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
    ) -> Option<AnnotInfo> {
        let kind = kind_of(annotation)?;
        if annotation.is_hidden() {
            return None;
        }
        let made = self.made.contains_key(&id);
        let has_quads = annotation.attachment_points().len() > 0;
        let abilities = match kind {
            AnnotKind::Highlight | AnnotKind::Underline | AnnotKind::Strikeout => {
                let mut a = ability::EDIT_TEXT | ability::REMOVE;
                if has_quads {
                    a |= ability::RECOLOUR | ability::RESTORE;
                }
                a
            }
            // PDFium draws every note the same, whatever its colour, so a note has no colour to pick.
            AnnotKind::Note => {
                ability::EDIT_TEXT | ability::MOVE | ability::REMOVE | ability::RESTORE
            }
            AnnotKind::Ink => {
                ability::EDIT_TEXT | ability::REMOVE | if made { ability::RESTORE } else { 0 }
            }
            AnnotKind::Stamp => ability::REMOVE | if made { ability::RESTORE } else { 0 },
            AnnotKind::Other => ability::REMOVE,
        };
        let bounds = annotation.bounds().ok()?;
        // A box far outside any page is cut to a size the window can lay out.
        let rect = mapper.rect(page, &bounds)?.map(|v| v.clamp(-1.0e6, 1.0e6));
        let color = color_of(annotation, kind);
        Some(AnnotInfo {
            id,
            kind,
            abilities,
            rect,
            color,
            text: clean_text(&annotation.contents().unwrap_or_default(), MAX_ANNOT_TEXT),
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
        // of a highlight is read from it.
        settle(&page);
        let mapper = Mapper::new(&page);
        let count = page.annotations().len();
        let ids = self.ids(page_index, count).clone();
        let mut out = Vec::new();
        for index in 0..count.min(MAX_SCANNED) {
            if out.len() >= MAX_ANNOTS_PER_PAGE {
                break;
            }
            let Ok(annotation) = page.annotations().get(index) else {
                continue;
            };
            if let Some(info) = self.describe(&annotation, ids[index], &page, &mapper) {
                out.push(info);
            }
        }
        Ok(out)
    }

    /// The annotation with `id`: its place in the list of the page, and what it is.
    fn find(
        &mut self,
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
        let info = self
            .describe(&annotation, id, page, &mapper)
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
        self.describe(&annotation, new_id, page, &mapper)
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
            if page.annotations_mut().delete_annotation(annotation).is_err() {
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
            return fail(ErrorCode::AnnotationRefused, "that place is not on the page");
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
        // Brings the list of ids in step before something is added to it.
        self.ids(page_index, before);
        let new_id = match id {
            Some(id) => id,
            None => self.fresh_id(),
        };
        // A drawing or a stamp can be put back only if the engine knows how it was made, and the
        // answer says so, so the recipe is kept before the answer is made.
        let keeps = remember
            && matches!(
                snapshot.recipe,
                Recipe::Ink { .. } | Recipe::Stamp { .. }
            );
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
        let mut page = load(document, page_index)?;
        let (position, info) = self.find(&page, page_index, id)?;
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
        {
            let mut annotation = page.annotations().get(position).map_err(|_| no_such())?;
            if let Some(text) = text {
                annotation
                    .set_contents(&clean_text(text, MAX_ANNOT_TEXT))
                    .map_err(refused)?;
            }
            if wants_move {
                let from = mapper.to_page(&page, 0.0, 0.0);
                let to = mapper.to_page(&page, dx, dy);
                let (Some(from), Some(to), Some(bounds)) = (from, to, bounds_of(&annotation))
                else {
                    return fail(ErrorCode::AnnotationRefused, "that place is not on the page");
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
        if let Some(color) = color {
            // PDFium cannot change the colour of an annotation that has its appearance stream, so
            // the annotation is made again with the new colour, and takes the place of the old one.
            let (snapshot, _) = self.snapshot_of(&page, position, &info)?;
            let Some(mut snapshot) = snapshot else {
                return fail(
                    ErrorCode::AnnotationRefused,
                    "this annotation does not allow that change",
                );
            };
            match &mut snapshot.recipe {
                Recipe::Markup { color: c, .. } | Recipe::Note { color: c, .. } => *c = color,
                _ => {
                    return fail(
                        ErrorCode::AnnotationRefused,
                        "this annotation does not allow that change",
                    )
                }
            }
            let before = page.annotations().len();
            let old = page
                .annotations_mut()
                .get(position)
                .map_err(|_| no_such())?;
            page.annotations_mut()
                .delete_annotation(old)
                .map_err(refused)?;
            self.ids(page_index, before).remove(position);
            return self.create_from(&mut page, document, page_index, &snapshot, Some(id), false);
        }
        settle(&page);
        let (_, info) = self.find(&page, page_index, id)?;
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
            AnnotKind::Ink | AnnotKind::Stamp => self.made.get(&info.id).cloned(),
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
        let mut page = load(document, page_index)?;
        let (position, info) = self.find(&page, page_index, id)?;
        if info.abilities & ability::REMOVE == 0 {
            return fail(
                ErrorCode::AnnotationRefused,
                "this annotation does not allow that change",
            );
        }
        let snapshot = if info.abilities & ability::RESTORE != 0 {
            self.snapshot_of(&page, position, &info)?.0
        } else {
            None
        };
        let before = page.annotations().len();
        let mut annotation = page
            .annotations_mut()
            .get(position)
            .map_err(|_| no_such())?;
        // PDFium takes the annotation off the page but leaves its dictionary in the document, and
        // a save writes it as an object nobody refers to. What the person wrote is cleared first,
        // so that a note that was taken away does not stay in the file.
        let _ = annotation.set_contents("");
        let _ = annotation.set_creator("");
        page.annotations_mut()
            .delete_annotation(annotation)
            .map_err(refused)?;
        self.ids(page_index, before).remove(position);
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
    }
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
                    annotation.set_stroke_color(pdf_color(*color)).map_err(refused)?;
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
            annotation.set_stroke_color(pdf_color(*color)).map_err(refused)?;
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
        } => write_ink(page, document, page_index, snapshot, *color, *width, strokes),
        Recipe::Stamp { stamp, x, y } => write_stamp(page, document, snapshot, *stamp, *x, *y),
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
    (value != 0).then_some(value as FPDF_DOCUMENT)
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
            paths.push(points.into_iter().map(|(x, y)| FS_POINTF { x, y }).collect());
        }
    }
    if paths.is_empty() {
        return fail(ErrorCode::AnnotationRefused, "that place is not on the page");
    }
    let pad = width / 2.0 + 1.0;
    let rect = FS_RECTF {
        left: left - pad,
        top: top + pad,
        right: right + pad,
        bottom: bottom - pad,
    };
    let Some(handle) = raw_document(document) else {
        return fail(ErrorCode::AnnotationRefused, "the document cannot be changed");
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
            fail(ErrorCode::AnnotationRefused, "the drawing could not be written")
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
    let (Some(o), Some(u), Some(v)) = (corner(0.0, 0.0), corner(1.0, 0.0), corner(0.0, 1.0)) else {
        return fail(ErrorCode::AnnotationRefused, "that place is not on the page");
    };
    let matrix = PdfMatrix::new(u.0 - o.0, u.1 - o.1, v.0 - o.0, v.1 - o.1, o.0, o.1);
    let corners: Vec<(f32, f32)> = [(0.0, 0.0), (width, 0.0), (0.0, height), (width, height)]
        .iter()
        .filter_map(|(dx, dy)| corner(*dx, *dy))
        .collect();
    if corners.len() != 4 {
        return fail(ErrorCode::AnnotationRefused, "that place is not on the page");
    }
    let bounds = [
        corners.iter().map(|c| c.0).fold(f32::MAX, f32::min),
        corners.iter().map(|c| c.1).fold(f32::MAX, f32::min),
        corners.iter().map(|c| c.0).fold(f32::MIN, f32::max),
        corners.iter().map(|c| c.1).fold(f32::MIN, f32::max),
    ];
    let color = pdf_color(stamp_color(stamp));
    let mut annotation = page.annotations_mut().create_stamp_annotation().map_err(refused)?;
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
    let mut label = PdfPageTextObject::new(document, stamp.label(), font, PdfPoints::new(STAMP_FONT))
        .map_err(refused)?;
    label.set_fill_color(color).map_err(refused)?;
    label.apply_matrix(PdfMatrix::new(
        1.0, 0.0, 0.0, 1.0, STAMP_PAD, STAMP_PAD + 0.2 * STAMP_FONT,
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
/// the box in page space, as Acrobat writes them) and the box around all of them as
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
        let cut = clean_text(&"é".repeat(50), 11);
        assert_eq!(cut.len(), 10);
        assert!(cut.chars().all(|c| c == 'é'));
        assert_eq!(clean_text("", 10), "");
    }

    #[test]
    fn a_line_left_to_right_is_one_box() {
        let chars: Vec<TextChar> = (0..5)
            .map(|i| ch('a', [10.0 + i as f32 * 6.0, 100.0, 16.0 + i as f32 * 6.0, 112.0]))
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
            .map(|i| ch('x', [50.0, 10.0 + i as f32 * 12.0, 62.0, 22.0 + i as f32 * 12.0]))
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
