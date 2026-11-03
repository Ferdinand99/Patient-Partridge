//! An independent check that redacted content is gone from a saved PDF.
//!
//! It is a test and CI tool. It takes the saved file and a list of things that must not be in it
//! (texts, image hashes, metadata values, field values, bookmark titles, attachment names) and
//! looks for them in every layer where a PDF can keep a copy:
//!
//! | layer       | what is searched                                                                  |
//! |-------------|-----------------------------------------------------------------------------------|
//! | `raw`       | the bytes of the file as written                                                  |
//! | `qdf`       | a copy that qpdf has normalised: streams decompressed, object streams opened      |
//! | `carve`     | every stream found by scanning the bytes (old revisions, orphans, object streams) |
//! | `decode`    | the text that every content stream shows, read through the fonts of the file      |
//! | `text`      | what Poppler `pdftotext` reads from the pages                                     |
//! | `structure` | the decoded strings, names and keys of the document, XMP, and forbidden keys      |
//! | `images`    | hashes of every image, and the redacted regions (render, images, geometry)        |
//! | `fonts`     | traces of removed text that stay in a subset font (only when asked for)           |
//!
//! Texts are searched as plain bytes, UTF-16 (both byte orders), inside hex strings and escaped
//! strings, and joined over the operators of one text object.
//!
//! The rule above all others: when the check is not sure, it says "could not check" (a problem,
//! exit status 2), never "clean". A code that a font cannot map, a stream that may be content and
//! cannot be read, a region too small to render, a mask that cannot be decoded: each is a problem.
//!
//! A redacted region is checked three ways, and it must pass all of them.
//!
//! 1. The render. The page is rendered by Poppler (`pdftoppm`, an independent renderer) at
//!    [`RENDER_DPI`], and every pixel whose whole square lies inside the region must have the fill
//!    colour of the region (or, when the spec names none, the middle colour of the region), with a
//!    margin of [`RENDER_MARGIN`] device pixels at the edge left out for anti-aliasing. This sees
//!    what a reader sees: masks, patterns, vector shapes and text that was turned into outlines.
//! 2. The images. Every image that lies over the region, also one that is turned, is decoded with
//!    its soft mask and mask, and the samples whose whole square lies inside the region must be one
//!    value. The pixels the region only partly covers are not looked at. A JPEG is looked at only
//!    where a whole block, and the blocks that its colour spreads from, lie inside the region; the
//!    rest of it is left out of the render check too, because the codec changed those pixels. A
//!    JPEG with no such block under the region, or with quantisation tables so coarse that a flat
//!    block may be off by more than 12 levels of 255, is "could not check".
//! 3. The geometry. One interpreter walks the content of the page with a complete graphics state
//!    (the stack of `q` and `Q`, the matrix, the clip, the colours, the dash pattern, the text
//!    state, the text and line matrices), through forms, soft mask groups, every state of every
//!    annotation appearance, the icons of a widget and the glyph procedures of Type 3 fonts. Every
//!    painting event is placed on the page: a glyph of any font, a path that is filled or stroked,
//!    a clip path, a shading. One that meets the region is a leak, whatever its colour, clip,
//!    transparency, blend mode, render mode or optional content: it is in the file, and shows when
//!    the box over it goes or the layer is turned on. A glyph counts when the centre of its cell is
//!    inside the region, in every spec; the ink of a glyph that straddles the edge is not counted
//!    (the one place where the check is not strict). Geometry that carries nothing about what was
//!    under the region is allowed: the box that redacts, a fill of one rectangle in one device
//!    colour that holds the whole region, a solid rule at most 2 points thick that crosses it, and
//!    a clip that is one rectangle that holds it (see `walk`). What the walk does not fully
//!    understand (a token that a strict reader refuses, a name that cannot be found, vertical
//!    writing, bounds that cannot be computed, a walk that needs more than its budget of steps or
//!    time) is "could not check".
//!
//! Content streams are found from the structure of the file: the contents of a page, forms,
//! appearances, patterns, Type 3 glyph procedures. A stream with such a role is always decoded as
//! content; a stream that is a font program, a character map, an ICC profile or other data by the
//! structure is data. Only a stream that nothing refers to is judged by how it looks, and then it
//! is read as content when its operators are content operators, whatever bytes its strings hold.
//! A stream that is neither an image, a font program nor content is listed as a note and checked
//! by the byte layers only.
//!
//! Text is read in the order of the stream, and also in the order of the page: the glyphs are
//! placed by the text matrix and the matrix of the page, and the runs of text are sorted by line
//! and then by position along the line, so that words that were drawn in another order than they
//! are read are joined as a reader joins them. A word that is hyphenated at the end of a line is
//! found too.
//!
//! Known limits.
//! - A font with no way to Unicode (a composite font without a `ToUnicode` map, an embedded
//!   symbolic font without an encoding, the glyph numbers of a font program) makes the text shown
//!   with it "could not check": the font program is not parsed.
//! - A code of a simple font or a `ToUnicode` map that has no character, or whose character is in
//!   a private use area, is read as a gap of up to three characters, and a text that a gap could
//!   turn into a phrase is "could not check". Another reading of the same font dictionary can
//!   clear the doubt; a reading of another font cannot.
//! - A phrase that is split over the text of a page and the text of a form that it draws, or over
//!   two annotation appearances, is not joined: each stream is read on its own. The same holds
//!   for a word that is shown by one glyph of a Type 3 font that draws it (the glyph is a gap of
//!   three characters at most).
//! - Geometry in objects that no page draws (an orphan form, a pattern that is not used) is not
//!   placed, because nothing says where it would be. The text of those objects is read.
//! - Geometry is placed by its bounds, and bounds err on the side of a leak: a curve is bounded by
//!   its control points, a stroke of straight segments by the segments and the widest line a viewer
//!   draws, a glyph by its cell (see above), so text on the line next to a word can meet a region.
//!   A line a few points from a region can be a leak. A font with no widths gives intervals for the
//!   places of its glyphs, and the whole box counts.
//! - A tool that starts a session of its own (`setsid` on Unix) is out of reach of the time limit;
//!   the tool and the processes in its group (a job object on Windows) are stopped.
//! - Images drawn through a tiling pattern are seen by the render only, and the check says so.
//!
//! It never calls PDFium and uses no PDF library of the app: the check stays independent of the
//! code under test. Values to search for are never printed unless asked for; a hit names the
//! needle by kind, position and a short digest.

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

