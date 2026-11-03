//! The engine's part of signing a document with a certificate.
//!
//! The engine never sees a key or a certificate. `Begin` writes the document out as for a save
//! (written twice, so that nothing that nothing refers to is in the file), adds the empty
//! signature to the end (see `signprep`), and keeps the bytes for the pieces that `SaveChunk`
//! hands out. `Fields` lists the signature fields the pages have, so the window can offer them.
//! `Fingerprint` describes the pages like the plain one, without counting a signature field among
//! the annotations, so that the document before signing and the file after it can be compared.
//!
//! Where the signature goes is given in points from the top left of the page as it is drawn; the
//! engine turns that into the coordinates of the page with the same conversion PDFium uses to draw
//! it, so that a page that is turned, or whose box does not start at zero, gets the signature where
//! the person put it, and upright.

use partridge_protocol::{
    ErrorCode, ResponseBody, SignField, SignPlace, SignRequest, SignResponse, SignSpec,
    MAX_SIGN_FIELDS, MIN_SIGN_SIDE,
};
use pdfium_render::prelude::*;

use crate::signlook::{self, Look};
use crate::signprep::{self, Place, Refusal};
use crate::text::Mapper;
use crate::{annots, fail, save, sigcheck, Engine, Failure};

/// A box that is too small to hold anything legible is a field without a look.
const SMALLEST_LOOK: f32 = 12.0;

fn refusal(r: Refusal) -> Failure {
    match r {
        Refusal::Encrypted => (
            ErrorCode::SignEncrypted,
            "the document is encrypted, so it cannot be signed".to_string(),
        ),
        Refusal::Xfa => (
            ErrorCode::SignXfa,
            "the document has an XFA form, so it cannot be signed".to_string(),
        ),
        Refusal::AlreadySigned => (
            ErrorCode::SignAlreadySigned,
            "the document holds signatures already".to_string(),
        ),
        Refusal::Structure(why) => (ErrorCode::SignStructure, why),
        Refusal::Refused(why) => (ErrorCode::SignRefused, why),
        Refusal::TooLarge => (
            ErrorCode::SaveTooLarge,
            "the document is too big to be signed".to_string(),
        ),
    }
}

/// Where a box of the drawn page lies on the page, and how its look is turned to fit.
struct Geometry {
    bounds: [f32; 4],
    matrix: [f32; 4],
    width: f32,
    height: f32,
}

fn geometry(page: &PdfPage, rect: [f32; 4]) -> Option<Geometry> {
    let mapper = Mapper::new(page);
    let (x, y) = (rect[0], rect[1]);
    let (width, height) = (rect[2] - rect[0], rect[3] - rect[1]);
    let corner = |dx: f32, dy: f32| mapper.to_page(page, x + dx, y + height - dy);
    let (o, u, v) = (corner(0.0, 0.0)?, corner(width, 0.0)?, corner(0.0, height)?);
    Some(Geometry {
        bounds: annots::page_bounds(page, &mapper, rect)?,
        matrix: [
            (u.0 - o.0) / width,
            (u.1 - o.1) / width,
            (v.0 - o.0) / height,
            (v.1 - o.1) / height,
        ],
        width,
        height,
    })
}

/// The box of a signature field of the page, by its full name, as drawn.
fn field_rect(page: &PdfPage, name: &str) -> Option<[f32; 4]> {
    let mapper = Mapper::new(page);
    for annotation in page.annotations().iter() {
        let Some(field) = annotation.as_form_field() else {
            continue;
        };
        if field.field_type() != PdfFormFieldType::Signature {
            continue;
        }
        if field.name().as_deref() != Some(name) {
            continue;
        }
        return mapper.rect(page, &annotation.bounds().ok()?);
    }
    None
}

pub(crate) fn handle(
    engine: &mut Engine,
    pdfium: &'static Pdfium,
    request: SignRequest,
    progress: &mut dyn FnMut(u32),
) -> Result<ResponseBody, Failure> {
    match request {
        SignRequest::Begin(spec) => begin(engine, pdfium, spec, progress),
        SignRequest::Fields { doc } => fields(engine, doc, progress),
        SignRequest::Fingerprint { doc } => {
            let document = engine.document(doc)?;
            engine.check_source(progress)?;
            let print = save::fingerprint_with(document, true, progress);
            engine.check_source(progress)?;
            Ok(print)
        }
    }
}

