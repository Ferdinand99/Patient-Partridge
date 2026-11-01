//! The Patient Partridge PDF engine.
//!
//! This is the only code that parses PDF files. It runs as its own process (`partridge-engine`),
//! so a PDF that crashes or hangs the parser cannot take the app down with it. It talks to the
//! app over standard input and output (see `partridge-protocol`) and has no network code.
//!
//! PDF JavaScript is off for three reasons that each hold on their own: the PDFium build we ship
//! has no JavaScript engine (it is not the "V8" variant), the form environment is created without
//! a JavaScript platform, and this crate never calls the document or page action functions.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use partridge_protocol::{
    ErrorCode, FontScript, PageSize, Request, RequestBody, Response, ResponseBody,
    MAX_RENDER_PIXELS, MAX_RENDER_SIDE, PROTOCOL_VERSION,
};
use pdfium_render::prelude::*;

#[cfg(unix)]
pub mod fds;
pub mod fonts;
pub mod limits;
pub mod sandbox;
pub mod startup;
pub mod stdio;
mod sysfontinfo;
pub mod text;

/// Command line of the engine.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    /// Folder that holds the PDFium library. The engine never searches anywhere else.
    pub pdfium_dir: Option<PathBuf>,
    /// Folder with fonts to read into memory before the sandbox is applied. Defaults to `fonts`
    /// inside the PDFium folder.
    pub fonts_dir: Option<PathBuf>,
    /// The document the app handed over: a descriptor number on Unix, a handle value on Windows.
    pub document: Option<u64>,
    /// Memory limit in bytes. Defaults to [`limits::MEMORY_LIMIT_BYTES`].
    pub memory_limit: Option<u64>,
    /// System fonts the app handed over, one for a script: a descriptor number on Unix, a handle
    /// value on Windows.
    pub fonts: Vec<(FontScript, u64)>,
}

/// Reads the engine's command line (without the program name).
pub fn parse_args<I: IntoIterator<Item = OsString>>(args: I) -> Result<Args, String> {
    let mut out = Args::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == "--pdfium-dir" {
            let value = it.next().ok_or("--pdfium-dir needs a folder")?;
            out.pdfium_dir = Some(PathBuf::from(value));
        } else if arg == "--fonts-dir" {
            let value = it.next().ok_or("--fonts-dir needs a folder")?;
            out.fonts_dir = Some(PathBuf::from(value));
        } else if arg == "--document" {
            out.document = Some(number(it.next(), "--document")?);
        } else if arg == "--font" {
            let value = it.next().ok_or("--font needs script=number")?;
            let text = value.to_string_lossy();
            let (name, number) = text.split_once('=').ok_or("--font needs script=number")?;
            let script = FontScript::from_name(name).ok_or("--font names an unknown script")?;
            let number = number.parse().map_err(|_| "--font needs a number")?;
            out.fonts.push((script, number));
        } else if arg == "--memory-limit" {
            out.memory_limit = Some(number(it.next(), "--memory-limit")?);
        } else {
            return Err(format!("unknown argument {}", arg.to_string_lossy()));
        }
    }
    Ok(out)
}

fn number(value: Option<OsString>, name: &str) -> Result<u64, String> {
    value
        .and_then(|v| v.to_str().and_then(|s| s.parse().ok()))
        .ok_or_else(|| format!("{name} needs a number"))
}

/// Reads the build number out of the `VERSION` file that ships with the PDFium binaries.
pub fn parse_pdfium_version(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix("BUILD="))
        .map(|build| format!("PDFium {}", build.trim()))
}

/// Checks the size of a requested bitmap against the limits.
pub fn check_render_size(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("bitmap size must not be zero".into());
    }
    if width > MAX_RENDER_SIDE || height > MAX_RENDER_SIDE {
        return Err(format!(
            "bitmap side is larger than {MAX_RENDER_SIDE} pixels"
        ));
    }
    if (width as u64) * (height as u64) > MAX_RENDER_PIXELS {
        return Err(format!("bitmap is larger than {MAX_RENDER_PIXELS} pixels"));
    }
    Ok(())
}

/// Largest page side PDF allows, in points (200 inches).
const MAX_PAGE_SIDE: f32 = 14_400.0;

