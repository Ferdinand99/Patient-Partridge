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
//! | `text`      | what Poppler `pdftotext` reads from the pages                                     |
//! | `structure` | the decoded strings, names and keys of the document, XMP, and forbidden keys      |
//! | `images`    | hashes of every image, as stored, decoded, and as Poppler `pdfimages` draws it    |
//!
//! Texts are searched as plain bytes, UTF-16 (both byte orders), inside hex strings and escaped
//! strings, and joined over the operators of one text object.
//!
//! It never calls PDFium and uses no PDF library of the app: the check stays independent of the
//! code under test. Values to search for are never printed unless asked for; a hit names the
//! needle by kind, position and a short digest.

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

pub mod carve;
pub mod content;
mod layers;
pub mod search;
pub mod strings;
mod structure;
pub mod tools;

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
    Text,
    Structure,
    Images,
}

impl Layer {
    pub const ALL: [Layer; 6] = [
        Layer::Raw,
        Layer::Qdf,
        Layer::Carve,
        Layer::Text,
        Layer::Structure,
        Layer::Images,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Layer::Raw => "raw",
            Layer::Qdf => "qdf",
            Layer::Carve => "carve",
            Layer::Text => "text",
            Layer::Structure => "structure",
            Layer::Images => "images",
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Spec {
    pub needles: Vec<Needle>,
    /// Dictionary keys that must be absent or hold nothing, such as `/Metadata` or `/Thumb`. See
    /// [`SCRUBBED_KEYS`].
    pub forbidden_keys: Vec<String>,
    /// The file must have one revision: one `%%EOF` and one `startxref`.
    pub single_revision: bool,
}

/// The document-level places that a redaction scrub empties (`/Info` counts as empty when it has
/// no value left).
pub const SCRUBBED_KEYS: [&str; 13] = [
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
    /// "image_sha256": [..], "forbidden_keys": [..], "scrubbed": true, "single_revision": true}`.
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
            } else {
                return Err(Error(format!("unknown key \"{key}\" in the spec")));
            }
        }
        Ok(spec)
    }

    fn validate(&self) -> Result<()> {
        for (i, n) in self.needles.iter().enumerate() {
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
}

impl Default for Options {
    fn default() -> Self {
        Options {
            layers: Layer::ALL.into_iter().collect(),
            features: Features::default(),
            check_valid: true,
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
    pub layers_run: Vec<Layer>,
}

impl Report {
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
        out
    }
}

/// Checks the saved file `pdf` against `spec` in the layers of `opts`.
pub fn verify(pdf: &Path, spec: &Spec, opts: &Options) -> Result<Report> {
    spec.validate()?;
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
    let ctx = layers::Ctx {
        path: pdf,
        data: &data,
        spec,
        opts,
        set: &set,
    };
    let mut report = Report::default();
    layers::run(&ctx, &mut report)?;
    Ok(report)
}
