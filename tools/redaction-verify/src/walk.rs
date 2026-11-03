//! The content interpreter of the region check: one walk over the content of a page, with a
//! complete graphics state, that turns everything that paints into events with a bounding box on
//! the page.
//!
//! The rule above all others: what the walk does not fully understand is "could not check", never
//! a pass. It does not try to model every quirk of a reader; it refuses to judge. What it
//! understands is the strict profile of [`crate::profile`]: operators with exactly the operands of
//! the specification, matrices and boxes of exactly six or four numbers, a short list of fonts
//! (the standard ones with the standard metrics, fonts whose program is in the file and agrees
//! with their widths), horizontal writing, and strokes with a miter limit of at most 10.
//!
//! The graphics state has the stack of `q` and `Q` (the matrix, the clip, the colours, the line, the
//! dash pattern, the miter limit, and the text state: font, size, character and word spacing,
//! horizontal scale, leading, rise and render mode), the text and line matrices of `BT .. ET`, and
//! the path that is being built. The interpreter follows forms (with `/Matrix` and `/BBox`), the
//! groups of soft masks, annotation appearances and the glyph procedures of Type 3 fonts, and reads
//! inline images. A tiling pattern is not followed: a fill with a pattern is the fill, and what the
//! pattern draws is looked at only for images. It does not depend on what is visible: a render mode
//! that draws nothing, optional content that is off, a transparency of 0, a white colour and a clip
//! that cuts everything away still count.
//!
//! Every event (a glyph of any font, the fill or stroke of a path, a clip path, a shading, and what
//! a Type 3 glyph paints) goes to one place, [`Walker::emit`], which tests it against the regions of
//! the page when it happens and keeps what meets one. So nothing is dropped when a page has a lot
//! of content. A page that needs more steps or more time than the budget, and the work that fonts,
//! patterns, font programs and glyph procedures need when they are read, is "could not check".
//!
//! The glyph rule, which the redaction engine must follow: the cell of a glyph is its advance box
//! across (from the origin by the width of the glyph times the size and `Tz`) and the ascent and
//! descent of its font up and down (within the box of the font program), moved by the text matrix
//! and the matrix of the page; the event is the bounds of the cell on the page. A glyph is under a
//! region when its cell overlaps the region with positive area (more than [`GLYPH_OVERLAP`] along
//! each axis), in every spec. A glyph whose width is not known (a code of a standard font that the
//! tables do not have) is an interval of places, and the widest cell it can have counts.
//!
//! Some geometry carries no information about what was under a region, and is allowed:
//! - the box that redacts: a fill of a single rectangle that is the region (within a pixel of the
//!   render check) in the colour that the region names;
//! - a fill of a single rectangle in one device colour that contains the whole region (a page
//!   background, the shading of a table cell);
//! - a straight horizontal or vertical line, or a thin rectangle, at most 2 points thick, that
//!   crosses the whole region from one side to the other (the rules of a table), when it is solid,
//!   has no constant alpha or blend mode and no clip ends at the region on its own axis;
//! - a clip path that is one rectangle that contains the whole region.
//!
//! All of these must be painted with no clip that cuts into the region and with no soft mask,
//! because a fill that a shape clips shows the shape.

use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::content::{
    apply, for_each_op_while, mul, shown, Matrix, Op, Operand, ShownItem, IDENTITY,
};
use crate::fontprog::Program;
use crate::pdfobj::{Doc, Entry, Obj};
use crate::profile::{self, exact_numbers};
use crate::stdfonts::Std;
use crate::{RENDER_DPI, RENDER_MARGIN};

pub type M = Matrix;

/// How far a painted rectangle may be from the region and still be the box that redacted it: the
/// edge that the render check leaves out, in points.
pub const BOX_TOLERANCE: f64 = RENDER_MARGIN as f64 * 72.0 / RENDER_DPI as f64;

/// The thickest rule that carries no information about what it crosses, in points.
pub const THIN: f64 = 2.0;

/// A clip that cuts nothing.
const UNBOUNDED: [f64; 4] = [f64::MIN, f64::MIN, f64::MAX, f64::MAX];

/// The deepest nesting of `q`.
const MAX_STACK: usize = 4096;

/// What the walk may use: steps (operators and glyphs) and time.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub steps_left: u64,
    pub deadline: Instant,
    pub steps_max: u64,
    pub time_max: Duration,
}

impl Budget {
    pub fn new(steps: u64, time: Duration) -> Budget {
        Budget {
            steps_left: steps,
            deadline: Instant::now() + time,
            steps_max: steps,
            time_max: time,
        }
    }
}

pub fn bounds(points: &[(f64, f64)]) -> [f64; 4] {
    let (x0, x1) = points
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p.0), h.max(p.0)));
    let (y0, y1) = points
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p.1), h.max(p.1)));
    [x0, y0, x1, y1]
}

/// A `/Matrix`: exactly six finite numbers (see [`crate::profile`]); anything else is `None`.
pub fn matrix_of(o: &Obj, doc: &Doc) -> Option<M> {
    exact_numbers(o, doc, 6).map(|v| [v[0], v[1], v[2], v[3], v[4], v[5]])
}

/// Whether two rectangles overlap (they do not when they only touch).
pub fn overlaps(a: &[f64; 4], b: &[f64; 4]) -> bool {
    a[2] > b[0] && a[0] < b[2] && a[3] > b[1] && a[1] < b[3]
}

/// The least that the cell of a glyph must reach into a region, along each axis, to overlap it
/// with positive area, in points. Positions are sums of widths in floating point, and a region is
/// a rounded number: a cell that ends where the region starts, to within this, does not overlap.
pub const GLYPH_OVERLAP: f64 = 0.01;

/// Whether the cell of a glyph and a region overlap with positive area (see [`GLYPH_OVERLAP`]).
fn cell_overlaps(cell: &[f64; 4], region: &[f64; 4]) -> bool {
    cell[2].min(region[2]) - cell[0].max(region[0]) > GLYPH_OVERLAP
        && cell[3].min(region[3]) - cell[1].max(region[1]) > GLYPH_OVERLAP
}

/// Whether `outer` holds `inner`, give or take `tol`.
pub fn holds(outer: &[f64; 4], inner: &[f64; 4], tol: f64) -> bool {
    outer[0] <= inner[0] + tol
        && outer[1] <= inner[1] + tol
        && outer[2] >= inner[2] - tol
        && outer[3] >= inner[3] - tol
}

/// An image that is drawn, with the matrix that maps its unit square to the page.
pub struct Placement {
    pub ctm: M,
    /// The image object, or the dictionary text and data of an inline image.
    pub image: Source,
}

pub enum Source {
    Object(u32),
    Inline(Vec<u8>, Vec<u8>),
}

/// A colour that was set, as far as the check needs to know it: a device colour, or something
/// else (a pattern, another colour space).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Colour {
    Rgb([u8; 3]),
    Other,
}

fn to_byte(v: f64) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The colour that these operands of `sc`, `scn`, `g`, `rg` or `k` give in a device colour space.
fn colour_of(n: &[f64]) -> Colour {
    match n.len() {
        1 => {
            let g = to_byte(n[0]);
            Colour::Rgb([g, g, g])
        }
        3 => Colour::Rgb([to_byte(n[0]), to_byte(n[1]), to_byte(n[2])]),
        4 => {
            let k = 1.0 - n[3].clamp(0.0, 1.0);
            Colour::Rgb([
                to_byte((1.0 - n[0].clamp(0.0, 1.0)) * k),
                to_byte((1.0 - n[1].clamp(0.0, 1.0)) * k),
                to_byte((1.0 - n[2].clamp(0.0, 1.0)) * k),
            ])
        }
        _ => Colour::Other,
    }
}

fn axis_aligned(m: &M) -> bool {
    let tiny = |v: f64| v.abs() <= 1e-9 * (m[0].abs() + m[1].abs() + m[2].abs() + m[3].abs());
    (tiny(m[1]) && tiny(m[2])) || (tiny(m[0]) && tiny(m[3]))
}

/// A rectangle on the page that something is checked against.
#[derive(Debug, Clone)]
pub struct Target {
    pub rect: [f64; 4],
    pub fill: Option<[u8; 3]>,
}

/// What met a target.
#[derive(Debug, Default)]
pub struct Found {
    pub count: usize,
    pub kinds: BTreeSet<&'static str>,
    /// Where the first one is: the place in the file and the bounds on the page.
    pub first: Option<(String, [f64; 4])>,
}

/// What kind of painting an event is.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Class {
    Fill,
    Stroke,
    FillStroke,
    Shading,
    Clip,
    /// A glyph of any font.
    Glyph,
}

/// The shape of a painted path, as far as the exceptions need it.
#[derive(Debug, Clone, Copy)]
enum Shape {
    Other,
    /// One axis-aligned rectangle (`re`), as it lies on the page.
    Rect([f64; 4]),
    /// One straight segment (`m`, `l`), its two ends on the page.
    Line([f64; 4]),
}

struct Event {
    class: Class,
    bbox: [f64; 4],
    shape: Shape,
    kind: &'static str,
    /// The fill and stroke colour of the state.
    fill: Colour,
    stroke: Colour,
    /// The width of the line on the page, for a stroke.
    width: f64,
    clip: ([f64; 4], bool),
    smask: bool,
    /// The stroke has a dash pattern: it puts ink only where the pattern says.
    dashed: bool,
    /// The state has a constant alpha below 1 or a blend mode other than Normal.
    translucent: bool,
    /// Why the event is outside the strict profile (see [`crate::profile`]): near a region, it is
    /// "could not check".
    unsupported: Option<&'static str>,
}

/// The graphics state.
#[derive(Clone)]
struct Gs {
    ctm: M,
    line_width: f64,
    miter: f64,
    fill: Colour,
    stroke: Colour,
    /// The bounds of the clip on the page, and whether the clip is exactly that rectangle.
    clip_box: [f64; 4],
    clip_exact: bool,
    smask: bool,
    /// The dash pattern is not solid (`d`, or `/D` of a state).
    dashed: bool,
    /// A constant alpha below 1 (`/ca`, `/CA`) or a blend mode other than Normal (`/BM`).
    translucent: bool,
    // The text state.
    font: Option<Rc<Font>>,
    size: f64,
    char_space: f64,
    word_space: f64,
    scale: f64,
    leading: f64,
    rise: f64,
    render: i64,
}

