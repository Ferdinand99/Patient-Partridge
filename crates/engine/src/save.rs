//! Serialising a document for a save, and describing it before and after.
//!
//! The engine writes nothing to disk, and the sandbox keeps it that way. A save therefore happens
//! in steps, all of them requests from the app: `DocInfo` and `Fingerprint` describe the document
//! as it is, `SaveBegin` has PDFium write the whole document into a buffer in memory, `SaveChunk`
//! hands that buffer over piece by piece, and `SaveEnd` frees it. The app writes the file.
//!
//! PDFium writes the document with `FPDF_SaveAsCopy` and no flags. That is a full rewrite: every
//! object is written again, so the bytes that a digital signature covers are not kept. The flag
//! for an incremental save (`FPDF_INCREMENTAL`, which appends the changes to the original bytes
//! and so keeps a signature valid) exists in PDFium's C interface, but `pdfium-render` does not
//! pass flags on, and the document handle that the call needs is not public. `DocInfo` is what
//! lets the app warn before a rewrite breaks a signature.

use std::io::{self, Write};
use std::sync::Mutex;

use partridge_protocol::{ErrorCode, ResponseBody, MAX_SAVE_BYTES};
use pdfium_render::prelude::*;

use crate::source::{fnv, Problem, SourceGuard};
use crate::Failure;

/// What in the bytes of a file belongs to a signature. A signature dictionary always has a
/// `/ByteRange`; a certified document has a `/DocMDP`; a form with signatures has `/SigFlags`.
/// The words are found whatever field they sit in, which is the point: PDFium's own count follows
/// the field list, and a signature somewhere it does not look would not be counted.
const SIGNATURE_WORDS: [&[u8]; 3] = [b"/ByteRange", b"/DocMDP", b"/SigFlags"];

