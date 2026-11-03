//! The picture of a signature: what a typed signature looks like as lines and curves, and where on
//! a page a picture goes.
//!
//! A typed signature is set in one of three bundled script fonts. The letters are not written into
//! the PDF as a font: their outlines are read here (with `skrifa`) and become filled paths of the
//! stamp, so the file carries a few hundred bytes of curves and no font at all, and every reader
//! draws the same shape. The font files are the ones the window shows its preview with.
//!
//! The text, the fonts and the outlines are the program's own, never a file's: nothing in here
//! parses a PDF.

use partridge_protocol::{SignatureFont, SignatureMark, MIN_SIGNATURE_SIDE};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, MetadataProvider};

const GREAT_VIBES: &[u8] = include_bytes!("../../../assets/fonts/signature/GreatVibes-Regular.ttf");
const ALLURA: &[u8] = include_bytes!("../../../assets/fonts/signature/Allura-Regular.ttf");
const KRISTI: &[u8] = include_bytes!("../../../assets/fonts/signature/Kristi-Regular.ttf");

/// The most a picture may be wider than tall, or taller than wide. A thinner picture could not be
/// given a legible size on a page.
pub const MAX_ASPECT: f32 = 60.0;

/// The bytes of a bundled font.
pub fn font_bytes(font: SignatureFont) -> &'static [u8] {
    match font {
        SignatureFont::GreatVibes => GREAT_VIBES,
        SignatureFont::Allura => ALLURA,
        SignatureFont::Kristi => KRISTI,
    }
}

/// One step of a path, in the units of the font, y up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seg {
    Move(f32, f32),
    Line(f32, f32),
    /// Two control points, then the end.
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

/// The letters of a text as paths, one path for each letter, and the box around all of them as
/// `[left, bottom, right, top]` in the units of the font.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedOutline {
    pub paths: Vec<Vec<Seg>>,
    pub bounds: [f32; 4],
}

/// Why a text could not be set.
#[derive(Debug, Clone, PartialEq)]
pub enum TypedError {
    /// There is nothing to draw: no text, or only spaces.
    Empty,
    /// The font has no letter for this character.
    Missing(char),
    /// The bundled font could not be read (a damaged build).
    Font,
}

struct Pen {
    path: Vec<Seg>,
    /// Where the current letter starts, in font units.
    dx: f32,
    at: (f32, f32),
    start: (f32, f32),
    bounds: Option<[f32; 4]>,
}

impl Pen {
    fn grow(&mut self, x: f32, y: f32) {
        let b = self.bounds.get_or_insert([x, y, x, y]);
        b[0] = b[0].min(x);
        b[1] = b[1].min(y);
        b[2] = b[2].max(x);
        b[3] = b[3].max(y);
    }

    fn cubic(&mut self, c1: (f32, f32), c2: (f32, f32), end: (f32, f32)) {
        let p0 = self.at;
        // The curve is walked in eight steps: the box is the box of the shape, not of its handles.
        for step in 1..8 {
            let t = step as f32 / 8.0;
            let u = 1.0 - t;
            let x = u * u * u * p0.0
                + 3.0 * u * u * t * c1.0
                + 3.0 * u * t * t * c2.0
                + t * t * t * end.0;
            let y = u * u * u * p0.1
                + 3.0 * u * u * t * c1.1
                + 3.0 * u * t * t * c2.1
                + t * t * t * end.1;
            self.grow(x, y);
        }
        self.grow(end.0, end.1);
        self.path
            .push(Seg::Cubic(c1.0, c1.1, c2.0, c2.1, end.0, end.1));
        self.at = end;
    }
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = (x + self.dx, y);
        self.grow(x, y);
        self.path.push(Seg::Move(x, y));
        self.at = (x, y);
        self.start = (x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = (x + self.dx, y);
        self.grow(x, y);
        self.path.push(Seg::Line(x, y));
        self.at = (x, y);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        // A quadratic curve is raised to the cubic curve that draws the same line.
        let (p0, q, p) = (self.at, (cx0 + self.dx, cy0), (x + self.dx, y));
        let c1 = (
            p0.0 + 2.0 / 3.0 * (q.0 - p0.0),
            p0.1 + 2.0 / 3.0 * (q.1 - p0.1),
        );
        let c2 = (p.0 + 2.0 / 3.0 * (q.0 - p.0), p.1 + 2.0 / 3.0 * (q.1 - p.1));
        self.cubic(c1, c2, p);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.cubic((cx0 + self.dx, cy0), (cx1 + self.dx, cy1), (x + self.dx, y));
    }