impl Gs {
    fn new(ctm: M, clip_box: [f64; 4]) -> Gs {
        Gs {
            ctm,
            line_width: 1.0,
            miter: 10.0,
            fill: Colour::Rgb([0, 0, 0]),
            stroke: Colour::Rgb([0, 0, 0]),
            clip_box,
            clip_exact: true,
            smask: false,
            dashed: false,
            translucent: false,
            font: None,
            size: 0.0,
            char_space: 0.0,
            word_space: 0.0,
            scale: 1.0,
            leading: 0.0,
            rise: 0.0,
            render: 0,
        }
    }
}

/// The path that is being built.
#[derive(Default)]
struct PathState {
    pts: Vec<(f64, f64)>,
    rects: usize,
    moves: usize,
    lines: usize,
    curves: usize,
    /// The straight segments of the path on the page (the sides of rectangles and the closing
    /// segments too), as `[x0, y0, x1, y1]`. Curves have none: their bounds are those of their
    /// control points.
    segs: Vec<[f64; 4]>,
    cur: Option<(f64, f64)>,
    start: Option<(f64, f64)>,
    /// The subpath that is open has a segment (a subpath of one point still paints a dot with
    /// round or square caps).
    sub_segs: bool,
}

impl PathState {
    fn line_to(&mut self, p: (f64, f64)) {
        if let Some(c) = self.cur {
            self.segs.push([c.0, c.1, p.0, p.1]);
        }
        self.cur = Some(p);
        self.sub_segs = true;
    }

    /// `m`: a new subpath. The one before it, if it had no segment, is a point.
    fn move_to(&mut self, p: (f64, f64)) {
        self.finish_subpath();
        self.cur = Some(p);
        self.start = Some(p);
        self.sub_segs = false;
    }

    /// The open subpath ends: a subpath with no segment is a zero-length segment, because a
    /// stroke with round or square caps paints a dot there.
    fn finish_subpath(&mut self) {
        if let (Some(s), false) = (self.start, self.sub_segs) {
            self.segs.push([s.0, s.1, s.0, s.1]);
            self.sub_segs = true;
        }
    }
}

impl PathState {
    /// The rectangle that the path is, if it is one: one `re`, or one `m` with three or four `l`
    /// that go round a rectangle, and no curve.
    fn as_rect(&self) -> Option<[f64; 4]> {
        if self.curves != 0 {
            return None;
        }
        let one_re = self.rects == 1 && self.moves == 0 && self.lines == 0;
        let polyline = self.rects == 0 && self.moves == 1 && (self.lines == 3 || self.lines == 4);
        if one_re || polyline {
            rect_of(&self.pts)
        } else {
            None
        }
    }
}

/// `h`: a segment back to where the subpath started.
fn close_path(p: &mut PathState) {
    if let (Some(c), Some(s)) = (p.cur, p.start) {
        if c != s {
            p.segs.push([c.0, c.1, s.0, s.1]);
            p.sub_segs = true;
        }
        p.cur = Some(s);
    }
}

/// The rectangle that these points are the corners of, if they are: every point at a corner of
/// their bounds, and (unless the rectangle is flat) every corner there.
fn rect_of(pts: &[(f64, f64)]) -> Option<[f64; 4]> {
    let b = bounds(pts);
    if b.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let eps = 1e-6 * (1.0 + (b[2] - b[0]).abs() + (b[3] - b[1]).abs());
    let at = |v: f64, lo: f64, hi: f64| (v - lo).abs() <= eps || (v - hi).abs() <= eps;
    if !pts
        .iter()
        .all(|p| at(p.0, b[0], b[2]) && at(p.1, b[1], b[3]))
    {
        return None;
    }
    let has = |x: f64, y: f64| {
        pts.iter()
            .any(|p| (p.0 - x).abs() <= eps && (p.1 - y).abs() <= eps)
    };
    (has(b[0], b[1]) && has(b[2], b[1]) && has(b[0], b[3]) && has(b[2], b[3])).then_some(b)
}