/// A page size that can be laid out: finite and positive and not above the PDF limit. A size that
/// is broken becomes a Letter page, a size that is too big is cut to the limit.
fn sane_size(width: f32, height: f32) -> PageSize {
    if width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0 {
        PageSize {
            width: width.min(MAX_PAGE_SIDE),
            height: height.min(MAX_PAGE_SIDE),
        }
    } else {
        PageSize {
            width: 612.0,
            height: 792.0,
        }
    }
}

type Failure = (ErrorCode, String);

fn fail<T>(code: ErrorCode, message: impl Into<String>) -> Result<T, Failure> {
    Err((code, message.into()))
}

/// The engine state: the loaded PDFium library and the documents that are open.
pub struct Engine {
    pdfium: Result<&'static Pdfium, String>,
    pdfium_version: String,
    docs: HashMap<u32, PdfDocument<'static>>,
    next_doc: u32,
    /// The one document the app handed over. `Open` takes it.
    document: Option<File>,
    /// The sandbox layers in force, as reported in the greeting. Empty when there is none.
    sandbox: String,
    /// When set, every request fails with this error and the engine reads nothing.
    refusal: Option<Failure>,
    /// The scripts for which PDFium asked for a font and the engine had none to give.
    missed_fonts: Arc<AtomicU8>,
}

impl Engine {
    /// Loads PDFium from `pdfium_dir`. When that fails the engine still starts and answers every
    /// request with an `EngineUnavailable` error, so the app can tell the user what is wrong.
    pub fn new(pdfium_dir: &Path) -> Engine {
        Engine::with_fonts(pdfium_dir, &pdfium_dir.join("fonts"))
    }

    /// As [`Engine::new`], with the folder to read fonts from. Everything the engine will need
    /// later is loaded here, because after the sandbox is applied it cannot open files: the library,
    /// the fonts, and the code paths of PDFium that are first used when a page is drawn.
    pub fn with_fonts(pdfium_dir: &Path, fonts_dir: &Path) -> Engine {
        Engine::with_system_fonts(pdfium_dir, fonts_dir, Vec::new())
    }

    /// As [`Engine::with_fonts`], with the system fonts the app handed over (one file for a
    /// script). They are read only when a page needs that script, through the handles.
    pub fn with_system_fonts(
        pdfium_dir: &Path,
        fonts_dir: &Path,
        system_fonts: Vec<(FontScript, File)>,
    ) -> Engine {
        let version = std::fs::read_to_string(pdfium_dir.join("VERSION"))
            .ok()
            .and_then(|text| parse_pdfium_version(&text))
            .unwrap_or_else(|| "PDFium".to_string());
        let loaded = load_pdfium(pdfium_dir, fonts_dir, system_fonts);
        let (pdfium, missed) = match loaded {
            Ok((pdfium, missed)) => {
                warm_up(pdfium);
                (Ok(pdfium), missed)
            }
            Err(message) => (Err(message), Arc::new(AtomicU8::new(0))),
        };
        Engine {
            missed_fonts: missed,
            pdfium,
            pdfium_version: version,
            docs: HashMap::new(),
            next_doc: 1,
            document: None,
            sandbox: String::new(),
            refusal: None,
        }
    }

    /// Hands the engine the document it may open. This does not read it.
    pub fn set_document(&mut self, file: File) {
        self.document = Some(file);
    }

    /// Records which sandbox layers are in force (for the greeting).
    pub fn set_sandbox(&mut self, layers: &str) {
        self.sandbox = layers.to_string();
    }

    /// Makes every request fail with `message`, and drops the document without reading it. Used
    /// when the sandbox could not be applied.
    pub fn refuse(&mut self, code: ErrorCode, message: &str) {
        self.document = None;
        self.refusal = Some((code, message.to_string()));
    }

    /// True once [`Engine::refuse`] was called: the engine answers the next request and stops.
    pub fn is_refusing(&self) -> bool {
        self.refusal.is_some()
    }

    /// Answers one request.
    pub fn handle(&mut self, request: Request) -> Response {
        let body = match self.dispatch(request.body) {
            Ok(body) => body,
            Err((code, message)) => ResponseBody::Error { code, message },
        };
        Response {
            id: request.id,
            body,
        }
    }