pub mod carve;
pub mod content;
pub mod fonts;
mod layers;
pub mod pdfobj;
mod regions;
pub mod roles;
pub mod search;
mod stdfonts;
pub mod strings;
mod structure;
pub mod textdec;
pub mod tools;
mod walk;

pub use search::{Encoding, Features, Form, Via};

/// Why the check could not be done (a tool is missing, the file cannot be read). A leak is not
/// an error: it is a hit in the [`Report`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    Raw,
    Qdf,
    Carve,
    Decode,
    Text,
    Structure,
    Images,
    Fonts,
}

impl Layer {
    pub const ALL: [Layer; 8] = [
        Layer::Raw,
        Layer::Qdf,
        Layer::Carve,
        Layer::Decode,
        Layer::Text,
        Layer::Structure,
        Layer::Images,
        Layer::Fonts,
    ];

    /// The layers that run unless the caller picks others. The font check reports what a subset
    /// font keeps of removed text, which a caller may accept, so it runs only when asked for.
    pub const DEFAULT: [Layer; 7] = [
        Layer::Raw,
        Layer::Qdf,
        Layer::Carve,
        Layer::Decode,
        Layer::Text,
        Layer::Structure,
        Layer::Images,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Layer::Raw => "raw",
            Layer::Qdf => "qdf",
            Layer::Carve => "carve",
            Layer::Decode => "decode",
            Layer::Text => "text",
            Layer::Structure => "structure",
            Layer::Images => "images",
            Layer::Fonts => "fonts",
        }
    }

    pub fn parse(name: &str) -> Option<Layer> {
        Layer::ALL.into_iter().find(|l| l.name() == name)
    }
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NeedleKind {
    /// Text that was drawn on a page, or any other text.
    Text,
    /// A value of the document information or of the XMP metadata.
    Metadata,
    /// The value of a form field.
    Field,
    /// The title of a bookmark (outline item).
    Bookmark,
    /// The name or description of an attachment.
    Attachment,
    /// The SHA-256 (hex) of an image: of the stream as stored, of the stream with its filters
    /// undone, or of the pixels that `pdfimages` writes (the PNM payload).
    ImageSha256,
}

