//! Draws one page of a PDF with PDFium and writes it as a binary PPM (P6), for the x-ray render of
//! the redaction verifier. It is a program of its own: a PDFium that crashes or hangs on a crafted
//! file ends this program and not the verifier, which treats it as "could not check".
//!
//! ```text
//! redaction-verify-pdfium --dir PDFIUM_FOLDER --pdf FILE --page N --width W --height H --out FILE.ppm
//! ```
//!
//! The page is drawn the way a reader draws it for the screen: annotations and form fields included,
//! no highlight of the fields, on a white page. `--page` counts from 1. Exit status 0 when the file
//! was written, 2 for anything else (the reason is on standard error).

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use pdfium_render::prelude::*;

struct Args {
    dir: PathBuf,
    pdf: PathBuf,
    page: i32,
    width: i32,
    height: i32,
    out: PathBuf,
}

fn args() -> Result<Args, String> {
    let mut dir = None;
    let mut pdf = None;
    let mut page = None;
    let mut width = None;
    let mut height = None;
    let mut out = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "--dir" => dir = Some(PathBuf::from(value("--dir")?)),
            "--pdf" => pdf = Some(PathBuf::from(value("--pdf")?)),
            "--page" => page = value("--page")?.parse::<i32>().ok(),
            "--width" => width = value("--width")?.parse::<i32>().ok(),
            "--height" => height = value("--height")?.parse::<i32>().ok(),
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            other => return Err(format!("unknown option {other}")),
        }
    }
    let need = "needs --dir, --pdf, --page, --width, --height and --out";
    Ok(Args {
        dir: dir.ok_or(need)?,
        pdf: pdf.ok_or(need)?,
        page: page.filter(|p| *p >= 1).ok_or(need)?,
        width: width.filter(|w| *w > 0).ok_or(need)?,
        height: height.filter(|h| *h > 0).ok_or(need)?,
        out: out.ok_or(need)?,
    })
}

fn run() -> Result<(), String> {
    let a = args()?;
    let library = Pdfium::pdfium_platform_library_name_at_path(&a.dir);
    let bindings = Pdfium::bind_to_library(&library).map_err(|e| {
        format!(
            "the PDFium library {} could not be loaded: {e}",
            library.display()
        )
    })?;
    let pdfium = Pdfium::new(bindings);
    let doc = pdfium
        .load_pdf_from_file(&a.pdf, None)
        .map_err(|e| format!("PDFium could not open {}: {e}", a.pdf.display()))?;
    let page = doc
        .pages()
        .get(a.page - 1)
        .map_err(|e| format!("PDFium could not read page {}: {e}", a.page))?;
    let config = PdfRenderConfig::new().set_target_size(a.width, a.height);
    let bitmap = page
        .render_with_config(&config)
        .map_err(|e| format!("PDFium could not draw page {}: {e}", a.page))?;
    let (w, h) = (bitmap.width() as usize, bitmap.height() as usize);
    let rgba = bitmap.as_rgba_bytes();
    if w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return Err("PDFium drew an empty bitmap".to_owned());
    }
    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    for px in rgba.chunks_exact(4).take(w * h) {
        ppm.extend_from_slice(&px[..3]);
    }
    let mut f = std::fs::File::create(&a.out).map_err(|e| e.to_string())?;
    f.write_all(&ppm).map_err(|e| e.to_string())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("redaction-verify-pdfium: {e}");
            ExitCode::from(2)
        }
    }
}