fn begin(
    engine: &mut Engine,
    pdfium: &'static Pdfium,
    spec: SignSpec,
    progress: &mut dyn FnMut(u32),
) -> Result<ResponseBody, Failure> {
    let doc = spec.doc;
    // A buffer from an earlier save or signature is gone before the next one is made.
    engine.saves.remove(&doc);
    let document = engine.document(doc)?;
    engine.check_source(progress)?;
    if save::security_revision(document) != 0 {
        return Err(refusal(Refusal::Encrypted));
    }
    let name = spec.name.trim();
    if name.is_empty() {
        return fail(ErrorCode::SignRefused, "the signature needs a name");
    }
    let pages = document.pages().len() as usize;

    // Where it goes, and what it looks like there.
    let lines = {
        let mut lines = vec![format!("Digitally signed by {name}")];
        if !spec.reason.trim().is_empty() {
            lines.push(format!("Reason: {}", spec.reason.trim()));
        }
        if !spec.location.trim().is_empty() {
            lines.push(format!("Place: {}", spec.location.trim()));
        }
        lines.push(format!("Date: {}", sigcheck::format_time(spec.signed_at)));
        lines
    };
    let make_look = |g: &Geometry| -> Result<Look, Failure> {
        signlook::build(
            g.width,
            g.height,
            g.matrix,
            &lines,
            spec.mark.as_ref(),
            spec.color,
        )
        .map_err(|why| (ErrorCode::SignRefused, why))
    };
    let (page_index, look, field_name, bounds): (usize, Option<Look>, Option<String>, [f32; 4]) =
        match &spec.place {
            SignPlace::Invisible => (0, None, None, [0.0; 4]),
            SignPlace::Rect { page, rect } => {
                let index = *page as usize;
                if index >= pages {
                    return fail(ErrorCode::PageOutOfRange, "the page does not exist");
                }
                let page = document
                    .pages()
                    .get(index as PdfPageIndex)
                    .map_err(|_| (ErrorCode::Corrupt, "the page could not be read".to_string()))?;
                let (pw, ph) = (page.width().value, page.height().value);
                let slack = 0.5;
                let inside = rect[0] >= -slack
                    && rect[1] >= -slack
                    && rect[2] <= pw + slack
                    && rect[3] <= ph + slack;
                if !inside
                    || rect[2] - rect[0] < MIN_SIGN_SIDE
                    || rect[3] - rect[1] < MIN_SIGN_SIDE
                {
                    return fail(
                        ErrorCode::SignRefused,
                        "the box of the signature is outside the page or too small",
                    );
                }
                let Some(g) = geometry(&page, *rect) else {
                    return fail(ErrorCode::SignRefused, "that place is not on the page");
                };
                let look = make_look(&g)?;
                (index, Some(look), None, g.bounds)
            }
            SignPlace::Field { page, name } => {
                let index = *page as usize;
                if index >= pages {
                    return fail(ErrorCode::PageOutOfRange, "the page does not exist");
                }
                let page = document
                    .pages()
                    .get(index as PdfPageIndex)
                    .map_err(|_| (ErrorCode::Corrupt, "the page could not be read".to_string()))?;
                let Some(rect) = field_rect(&page, name) else {
                    return fail(
                        ErrorCode::SignRefused,
                        "the page has no signature field of that name",
                    );
                };
                let look = if rect[2] - rect[0] >= SMALLEST_LOOK && rect[3] - rect[1] >= SMALLEST_LOOK {
                    match geometry(&page, rect) {
                        Some(g) => Some(make_look(&g)?),
                        None => None,
                    }
                } else {
                    None
                };
                (index, look, Some(name.clone()), [0.0; 4])
            }
        };

    let password = engine.passwords.get(&doc).map(|s| s.0.as_str());
    let base = save::serialise_clean(pdfium, document, password, progress)?;
    engine.check_source(progress)?;
    let place = match (&spec.place, &field_name) {
        (SignPlace::Invisible, _) => Place::Invisible,
        (SignPlace::Rect { .. }, _) => Place::New { bounds },
        (SignPlace::Field { .. }, Some(name)) => Place::Existing { name },
        (SignPlace::Field { .. }, None) => unreachable!("a field has a name"),
    };
    let prepared = signprep::prepare(
        base,
        pages,
        &signprep::Spec {
            page: page_index,
            place,
            name,
            reason: spec.reason.trim(),
            location: spec.location.trim(),
            signed_at: spec.signed_at,
            reserve: spec.reserve as usize,
            look: look.as_ref(),
        },
    )
    .map_err(refusal)?;
    let size = prepared.bytes.len() as u32;
    let response = SignResponse::Ready {
        size,
        hole_start: prepared.hole_start as u32,
        hole_end: prepared.hole_end as u32,
        field: prepared.field,
    };
    engine.saves.insert(doc, prepared.bytes);
    Ok(ResponseBody::Sign(response))
}

fn fields(
    engine: &mut Engine,
    doc: u32,
    progress: &mut dyn FnMut(u32),
) -> Result<ResponseBody, Failure> {
    let document = engine.document(doc)?;
    engine.check_source(progress)?;
    let mut out: Vec<SignField> = Vec::new();
    'pages: for (index, page) in document.pages().iter().enumerate() {
        let mapper = Mapper::new(&page);
        for annotation in page.annotations().iter() {
            let Some(field) = annotation.as_form_field() else {
                continue;
            };
            if field.field_type() != PdfFormFieldType::Signature {
                continue;
            }
            let (Some(name), Ok(bounds)) = (field.name(), annotation.bounds()) else {
                continue;
            };
            let Some(rect) = mapper.rect(&page, &bounds) else {
                continue;
            };
            if name.is_empty()
                || name.len() > partridge_protocol::MAX_SIGN_TEXT_BYTES
                || !partridge_protocol::is_plain_text(&name)
            {
                continue;
            }
            out.push(SignField {
                page: index as u32,
                rect,
                name,
            });
            if out.len() >= MAX_SIGN_FIELDS {
                break 'pages;
            }
        }
        if index % 32 == 31 {
            progress(index as u32 + 1);
        }
    }
    engine.check_source(progress)?;
    Ok(ResponseBody::Sign(SignResponse::Fields { fields: out }))
}