/// Whether the segment meets the rectangle (touching counts).
fn segment_meets(s: &[f64; 4], r: &[f64; 4]) -> bool {
    let (dx, dy) = (s[2] - s[0], s[3] - s[1]);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [
        (-dx, s[0] - r[0]),
        (dx, r[2] - s[0]),
        (-dy, s[1] - r[1]),
        (dy, r[3] - s[1]),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

/// The text object: the text matrix and the line matrix, and how far the position has moved
/// along the line since the matrix was set, as an interval because the width of a glyph of a
/// standard font is only known to the tolerance of the tables, and not at all for some codes.
struct Text {
    tm: M,
    tlm: M,
    ax: [f64; 2],
}

impl Text {
    fn new() -> Text {
        Text {
            tm: IDENTITY,
            tlm: IDENTITY,
            ax: [0.0; 2],
        }
    }

    fn set(&mut self, m: M) {
        self.tlm = m;
        self.tm = m;
        self.ax = [0.0; 2];
    }

    fn move_by(&mut self, tx: f64, ty: f64) {
        let m = mul(&[1.0, 0.0, 0.0, 1.0, tx, ty], &self.tlm);
        self.set(m);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum FontKind {
    Simple,
    Cid,
    Type3,
}

/// How the codes of a composite font reach the glyphs of its TrueType program.
enum CidToGid {
    Identity,
    Map(Vec<u16>),
}

/// A font, as far as the position and the size of its glyphs need. All sizes are in em: the units
/// of the text space for a font size of 1.
pub struct Font {
    kind: FontKind,
    /// Type 3: the glyph procedure of each code that has one (object numbers), the matrix of the
    /// glyph space, and the resources of the font.
    procs: HashMap<u32, u32>,
    t3_matrix: M,
    t3_res: Option<Obj>,
    /// Why text shown with this font is outside the strict profile: it cannot be placed.
    problem: Option<String>,
    /// The vertical extent of the cell of a glyph, in em: from below the baseline to above it.
    cell_y: [f64; 2],
    first: i64,
    last: i64,
    widths: Vec<f64>,
    have_widths: bool,
    dw: f64,
    w_map: HashMap<u32, f64>,
    /// Composite font with `Identity-H`: every code is two bytes. A string of any other font is
    /// read byte by byte.
    two_byte: bool,
    /// A standard font that the file does not embed: its widths are the ones of the tables, and
    /// whether its encoding is `StandardEncoding`.
    std: Option<Std>,
    std_standard: bool,
    /// The embedded program (the advances of a TrueType program are compared with the widths of
    /// the file), and how a composite font reaches its glyphs.
    program: Option<Rc<Program>>,
    cid_to_gid: CidToGid,
}

/// The narrowest and widest advance that a glyph of a standard font that has no widths may have, in
/// em (the widths that a viewer takes from its own copy of the font).
const ADVANCE_RANGE: [f64; 2] = [0.15, 1.05];

/// How far an advance of a TrueType program may be from the width in the file.
fn advance_tolerance(w: f64) -> f64 {
    (profile::ADVANCE_TOLERANCE * w.abs()).max(profile::ADVANCE_ABS)
}

impl Font {
    fn plain() -> Font {
        Font {
            kind: FontKind::Simple,
            procs: HashMap::new(),
            t3_matrix: [0.001, 0.0, 0.0, 0.001, 0.0, 0.0],
            t3_res: None,
            problem: None,
            cell_y: crate::stdfonts::CELL_Y,
            first: 0,
            last: -1,
            widths: Vec::new(),
            have_widths: false,
            dw: 1.0,
            w_map: HashMap::new(),
            two_byte: false,
            std: None,
            std_standard: false,
            program: None,
            cid_to_gid: CidToGid::Identity,
        }
    }

    /// A font that the strict profile does not take: text shown with it is "could not check".
    fn refused(why: String) -> Font {
        Font {
            problem: Some(why),
            ..Font::plain()
        }
    }

    /// The declared width of a code of a simple or Type 3 font, if it is in `/Widths`.
    fn declared(&self, code: u32) -> Option<f64> {
        if i64::from(code) > self.last {
            return None;
        }
        let i = usize::try_from(i64::from(code) - self.first).ok()?;
        self.widths.get(i).copied()
    }

    /// The width of the glyph for a code, in em: `Ok(Some(w))` when it is known, `Ok(None)` when
    /// a standard font has no width for it in the tables (the glyph is placed by an interval), and
    /// an error when the width is outside the strict profile.
    fn width(&self, code: u32, whole_code: bool) -> Result<Option<f64>, String> {
        match self.kind {
            FontKind::Cid => {
                if !(self.two_byte && whole_code) {
                    return Err(
                        "a string of the composite font ends in the middle of a code".to_owned(),
                    );
                }
                let w = self.w_map.get(&code).copied().unwrap_or(self.dw);
                if !(profile::MIN_WIDTH..=profile::MAX_WIDTH).contains(&w) {
                    return Err(format!(
                        "the width {w} em of the code {code} is not a width"
                    ));
                }
                if let Some(adv) = self.program.as_ref().and_then(|p| p.advances.as_ref()) {
                    let gid = match &self.cid_to_gid {
                        CidToGid::Identity => Some(code as usize),
                        CidToGid::Map(m) => m.get(code as usize).map(|g| usize::from(*g)),
                    };
                    let a = gid.and_then(|g| adv.get(g)).ok_or_else(|| {
                        format!("the code {code} has no glyph in the font program")
                    })?;
                    if (w - a).abs() > advance_tolerance(*a) {
                        return Err(format!(
                            "the width {w} em of the code {code} is not the advance {a:.3} em of its glyph in the font program"
                        ));
                    }
                    // The glyph is where its advance box is: its ink does not reach far from it.
                    let ink = gid.and_then(|g| self.program.as_ref().and_then(|p| p.ink_x(g)));
                    if let Some([x0, x1]) = ink {
                        if x0 < -profile::INK_SLACK || x1 > w + profile::INK_SLACK {
                            return Err(format!(
                                "the ink of the glyph of the code {code} reaches from {x0:.2} em to {x1:.2} em, more than {} em beyond its advance box of {w} em",
                                profile::INK_SLACK
                            ));
                        }
                    }
                }
                Ok(Some(w))
            }
            FontKind::Type3 => self
                .declared(code)
                .map(Some)
                .ok_or_else(|| format!("the code {code} is outside /FirstChar../LastChar")),
            FontKind::Simple => {
                let declared = self.declared(code);
                if let Some(std) = self.std {
                    if !self.have_widths {
                        return Ok(std.width(code, self.std_standard));
                    }
                    // Widths in the file were checked against the tables for the codes 32 to 126
                    // when the font was read; a code outside them cannot be checked.
                    return match declared {
                        Some(w) if (32..=126).contains(&code) => Ok(Some(w)),
                        _ => Err(format!(
                            "the code {code} of a standard font with /Widths is outside the codes 32 to 126 that the standard metrics check"
                        )),
                    };
                }
                let Some(w) = declared else {
                    return Err(format!("the code {code} is outside /FirstChar../LastChar"));
                };
                let least = if code == 32 { 0.0 } else { profile::MIN_WIDTH };
                if !(least..=profile::MAX_WIDTH).contains(&w) {
                    return Err(format!(
                        "the width {w} em of the code {code} is not a width"
                    ));
                }
                let advance_ok = self.program.as_ref().is_none_or(|p| {
                    w == 0.0 || p.advances.is_none() || p.has_advance_near(w, advance_tolerance)
                });
                if !advance_ok {
                    return Err(format!(
                        "the width {w} em of the code {code} is not the advance of any glyph of the font program"
                    ));
                }
                Ok(Some(w))
            }
        }
    }

    /// The codes of a shown string: the code, and whether it is a whole code of the font.
    fn codes(&self, bytes: &[u8]) -> Vec<(u32, bool)> {
        if self.kind == FontKind::Cid && self.two_byte {
            bytes
                .chunks(2)
                .map(|c| {
                    if c.len() == 2 {
                        (u32::from(c[0]) << 8 | u32::from(c[1]), true)
                    } else {
                        (u32::from(c[0]), false)
                    }
                })
                .collect()
        } else {
            bytes.iter().map(|b| (u32::from(*b), false)).collect()
        }
    }
}

pub struct Walker<'a, 'd> {
    doc: &'a Doc<'d>,
    pub targets: Vec<Target>,
    /// What met each target.
    pub found: Vec<Found>,
    pub out: Vec<Placement>,
    pub problems: Vec<String>,
    /// The bounds of the page, form, pattern or glyph that is being walked, in points: nothing
    /// that it paints is outside.
    container: [f64; 4],
    /// Where in the file the walk is, for the places in messages.
    trail: Vec<String>,
    fonts: HashMap<u32, Rc<Font>>,
    /// The embedded font programs read so far, by object number.
    programs: HashMap<u32, Result<Rc<Program>, String>>,
    /// The decoded glyph procedures of Type 3 fonts, by object number.
    procs: HashMap<u32, Option<Rc<Vec<u8>>>>,
    /// Whether a tiling pattern draws an image, and why its content is outside the strict
    /// profile, by object number.
    patterns: HashMap<u32, (bool, Option<String>)>,
    /// Annotations that a viewer draws from their own entries (an ink path, a line) because they
    /// have no appearance stream: their rectangle and subtype.
    pub bare: Vec<([f64; 4], String)>,
    /// Annotations with no appearance stream that show text from their entries (`/Contents`,
    /// `/RC`, `/V`): their rectangle and subtype.
    pub bare_text: Vec<([f64; 4], String)>,
    pub budget: Budget,
    ticks: u64,
    aborted: bool,
}

/// The state of one stream that is being interpreted.
struct Frame<'r> {
    res: &'r Obj,
    depth: usize,
    gs: Gs,
    stack: Vec<Gs>,
    path: PathState,
    text: Text,
    pending_clip: bool,
    text_clip: bool,
    /// How deep the compatibility sections (`BX` .. `EX`) are: an unknown operator is allowed in
    /// one.
    compat: usize,
}

impl<'a, 'd> Walker<'a, 'd> {
    pub fn new(
        doc: &'a Doc<'d>,
        container: [f64; 4],
        targets: Vec<Target>,
        budget: Budget,
    ) -> Self {
        let found = targets.iter().map(|_| Found::default()).collect();
        Walker {
            doc,
            targets,
            found,
            out: Vec::new(),
            problems: Vec::new(),
            container,
            trail: Vec::new(),
            fonts: HashMap::new(),
            programs: HashMap::new(),
            procs: HashMap::new(),
            patterns: HashMap::new(),
            bare: Vec::new(),
            bare_text: Vec::new(),
            budget,
            ticks: 0,
            aborted: false,
        }
    }

    /// The document that is walked.
    pub fn doc(&self) -> &'a Doc<'d> {
        self.doc
    }

    pub fn problem(&mut self, msg: String) {
        if self.problems.len() < 100 && !self.problems.contains(&msg) {
            self.problems.push(msg);
        }
    }

    fn place(&self) -> String {
        if self.trail.is_empty() {
            "the page content".to_owned()
        } else {
            self.trail.join(", in ")
        }
    }

    /// Spends `n` steps. False when the budget is used up (and the walk must stop).
    fn tick(&mut self, n: u64) -> bool {
        if self.aborted {
            return false;
        }
        if self.budget.steps_left < n {
            self.stop("steps");
            return false;
        }
        self.budget.steps_left -= n;
        self.ticks += n;
        if self.ticks >= 4096 {
            self.ticks = 0;
            if Instant::now() >= self.budget.deadline {
                self.stop("time");
                return false;
            }
        }
        true
    }

    fn stop(&mut self, what: &str) {
        self.aborted = true;
        let limit = if what == "steps" {
            format!("{} steps", self.budget.steps_max)
        } else {
            format!("{} s", self.budget.time_max.as_secs_f64())
        };
        self.problem(format!(
            "the walk over the content of the page used up its budget of {limit} (--walk-steps and --walk-budget set it), so the rest of the content was not checked"
        ));
    }

    /// Whether `e` is geometry that carries nothing about the region `t`.
    fn is_free(&self, e: &Event, t: &Target) -> bool {
        match e.class {
            // A glyph counts by its cell (see emit_segs), in every spec.
            Class::Glyph => false,
            Class::Shading => false,
            Class::Clip => matches!(e.shape, Shape::Rect(r) if holds(&r, &t.rect, BOX_TOLERANCE)),
            Class::FillStroke => false,
            Class::Fill | Class::Stroke => {
                // A clip or a soft mask that cuts into the region makes the fill the shape of it.
                if e.smask || !e.clip.1 || !holds(&e.clip.0, &t.rect, BOX_TOLERANCE) {
                    return false;
                }
                match (e.class, e.shape) {
                    (Class::Fill, Shape::Rect(r)) => {
                        let Colour::Rgb(c) = e.fill else { return false };
                        let same_place = r
                            .iter()
                            .zip(&t.rect)
                            .all(|(a, b)| (a - b).abs() <= BOX_TOLERANCE);
                        let is_box = same_place
                            && t.fill
                                .is_none_or(|f| c.iter().zip(&f).all(|(a, b)| a.abs_diff(*b) <= 2));
                        // One colour over the whole region, or a thin strip across it.
                        let covers = holds(&r, &t.rect, BOX_TOLERANCE);
                        let strip = crosses_thin_rect(&r, &t.rect)
                            .is_some_and(|horizontal| rule_is_plain(e, &t.rect, horizontal));
                        is_box || covers || strip
                    }
                    (Class::Stroke, Shape::Line(l)) => {
                        matches!(e.stroke, Colour::Rgb(_))
                            && e.width <= THIN
                            && !e.dashed
                            && crosses_line(&l, &t.rect)
                                .is_some_and(|horizontal| rule_is_plain(e, &t.rect, horizontal))
                    }
                    _ => false,
                }
            }
        }
    }

    /// One painting event: tested against the targets now, so that nothing is dropped.
    fn emit(&mut self, e: Event) {
        self.emit_segs(e, &[], 0.0);
    }

    /// The same for the stroke of straight segments: a segment that is not within `reach` of a
    /// target leaves it alone, whatever the bounds of the whole path say.
    fn emit_segs(&mut self, e: Event, segs: &[[f64; 4]], reach: f64) {
        if e.bbox.iter().any(|v| !v.is_finite()) {
            let what = match e.class {
                Class::Glyph => "text",
                _ => "a path",
            };
            self.problem(format!(
                "{what} in {} has coordinates that cannot be read, so where it paints is not known",
                self.place()
            ));
            return;
        }
        for i in 0..self.targets.len() {
            let t = self.targets[i].clone();
            // A glyph is under a region when its cell overlaps it with positive area; every other
            // event when its box meets it.
            let meets = if e.class == Class::Glyph {
                cell_overlaps(&e.bbox, &t.rect)
            } else {
                overlaps(&e.bbox, &t.rect)
            };
            if !meets {
                continue;
            }
            if !segs.is_empty() {
                let wide = [
                    t.rect[0] - reach,
                    t.rect[1] - reach,
                    t.rect[2] + reach,
                    t.rect[3] + reach,
                ];
                if !segs.iter().any(|s| segment_meets(s, &wide)) {
                    continue;
                }
            }
            if let Some(why) = e.unsupported {
                self.problem(format!(
                    "{why} in {} lies near a redaction region, so what it paints there is not known",
                    self.place()
                ));
                continue;
            }
            if self.is_free(&e, &t) {
                continue;
            }
            let first = if self.found[i].first.is_none() {
                Some((self.place(), e.bbox))
            } else {
                None
            };
            let f = &mut self.found[i];
            f.count += 1;
            f.kinds.insert(e.kind);
            if first.is_some() {
                f.first = first;
            }
        }
    }

    fn near_a_target(&self, bbox: &[f64; 4]) -> bool {
        self.targets.iter().any(|t| overlaps(bbox, &t.rect))
    }

    /// A path that is painted. The bounds are those of the points (a curve stays inside the hull
    /// of its control points), and of the line when it is stroked.
    fn paint_path(&mut self, f: &Frame, class: Class) {
        let path = &f.path;
        if path.pts.is_empty() {
            return;
        }
        let gs = &f.gs;
        let stroke = class != Class::Fill;
        let mut bbox = bounds(&path.pts);
        let scale = gs.ctm[0].hypot(gs.ctm[1]).max(gs.ctm[2].hypot(gs.ctm[3]));
        let width = gs.line_width.abs() * scale;
        // How far the ink of a stroke reaches from the path.
        let mut reach = 0.0;
        let mut unsupported = None;
        if stroke {
            // The widest line a viewer draws: a hairline is one device pixel, a join may reach
            // the miter limit times the half width. A limit above the profile's is refused when the
            // stroke is near a region, and its reach is the real one.
            let half = width.max(0.5) / 2.0;
            let joins = path.pts.len() > 2;
            if gs.miter > profile::MAX_MITER {
                unsupported = Some("a stroke with a miter limit above 10");
            } else if joins && gs.dashed {
                unsupported = Some("a dashed stroke of a path with joins");
            }
            reach = half * if joins { gs.miter.max(1.5) } else { 1.5 };
            bbox = [
                bbox[0] - reach,
                bbox[1] - reach,
                bbox[2] + reach,
                bbox[3] + reach,
            ];
        }
        let plain = path.curves == 0;
        let shape = if class == Class::Fill && path.as_rect().is_some() {
            Shape::Rect(bounds(&path.pts))
        } else if class == Class::Stroke
            && path.rects == 0
            && path.moves == 1
            && path.lines == 1
            && plain
            && path.pts.len() == 2
        {
            let b = bounds(&path.pts);
            Shape::Line(b)
        } else {
            Shape::Other
        };
        let kind = match class {
            Class::FillStroke => "fill and stroke of a path",
            Class::Fill => "fill of a path",
            _ => "stroke of a path",
        };
        // The ink of a stroke of straight segments is near the segments, not all over the bounds.
        let segs: &[[f64; 4]] = if class == Class::Stroke && plain {
            &path.segs
        } else {
            &[]
        };
        self.emit_segs(
            Event {
                class,
                bbox,
                shape,
                kind,
                fill: gs.fill,
                stroke: gs.stroke,
                width,
                clip: (gs.clip_box, gs.clip_exact),
                smask: gs.smask,
                dashed: gs.dashed,
                translucent: gs.translucent,
                unsupported,
            },
            segs,
            reach,
        );
    }

    /// The path that was built is a clip: the clip path is geometry, and the clip is narrowed.
    fn apply_clip(&mut self, f: &mut Frame) {
        f.pending_clip = false;
        if f.path.pts.is_empty() {
            return;
        }
        let bbox = bounds(&f.path.pts);
        let one_rect = f.path.as_rect().is_some();
        let gs = &f.gs;
        self.emit(Event {
            class: Class::Clip,
            bbox,
            shape: if one_rect {
                Shape::Rect(bbox)
            } else {
                Shape::Other
            },
            kind: "clip path",
            fill: gs.fill,
            stroke: gs.stroke,
            width: 0.0,
            clip: (gs.clip_box, gs.clip_exact),
            smask: gs.smask,
            dashed: gs.dashed,
            translucent: gs.translucent,
            unsupported: None,
        });
        let g = &mut f.gs;
        g.clip_box = [
            g.clip_box[0].max(bbox[0]),
            g.clip_box[1].max(bbox[1]),
            g.clip_box[2].min(bbox[2]),
            g.clip_box[3].min(bbox[3]),
        ];
        g.clip_exact &= one_rect;
    }

    /// `sh`: a shading paints the whole clip, which this check does not follow, so the bounds are
    /// those of the shading (`/BBox`) or else of the page, form, pattern or glyph.
    fn shading(&mut self, name: &[u8], f: &Frame) {
        let gs = &f.gs;
        let name: String = name.iter().map(|b| char::from(*b)).collect();
        let sh = self.doc.dget(self.doc.dget(f.res, "Shading"), &name);
        let mut bbox = self.container;
        if sh.is_dict() {
            if let Some(b) = sh.get("BBox") {
                let Some(b) = exact_numbers(b, self.doc, 4) else {
                    self.problem(format!(
                        "the shading /{name} in {} has a /BBox that is not four numbers, so where it paints is not known",
                        self.place()
                    ));
                    return;
                };
                let pts = [
                    apply(&gs.ctm, b[0], b[1]),
                    apply(&gs.ctm, b[2], b[1]),
                    apply(&gs.ctm, b[0], b[3]),
                    apply(&gs.ctm, b[2], b[3]),
                ];
                bbox = bounds(&pts);
            }
        } else {
            self.problem(format!(
                "the shading /{name} in {} cannot be found, so where it paints is not known",
                self.place()
            ));
            return;
        }
        self.emit(Event {
            class: Class::Shading,
            bbox,
            shape: Shape::Other,
            kind: "shading",
            fill: Colour::Other,
            stroke: Colour::Other,
            width: 0.0,
            clip: (gs.clip_box, gs.clip_exact),
            smask: gs.smask,
            dashed: gs.dashed,
            translucent: gs.translucent,
            unsupported: None,
        });
    }

    /// Interprets the content of a page, form, pattern or appearance: `ctm` maps its space to the
    /// page, and `outer` is the graphics state that it starts with (a form inherits it).
    fn content(&mut self, data: &[u8], res: &Obj, gs: Gs, depth: usize) {
        if depth > 8 {
            self.problem("forms are nested more than 8 deep".to_owned());
            return;
        }
        let mut f = Frame {
            res,
            depth,
            gs,
            stack: Vec::new(),
            path: PathState::default(),
            text: Text::new(),
            pending_clip: false,
            text_clip: false,
            compat: 0,
        };
        for_each_op_while(data, &mut |op: &Op| {
            if !self.tick(1) {
                return false;
            }
            // The strict profile: the operators of the specification, with the operands that it
            // gives them. What does not fit is not executed, and is "could not check".
            if profile::is_known(op.name) {
                if let Some(why) = profile::operand_problem(op) {
                    self.problem(format!(
                        "{why} in {}, and readers differ on such an operator, so the content was not checked",
                        self.place()
                    ));
                    return !self.aborted;
                }
            } else if f.compat == 0 {
                self.problem(format!(
                    "the operator {} in {} is not one of the operators of the specification, so what it does is not known",
                    String::from_utf8_lossy(op.name),
                    self.place()
                ));
                return !self.aborted;
            }
            if op.inline_unsure {
                self.problem(format!(
                    "an inline image in {} has an end that is not certain (no length that fits, and no EI that the content after it confirms), so the content after it may have been read wrongly",
                    self.place()
                ));
            }
            if op.bad {
                self.problem(format!(
                    "an operand of the operator {} in {} is not a clean PDF token (a number with a stray sign or letter, a name with a bad escape, a string that does not end), and a viewer may read it differently, so the content was not checked",
                    String::from_utf8_lossy(op.name),
                    self.place()
                ));
            }
            self.exec(&mut f, op);
            !self.aborted
        });
    }

    fn malformed(&mut self, what: &str) {
        self.problem(format!(
            "{what} in {} has the wrong operands, so where it paints is not known",
            self.place()
        ));
    }

    fn exec(&mut self, f: &mut Frame, op: &Op) {
        let n = op.nums();
        match op.name {
            b"q" => {
                if f.stack.len() < MAX_STACK {
                    f.stack.push(f.gs.clone());
                } else {
                    self.problem(format!(
                        "q is nested more than {MAX_STACK} deep in {}",
                        self.place()
                    ));
                }
            }
            b"Q" => {
                if let Some(g) = f.stack.pop() {
                    f.gs = g;
                }
            }
            b"cm" => {
                if n.len() == 6 {
                    f.gs.ctm = mul(&[n[0], n[1], n[2], n[3], n[4], n[5]], &f.gs.ctm);
                } else {
                    self.malformed("a cm operator");
                }
            }
            b"w" => {
                if let Some(v) = n.first() {
                    f.gs.line_width = *v;
                }
            }
            b"M" => {
                if let Some(v) = n.first() {
                    f.gs.miter = *v;
                }
            }
            b"g" | b"rg" | b"k" => f.gs.fill = colour_of(&n),
            b"G" | b"RG" | b"K" => f.gs.stroke = colour_of(&n),
            b"cs" | b"CS" => {
                let c = match op.name_arg() {
                    Some(b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK") => Colour::Rgb([0, 0, 0]),
                    _ => Colour::Other,
                };
                if op.name == b"cs" {
                    f.gs.fill = c;
                } else {
                    f.gs.stroke = c;
                }
            }
            // A pattern colour: its images are drawn wherever the pattern is painted.
            b"scn" | b"SCN" | b"sc" | b"SC" => {
                let fill = matches!(op.name, b"scn" | b"sc");
                let c = if let Some(name) = op.name_arg() {
                    self.pattern(name, f.res);
                    Colour::Other
                } else {
                    colour_of(&n)
                };
                if fill {
                    f.gs.fill = c;
                } else {
                    f.gs.stroke = c;
                }
            }
            b"m" | b"l" => {
                if n.len() == 2 {
                    let p = apply(&f.gs.ctm, n[0], n[1]);
                    f.path.pts.push(p);
                    if op.name == b"m" {
                        f.path.moves += 1;
                        f.path.move_to(p);
                    } else {
                        f.path.lines += 1;
                        f.path.line_to(p);
                    }
                } else {
                    self.malformed("a path operator");
                }
            }
            b"c" => {
                if n.len() == 6 {
                    for i in 0..3 {
                        f.path.pts.push(apply(&f.gs.ctm, n[2 * i], n[2 * i + 1]));
                    }
                    f.path.curves += 1;
                    f.path.cur = f.path.pts.last().copied();
                    f.path.sub_segs = true;
                } else {
                    self.malformed("a path operator");
                }
            }
            b"v" | b"y" => {
                if n.len() == 4 {
                    for i in 0..2 {
                        f.path.pts.push(apply(&f.gs.ctm, n[2 * i], n[2 * i + 1]));
                    }
                    f.path.curves += 1;
                    f.path.cur = f.path.pts.last().copied();
                    f.path.sub_segs = true;
                } else {
                    self.malformed("a path operator");
                }
            }
            b"re" => {
                if n.len() == 4 {
                    let (x, y, w, h) = (n[0], n[1], n[2], n[3]);
                    // The sides, in the order the path runs: a rectangle is closed.
                    let c = [
                        apply(&f.gs.ctm, x, y),
                        apply(&f.gs.ctm, x + w, y),
                        apply(&f.gs.ctm, x + w, y + h),
                        apply(&f.gs.ctm, x, y + h),
                    ];
                    f.path.finish_subpath();
                    f.path.pts.extend_from_slice(&c);
                    for i in 0..4 {
                        let (a, b) = (c[i], c[(i + 1) % 4]);
                        f.path.segs.push([a.0, a.1, b.0, b.1]);
                    }
                    f.path.rects += 1;
                    f.path.cur = Some(c[0]);
                    f.path.start = Some(c[0]);
                    f.path.sub_segs = true;
                } else {
                    self.malformed("a path operator");
                }
            }
            b"BX" => f.compat += 1,
            b"EX" => f.compat = f.compat.saturating_sub(1),
            b"h" => close_path(&mut f.path),
            b"W" | b"W*" => f.pending_clip = true,
            b"f" | b"F" | b"f*" => self.painted(f, Class::Fill),
            b"S" => self.painted(f, Class::Stroke),
            b"s" => {
                close_path(&mut f.path);
                self.painted(f, Class::Stroke);
            }
            b"B" | b"B*" => self.painted(f, Class::FillStroke),
            b"b" | b"b*" => {
                close_path(&mut f.path);
                self.painted(f, Class::FillStroke);
            }
            b"n" => {
                if f.pending_clip {
                    self.apply_clip(f);
                }
                f.path = PathState::default();
            }
            b"sh" => match op.name_arg() {
                Some(name) => self.shading(name, f),
                None => self.malformed("an sh operator"),
            },
            b"BI" => {
                if let Some((d, data)) = op.inline {
                    let ctm = f.gs.ctm;
                    if self.near_a_target(&unit_bounds(&ctm)) {
                        self.out.push(Placement {
                            ctm,
                            image: Source::Inline(d.to_vec(), data.to_vec()),
                        });
                    }
                }
            }
            b"Do" => {
                let Some(name) = op.name_arg() else {
                    self.problem(format!(
                        "a Do operator in {} has no name, so what it draws is not known",
                        self.place()
                    ));
                    return;
                };
                let name: String = name.iter().map(|b| char::from(*b)).collect();
                let xo = self.doc.dget(f.res, "XObject");
                let Some(Obj::Ref(n)) = self.doc.resolve(xo).get(&name) else {
                    self.problem(format!(
                        "the XObject /{name} drawn in {} cannot be found in the resources, so what it draws is not known",
                        self.place()
                    ));
                    return;
                };
                let n = *n;
                self.xobject(n, f.res, &f.gs, f.depth);
            }
            // The dash pattern: a line that is dashed puts ink only where the pattern says.
            b"d" => {
                f.gs.dashed = op.args.iter().any(|a| match a {
                    Operand::Array(items) => dash_array_is_dashed(items),
                    _ => false,
                });
            }
            b"gs" => self.ext_gstate(f, op),
            // Text.
            b"BT" => {
                f.text = Text::new();
            }
            b"ET" => {
                if f.text_clip {
                    // Glyphs that were added to the clip: the clip is their shapes.
                    f.gs.clip_exact = false;
                    f.text_clip = false;
                }
            }
            b"Tf" => {
                if let Some(name) = op.name_arg() {
                    f.gs.font = self.font_named(name, f.res);
                }
                if let Some(s) = n.last() {
                    f.gs.size = *s;
                }
            }
            b"TL" => {
                if let Some(v) = n.last() {
                    f.gs.leading = *v;
                }
            }
            b"Tc" => f.gs.char_space = n.last().copied().unwrap_or(0.0),
            b"Tw" => f.gs.word_space = n.last().copied().unwrap_or(0.0),
            b"Tz" => f.gs.scale = n.last().copied().unwrap_or(100.0) / 100.0,
            b"Ts" => f.gs.rise = n.last().copied().unwrap_or(0.0),
            b"Tr" => f.gs.render = n.last().copied().unwrap_or(0.0) as i64,
            b"Tm" => {
                if n.len() == 6 {
                    f.text.set([n[0], n[1], n[2], n[3], n[4], n[5]]);
                } else {
                    self.malformed("a Tm operator");
                }
            }
            b"Td" | b"TD" => {
                if n.len() == 2 {
                    if op.name == b"TD" {
                        f.gs.leading = -n[1];
                    }
                    f.text.move_by(n[0], n[1]);
                } else {
                    self.malformed("a text move");
                }
            }
            b"T*" => f.text.move_by(0.0, -f.gs.leading),
            b"Tj" | b"TJ" | b"'" | b"\"" => {
                if matches!(op.name, b"'" | b"\"") {
                    if op.name == b"\"" && n.len() >= 2 {
                        f.gs.word_space = n[0];
                        f.gs.char_space = n[1];
                    }
                    f.text.move_by(0.0, -f.gs.leading);
                }
                if let Some(items) = shown(op) {
                    self.show(f, &items);
                }
            }
            // Type 3 glyph procedures (`d0`, `d1`), marked content and compatibility sections
            // change nothing that is placed: what they hide still counts.
            _ => {}
        }
    }

    fn painted(&mut self, f: &mut Frame, class: Class) {
        f.path.finish_subpath();
        self.paint_path(f, class);
        if f.pending_clip {
            self.apply_clip(f);
        }
        f.path = PathState::default();
    }

    fn ext_gstate(&mut self, f: &mut Frame, op: &Op) {
        let Some(name) = op.name_arg() else {
            self.problem(format!(
                "a gs operator in {} has no name, so the state it sets is not known",
                self.place()
            ));
            return;
        };
        let name: String = name.iter().map(|b| char::from(*b)).collect();
        let state = self.doc.dget(self.doc.dget(f.res, "ExtGState"), &name);
        if !state.is_dict() {
            self.problem(format!(
                "the graphics state /{name} set in {} cannot be found, so the state it sets is not known",
                self.place()
            ));
            return;
        }
        // The dash pattern, the constant alpha and the blend mode decide whether a rule carries
        // only its position.
        if let Some(a) = self.doc.dget(state, "D").array() {
            if let Some(Obj::Array(items)) = a.first().map(|x| self.doc.resolve(x)) {
                let ops: Vec<Operand> = items
                    .iter()
                    .map(|x| self.doc.resolve(x).num().map_or(Operand::Bad, Operand::Num))
                    .collect();
                f.gs.dashed = dash_array_is_dashed(&ops);
            }
        }
        for key in ["ca", "CA"] {
            if self.doc.dget(state, key).num().is_some_and(|v| v < 1.0) {
                f.gs.translucent = true;
            }
        }
        if let Some(bm) = state.get("BM").map(|b| self.doc.resolve(b)) {
            let normal = match bm {
                Obj::Name(n) => matches!(n.as_str(), "Normal" | "Compatible"),
                Obj::Array(a) => a.first().is_some_and(|x| {
                    matches!(
                        self.doc.resolve(x).name(),
                        Some("Normal") | Some("Compatible")
                    )
                }),
                _ => false,
            };
            if !normal {
                f.gs.translucent = true;
            }
        }
        if let Some(w) = self.doc.dget(state, "LW").num() {
            f.gs.line_width = w;
        }
        if let Some(m) = self.doc.dget(state, "ML").num() {
            f.gs.miter = m;
        }
        if let Some(Obj::Array(a)) = state.get("Font").map(|x| self.doc.resolve(x)) {
            if let (Some(font), Some(size)) =
                (a.first(), a.get(1).and_then(|s| self.doc.resolve(s).num()))
            {
                f.gs.font = Some(self.font_from(font, "(from a graphics state)", f.res));
                f.gs.size = size;
            }
        }
        // A soft mask is a form that is drawn in the space of the moment it is set.
        match state.get("SMask").map(|m| self.doc.resolve(m)) {
            Some(Obj::Name(_)) | None => {
                if state.get("SMask").is_some() {
                    f.gs.smask = false;
                }
            }
            Some(mask) => {
                // The group is drawn in the space of this moment, and what it paints is not itself
                // masked.
                if let Some(Obj::Ref(n)) = mask.get("G") {
                    let n = *n;
                    let mut inner = f.gs.clone();
                    inner.smask = false;
                    self.xobject(n, f.res, &inner, f.depth);
                }
                f.gs.smask = true;
            }
        }
    }

    /// Shown text: every glyph is an event at its place. The position follows the text matrix, the
    /// advance of each glyph, the spacing, the scale and the kerning of a `TJ`.
    ///
    /// The cell of a glyph is its advance box across, from the origin by the width of the glyph
    /// (scaled by the horizontal scale and the font size), and from the descent to the ascent of
    /// the font vertically (within the box of the font program), through the text matrix and the
    /// matrix of the page. The event is the bounds of that cell on the page; a glyph is under a
    /// region when its cell overlaps it with positive area (see [`GLYPH_OVERLAP`]). Where a
    /// standard font has no width for a code, the position is an interval and the cell is the
    /// widest it can be. Text shown with a font outside the strict profile is "could not check".
    fn show(&mut self, f: &mut Frame, items: &[ShownItem]) {
        let Some(font) = f.gs.font.clone() else {
            self.problem(format!(
                "text is shown in {} with no font selected, so where it is drawn is not known",
                self.place()
            ));
            return;
        };
        let gs = f.gs.clone();
        let (size, h) = (gs.size, gs.scale);
        if (4..=7).contains(&gs.render) {
            f.text_clip = true;
        }
        if let Some(why) = &font.problem {
            self.problem(format!("text in {}: {why}", self.place()));
            return;
        }
        let (cell_y0, cell_y1) = (font.cell_y[0], font.cell_y[1]);
        let mut width_problem = false;
        for it in items {
            match it {
                ShownItem::Kern(k) => {
                    let d = k / 1000.0 * size * h;
                    f.text.ax = [f.text.ax[0] - d, f.text.ax[1] - d];
                }
                ShownItem::Str(bytes) => {
                    for (code, whole) in font.codes(bytes) {
                        if !self.tick(1) {
                            return;
                        }
                        let w = match font.width(code, whole) {
                            Ok(w) => w,
                            Err(why) => {
                                if !width_problem {
                                    width_problem = true;
                                    self.problem(format!("text in {}: {why}", self.place()));
                                }
                                None
                            }
                        };
                        let space = if code == 32 && !whole {
                            gs.word_space
                        } else {
                            0.0
                        };
                        // Where the glyph is, in the text space of the line: the pen is an
                        // interval, and the glyph is as wide as its width, or as the widest glyph
                        // when the width is not known.
                        let ax = f.text.ax;
                        let top = w.unwrap_or(ADVANCE_RANGE[1]);
                        let xs = [ax[0], ax[1] + top * size * h];
                        let x = [xs[0].min(xs[1]), xs[0].max(xs[1])];
                        let ys = [gs.rise + cell_y0 * size, gs.rise + cell_y1 * size];
                        let y = [ys[0].min(ys[1]), ys[0].max(ys[1])];
                        let m = mul(&f.text.tm, &gs.ctm);
                        let pts = [
                            apply(&m, x[0], y[0]),
                            apply(&m, x[1], y[0]),
                            apply(&m, x[0], y[1]),
                            apply(&m, x[1], y[1]),
                        ];
                        // The place is known when the interval of the pen is narrow.
                        let pen_known = (ax[1] - ax[0]).abs() <= 0.25 * (size * h).abs();
                        self.emit(Event {
                            class: Class::Glyph,
                            bbox: bounds(&pts),
                            shape: Shape::Other,
                            kind: "glyph of text",
                            fill: gs.fill,
                            stroke: gs.stroke,
                            width: 0.0,
                            clip: (gs.clip_box, gs.clip_exact),
                            smask: gs.smask,
                            dashed: gs.dashed,
                            translucent: gs.translucent,
                            unsupported: None,
                        });
                        // The glyph procedure of a Type 3 font is walked, to place what it paints.
                        if font.kind == FontKind::Type3 {
                            if !font.procs.contains_key(&code) {
                                self.problem(format!(
                                    "the code {code} of a Type 3 font shown in {} is not named by /Differences, and a viewer names it through the base encoding, so what it paints is not known",
                                    self.place()
                                ));
                            } else if pen_known {
                                let origin = [size * h, 0.0, 0.0, size, ax[0], gs.rise];
                                let glyph_to_page = mul(&font.t3_matrix, &mul(&origin, &m));
                                self.type3_glyph(&font, code, &gs, glyph_to_page, f.res, f.depth);
                            } else {
                                self.problem(format!(
                                    "Type 3 text in {} has a position that is not known, so what its glyphs paint was not placed",
                                    self.place()
                                ));
                            }
                        }
                        // The advance.
                        let adv = |wv: f64| (wv * size + gs.char_space + space) * h;
                        let (lo, hi) = match w {
                            Some(w) => {
                                let a = adv(w);
                                (a, a)
                            }
                            None => {
                                let (a, b) = (adv(ADVANCE_RANGE[0]), adv(ADVANCE_RANGE[1]));
                                (a.min(b), a.max(b))
                            }
                        };
                        f.text.ax = [f.text.ax[0] + lo, f.text.ax[1] + hi];
                    }
                }
            }
        }
    }

    /// The glyph procedure of a Type 3 font for a code is walked with the matrix that takes its
    /// glyph space to the page: what it paints (paths, images, forms) is placed like any content.
    fn type3_glyph(
        &mut self,
        font: &Font,
        code: u32,
        gs: &Gs,
        glyph_to_page: M,
        page_res: &Obj,
        depth: usize,
    ) {
        let Some(&obj) = font.procs.get(&code) else {
            return;
        };
        let Some(data) = self.proc_data(obj) else {
            return;
        };
        let res = font.t3_res.clone().unwrap_or_else(|| page_res.clone());
        let mut inner = gs.clone();
        inner.ctm = glyph_to_page;
        self.trail
            .push("a glyph procedure of a Type 3 font".to_owned());
        self.content(&data, &res, inner, depth + 1);
        self.trail.pop();
    }

    /// The decoded content of a glyph procedure, read once. A procedure that cannot be read is a
    /// problem: what it paints is not known.
    fn proc_data(&mut self, obj: u32) -> Option<Rc<Vec<u8>>> {
        if let Some(c) = self.procs.get(&obj) {
            return c.clone();
        }
        let doc = self.doc;
        let read = doc.latest(obj).and_then(|e| doc.stream_data(e));
        let data = match read {
            Some(d) if d.problem.is_none() && d.stopped_at.is_none() => {
                // Reading a procedure is work: counted against the budget.
                if !self.tick(1 + d.data.len() as u64 / 256) {
                    return None;
                }
                Some(Rc::new(d.data))
            }
            _ => {
                self.problem(format!(
                    "the glyph procedure object {obj} of a Type 3 font cannot be read, so what it paints is not known"
                ));
                None
            }
        };
        self.procs.insert(obj, data.clone());
        data
    }

    /// A tiling pattern that draws an image: where it is painted is not followed (that needs the
    /// path), so the image is not placed. The render check sees the pixels; the object-level check
    /// cannot, and says so. A pattern that cannot be found is "could not check", and what is read
    /// of a pattern is counted against the budget and kept.
    fn pattern(&mut self, name: &[u8], res: &Obj) {
        let name: String = name.iter().map(|b| char::from(*b)).collect();
        let pats = self.doc.dget(res, "Pattern");
        let Some(Obj::Ref(n)) = self.doc.resolve(pats).get(&name) else {
            self.problem(format!(
                "the pattern /{name} in {} cannot be found, so what it paints is not known",
                self.place()
            ));
            return;
        };
        let n = *n;
        let (draws, refused) = match self.patterns.get(&n) {
            Some(d) => d.clone(),
            None => {
                let Some(e) = self.doc.latest(n) else {
                    self.problem(format!(
                        "the pattern /{name} in {} cannot be found, so what it paints is not known",
                        self.place()
                    ));
                    return;
                };
                let mut work = Work {
                    used: 0,
                    limit: self.budget.steps_left,
                    refused: None,
                };
                let d = e.stream.is_some() && draws_image(self.doc, e, res, 0, &mut work);
                if !self.tick(work.used.max(1)) {
                    return;
                }
                self.patterns.insert(n, (d, work.refused.clone()));
                (d, work.refused)
            }
        };
        if let Some(why) = refused {
            self.problem(format!(
                "the content of the tiling pattern /{name} is outside the strict profile ({why}), so what it paints is not known"
            ));
        }
        if draws {
            let msg = format!(
                "an image is drawn through the tiling pattern /{name}: where the pattern is painted is not followed, so the pixels of that image were checked only in the render"
            );
            self.problem(msg);
        }
    }

    /// The font of a resource name. A name that is not there is "could not check": the text shown
    /// with it has no font to place it by.
    fn font_named(&mut self, name: &[u8], res: &Obj) -> Option<Rc<Font>> {
        let name: String = name.iter().map(|b| char::from(*b)).collect();
        let fonts = self.doc.dget(res, "Font");
        let Some(entry) = fonts.get(&name) else {
            self.problem(format!(
                "the font /{name} selected in {} cannot be found, so the text shown with it cannot be placed",
                self.place()
            ));
            return None;
        };
        Some(self.font_from(entry, &format!("/{name}"), res))
    }

    fn font_from(&mut self, entry: &Obj, label: &str, res: &Obj) -> Rc<Font> {
        let key = match entry {
            Obj::Ref(n) => Some(*n),
            _ => None,
        };
        if let Some(k) = key {
            if let Some(c) = self.fonts.get(&k) {
                return c.clone();
            }
        }
        let doc = self.doc;
        let font = Rc::new(self.load_font(doc.resolve(entry), label, res));
        if let Some(k) = key {
            self.fonts.insert(k, font.clone());
        }
        font
    }

    /// The embedded program of a font descriptor: `key` is the entry that names it (`FontFile`,
    /// `FontFile2` or `FontFile3`), and the program must be of the kind that the entry says. It is
    /// read once, and what it takes to read is counted against the budget.
    fn program(&mut self, fd: &Obj, key: &str) -> Result<Rc<Program>, String> {
        use crate::fontprog::Format;
        let Some(Obj::Ref(n)) = fd.get(key) else {
            return Err(format!("its /{key} is not a stream"));
        };
        let n = *n;
        if let Some(c) = self.programs.get(&n) {
            return c.clone();
        }
        let doc = self.doc;
        let read = doc.latest(n).and_then(|e| doc.stream_data(e));
        let result = match read {
            Some(d) if d.problem.is_none() && d.stopped_at.is_none() => {
                if self.tick(1 + d.data.len() as u64 / 4096) {
                    crate::fontprog::parse(&d.data).and_then(|p| {
                        let ok = match key {
                            "FontFile" => p.format == Format::Type1,
                            "FontFile2" => p.format == Format::TrueType,
                            _ => matches!(p.format, Format::Cff | Format::OpenTypeCff),
                        };
                        if ok {
                            Ok(Rc::new(p))
                        } else {
                            Err(format!("it is not the kind of program that /{key} holds"))
                        }
                    })
                } else {
                    Err("the walk used up its budget".to_owned())
                }
            }
            _ => Err(format!("its /{key} cannot be read")),
        };
        self.programs.insert(n, result.clone());
        result
    }

    /// The glyph cell of an embedded font: the ascent and descent of the descriptor within the
    /// box of the program, and the checks of the profile that the box of the program lies near it.
    fn program_cell(&self, fd: &Obj, program: &Program, max_w: f64) -> Result<[f64; 2], String> {
        let [x0, y0, x1, y1] = program.bbox;
        let em = |key: &str| {
            self.doc
                .dget(fd, key)
                .num()
                .map(|v| v / 1000.0)
                .filter(|v| v.is_finite())
        };
        let asc = em("Ascent").filter(|v| (0.2..=2.0).contains(v));
        let dsc = em("Descent")
            .map(|v| -v.abs())
            .filter(|v| (-1.5..=0.0).contains(v));
        let lo = dsc.map_or(y0, |d| d.max(y0));
        let hi = asc.map_or(y1, |a| a.min(y1));
        if lo >= hi {
            return Err("its ascent and descent leave no cell for a glyph".to_owned());
        }
        if y1 - hi > profile::PROGRAM_SLACK_Y || lo - y0 > profile::PROGRAM_SLACK_Y || y1 - y0 > 3.0
        {
            return Err(format!(
                "the box of its program reaches from {y0:.2} em to {y1:.2} em, more than {} em beyond the cell of a glyph ({lo:.2} to {hi:.2} em), so its ink may be far from the cell",
                profile::PROGRAM_SLACK_Y
            ));
        }
        if x0 < -profile::PROGRAM_SLACK_X || x1 > max_w.max(1.0) + profile::PROGRAM_SLACK_X {
            return Err(format!(
                "the box of its program reaches from {x0:.2} em to {x1:.2} em across, so its ink may be far from the advance of a glyph"
            ));
        }
        Ok([lo, hi])
    }

    /// `/FirstChar`, `/LastChar` and `/Widths` of a simple or Type 3 font: `/Widths` is exactly
    /// `/LastChar - /FirstChar + 1` numbers. `None` when the font has no `/Widths`.
    fn simple_widths(&self, font: &Obj) -> Result<Option<(i64, i64, Vec<f64>)>, String> {
        let doc = self.doc;
        let Some(w) = font.get("Widths") else {
            return Ok(None);
        };
        let Some(arr) = doc.resolve(w).array() else {
            return Err("its /Widths is not an array".to_owned());
        };
        let int = |k: &str| match doc.dget(font, k) {
            Obj::Int(v) if (0..=255).contains(v) => Some(*v),
            _ => None,
        };
        let (Some(first), Some(last)) = (int("FirstChar"), int("LastChar")) else {
            return Err(
                "its /FirstChar and /LastChar are not whole numbers from 0 to 255".to_owned(),
            );
        };
        if usize::try_from(last - first + 1).ok() != Some(arr.len()) {
            return Err(format!(
                "its /Widths has {} entries, and /FirstChar {first} to /LastChar {last} need {}",
                arr.len(),
                last - first + 1
            ));
        }
        let widths: Option<Vec<f64>> = arr
            .iter()
            .map(|x| doc.resolve(x).num().filter(|v| v.is_finite()))
            .collect();
        match widths {
            Some(v) => Ok(Some((first, last, v.iter().map(|w| w / 1000.0).collect()))),
            None => Err("an entry of its /Widths is not a number".to_owned()),
        }
    }

    fn load_font(&mut self, font: &Obj, label: &str, res: &Obj) -> Font {
        let subtype = self.doc.dget(font, "Subtype").name().unwrap_or("");
        let loaded = match subtype {
            "Type3" => self.load_type3(font, label, res),
            "Type0" => self.load_type0(font),
            "Type1" | "MMType1" | "TrueType" => self.load_simple(font),
            other => Err(format!(
                "a font of the type {other:?} is not in the profile"
            )),
        };
        loaded.unwrap_or_else(|why| Font::refused(format!("the font {label} is outside the strict profile: {why}, so where its glyphs are drawn is not known")))
    }

    /// The key (`FontFile`, `FontFile2`, `FontFile3`) of the program that a descriptor holds.
    fn font_file_key(&self, fd: &Obj) -> Option<&'static str> {
        ["FontFile", "FontFile2", "FontFile3"]
            .into_iter()
            .find(|k| fd.get(k).is_some())
    }

    /// A simple font: a standard font that is not embedded, or a font whose program is in the file.
    fn load_simple(&mut self, font: &Obj) -> Result<Font, String> {
        let doc = self.doc;
        let fd = doc.dget(font, "FontDescriptor");
        let base = doc.dget(font, "BaseFont").name().unwrap_or("");
        let widths = self.simple_widths(font)?;
        let (first, last, list) = widths.clone().unwrap_or((0, -1, Vec::new()));
        let mut f = Font {
            first,
            last,
            have_widths: widths.is_some(),
            widths: list,
            ..Font::plain()
        };
        if let Some(key) = self.font_file_key(fd) {
            if widths.is_none() {
                return Err("its program is in the file and it has no /Widths".to_owned());
            }
            let program = self.program(fd, key)?;
            let max_w = f.widths.iter().copied().fold(0.0, f64::max);
            f.cell_y = self.program_cell(fd, &program, max_w)?;
            f.program = Some(program);
            return Ok(f);
        }
        // Not embedded: a standard font that the tool has the metrics of.
        let std = crate::stdfonts::for_name(base)
            .ok_or_else(|| format!("{base:?} is not embedded and not one of the standard fonts"))?;
        let enc = doc.dget(font, "Encoding");
        f.std_standard = match enc {
            Obj::Null => true,
            Obj::Name(n) if n == "WinAnsiEncoding" || n == "MacRomanEncoding" => false,
            _ => {
                return Err(
                    "the encoding of a standard font is not absent, WinAnsiEncoding or MacRomanEncoding (a /Differences, a base encoding, another name)".to_owned(),
                )
            }
        };
        if f.have_widths {
            for code in 32..=126u32 {
                if let Some(w) = f.declared(code) {
                    let t = std.width(code, f.std_standard).unwrap_or(0.0);
                    if (w - t).abs() > (profile::STD_TOLERANCE * t).max(0.0005) {
                        return Err(format!(
                            "the width {:.0} of the code {code} is not within {} of the standard width {:.0}",
                            w * 1000.0,
                            profile::STD_TOLERANCE,
                            t * 1000.0
                        ));
                    }
                }
            }
        }
        f.std = Some(std);
        Ok(f)
    }

    /// A composite font: `Identity-H` with a descendant whose program is in the file.
    fn load_type0(&mut self, font: &Obj) -> Result<Font, String> {
        let doc = self.doc;
        let enc = doc.dget(font, "Encoding");
        if enc.name() != Some("Identity-H") {
            return Err("its character map is not the name Identity-H (a stream, a vertical map and every other map are not read)".to_owned());
        }
        if doc.dget(font, "WMode").int().unwrap_or(0) == 1 {
            return Err("its writing mode is vertical".to_owned());
        }
        let desc = doc
            .dget(font, "DescendantFonts")
            .array()
            .and_then(|a| (a.len() == 1).then(|| doc.resolve(&a[0])))
            .ok_or("it has not exactly one descendant font")?;
        let key = match doc.dget(desc, "Subtype").name() {
            Some("CIDFontType2") => "FontFile2",
            Some("CIDFontType0") => "FontFile3",
            _ => return Err("its descendant is not a CIDFontType2 or CIDFontType0".to_owned()),
        };
        let fd = doc.dget(desc, "FontDescriptor");
        if fd.get(key).is_none() {
            return Err("its program is not in the file".to_owned());
        }
        let program = self.program(fd, key)?;
        let dw = doc.dget(desc, "DW").num().unwrap_or(1000.0) / 1000.0;
        let mut w_map: HashMap<u32, f64> = HashMap::new();
        let mut max_w = dw;
        if let Some(w) = doc.dget(desc, "W").array() {
            // Reading the widths is work.
            self.tick(w.len() as u64 / 16 + 1);
            let mut i = 0;
            while i < w.len() && w_map.len() < 200_000 {
                let Some(c) = doc.resolve(&w[i]).int() else {
                    i += 1;
                    continue;
                };
                match w.get(i + 1).map(|x| doc.resolve(x)) {
                    Some(Obj::Array(list)) => {
                        for (k, v) in list.iter().enumerate() {
                            if let Some(v) = doc.resolve(v).num() {
                                w_map.insert((c + k as i64) as u32, v / 1000.0);
                                max_w = max_w.max(v / 1000.0);
                            }
                        }
                        i += 2;
                    }
                    Some(_) => {
                        let c2 = w.get(i + 1).and_then(|x| doc.resolve(x).int());
                        let v = w.get(i + 2).and_then(|x| doc.resolve(x).num());
                        if let (Some(c2), Some(v)) = (c2, v) {
                            let span = (c2 - c).clamp(0, 70_000);
                            self.tick(span as u64 / 64 + 1);
                            for k in 0..=span {
                                w_map.insert((c + k) as u32, v / 1000.0);
                            }
                            max_w = max_w.max(v / 1000.0);
                        }
                        i += 3;
                    }
                    None => break,
                }
            }
        }
        let cid_to_gid = match doc.dget(desc, "CIDToGIDMap") {
            Obj::Null => CidToGid::Identity,
            Obj::Name(n) if n == "Identity" => CidToGid::Identity,
            _ => match desc.get("CIDToGIDMap") {
                Some(Obj::Ref(n)) if key == "FontFile2" => {
                    let data = doc
                        .latest(*n)
                        .and_then(|e| doc.stream_data(e))
                        .filter(|d| d.problem.is_none() && d.stopped_at.is_none())
                        .ok_or("its /CIDToGIDMap cannot be read")?;
                    self.tick(1 + data.data.len() as u64 / 4096);
                    CidToGid::Map(
                        data.data
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|c| u16::from_be_bytes(*c))
                            .collect(),
                    )
                }
                _ => return Err("its /CIDToGIDMap is not Identity or a stream".to_owned()),
            },
        };
        Ok(Font {
            kind: FontKind::Cid,
            cell_y: self.program_cell(fd, &program, max_w)?,
            dw,
            w_map,
            two_byte: true,
            program: Some(program),
            cid_to_gid,
            ..Font::plain()
        })
    }

    /// A Type 3 font: the numbers that place its glyphs, and the glyph procedure of each code
    /// (through the names of `/Encoding /Differences` and `/CharProcs`), which is walked where the
    /// glyph is shown. Every shown code must be named by `/Differences` (a viewer would name the
    /// others through the base encoding).
    fn load_type3(&mut self, font: &Obj, label: &str, _res: &Obj) -> Result<Font, String> {
        let doc = self.doc;
        let m = match font.get("FontMatrix") {
            Some(m) => exact_numbers(m, doc, 6),
            None => Some(vec![0.001, 0.0, 0.0, 0.001, 0.0, 0.0]),
        }
        .ok_or("its /FontMatrix is not six numbers")?;
        if m[1] != 0.0 || m[2] != 0.0 || m[0] == 0.0 || m[3] == 0.0 {
            return Err("its /FontMatrix turns, skews or collapses the glyphs".to_owned());
        }
        let matrix = [m[0], m[1], m[2], m[3], m[4], m[5]];
        // The cell of a Type 3 glyph is its advance box, and as tall as the font box says (what the
        // glyph paints is walked, and placed by its own coordinates).
        let cell_y = match font
            .get("FontBBox")
            .and_then(|b| exact_numbers(b, doc, 4))
            .filter(|b| b[3] != b[1])
        {
            Some(b) => {
                let (a, c) = (apply(&matrix, 0.0, b[1]).1, apply(&matrix, 0.0, b[3]).1);
                [a.min(c), a.max(c)]
            }
            None => [-0.2, 1.0],
        };
        let widths = self.simple_widths(font)?;
        let charprocs = doc.dget(font, "CharProcs");
        let mut procs: HashMap<u32, u32> = HashMap::new();
        let diffs = doc.dget(doc.dget(font, "Encoding"), "Differences").array();
        match diffs {
            Some(a) => {
                let mut code: i64 = 0;
                for x in a {
                    match doc.resolve(x) {
                        Obj::Name(nm) => {
                            if let (Some(Obj::Ref(p)), Ok(c)) =
                                (charprocs.get(nm), u32::try_from(code))
                            {
                                procs.insert(c, *p);
                            }
                            code += 1;
                        }
                        other => {
                            if let Some(c) = other.int() {
                                code = c;
                            }
                        }
                    }
                }
            }
            None if !charprocs.entries().is_empty() => {
                return Err(format!(
                    "the Type 3 font {label} has no /Encoding with /Differences, so the glyph that a code shows is not known"
                ));
            }
            None => {}
        }
        if widths.is_none() && !procs.is_empty() {
            return Err(format!(
                "the Type 3 font {label} has no /Widths, so where its glyphs are shown is not known"
            ));
        }
        let (first, last, list) = widths.clone().unwrap_or((0, -1, Vec::new()));
        Ok(Font {
            kind: FontKind::Type3,
            cell_y,
            first,
            last,
            have_widths: widths.is_some(),
            // The widths of a Type 3 font are in the glyph space.
            widths: list.iter().map(|w| w * 1000.0 * matrix[0]).collect(),
            procs,
            t3_matrix: matrix,
            t3_res: font.get("Resources").map(|r| doc.resolve(r).clone()),
            ..Font::plain()
        })
    }

    fn xobject(&mut self, n: u32, res: &Obj, gs: &Gs, depth: usize) {
        let Some(e) = self.doc.latest(n) else {
            self.problem(format!(
                "the XObject object {n} drawn in {} cannot be found, so what it draws is not known",
                self.place()
            ));
            return;
        };
        match self.doc.dget(&e.value, "Subtype").name() {
            Some("Image") => {
                let bb = unit_bounds(&gs.ctm);
                if self.near_a_target(&bb) {
                    self.out.push(Placement {
                        ctm: gs.ctm,
                        image: Source::Object(n),
                    });
                }
            }
            Some("Form") => {
                if !self.tick(1) {
                    return;
                }
                let Some(d) = self.doc.stream_data(e) else {
                    return;
                };
                if let Some(p) = &d.problem {
                    self.problem(format!("form object {n}: {p}"));
                }
                if let Some(stop) = &d.stopped_at {
                    self.problem(format!(
                        "form object {n} has the filter {stop}, which is not decoded, so its content was not walked"
                    ));
                }
                let m = match e.value.get("Matrix") {
                    None => IDENTITY,
                    Some(m) => {
                        let Some(m) = matrix_of(m, self.doc) else {
                            self.problem(format!(
                                "the form object {n} drawn in {} has a /Matrix that is not six numbers, and readers read such a matrix differently, so where it paints is not known",
                                self.place()
                            ));
                            return;
                        };
                        m
                    }
                };
                let Some(b) = e
                    .value
                    .get("BBox")
                    .and_then(|b| exact_numbers(b, self.doc, 4))
                else {
                    self.problem(format!(
                        "the form object {n} drawn in {} has no /BBox of four numbers, so where it paints is not known",
                        self.place()
                    ));
                    return;
                };
                let own = e
                    .value
                    .get("Resources")
                    .map(|r| self.doc.resolve(r).clone());
                let res2 = own.unwrap_or_else(|| res.clone());
                let full = mul(&m, &gs.ctm);
                // Nothing that the form paints is outside its box, and the box clips it.
                let saved = self.container;
                let mut inner = gs.clone();
                inner.ctm = full;
                {
                    let pts = [
                        apply(&full, b[0], b[1]),
                        apply(&full, b[2], b[1]),
                        apply(&full, b[0], b[3]),
                        apply(&full, b[2], b[3]),
                    ];
                    let bb = bounds(&pts);
                    self.container = bb;
                    inner.clip_box = [
                        inner.clip_box[0].max(bb[0]),
                        inner.clip_box[1].max(bb[1]),
                        inner.clip_box[2].min(bb[2]),
                        inner.clip_box[3].min(bb[3]),
                    ];
                    inner.clip_exact &= axis_aligned(&full);
                }
                self.trail.push(format!("form object {n}"));
                self.content(&d.data, &res2, inner, depth + 1);
                self.trail.pop();
                self.container = saved;
            }
            other => self.problem(format!(
                "the XObject object {n} drawn in {} is neither an image nor a form ({}), so what it draws is not known",
                self.place(),
                other.unwrap_or("no subtype")
            )),
        }
    }

    /// Walks the content of a page.
    pub fn page(&mut self, data: &[u8], res: &Obj) {
        let gs = Gs::new(IDENTITY, UNBOUNDED);
        self.content(data, res, gs, 0);
    }

    /// A form that is drawn with the matrix `ctm` (an appearance stream).
    pub fn appearance(&mut self, n: u32, res: &Obj, ctm: M, container: [f64; 4]) {
        self.container = container;
        let gs = Gs::new(ctm, UNBOUNDED);
        self.trail.push("an annotation appearance".to_owned());
        self.xobject(n, res, &gs, 0);
        self.trail.pop();
    }
}

/// Whether a dash array puts gaps in a line: a solid line is an empty array (or one of zeros).
fn dash_array_is_dashed(items: &[Operand]) -> bool {
    items.iter().any(|x| match x {
        Operand::Num(v) => *v != 0.0,
        _ => true,
    })
}

/// The bounds on the page of the unit square under `ctm`.
fn unit_bounds(ctm: &M) -> [f64; 4] {
    bounds(&[
        apply(ctm, 0.0, 0.0),
        apply(ctm, 1.0, 0.0),
        apply(ctm, 0.0, 1.0),
        apply(ctm, 1.0, 1.0),
    ])
}

/// A horizontal or vertical rectangle at most [`THIN`] thick that crosses the region from one side
/// to the other: `Some(true)` when it runs along x.
fn crosses_thin_rect(r: &[f64; 4], t: &[f64; 4]) -> Option<bool> {
    let (w, h) = (r[2] - r[0], r[3] - r[1]);
    if h <= THIN && r[0] <= t[0] + BOX_TOLERANCE && r[2] >= t[2] - BOX_TOLERANCE {
        Some(true)
    } else if w <= THIN && r[1] <= t[1] + BOX_TOLERANCE && r[3] >= t[3] - BOX_TOLERANCE {
        Some(false)
    } else {
        None
    }
}

/// A horizontal or vertical segment that crosses the region from one side to the other:
/// `Some(true)` when it runs along x.
fn crosses_line(l: &[f64; 4], t: &[f64; 4]) -> Option<bool> {
    let (dx, dy) = (l[2] - l[0], l[3] - l[1]);
    if dy.abs() < 0.01 && l[0] <= t[0] + BOX_TOLERANCE && l[2] >= t[2] - BOX_TOLERANCE {
        Some(true)
    } else if dx.abs() < 0.01 && l[1] <= t[1] + BOX_TOLERANCE && l[3] >= t[3] - BOX_TOLERANCE {
        Some(false)
    } else {
        None
    }
}

/// A rule that crosses a region carries one dimension, and only if it is painted plainly: with
/// no constant alpha or blend mode, and through no clip that ends at the region on the rule's own
/// axis (a clip that is the region turns a set of rules into a picture that nothing else shows).
fn rule_is_plain(e: &Event, t: &[f64; 4], along_x: bool) -> bool {
    let c = &e.clip.0;
    let reaches = if along_x {
        c[0] <= t[0] - 1.0 && c[2] >= t[2] + 1.0
    } else {
        c[1] <= t[1] - 1.0 && c[3] >= t[3] + 1.0
    };
    !e.translucent && reaches
}

/// How much a scan over content has done, and how much it may do: what a helper reads before the
/// walk proper is work too, and counts against the same budget.
pub struct Work {
    pub used: u64,
    pub limit: u64,
    /// Why the content scanned is outside the strict profile (see [`crate::profile`]).
    pub refused: Option<String>,
}

/// Whether the content of a form or pattern draws an image (also through a form it draws). It uses
/// its own resources, or `fallback` where it has none. Every stream and every operator read is one
/// unit of `work`; past the limit the scan stops (and the caller finds the budget used up).
pub fn draws_image(doc: &Doc, e: &Entry, fallback: &Obj, depth: usize, work: &mut Work) -> bool {
    work.used += 1;
    if work.used > work.limit {
        return false;
    }
    let Some(d) = doc.stream_data(e) else {
        return false;
    };
    let res = e
        .value
        .get("Resources")
        .map_or_else(|| fallback.clone(), |r| doc.resolve(r).clone());
    let mut found = false;
    let mut compat = 0usize;
    for_each_op_while(&d.data, &mut |op: &Op| {
        work.used += 1;
        if work.used > work.limit {
            return false;
        }
        if profile::is_known(op.name) {
            if let Some(why) = profile::operand_problem(op) {
                work.refused.get_or_insert(why);
            }
        } else if compat == 0 {
            work.refused.get_or_insert_with(|| {
                format!(
                    "the operator {} is not one of the specification",
                    String::from_utf8_lossy(op.name)
                )
            });
        }
        match op.name {
            b"BX" => compat += 1,
            b"EX" => compat = compat.saturating_sub(1),
            b"BI" => found = true,
            b"Do" => {
                if let Some(name) = op.name_arg() {
                    let name: String = name.iter().map(|b| char::from(*b)).collect();
                    let xo = doc.dget(&res, "XObject");
                    if let Some(Obj::Ref(n)) = doc.resolve(xo).get(&name) {
                        if let Some(x) = doc.latest(*n) {
                            match doc.dget(&x.value, "Subtype").name() {
                                Some("Image") => found = true,
                                Some("Form") if depth < 4 => {
                                    found |= draws_image(doc, x, &res, depth + 1, work);
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        !found
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_that_crosses_the_region_carries_nothing() {
        let t = [100.0, 100.0, 150.0, 130.0];
        assert_eq!(crosses_line(&[50.0, 110.0, 200.0, 110.0], &t), Some(true));
        assert_eq!(crosses_line(&[120.0, 0.0, 120.0, 500.0], &t), Some(false));
        // Ends inside the region, or slanted.
        assert_eq!(crosses_line(&[50.0, 110.0, 120.0, 110.0], &t), None);
        assert_eq!(crosses_line(&[50.0, 100.0, 200.0, 140.0], &t), None);
        // A thin strip, and a thick one.
        assert_eq!(
            crosses_thin_rect(&[50.0, 110.0, 200.0, 111.5], &t),
            Some(true)
        );
        assert_eq!(crosses_thin_rect(&[50.0, 110.0, 200.0, 114.0], &t), None);
    }

    #[test]
    fn widths_come_from_the_font_and_codes_from_its_encoding() {
        // An embedded simple font: widths from /FirstChar to /LastChar, a code outside is refused.
        let mut f = Font::plain();
        f.have_widths = true;
        f.first = 65;
        f.last = 66;
        f.widths = vec![0.5, 0.6];
        assert_eq!(f.width(65, false), Ok(Some(0.5)));
        assert_eq!(f.width(66, false), Ok(Some(0.6)));
        assert!(f.width(70, false).is_err());
        assert!(f.width(64, false).is_err());
        // A standard font with no widths: the tables, and an interval for a code outside them.
        let mut s = Font::plain();
        s.std = crate::stdfonts::for_name("Helvetica");
        assert_eq!(s.width(65, false), Ok(Some(0.667)));
        assert_eq!(s.width(200, false), Ok(None));
        s.std_standard = true;
        assert_eq!(s.width(39, false), Ok(Some(0.222)));
        // A composite font: two-byte codes, the width from /W or /DW.
        let mut c = Font::plain();
        c.kind = FontKind::Cid;
        c.two_byte = true;
        c.dw = 0.6;
        c.w_map.insert(3, 0.2);
        assert_eq!(c.codes(&[0, 3, 0, 4]), [(3, true), (4, true)]);
        assert_eq!(c.width(3, true), Ok(Some(0.2)));
        assert_eq!(c.width(4, true), Ok(Some(0.6)));
        assert!(c.width(3, false).is_err());
    }

    #[test]
    fn the_cell_of_a_glyph_overlaps_a_region_with_positive_area() {
        let region = [100.0, 100.0, 150.0, 130.0];
        assert!(cell_overlaps(&[140.0, 110.0, 160.0, 120.0], &region));
        // Touching, or ending where the region starts to within rounding.
        assert!(!cell_overlaps(&[90.0, 110.0, 100.0, 120.0], &region));
        assert!(!cell_overlaps(&[90.0, 110.0, 100.004, 120.0], &region));
        assert!(!cell_overlaps(&[110.0, 130.0, 120.0, 140.0], &region));
        assert!(cell_overlaps(&[90.0, 110.0, 101.0, 120.0], &region));
    }
}
