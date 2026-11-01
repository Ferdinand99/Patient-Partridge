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

use partridge_protocol::{
    ErrorCode, PageSize, Request, RequestBody, Response, ResponseBody, MAX_RENDER_PIXELS,
    MAX_RENDER_SIDE, PROTOCOL_VERSION,
};
use pdfium_render::prelude::*;

/// Most documents the engine keeps open at once.
pub const MAX_OPEN_DOCUMENTS: usize = 64;

/// Command line of the engine.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    /// Folder that holds the PDFium library. The engine never searches anywhere else.
    pub pdfium_dir: Option<PathBuf>,
}

/// Reads the engine's command line (without the program name).
pub fn parse_args<I: IntoIterator<Item = OsString>>(args: I) -> Result<Args, String> {
    let mut out = Args::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == "--pdfium-dir" {
            let value = it.next().ok_or("--pdfium-dir needs a folder")?;
            out.pdfium_dir = Some(PathBuf::from(value));
        } else {
            return Err(format!("unknown argument {}", arg.to_string_lossy()));
        }
    }
    Ok(out)
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

/// A page size that can be laid out: finite and positive, else a Letter page.
fn sane_size(width: f32, height: f32) -> PageSize {
    if width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0 {
        PageSize { width, height }
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
}

impl Engine {
    /// Loads PDFium from `pdfium_dir`. When that fails the engine still starts and answers every
    /// request with an `EngineUnavailable` error, so the app can tell the user what is wrong.
    pub fn new(pdfium_dir: &Path) -> Engine {
        let version = std::fs::read_to_string(pdfium_dir.join("VERSION"))
            .ok()
            .and_then(|text| parse_pdfium_version(&text))
            .unwrap_or_else(|| "PDFium".to_string());
        Engine {
            pdfium: load_pdfium(pdfium_dir),
            pdfium_version: version,
            docs: HashMap::new(),
            next_doc: 1,
        }
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
                })
            }
            RequestBody::Open { path, password } => self.open(pdfium, &path, password.as_deref()),
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
        }
    }

    fn open(
        &mut self,
        pdfium: &'static Pdfium,
        path: &str,
        password: Option<&str>,
    ) -> Result<ResponseBody, Failure> {
        if self.docs.len() >= MAX_OPEN_DOCUMENTS {
            return fail(ErrorCode::InvalidRequest, "too many documents are open");
        }
        let file = File::open(path)
            .map_err(|e| (ErrorCode::Io, format!("the file could not be opened: {e}")))?;
        let is_file = file.metadata().map(|m| m.is_file()).unwrap_or(false);
        if !is_file {
            return fail(ErrorCode::Io, "the path is not a file");
        }
        // The file is read on demand by PDFium; it is not loaded into memory as a whole.
        let doc = pdfium
            .load_pdf_from_reader(file, password)
            .map_err(|e| map_open_error(e, password.is_some()))?;
        let sizes = doc.pages().page_sizes().map_err(|_| {
            (
                ErrorCode::Corrupt,
                "the page list could not be read".to_string(),
            )
        })?;
        let pages = sizes
            .iter()
            .map(|r| sane_size(r.width().value, r.height().value))
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

fn load_pdfium(dir: &Path) -> Result<&'static Pdfium, String> {
    let dir = std::fs::canonicalize(dir)
        .map_err(|e| format!("the PDFium folder {} is not usable: {e}", dir.display()))?;
    let library = Pdfium::pdfium_platform_library_name_at_path(&dir);
    let bindings = Pdfium::bind_to_library(&library).map_err(|e| {
        format!(
            "the PDFium library {} could not be loaded: {e}",
            library.display()
        )
    })?;
    // The engine lives as long as the process: leaking gives every document a 'static library.
    Ok(Box::leak(Box::new(Pdfium::new(bindings))))
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