impl NeedleKind {
    pub fn name(self) -> &'static str {
        match self {
            NeedleKind::Text => "text",
            NeedleKind::Metadata => "metadata",
            NeedleKind::Field => "field",
            NeedleKind::Bookmark => "bookmark",
            NeedleKind::Attachment => "attachment",
            NeedleKind::ImageSha256 => "image-sha256",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Needle {
    pub kind: NeedleKind,
    pub value: String,
}

/// What must be gone from the file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Spec {
    pub needles: Vec<Needle>,
    /// Dictionary keys that must be absent or hold nothing, such as `/Metadata` or `/Thumb`. See
    /// [`SCRUBBED_KEYS`].
    pub forbidden_keys: Vec<String>,
    /// The file must have one revision: one `%%EOF` and one `startxref`.
    pub single_revision: bool,
    /// Rectangles that were redacted: the images under them must be blank.
    pub regions: Vec<Region>,
}

/// The most steps that the walk over the content of the pages of the regions takes by default.
pub const DEFAULT_WALK_STEPS: u64 = 100_000_000;

/// The most time that this walk takes by default.
pub const DEFAULT_WALK_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);

/// The resolution of the render that checks a region.
pub const RENDER_DPI: u32 = 144;

/// The device pixels at the edge of a region that the render check leaves out: the edge of a fill
/// is anti-aliased, and the position of the region can be off by a fraction of a pixel.
pub const RENDER_MARGIN: usize = 2;

/// A rectangle on a page, in the default user space of the page (points, y up), under which the
/// pixels of every image must be blank and the rendered page must be the fill colour.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    /// The page, counted from 1.
    pub page: usize,
    /// `[x0, y0, x1, y1]`.
    pub rect: [f64; 4],
    /// How far a covered pixel may differ from the others, per colour component. The default is
    /// 0, and a value derived from the quantisation tables for an image stored as JPEG.
    pub tolerance: Option<u8>,
    /// The colour the redaction was painted with (red, green, blue). When it is given, the
    /// rendered pixels must have it; when it is not, they must be of one colour.
    pub fill: Option<[u8; 3]>,
}

/// The document-level places that a redaction scrub empties (`/Info` counts as empty when it has
/// no value left). `/V` and `/DV` are looked at in form fields only.
pub const SCRUBBED_KEYS: [&str; 14] = [
    "/Metadata",
    "/Outlines",
    "/StructTreeRoot",
    "/Thumb",
    "/PieceInfo",
    "/EmbeddedFiles",
    "/AF",
    "/JavaScript",
    "/AA",
    "/OpenAction",
    "/RC",
    "/V",
    "/DV",
    "/Info",
];

impl Spec {
    fn add(mut self, kind: NeedleKind, value: &str) -> Self {
        self.needles.push(Needle {
            kind,
            value: value.to_owned(),
        });
        self
    }

    pub fn text(self, value: &str) -> Self {
        self.add(NeedleKind::Text, value)
    }

    pub fn metadata(self, value: &str) -> Self {
        self.add(NeedleKind::Metadata, value)
    }

    pub fn field(self, value: &str) -> Self {
        self.add(NeedleKind::Field, value)
    }

    pub fn bookmark(self, value: &str) -> Self {
        self.add(NeedleKind::Bookmark, value)
    }

    pub fn attachment(self, value: &str) -> Self {
        self.add(NeedleKind::Attachment, value)
    }

    pub fn image_sha256(self, hex: &str) -> Self {
        self.add(NeedleKind::ImageSha256, &hex.to_ascii_lowercase())
    }

    pub fn forbid(mut self, keys: &[&str]) -> Self {
        self.forbidden_keys
            .extend(keys.iter().map(|k| (*k).to_owned()));
        self
    }

    /// The document-level rules of a scrub: [`SCRUBBED_KEYS`] and one revision.
    pub fn scrubbed(self) -> Self {
        let mut s = self.forbid(&SCRUBBED_KEYS);
        s.single_revision = true;
        s
    }