    fn dispatch(&mut self, body: RequestBody) -> Result<ResponseBody, Failure> {
        if let Some(refusal) = &self.refusal {
            return Err(refusal.clone());
        }
        let pdfium = match &self.pdfium {
            Ok(p) => *p,
            Err(message) => return fail(ErrorCode::EngineUnavailable, message.clone()),
        };
        match body {
            RequestBody::Hello { protocol_version } => {
                if protocol_version != PROTOCOL_VERSION {
                    return fail(
                        ErrorCode::ProtocolMismatch,
                        format!(
                            "the app speaks protocol {protocol_version}, the engine {PROTOCOL_VERSION}"
                        ),
                    );
                }
                Ok(ResponseBody::Hello {
                    protocol_version: PROTOCOL_VERSION,
                    engine_version: env!("CARGO_PKG_VERSION").to_string(),
                    pdfium_version: self.pdfium_version.clone(),
                    sandbox: self.sandbox.clone(),
                })
            }
            RequestBody::Open { password } => self.open(pdfium, password.as_deref()),
            RequestBody::Render {
                doc,
                page,
                width,
                height,
            } => self.render(doc, page, width, height),
            RequestBody::Close { doc } => {
                // Closing an unknown id is not an error: the end state is the same.
                self.docs.remove(&doc);
                Ok(ResponseBody::Closed)
            }
            RequestBody::Outline { doc } => {
                let document = self.document(doc)?;
                Ok(ResponseBody::Outline {
                    items: text::outline(document),
                })
            }
            RequestBody::PageText { doc, page } => self.with_page(doc, page, |p| {
                let chars = text::page_chars(p, true).map_err(|_| {
                    (
                        ErrorCode::Corrupt,
                        "the text of the page could not be read".to_string(),
                    )
                })?;
                Ok(ResponseBody::PageText {
                    text: chars.iter().map(|c| c.ch).collect(),
                    rects: chars.iter().map(|c| c.rect).collect(),
                })
            }),
            RequestBody::PageLinks { doc, page } => {
                let count = self.document(doc)?.pages().len() as usize;
                self.with_page(doc, page, |p| {
                    Ok(ResponseBody::PageLinks {
                        links: text::page_links(p, count),
                    })
                })
            }
            RequestBody::FontReport => Ok(ResponseBody::FontReport {
                missed: fonts::scripts_of_bits(self.missed_fonts.load(Ordering::Relaxed)),
            }),
            RequestBody::Search { doc, page, query } => self.with_page(doc, page, |p| {
                let chars = text::page_chars(p, false).map_err(|_| {
                    (
                        ErrorCode::Corrupt,
                        "the text of the page could not be read".to_string(),
                    )
                })?;
                let plain: Vec<char> = chars.iter().map(|c| c.ch).collect();
                Ok(ResponseBody::Matches {
                    matches: text::find_matches(&plain, &query),
                })
            }),
        }
    }