/// A writer that refuses to grow past its limit and says so, so that a document that is too big
/// can be told from a document PDFium could not write. It also says how far it has come.
struct Capped {
    bytes: Vec<u8>,
    limit: usize,
    overflowed: bool,
    since_progress: usize,
    /// Called with the number of bytes written, about every MiB. The pointer lives only as long
    /// as the call to `serialise_up_to` that made it (see there).
    progress: *mut (dyn FnMut(u32) + 'static),
}

/// How many bytes are written between two reports of progress.
const PROGRESS_STEP: usize = 1024 * 1024;

impl Write for Capped {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.bytes.len() + data.len() > self.limit {
            self.overflowed = true;
            return Err(io::Error::other(
                "the document is bigger than the save limit",
            ));
        }
        // A buffer that cannot grow is a failed save, not an engine that aborts and takes the
        // changes of the document with it.
        if self.bytes.try_reserve(data.len()).is_err() {
            self.overflowed = true;
            return Err(io::Error::other("no memory for the saved document"));
        }
        self.bytes.extend_from_slice(data);
        self.since_progress += data.len();
        if self.since_progress >= PROGRESS_STEP {
            self.since_progress = 0;
            // SAFETY: the pointer was made from a `&mut dyn FnMut` that outlives the call to
            // `save_to_writer`, which is the only time this method runs.
            unsafe { (*self.progress)(self.bytes.len().min(u32::MAX as usize) as u32) };
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Writes the document out as PDFium would write a copy of it, with every change made so far.
pub fn serialise(document: &PdfDocument<'_>) -> Result<Vec<u8>, Failure> {
    serialise_up_to(document, MAX_SAVE_BYTES, &mut |_| {})
}

/// As [`serialise`], with a limit of `limit` bytes (a test makes it small), and `progress` told
/// how many bytes are written now and then.
pub fn serialise_up_to(
    document: &PdfDocument<'_>,
    limit: usize,
    progress: &mut dyn FnMut(u32),
) -> Result<Vec<u8>, Failure> {
    // SAFETY: only the lifetime of the closure is changed, so that the writer (which must be
    // `'static` for `save_to_writer`) can hold it. `out` does not outlive this function, and the
    // pointer is used only inside the call to `save_to_writer`.
    let progress: *mut (dyn FnMut(u32) + 'static) = unsafe {
        std::mem::transmute::<*mut (dyn FnMut(u32) + '_), *mut (dyn FnMut(u32) + 'static)>(
            progress as *mut (dyn FnMut(u32) + '_),
        )
    };
    let mut out = Capped {
        bytes: Vec::new(),
        limit,
        overflowed: false,
        since_progress: 0,
        progress,
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

/// The failure to give when the file the document is read from is not to be trusted.
pub fn source_failure(problem: Problem) -> Failure {
    match problem {
        Problem::Changed => (
            ErrorCode::SourceChanged,
            "the file this document is read from has changed since it was opened".to_string(),
        ),
        Problem::Unreadable => (
            ErrorCode::SourceUnreadable,
            "a part of the file this document is read from could not be read".to_string(),
        ),
        Problem::TimedOut => (
            ErrorCode::Internal,
            "looking at the file this document is read from took longer than its time limit"
                .to_string(),
        ),
    }
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

/// How many signature fields the pages show: every widget annotation that PDFium's form code
/// takes for a signature field, whatever its place in the field tree and whether it has the type
/// itself or gets it from a parent.
fn signature_widgets(document: &PdfDocument<'_>, progress: &mut dyn FnMut(u32)) -> u32 {
    let mut count = 0u32;
    for (index, page) in document.pages().iter().enumerate() {
        for annotation in page.annotations().iter() {
            if let Some(field) = annotation.as_form_field() {
                if field.field_type() == PdfFormFieldType::Signature {
                    count += 1;
                }
            }
        }
        if index % 32 == 31 {
            progress(index as u32 + 1);
        }
    }
    count
}

/// How many signatures a document holds, as far as its fields tell. PDFium lists a signature
/// field whether or not anybody has signed it, so a form with a place to sign would count as
/// signed. Only a field with contents counts here; a field whose contents cannot be asked for
/// counts as well, because a warning that was not needed is cheap.
///
/// PDFium's list follows the entries of `/AcroForm /Fields`. The widgets on the pages also find a
/// field below a grouping field, but cannot say whether it holds a signature, so the widgets
/// beyond the fields PDFium listed are counted as signed. The result is never more than the
/// larger of the two counts, and an unsigned field is the only thing that makes it smaller.
fn signatures_held(listed: &PdfSignatures<'_>, widgets: u32) -> u32 {
    let total = listed.len();
    let signed = (0..total)
        .filter(|&index| match listed.get(index) {
            Ok(signature) => !signature.bytes().is_empty(),
            Err(_) => true,
        })
        .count() as u32;
    signed + widgets.saturating_sub(u32::from(total))
}

/// What the app needs to know before it saves.
pub fn doc_info(
    document: &PdfDocument<'_>,
    source: Option<&Mutex<SourceGuard>>,
    progress: &mut dyn FnMut(u32),
) -> ResponseBody {
    // PDFium reads the file through the guard while it looks at the pages, so the guard must not
    // be held here.
    let widgets = signature_widgets(document, progress);
    let held = signatures_held(document.signatures(), widgets);
    // When the file cannot be scanned, the document is treated as possibly signed.
    let maybe_signed = match source {
        Some(guard) => crate::source::lock(guard)
            .contains_any(&SIGNATURE_WORDS, progress)
            .unwrap_or(true),
        None => false,
    };
    let touched = source.is_some_and(|guard| crate::source::lock(guard).touched());
    ResponseBody::DocInfo {
        pages: document.pages().len() as u32,
        security: security_revision(document),
        signatures: held,
        maybe_signed,
        touched,
    }
}

/// A digest of what a reader sees of the document, page by page: the size, the text, the number
/// of page objects and of annotations. The same document written out and read again gives the
/// same digest; a page that lost its text or its drawing instructions does not.
pub fn fingerprint(document: &PdfDocument<'_>, progress: &mut dyn FnMut(u32)) -> ResponseBody {
    let pages = document.pages();
    let mut failed = 0u32;
    let mut digest: u64 = 0xcbf2_9ce4_8422_2325;
    let mut put = |bytes: &[u8]| {
        digest = fnv(&[&digest.to_le_bytes()[..], bytes].concat());
    };
    for index in 0..pages.len() {
        let Ok(page) = pages.get(index) else {
            failed += 1;
            put(b"unreadable page");
            continue;
        };
        put(&page.width().value.to_bits().to_le_bytes());
        put(&page.height().value.to_bits().to_le_bytes());
        match crate::text::page_chars(&page, false) {
            Ok(chars) => {
                let text: String = chars.iter().map(|c| c.ch).collect();
                put(&(chars.len() as u64).to_le_bytes());
                put(text.as_bytes());
            }
            Err(_) => {
                failed += 1;
                put(b"unreadable text");
            }
        }
        put(&(page.objects().len() as u64).to_le_bytes());
        put(&(page.annotations().len() as u64).to_le_bytes());
        if index % 32 == 31 {
            progress(index as u32 + 1);
        }
    }
    ResponseBody::Fingerprint {
        pages: pages.len() as u32,
        failed,
        digest,
    }
}