    /// Reads a spec from JSON:
    /// `{"text": [..], "metadata": [..], "fields": [..], "bookmarks": [..], "attachments": [..],
    /// "image_sha256": [..], "forbidden_keys": [..], "scrubbed": true, "single_revision": true,
    /// "regions": [{"page": 1, "rect": [x0, y0, x1, y1], "tolerance": 0, "fill": [0, 0, 0]}]}`.
    /// A fill may also be written `"#RRGGBB"`.
    pub fn from_json(json: &str) -> Result<Spec> {
        let v: serde_json::Value =
            serde_json::from_str(json).map_err(|e| Error(format!("the spec is not JSON: {e}")))?;
        let obj = v
            .as_object()
            .ok_or_else(|| Error("the spec must be a JSON object".into()))?;
        let mut spec = Spec::default();
        for (key, val) in obj {
            let strings = |val: &serde_json::Value| -> Result<Vec<String>> {
                val.as_array()
                    .ok_or_else(|| Error(format!("\"{key}\" must be a list of strings")))?
                    .iter()
                    .map(|s| {
                        s.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| Error(format!("\"{key}\" must be a list of strings")))
                    })
                    .collect()
            };
            let kind = match key.as_str() {
                "text" => Some(NeedleKind::Text),
                "metadata" => Some(NeedleKind::Metadata),
                "fields" => Some(NeedleKind::Field),
                "bookmarks" => Some(NeedleKind::Bookmark),
                "attachments" => Some(NeedleKind::Attachment),
                "image_sha256" => Some(NeedleKind::ImageSha256),
                _ => None,
            };
            if let Some(kind) = kind {
                for s in strings(val)? {
                    spec = spec.add(kind, &s);
                }
            } else if key == "forbidden_keys" {
                spec.forbidden_keys.extend(strings(val)?);
            } else if key == "scrubbed" {
                if val.as_bool() == Some(true) {
                    spec = spec.scrubbed();
                }
            } else if key == "single_revision" {
                spec.single_revision = val.as_bool() == Some(true);
            } else if key == "regions" {
                spec.regions.extend(regions_from_json(val)?);
            } else {
                return Err(Error(format!("unknown key \"{key}\" in the spec")));
            }
        }
        Ok(spec)
    }

    /// A needle named by kind, position and a short digest, as a hit names it (never its value).
    pub fn label(&self, i: usize) -> String {
        self.needles.get(i).map_or_else(
            || format!("needle {i}"),
            |n| {
                let digest = carve::sha256_hex(n.value.as_bytes());
                format!("{} #{i} [sha256 {}]", n.kind.name(), &digest[..8])
            },
        )
    }

    fn validate(&self) -> Result<()> {
        for (i, r) in self.regions.iter().enumerate() {
            if r.page == 0 || r.rect.iter().any(|v| !v.is_finite()) {
                return Err(Error(format!(
                    "region {} needs a page from 1 and four finite numbers",
                    i + 1
                )));
            }
        }
        for (i, n) in self.needles.iter().enumerate() {
            if n.kind != NeedleKind::ImageSha256
                && search::strip_norm(&n.value).is_empty()
                && !n.value.is_empty()
            {
                return Err(Error(format!(
                    "needle {i} ({}) is only white space and would match everywhere",
                    n.kind.name()
                )));
            }
            if n.value.is_empty() {
                return Err(Error(format!(
                    "needle {i} ({}) is empty and would match everywhere",
                    n.kind.name()
                )));
            }
            if n.kind == NeedleKind::ImageSha256
                && (n.value.len() != 64 || !n.value.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(Error(format!(
                    "needle {i} (image-sha256) is not 64 hex digits"
                )));
            }
        }
        Ok(())
    }
}

/// Which layers run, and which parts of the search are on.
#[derive(Debug, Clone)]
pub struct Options {
    pub layers: BTreeSet<Layer>,
    pub features: Features,
    /// Report `qpdf --check` findings as problems.
    pub check_valid: bool,
    /// The user password of the file. A file without one opens with an empty password.
    pub password: Option<String>,
    /// How long one call of qpdf or Poppler may take before it is killed and the check ends with
    /// an error. The default is the variable `VERIFIER_TOOL_TIMEOUT` (seconds), or 120 s.
    pub tool_timeout: std::time::Duration,
    /// How long all the calls for the file may take together. The default is the variable
    /// `VERIFIER_FILE_BUDGET` (seconds), or 900 s.
    pub file_budget: std::time::Duration,
    /// The most steps (operators and glyphs) that the walk over the content of the pages of the
    /// regions may take before the check ends with "could not check". The default is
    /// [`DEFAULT_WALK_STEPS`].
    pub walk_steps: u64,
    /// The most time that this walk may take for the file. The default is [`DEFAULT_WALK_BUDGET`].
    pub walk_budget: std::time::Duration,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            layers: Layer::DEFAULT.into_iter().collect(),
            features: Features::default(),
            check_valid: true,
            password: None,
            tool_timeout: tools::tool_timeout_from_env(),
            file_budget: tools::file_budget_from_env(),
            walk_steps: DEFAULT_WALK_STEPS,
            walk_budget: DEFAULT_WALK_BUDGET,
        }
    }
}

