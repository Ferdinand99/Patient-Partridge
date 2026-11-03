//! The form designer's requests, answered by the engine: reads the document as PDFium writes it,
//! gives the page geometry that the writer needs, and puts the result in place of the open
//! document.
//!
//! A change is one reload of the document, whatever the number of changes in the batch. The copy of
//! the document before it is kept as one of the copies the page tools keep, so a step back and a
//! step forward are the page tools' `Restore`. What the engine knows of the annotations and of the
//! form is brought in step with the new document.

use std::collections::{BTreeSet, HashMap};

use partridge_protocol::ErrorCode;
use partridge_protocol::{
    AuthorOp, AuthorRequest, AuthorResponse, DesignedWidget, FormKind, PagesResponse, ResponseBody,
    MAX_AUTHOR_ANSWER_BYTES, MAX_AUTHOR_NAMES, MAX_AUTHOR_OPS, MAX_SAVE_BYTES, MAX_UNDO_BYTES,
};
use pdfium_render::prelude::*;

use super::{Author, AuthorError, PageGeo, Unsupported};
use crate::{annots, fail, forms, pages, save, Engine, Failure};

/// Most bytes of a written-out document kept for the designer's reads.
const KEEP_LIMIT: usize = 64 * 1024 * 1024;

fn unsupported(why: Unsupported) -> Failure {
    (ErrorCode::AuthorUnsupported, why.message())
}

/// The reasons the designer cannot work on a document that PDFium already tells.
fn check_document(pdfium: &'static Pdfium, document: &PdfDocument<'_>) -> Result<(), Failure> {
    if save::security_revision(document) != 0 {
        return Err(unsupported(Unsupported::Encrypted));
    }
    if forms::kind_of(pdfium, document) == Some(FormKind::Xfa) {
        return fail(
            ErrorCode::AuthorUnsupported,
            "this form is an XFA form, which the form designer does not support",
        );
    }
    Ok(())
}

fn geometry_of(document: &PdfDocument<'_>, page: usize) -> Result<PageGeo, Failure> {
    let p = document.pages().get(page as PdfPageIndex).map_err(|_| {
        (
            ErrorCode::PageOutOfRange,
            "the page does not exist".to_string(),
        )
    })?;
    let visible = pages::visible_box(&p).ok_or((
        ErrorCode::AuthorUnsupported,
        "the page has no size the form designer can use".to_string(),
    ))?;
    let turns = p.rotation().map(pages::turns_of).unwrap_or(0);
    Ok(PageGeo { turns, visible })
}

/// The document written out: the copy from the last read when nothing has changed it since, else
/// a new one.
fn bytes_for(
    engine: &mut Engine,
    doc: u32,
    progress: &mut dyn FnMut(u32),
) -> Result<Vec<u8>, Failure> {
    if let Some(bytes) = engine.author_bytes.remove(&doc) {
        return Ok(bytes);
    }
    engine.check_source(progress)?;
    let document = engine.document(doc)?;
    let bytes = save::serialise_up_to(document, MAX_SAVE_BYTES, progress)?;
    engine.check_source(progress)?;
    Ok(bytes)
}

fn keep(engine: &mut Engine, doc: u32, bytes: Vec<u8>) {
    if bytes.len() <= KEEP_LIMIT {
        engine.author_bytes.insert(doc, bytes);
    }
}

fn failure(error: AuthorError) -> Failure {
    error.into_failure()
}

pub(crate) fn handle(
    engine: &mut Engine,
    pdfium: &'static Pdfium,
    request: AuthorRequest,
    progress: &mut dyn FnMut(u32),
) -> Result<ResponseBody, Failure> {
    match request {
        AuthorRequest::Check { doc } => check(engine, pdfium, doc, progress),
        AuthorRequest::List { doc, page } => list(engine, pdfium, doc, page, progress),
        AuthorRequest::Apply { doc, ops } => apply(engine, pdfium, doc, ops, progress),
    }
}

fn check(
    engine: &mut Engine,
    pdfium: &'static Pdfium,
    doc: u32,
    progress: &mut dyn FnMut(u32),
) -> Result<ResponseBody, Failure> {
    let count = {
        let document = engine.document(doc)?;
        check_document(pdfium, document)?;
        document.pages().len() as usize
    };
    let bytes = bytes_for(engine, doc, progress)?;
    // The pages are the ones PDFium counts: a page tree that lists another number is refused.
    let mut author = Author::with_count(bytes, HashMap::new(), count).map_err(failure)?;
    let mut names = author.names().map_err(failure)?;
    names.truncate(MAX_AUTHOR_NAMES);
    keep(engine, doc, author.into_bytes());
    Ok(ResponseBody::Author(AuthorResponse::Ready { names }))
}