    fn close(&mut self) {
        self.path.push(Seg::Close);
        self.at = self.start;
    }
}

/// Sets `text` in `font`: the outline of each letter, side by side by the advance of the font.
/// No kerning and no ligatures: the window shows its preview without them too.
pub fn typed_outline(font: SignatureFont, text: &str) -> Result<TypedOutline, TypedError> {
    let face = FontRef::new(font_bytes(font)).map_err(|_| TypedError::Font)?;
    let charmap = face.charmap();
    let metrics = face.glyph_metrics(Size::unscaled(), LocationRef::default());
    let outlines = face.outline_glyphs();
    let mut pen = Pen {
        path: Vec::new(),
        dx: 0.0,
        at: (0.0, 0.0),
        start: (0.0, 0.0),
        bounds: None,
    };
    let mut paths = Vec::new();
    let mut advance = 0.0f32;
    for c in text.chars() {
        let glyph = charmap
            .map(c)
            .filter(|g| g.to_u32() != 0)
            .ok_or(TypedError::Missing(c))?;
        pen.dx = advance;
        pen.path = Vec::new();
        if let Some(outline) = outlines.get(glyph) {
            outline
                .draw(
                    DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
                    &mut pen,
                )
                .map_err(|_| TypedError::Font)?;
        }
        if !pen.path.is_empty() {
            paths.push(std::mem::take(&mut pen.path));
        }
        advance += metrics.advance_width(glyph).unwrap_or(0.0);
    }
    let bounds = pen.bounds.ok_or(TypedError::Empty)?;
    if paths.is_empty() || !(bounds[2] > bounds[0] && bounds[3] > bounds[1]) {
        return Err(TypedError::Empty);
    }
    Ok(TypedOutline { paths, bounds })
}

/// The proportions of a picture, width over height, or `None` when it cannot be drawn.
pub fn aspect_of(mark: &SignatureMark) -> Result<f32, String> {
    let aspect = match mark {
        SignatureMark::Strokes { width, height, .. } => width / height,
        SignatureMark::Typed { font, text } => {
            let outline = typed_outline(*font, text).map_err(|e| match e {
                TypedError::Empty => "there is nothing to draw in that text".to_string(),
                TypedError::Missing(c) => format!("the font has no letter for {c:?}"),
                TypedError::Font => "the font could not be read".to_string(),
            })?;
            (outline.bounds[2] - outline.bounds[0]) / (outline.bounds[3] - outline.bounds[1])
        }
        SignatureMark::Image { width, height, .. } => *width as f32 / *height as f32,
    };
    if aspect.is_finite() && (1.0 / MAX_ASPECT..=MAX_ASPECT).contains(&aspect) {
        Ok(aspect)
    } else {
        Err("that picture is too long and thin to be placed".to_string())
    }
}

/// The box a new picture takes: it is fitted whole into `want`, with its proportions, centred in
/// it, and kept on the page. Boxes are `[left, top, right, bottom]` in points on the page as it
/// is drawn. `None` when no box of at least the smallest size fits.
pub fn fit_box(aspect: f32, want: [f32; 4], page: (f32, f32)) -> Option<[f32; 4]> {
    let (rw, rh) = (want[2] - want[0], want[3] - want[1]);
    if !(rw > 0.0 && rh > 0.0 && aspect > 0.0 && page.0 > 0.0 && page.1 > 0.0) {
        return None;
    }
    let (mut w, mut h) = if rw / rh > aspect {
        (rh * aspect, rh)
    } else {
        (rw, rw / aspect)
    };
    if w > page.0 {
        w = page.0;
        h = w / aspect;
    }
    if h > page.1 {
        h = page.1;
        w = h * aspect;
    }
    if w < MIN_SIGNATURE_SIDE || h < MIN_SIGNATURE_SIDE {
        return None;
    }
    let x = (want[0] + (rw - w) / 2.0).clamp(0.0, (page.0 - w).max(0.0));
    let y = (want[1] + (rh - h) / 2.0).clamp(0.0, (page.1 - h).max(0.0));
    Some([x, y, x + w, y + h])
}