impl Options {
    pub fn only(layers: &[Layer]) -> Self {
        Options {
            layers: layers.iter().copied().collect(),
            ..Options::default()
        }
    }

    pub fn without(layers: &[Layer]) -> Self {
        let mut o = Options::default();
        for l in layers {
            o.layers.remove(l);
        }
        o
    }
}

/// What a hit is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    /// The needle with this index in [`Spec::needles`].
    Needle(usize),
    /// A rule of the spec was broken (a forbidden key, more than one revision).
    Rule(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub layer: Layer,
    pub subject: Subject,
    /// Offset in the searched buffer: the file for `raw`, the normalised copy for `qdf`, the
    /// decoded stream for `carve`, the text of the tool for `text`. `None` where there is none.
    pub offset: Option<usize>,
    /// Where in the document, in words.
    pub location: String,
    pub form: Option<Form>,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub hits: Vec<Hit>,
    /// Things that stopped a layer from giving a full answer (a file that qpdf cannot read, a
    /// tool that failed). A report with a problem is not clean.
    pub problems: Vec<String>,
    /// Things worth knowing that do not change the answer: a stream that is neither an image, a
    /// font nor content, which only the byte layers looked at.
    pub notes: Vec<String>,
    pub layers_run: Vec<Layer>,
    /// The tools that did the work, with their versions.
    pub tools: Vec<String>,
}

impl Report {
    /// Adds a problem once. A report with a problem is not clean.
    pub fn problem(&mut self, msg: impl Into<String>) {
        let m = msg.into();
        if self.problems.len() < 100 && !self.problems.contains(&m) {
            self.problems.push(m);
        }
    }

    /// Adds a note once.
    pub fn note(&mut self, msg: impl Into<String>) {
        let m = msg.into();
        if self.notes.len() < 100 && !self.notes.contains(&m) {
            self.notes.push(m);
        }
    }

    /// No leak and no problem.
    pub fn is_clean(&self) -> bool {
        self.hits.is_empty() && self.problems.is_empty()
    }

    pub fn hit_layers(&self) -> BTreeSet<Layer> {
        self.hits.iter().map(|h| h.layer).collect()
    }

    pub fn hits_in(&self, layer: Layer) -> impl Iterator<Item = &Hit> {
        self.hits.iter().filter(move |h| h.layer == layer)
    }

    /// Indexes of the needles that were found.
    pub fn needles_hit(&self) -> BTreeSet<usize> {
        self.hits
            .iter()
            .filter_map(|h| match h.subject {
                Subject::Needle(i) => Some(i),
                Subject::Rule(_) => None,
            })
            .collect()
    }

    pub fn rules_hit(&self) -> BTreeSet<String> {
        self.hits
            .iter()
            .filter_map(|h| match &h.subject {
                Subject::Rule(r) => Some(r.clone()),
                Subject::Needle(_) => None,
            })
            .collect()
    }