    fn document(&self, doc: u32) -> Result<&PdfDocument<'static>, Failure> {
        self.docs
            .get(&doc)
            .ok_or((ErrorCode::UnknownDocument, "no such document".to_string()))
    }

    /// Loads one page of an open document and runs `f` on it.
    fn with_page<T>(
        &self,
        doc: u32,
        page: u32,
        f: impl FnOnce(&PdfPage) -> Result<T, Failure>,
    ) -> Result<T, Failure> {
        let document = self.document(doc)?;
        let pages = document.pages();
        if page as i64 >= pages.len() as i64 {
            return fail(ErrorCode::PageOutOfRange, "the page does not exist");
        }
        let page = pages
            .get(page as PdfPageIndex)
            .map_err(|_| (ErrorCode::Corrupt, "the page could not be read".to_string()))?;
        f(&page)
    }

    fn open(
        &mut self,
        pdfium: &'static Pdfium,
        password: Option<&str>,
    ) -> Result<ResponseBody, Failure> {
        // The engine does not open files. It reads the one the app handed over, once.
        let Some(file) = self.document.take() else {
            return fail(
                ErrorCode::InvalidRequest,
                "no document was handed to the engine, or it was opened already",
            );
        };
        let is_file = file.metadata().map(|m| m.is_file()).unwrap_or(false);
        if !is_file {
            return fail(ErrorCode::Io, "the handle is not a file");
        }
        // The file is read on demand by PDFium (through a custom reader that seeks and reads the
        // inherited handle); it is not loaded into memory as a whole.
        let doc = pdfium
            .load_pdf_from_reader(file, password)
            .map_err(|e| map_open_error(e, password.is_some()))?;
        // One unreadable page must not refuse the whole document: it gets a placeholder size.
        let page_list = doc.pages();
        let pages = (0..page_list.len())
            .map(|i| match page_list.page_size(i) {
                Ok(r) => sane_size(r.width().value, r.height().value),
                Err(_) => sane_size(0.0, 0.0),
            })
            .collect();
        let id = self.next_doc;
        self.next_doc = self.next_doc.wrapping_add(1).max(1);
        self.docs.insert(id, doc);
        Ok(ResponseBody::Opened { doc: id, pages })
    }

    fn render(
        &mut self,
        doc: u32,
        page: u32,
        width: u32,
        height: u32,
    ) -> Result<ResponseBody, Failure> {
        check_render_size(width, height).map_err(|m| (ErrorCode::InvalidRequest, m))?;
        let document = self
            .docs
            .get(&doc)
            .ok_or((ErrorCode::UnknownDocument, "no such document".to_string()))?;
        let pages = document.pages();
        if page as i64 >= pages.len() as i64 {
            return fail(ErrorCode::PageOutOfRange, "the page does not exist");
        }
        let page = pages
            .get(page as PdfPageIndex)
            .map_err(|_| (ErrorCode::Corrupt, "the page could not be read".to_string()))?;
        let config = PdfRenderConfig::new().set_target_size(width as Pixels, height as Pixels);
        let bitmap = page.render_with_config(&config).map_err(|_| {
            (
                ErrorCode::RenderFailed,
                "the page could not be drawn".to_string(),
            )
        })?;
        if bitmap.width() as u32 != width || bitmap.height() as u32 != height {
            return fail(
                ErrorCode::RenderFailed,
                "the page was drawn at a wrong size",
            );
        }
        Ok(ResponseBody::Rendered {
            width,
            height,
            rgba: bitmap.as_rgba_bytes(),
        })
    }
}

