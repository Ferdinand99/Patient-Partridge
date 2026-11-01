//! Serialising a document for a save.
//!
//! The engine writes nothing to disk, and the sandbox keeps it that way. A save therefore happens
//! in three steps, all of them requests from the app: `SaveBegin` has PDFium write the whole
//! document into a buffer in memory, `SaveChunk` hands that buffer over piece by piece, and
//! `SaveEnd` frees it. The app writes the file.
//!
//! PDFium writes the document with `FPDF_SaveAsCopy` and no flags. That is a full rewrite: every
//! object is written again, so the bytes that a digital signature covers are not kept. The flag
//! for an incremental save (`FPDF_INCREMENTAL`, which appends the changes to the original bytes
//! and so keeps a signature valid) exists in PDFium's C interface, but `pdfium-render` does not
//! pass flags on, and the document handle that the call needs is not public. `signatures` in
//! `DocInfo` is what lets the app warn before a rewrite breaks a signature.

use std::io::{self, Write};

use partridge_protocol::{ErrorCode, ResponseBody, MAX_SAVE_BYTES};
use pdfium_render::prelude::*;

use crate::Failure;

/// A writer that refuses to grow past [`MAX_SAVE_BYTES`] and says so, so that a document that is
/// too big can be told from a document PDFium could not write.
struct Capped {
    bytes: Vec<u8>,
    limit: usize,
    overflowed: bool,
}

impl Write for Capped {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.bytes.len() + data.len() > self.limit {
            self.overflowed = true;
            return Err(io::Error::other(
                "the document is bigger than the save limit",
            ));
        }
        self.bytes.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Writes the document out as PDFium would write a copy of it, with every change made so far.
pub fn serialise(document: &PdfDocument<'_>) -> Result<Vec<u8>, Failure> {
    serialise_up_to(document, MAX_SAVE_BYTES)
}

/// As [`serialise`], with a limit of `limit` bytes (a test makes it small).
pub fn serialise_up_to(document: &PdfDocument<'_>, limit: usize) -> Result<Vec<u8>, Failure> {
    let mut out = Capped {
        bytes: Vec::new(),
        limit,
        overflowed: false,
    };
    let result = document.save_to_writer(&mut out);
    if out.overflowed {
        return Err((
            ErrorCode::SaveTooLarge,
            "the document is too big to be saved".to_string(),
        ));
    }
    if result.is_err() {
        return Err((
            ErrorCode::SaveFailed,
            "the document could not be written".to_string(),
        ));
    }
    // A file that does not even start like a PDF is not handed over.
    if !out.bytes.starts_with(b"%PDF-") {
        return Err((
            ErrorCode::SaveFailed,
            "the document was written in a form that is not a PDF".to_string(),
        ));
    }
    Ok(out.bytes)
}

/// The revision of the standard security handler: 2 to 4 as PDFium reports it, 0 for a document
/// that is not encrypted, and 255 for an encrypted document whose revision PDFium does not name
/// (the newer AES-256 handlers).
pub fn security_revision(document: &PdfDocument<'_>) -> u8 {
    match document.permissions().security_handler_revision() {
        Ok(PdfSecurityHandlerRevision::Unprotected) => 0,
        Ok(PdfSecurityHandlerRevision::Revision2) => 2,
        Ok(PdfSecurityHandlerRevision::Revision3) => 3,
        Ok(PdfSecurityHandlerRevision::Revision4) => 4,
        Err(_) => 255,
    }
}

/// What the app needs to know before it saves.
pub fn doc_info(document: &PdfDocument<'_>) -> ResponseBody {
    ResponseBody::DocInfo {
        pages: document.pages().len() as u32,
        security: security_revision(document),
        signatures: document.signatures().len() as u32,
    }
}