    /// One line per hit. The needle is named by kind, position and a digest, not by value,
    /// unless `show_values`.
    pub fn render(&self, spec: &Spec, show_values: bool) -> String {
        let mut out = String::new();
        for h in &self.hits {
            let who = match &h.subject {
                Subject::Needle(i) => spec.needles.get(*i).map_or_else(
                    || format!("needle {i}"),
                    |n| {
                        let digest = carve::sha256_hex(n.value.as_bytes());
                        if show_values {
                            format!("{} #{i} \"{}\"", n.kind.name(), n.value)
                        } else {
                            format!("{} #{i} [sha256 {}]", n.kind.name(), &digest[..8])
                        }
                    },
                ),
                Subject::Rule(r) => format!("rule: {r}"),
            };
            let offset = h.offset.map_or(String::new(), |o| format!(" offset={o}"));
            let form = h.form.map_or(String::new(), |f| format!(" form={f}"));
            out.push_str(&format!(
                "LEAK layer={}{offset}{form} {who} at {}\n",
                h.layer, h.location
            ));
        }
        for p in &self.problems {
            out.push_str(&format!("PROBLEM {p}\n"));
        }
        for n in &self.notes {
            out.push_str(&format!("NOTE {n}\n"));
        }
        if self.is_clean() {
            out.push_str(&format!(
                "clean: nothing found in {}\n",
                self.layers_run
                    .iter()
                    .map(|l| l.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.tools.is_empty() {
            out.push_str(&format!("tools: {}\n", self.tools.join("; ")));
        }
        out
    }
}

/// Checks the saved file `pdf` against `spec` in the layers of `opts`.
pub fn verify(pdf: &Path, spec: &Spec, opts: &Options) -> Result<Report> {
    spec.validate()?;
    if spec.needles.is_empty()
        && spec.forbidden_keys.is_empty()
        && !spec.single_revision
        && spec.regions.is_empty()
    {
        return Err(Error(
            "the spec has no needle, no rule and no region: there is nothing to check".into(),
        ));
    }
    if opts.layers.is_empty() {
        return Err(Error(
            "no layer is selected: there is nothing to check".into(),
        ));
    }
    let data =
        std::fs::read(pdf).map_err(|e| Error(format!("cannot read {}: {e}", pdf.display())))?;
    let texts: Vec<(usize, &str)> = spec
        .needles
        .iter()
        .enumerate()
        .filter(|(_, n)| n.kind != NeedleKind::ImageSha256)
        .map(|(i, n)| (i, n.value.as_str()))
        .collect();
    let set = search::NeedleSet::new(&texts, opts.features);
    let password = match &opts.password {
        Some(p) => {
            let mut f = tempfile::NamedTempFile::new()?;
            std::io::Write::write_all(&mut f, p.as_bytes())?;
            Some(f)
        }
        None => None,
    };
    let ctx = layers::Ctx {
        path: pdf,
        data: &data,
        spec,
        opts,
        set: &set,
        password,
        qdf: std::cell::OnceCell::new(),
        plain: std::cell::OnceCell::new(),
        revisions: std::cell::OnceCell::new(),
    };
    tools::begin(opts.tool_timeout, opts.file_budget);
    let mut report = Report {
        tools: tools::versions(),
        ..Report::default()
    };
    let ran = layers::run(&ctx, &mut report);
    // A tool that was killed for taking too long leaves the file partly checked: whatever else was
    // found, the result is "could not check", never a report.
    if let Some(why) = tools::stopped() {
        return Err(Error(why));
    }
    ran?;
    Ok(report)
}

/// `RRGGBB` or `#RRGGBB`.
pub fn parse_fill(h: &str) -> Option<[u8; 3]> {
    let h = h.strip_prefix('#').unwrap_or(h);
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([c(0)?, c(2)?, c(4)?])
}

fn regions_from_json(val: &serde_json::Value) -> Result<Vec<Region>> {
    let bad =
        || Error("\"regions\" must be a list of {\"page\": N, \"rect\": [x0, y0, x1, y1]}".into());
    let list = val.as_array().ok_or_else(bad)?;
    list.iter()
        .map(|r| {
            let o = r.as_object().ok_or_else(bad)?;
            let page = o
                .get("page")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(bad)?;
            let rect: Vec<f64> = o
                .get("rect")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(bad)?
                .iter()
                .map(|n| n.as_f64().ok_or_else(bad))
                .collect::<Result<_>>()?;
            let rect: [f64; 4] = rect.try_into().map_err(|_| bad())?;
            let tolerance = match o.get("tolerance") {
                None => None,
                Some(t) => Some(u8::try_from(t.as_u64().ok_or_else(bad)?).map_err(|_| bad())?),
            };
            let fill = match o.get("fill") {
                None => None,
                Some(serde_json::Value::String(h)) => Some(parse_fill(h).ok_or_else(bad)?),
                Some(serde_json::Value::Array(a)) => {
                    let v: Vec<u8> = a
                        .iter()
                        .map(|n| {
                            n.as_u64()
                                .and_then(|n| u8::try_from(n).ok())
                                .ok_or_else(bad)
                        })
                        .collect::<Result<_>>()?;
                    Some(<[u8; 3]>::try_from(v).map_err(|_| bad())?)
                }
                Some(_) => return Err(bad()),
            };
            Ok(Region {
                page: page as usize,
                rect,
                tolerance,
                fill,
            })
        })
        .collect()
}