/// The loaded library, and the scripts the font source had to refuse (a bit for each).
type Loaded = (&'static Pdfium, Arc<AtomicU8>);

fn load_pdfium(
    dir: &Path,
    fonts_dir: &Path,
    system_fonts: Vec<(FontScript, File)>,
) -> Result<Loaded, String> {
    let dir = std::fs::canonicalize(dir)
        .map_err(|e| format!("the PDFium folder {} is not usable: {e}", dir.display()))?;
    let library = Pdfium::pdfium_platform_library_name_at_path(&dir);
    let bindings = Pdfium::bind_to_library(&library).map_err(|e| {
        format!(
            "the PDFium library {} could not be loaded: {e}",
            library.display()
        )
    })?;
    let pdfium = Pdfium::new(bindings);
    // PDFium must not look for fonts on the system: it cannot open files once the sandbox is on.
    // It gets the fonts of the `fonts` folder and the system fonts the app handed over from
    // memory, and its built-in ones for the rest.
    let mut provider = fonts::MemoryFonts::load_dir(fonts_dir);
    for (script, file) in system_fonts {
        provider.add_system(script, file);
    }
    let missed = provider.missed_scripts();
    // The font source lives as long as the process, like the library.
    let source = Box::leak(sysfontinfo::SysFontInfo::new(provider));
    // SAFETY: the pointer is to a structure that is never freed, built for this interface.
    unsafe { pdfium.bindings().FPDF_SetSystemFontInfo(source.as_ptr()) };
    // The engine lives as long as the process: leaking gives every document a 'static library.
    Ok((Box::leak(Box::new(pdfium)), missed))
}

/// A small document in the engine itself. Drawing it before the sandbox is applied makes PDFium
/// set up everything it sets up lazily (font tables, decoders, caches), so that this does not
/// happen later, when the engine can no longer ask the system for anything.
const WARM_UP_PDF: &[u8] = include_bytes!("warm-up.pdf");

fn warm_up(pdfium: &'static Pdfium) {
    let Ok(doc) = pdfium.load_pdf_from_byte_slice(WARM_UP_PDF, None) else {
        return;
    };
    let pages = doc.pages();
    for index in 0..pages.len() {
        let Ok(page) = pages.get(index) else { continue };
        let config = PdfRenderConfig::new().set_target_size(240, 320);
        let _ = page.render_with_config(&config).map(|b| b.as_rgba_bytes());
        let _ = text::page_chars(&page, true);
        let _ = text::page_links(&page, pages.len() as usize);
    }
    let _ = text::outline(&doc);
}

fn map_open_error(error: PdfiumError, password_given: bool) -> Failure {
    match error {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            if password_given {
                (
                    ErrorCode::PasswordIncorrect,
                    "the password is not correct".to_string(),
                )
            } else {
                (
                    ErrorCode::PasswordRequired,
                    "the file is protected by a password".to_string(),
                )
            }
        }
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FormatError) => (
            ErrorCode::Corrupt,
            "the file is not a valid PDF or is damaged".to_string(),
        ),
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::SecurityError) => (
            ErrorCode::Unsupported,
            "the file uses a security setting that is not supported".to_string(),
        ),
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FileError)
        | PdfiumError::IoError(_) => (ErrorCode::Io, "the file could not be read".to_string()),
        _ => (
            ErrorCode::Corrupt,
            "the file could not be opened as a PDF".to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn args_are_parsed() {
        assert_eq!(parse_args(os(&[])).unwrap(), Args::default());
        assert_eq!(
            parse_args(os(&["--pdfium-dir", "/opt/pdfium"]))
                .unwrap()
                .pdfium_dir,
            Some(PathBuf::from("/opt/pdfium"))
        );
        assert!(parse_args(os(&["--pdfium-dir"])).is_err());
        assert!(parse_args(os(&["--open", "x.pdf"])).is_err());
        let args = parse_args(os(&["--font", "japanese=4", "--font", "thai=5"])).unwrap();
        assert_eq!(
            args.fonts,
            vec![(FontScript::Japanese, 4), (FontScript::Thai, 5)]
        );
        assert!(parse_args(os(&["--font", "latin=4"])).is_err());
        assert!(parse_args(os(&["--font", "japanese"])).is_err());
    }

    #[test]
    fn version_file_is_read() {
        let text = "MAJOR=147\nMINOR=0\nBUILD=8125\nPATCH=0\n";
        assert_eq!(parse_pdfium_version(text).as_deref(), Some("PDFium 8125"));
        assert_eq!(parse_pdfium_version("nothing here"), None);
    }

    #[test]
    fn render_sizes_are_limited() {
        assert!(check_render_size(1200, 1600).is_ok());
        assert!(check_render_size(0, 10).is_err());
        assert!(check_render_size(10, 0).is_err());
        assert!(check_render_size(MAX_RENDER_SIDE + 1, 1).is_err());
        assert!(check_render_size(MAX_RENDER_SIDE, MAX_RENDER_SIDE).is_err());
        assert!(check_render_size(4000, 6000).is_ok());
    }

    #[test]
    fn broken_page_sizes_fall_back_to_letter() {
        assert_eq!(sane_size(100.0, 200.0).width, 100.0);
        for (w, h) in [
            (0.0, 10.0),
            (-1.0, 10.0),
            (f32::NAN, 10.0),
            (10.0, f32::INFINITY),
        ] {
            let s = sane_size(w, h);
            assert_eq!((s.width, s.height), (612.0, 792.0));
        }
        // A huge MediaBox is cut to what PDF allows.
        let huge = sane_size(1.0e9, 100.0);
        assert_eq!((huge.width, huge.height), (14_400.0, 100.0));
    }

    #[test]
    fn a_missing_library_makes_every_request_fail_clearly() {
        let mut engine = Engine::new(Path::new("/definitely/not/here"));
        let response = engine.handle(Request {
            id: 9,
            body: RequestBody::Hello {
                protocol_version: PROTOCOL_VERSION,
            },
        });
        assert_eq!(response.id, 9);
        assert!(matches!(
            response.body,
            ResponseBody::Error {
                code: ErrorCode::EngineUnavailable,
                ..
            }
        ));
    }
}
