//! Running the independent tools: qpdf and Poppler. The verifier never loads PDFium and never
//! uses a PDF library of the app, so a bug in the code under test cannot hide in the check.

use std::ffi::OsStr;
use std::process::Command;

use crate::Error;

#[derive(Debug)]
pub struct Output {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

/// The program for `name`, from the environment variable `var` if it is set.
fn program(var: &str, name: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| name.to_owned())
}

fn run(var: &str, name: &str, args: &[&OsStr]) -> Result<Output, Error> {
    let prog = program(var, name);
    let out = Command::new(&prog).args(args).output().map_err(|e| {
        Error(format!(
            "cannot run {prog}: {e}. The verifier needs qpdf and Poppler (pdftotext, pdfimages) on the PATH, \
             or {var} set to the program"
        ))
    })?;
    Ok(Output {
        status: out.status.code().unwrap_or(-1),
        stdout: out.stdout,
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

pub fn qpdf(args: &[&OsStr]) -> Result<Output, Error> {
    run("QPDF", "qpdf", args)
}

pub fn pdftotext(args: &[&OsStr]) -> Result<Output, Error> {
    run("PDFTOTEXT", "pdftotext", args)
}

pub fn pdfimages(args: &[&OsStr]) -> Result<Output, Error> {
    run("PDFIMAGES", "pdfimages", args)
}

/// Poppler's renderer. It draws the pages that the redacted regions are checked on.
pub fn pdftoppm(args: &[&OsStr]) -> Result<Output, Error> {
    run("PDFTOPPM", "pdftoppm", args)
}

/// Versions of the tools, for the report.
pub fn versions() -> Vec<String> {
    let mut out = Vec::new();
    for (var, name, flag) in [
        ("QPDF", "qpdf", "--version"),
        ("PDFTOTEXT", "pdftotext", "-v"),
    ] {
        if let Ok(o) = run(var, name, &[OsStr::new(flag)]) {
            let text = if o.stdout.is_empty() {
                o.stderr
            } else {
                String::from_utf8_lossy(&o.stdout).into_owned()
            };
            if let Some(line) = text.lines().next() {
                out.push(line.to_owned());
            }
        }
    }
    out
}