/// The box a picture takes when it is moved and resized: the top left corner and the width of
/// `want`, with the proportions the picture has. Too small a width is made the smallest that
/// works, too big a one the biggest the page holds.
pub fn place_box(aspect: f32, want: [f32; 4], page: (f32, f32)) -> Option<[f32; 4]> {
    if !(aspect > 0.0 && page.0 > 0.0 && page.1 > 0.0 && want.iter().all(|v| v.is_finite())) {
        return None;
    }
    let widest = page.0.min(page.1 * aspect);
    let narrowest = MIN_SIGNATURE_SIDE.max(MIN_SIGNATURE_SIDE * aspect);
    if narrowest > widest {
        return None;
    }
    let w = (want[2] - want[0]).abs().clamp(narrowest, widest);
    let h = w / aspect;
    let x = want[0].clamp(0.0, (page.0 - w).max(0.0));
    let y = want[1].clamp(0.0, (page.1 - h).max(0.0));
    Some([x, y, x + w, y + h])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_font_sets_a_name_as_curves_with_a_box() {
        for font in SignatureFont::ALL {
            let out = typed_outline(font, "Anna Alvorlig").unwrap();
            // One path for each letter that has an outline: the space has none.
            assert_eq!(out.paths.len(), 12, "{font:?}");
            assert!(out.bounds[2] > out.bounds[0] && out.bounds[3] > out.bounds[1]);
            assert!(out
                .paths
                .iter()
                .flatten()
                .any(|s| matches!(s, Seg::Cubic(..))));
            assert!(out.paths.iter().all(|p| matches!(p[0], Seg::Move(..))));
            // The letters stand side by side: later letters are to the right of earlier ones.
            let first = out.paths.first().unwrap();
            let last = out.paths.last().unwrap();
            let x_of = |p: &Vec<Seg>| match p[0] {
                Seg::Move(x, _) => x,
                _ => 0.0,
            };
            assert!(x_of(last) > x_of(first), "{font:?}");
        }
    }

    #[test]
    fn the_box_holds_every_point_of_every_letter() {
        let out = typed_outline(SignatureFont::GreatVibes, "Hanne").unwrap();
        for seg in out.paths.iter().flatten() {
            let ends = match *seg {
                Seg::Move(x, y) | Seg::Line(x, y) => vec![(x, y)],
                Seg::Cubic(_, _, _, _, x, y) => vec![(x, y)],
                Seg::Close => vec![],
            };
            for (x, y) in ends {
                assert!(
                    x >= out.bounds[0] - 0.01 && x <= out.bounds[2] + 0.01,
                    "x {x}"
                );
                assert!(
                    y >= out.bounds[1] - 0.01 && y <= out.bounds[3] + 0.01,
                    "y {y}"
                );
            }
        }
    }

    #[test]
    fn letters_of_the_subset_work_and_others_are_refused() {
        for text in [
            "\u{e6}\u{f8}\u{e5} \u{c6}\u{d8}\u{c5}",
            "Zo\u{eb} Mu\u{f1}oz",
            "O'Neil-Smith",
        ] {
            assert!(typed_outline(SignatureFont::Allura, text).is_ok(), "{text}");
        }
        assert_eq!(
            typed_outline(SignatureFont::Kristi, "A\u{65e5}"),
            Err(TypedError::Missing('\u{65e5}'))
        );
        assert_eq!(
            typed_outline(SignatureFont::Kristi, "\u{1F600}"),
            Err(TypedError::Missing('\u{1F600}'))
        );
    }

    #[test]
    fn nothing_to_draw_is_refused() {
        assert_eq!(
            typed_outline(SignatureFont::GreatVibes, ""),
            Err(TypedError::Empty)
        );
        assert_eq!(
            typed_outline(SignatureFont::GreatVibes, "   "),
            Err(TypedError::Empty)
        );
    }

    #[test]
    fn the_proportions_of_a_picture_are_its_own() {
        let strokes = SignatureMark::Strokes {
            width: 300.0,
            height: 100.0,
            line: 2.0,
            strokes: vec![vec![[1.0, 1.0]]],
        };
        assert_eq!(aspect_of(&strokes).unwrap(), 3.0);
        let image = SignatureMark::Image {
            width: 400,
            height: 100,
            rgba: vec![0; 400 * 100 * 4],
        };
        assert_eq!(aspect_of(&image).unwrap(), 4.0);
        let typed = SignatureMark::Typed {
            font: SignatureFont::GreatVibes,
            text: "Anna".into(),
        };
        assert!(aspect_of(&typed).unwrap() > 1.0);
        let thin = SignatureMark::Image {
            width: 1_024,
            height: 4,
            rgba: vec![0; 1_024 * 4 * 4],
        };
        assert!(aspect_of(&thin).is_err());
        let missing = SignatureMark::Typed {
            font: SignatureFont::GreatVibes,
            text: "\u{65e5}".into(),
        };
        assert!(aspect_of(&missing).unwrap_err().contains("no letter"));
    }

    #[test]
    fn a_new_picture_is_fitted_whole_into_the_box_and_kept_on_the_page() {
        let page = (600.0, 800.0);
        // Wider than the box allows: the width rules, the picture sits in the middle.
        let b = fit_box(3.0, [100.0, 100.0, 220.0, 200.0], page).unwrap();
        assert!((b[2] - b[0] - 120.0).abs() < 0.01 && (b[3] - b[1] - 40.0).abs() < 0.01);
        assert!((b[1] - 130.0).abs() < 0.01);
        // Taller than the box allows: the height rules.
        let b = fit_box(1.0, [100.0, 100.0, 300.0, 150.0], page).unwrap();
        assert!((b[2] - b[0] - 50.0).abs() < 0.01);
        assert!((b[0] - 175.0).abs() < 0.01);
        // Hanging off the page: pulled back.
        let b = fit_box(2.0, [550.0, 780.0, 700.0, 900.0], page).unwrap();
        assert!(b[2] <= 600.0 + 0.01 && b[3] <= 800.0 + 0.01 && b[0] >= 0.0 && b[1] >= 0.0);
        // Bigger than the page: made to fit.
        let b = fit_box(1.0, [-500.0, -500.0, 5000.0, 5000.0], page).unwrap();
        assert!(b[2] - b[0] <= 600.0 + 0.01 && b[3] - b[1] <= 800.0 + 0.01);
        // Too small, or not a box.
        assert!(fit_box(1.0, [0.0, 0.0, 7.0, 7.0], page).is_none());
        assert!(fit_box(1.0, [10.0, 10.0, 10.0, 50.0], page).is_none());
        assert!(fit_box(1.0, [10.0, 10.0, f32::NAN, 50.0], page).is_none());
    }

    #[test]
    fn moving_and_resizing_keeps_the_proportions() {
        let page = (600.0, 800.0);
        let b = place_box(2.0, [50.0, 60.0, 250.0, 70.0], page).unwrap();
        // The width of the box, whatever its height says.
        assert!((b[2] - b[0] - 200.0).abs() < 0.01 && (b[3] - b[1] - 100.0).abs() < 0.01);
        assert_eq!((b[0], b[1]), (50.0, 60.0));
        // Too small: the smallest that works. Too big: the biggest the page holds.
        let b = place_box(2.0, [0.0, 0.0, 1.0, 1.0], page).unwrap();
        assert!(b[3] - b[1] >= MIN_SIGNATURE_SIDE - 0.001);
        let b = place_box(2.0, [0.0, 0.0, 9999.0, 9999.0], page).unwrap();
        assert!((b[2] - b[0] - 600.0).abs() < 0.01);
        let b = place_box(0.2, [0.0, 0.0, 9999.0, 9999.0], page).unwrap();
        assert!((b[3] - b[1] - 800.0).abs() < 0.01);
        // Kept on the page.
        let b = place_box(2.0, [500.0, 790.0, 700.0, 900.0], page).unwrap();
        assert!(b[2] <= 600.0 + 0.01 && b[3] <= 800.0 + 0.01);
        assert!(place_box(2.0, [f32::NAN, 0.0, 1.0, 1.0], page).is_none());
    }
}