/// How many bytes a listed widget takes on the wire, near enough.
fn weight(w: &DesignedWidget) -> usize {
    128 + w.name.len()
        + w.export.len()
        + w.props.tooltip.len()
        + w.props.default_value.len()
        + w.props.choices.iter().map(|c| c.len() + 4).sum::<usize>()
}

fn list(
    engine: &mut Engine,
    pdfium: &'static Pdfium,
    doc: u32,
    page: u32,
    progress: &mut dyn FnMut(u32),
) -> Result<ResponseBody, Failure> {
    let (count, geo) = {
        let document = engine.document(doc)?;
        check_document(pdfium, document)?;
        let count = document.pages().len() as usize;
        if page as usize >= count {
            return fail(ErrorCode::PageOutOfRange, "the page does not exist");
        }
        (count, geometry_of(document, page as usize)?)
    };
    let bytes = bytes_for(engine, doc, progress)?;
    let mut author =
        Author::with_count(bytes, HashMap::from([(page as usize, geo)]), count).map_err(failure)?;
    let mut items = author.list_page(page as usize).map_err(failure)?;
    keep(engine, doc, author.into_bytes());
    let mut used = 64;
    let mut kept = 0;
    for item in &items {
        used += weight(item);
        if used > MAX_AUTHOR_ANSWER_BYTES {
            break;
        }
        kept += 1;
    }
    items.truncate(kept);
    Ok(ResponseBody::Author(AuthorResponse::Widgets { items }))
}

/// The pages the changes of a batch put something on, which need their geometry.
fn pages_named(ops: &[AuthorOp]) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    for op in ops {
        match op {
            AuthorOp::Add(field) => out.extend(field.widgets.iter().map(|w| w.page as usize)),
            AuthorOp::AddWidget { widget, .. } => {
                out.insert(widget.page as usize);
            }
            AuthorOp::Move { page, .. } => {
                out.insert(*page as usize);
            }
            AuthorOp::Remove { .. }
            | AuthorOp::RemoveWidget { .. }
            | AuthorOp::Update { .. }
            | AuthorOp::Reorder { .. } => {}
        }
    }
    out
}

fn apply(
    engine: &mut Engine,
    pdfium: &'static Pdfium,
    doc: u32,
    ops: Vec<AuthorOp>,
    progress: &mut dyn FnMut(u32),
) -> Result<ResponseBody, Failure> {
    if ops.len() > MAX_AUTHOR_OPS {
        return fail(
            ErrorCode::InvalidRequest,
            format!("a batch holds at most {MAX_AUTHOR_OPS} changes"),
        );
    }
    let count = pages::designer_ready(engine, doc)?;
    let mut geo = HashMap::new();
    {
        let document = engine.document(doc)?;
        check_document(pdfium, document)?;
        for page in pages_named(&ops) {
            if page >= count {
                return fail(ErrorCode::PageOutOfRange, "the page does not exist");
            }
            geo.insert(page, geometry_of(document, page)?);
        }
    }
    if ops.is_empty() {
        let document = engine.document(doc)?;
        return Ok(ResponseBody::Pages(PagesResponse::Changed {
            pages: pages::page_sizes(document),
            snapshot: 0,
            count: 0,
            form_fields: 0,
        }));
    }
    let bytes = bytes_for(engine, doc, progress)?;
    let original_len = bytes.len();
    let mut author = Author::with_count(bytes, geo, count).map_err(failure)?;
    author.apply(&ops).map_err(failure)?;
    let outcome = author.finish().map_err(failure)?;
    if outcome.bytes.len() > MAX_SAVE_BYTES {
        return fail(
            ErrorCode::AuthorRefused,
            "the document would be too big to save with these fields",
        );
    }
    let before = (original_len <= MAX_UNDO_BYTES).then(|| outcome.bytes[..original_len].to_vec());
    let scan = engine.forms.get(&doc).map(forms::FormEnv::scan_to_keep);
    let super::Outcome {
        bytes,
        maps,
        redraw,
    } = outcome;
    let token = pages::replace_document(engine, pdfium, doc, before, bytes)?;
    if let Some(known) = engine.annots.get_mut(&doc) {
        known.reconcile(&maps);
    }
    let document = engine.document(doc)?;
    for page in &redraw {
        annots::settle_page(document, *page as u32);
    }
    if let Some(scan) = scan {
        // The new fields carry no action, so what the search of the form found still holds.
        if let Ok(env) = forms::FormEnv::create_with_scan(pdfium, document, scan) {
            engine.forms.insert(doc, env);
        }
    }
    let document = engine.document(doc)?;
    Ok(ResponseBody::Pages(PagesResponse::Changed {
        pages: pages::page_sizes(document),
        snapshot: token,
        count: ops.len() as u32,
        form_fields: 0,
    }))
}
