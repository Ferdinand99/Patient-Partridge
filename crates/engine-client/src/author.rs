//! The form designer, from the app's side: asking the engine for the widgets of a page, and for a
//! batch of changes to be made.
//!
//! What is sent is held to the caps here, so that the encoder never has to cut anything (a name that
//! is cut is another name). The answers are checked before they are passed on.

use partridge_protocol::{
    AuthorOp, AuthorRequest, AuthorResponse, DesignField, DesignProps, DesignWidget,
    DesignedWidget, ErrorCode, RequestBody, ResponseBody, MAX_AUTHOR_NAMES, MAX_AUTHOR_OPS,
    MAX_AUTHOR_WIDGETS, MAX_EXPORT_VALUE, MAX_FORM_FIELDS_PER_PAGE, MAX_FORM_NAME,
    MAX_FORM_OPTIONS, MAX_FORM_OPTION_LABEL, MAX_FORM_TOOLTIP, MAX_FORM_VALUE, MAX_LISTED_WIDGETS,
};

use crate::pages::{accept_changed, PagesChanged};
use crate::{ClientError, EngineClient};

fn refuse(message: &str) -> ClientError {
    ClientError::Engine {
        code: ErrorCode::InvalidRequest,
        message: message.into(),
    }
}

fn props_ok(p: &DesignProps) -> bool {
    p.tooltip.len() <= MAX_FORM_TOOLTIP
        && p.default_value.len() <= MAX_FORM_VALUE
        && p.export_value.len() <= MAX_EXPORT_VALUE
        && p.choices.len() <= MAX_FORM_OPTIONS
        && p.choices.iter().all(|c| c.len() <= MAX_FORM_OPTION_LABEL)
}

fn widget_ok(w: &DesignWidget) -> bool {
    w.export.len() <= MAX_EXPORT_VALUE
        && w.rect
            .iter()
            .all(|v| v.is_finite() && v.abs() <= partridge_protocol::MAX_BOX_VALUE)
}

fn field_ok(f: &DesignField) -> bool {
    f.name.len() <= MAX_FORM_NAME
        && props_ok(&f.props)
        && f.widgets.len() <= MAX_AUTHOR_WIDGETS
        && f.widgets.iter().all(widget_ok)
}

/// A batch the protocol can carry whole.
fn ops_ok(ops: &[AuthorOp]) -> Result<(), ClientError> {
    if ops.len() > MAX_AUTHOR_OPS {
        return Err(refuse("too many changes at once"));
    }
    let fine = ops.iter().all(|op| match op {
        AuthorOp::Add(f) => field_ok(f),
        AuthorOp::Remove { name } | AuthorOp::RemoveWidget { name, .. } => {
            name.len() <= MAX_FORM_NAME
        }
        AuthorOp::AddWidget { name, widget } => name.len() <= MAX_FORM_NAME && widget_ok(widget),
        AuthorOp::Move { name, rect, .. } => {
            name.len() <= MAX_FORM_NAME
                && rect
                    .iter()
                    .all(|v| v.is_finite() && v.abs() <= partridge_protocol::MAX_BOX_VALUE)
        }
        AuthorOp::Update {
            name,
            new_name,
            props,
            exports,
        } => {
            name.len() <= MAX_FORM_NAME
                && new_name.len() <= MAX_FORM_NAME
                && props_ok(props)
                && exports.len() <= MAX_AUTHOR_WIDGETS
                && exports.iter().all(|e| e.len() <= MAX_EXPORT_VALUE)
        }
        AuthorOp::Reorder { order, .. } => {
            order.len() <= MAX_FORM_FIELDS_PER_PAGE
                && order.iter().all(|r| r.name.len() <= MAX_FORM_NAME)
        }
    });
    if fine {
        Ok(())
    } else {
        Err(refuse("a name or a value is too long"))
    }
}

impl EngineClient {
    /// Whether the form designer can work on the document, and the names of the fields it has.
    pub fn author_check(&self, doc: u64) -> Result<Vec<String>, ClientError> {
        self.call(
            doc,
            "form designer check",
            |doc| RequestBody::Author(AuthorRequest::Check { doc }),
            |body| match body {
                ResponseBody::Author(AuthorResponse::Ready { names })
                    if names.len() <= MAX_AUTHOR_NAMES =>
                {
                    Ok(names)
                }
                other => Err(other),
            },
        )
    }

    /// The widgets of one page that the designer lists.
    pub fn author_list(&self, doc: u64, page: u32) -> Result<Vec<DesignedWidget>, ClientError> {
        self.call(
            doc,
            "form designer list",
            |doc| RequestBody::Author(AuthorRequest::List { doc, page }),
            |body| match body {
                ResponseBody::Author(AuthorResponse::Widgets { items })
                    if items.len() <= MAX_LISTED_WIDGETS
                        && items
                            .iter()
                            .all(|w| w.widget < w.widgets.max(1) && w.rect.iter().all(|v| v.is_finite())) =>
                {
                    Ok(items)
                }
                other => Err(other),
            },
        )
    }

    /// Makes the changes of a batch with one reload of the document. A name or a value over the caps
    /// is refused here, before it is sent.
    pub fn author_apply(&self, doc: u64, ops: &[AuthorOp]) -> Result<PagesChanged, ClientError> {
        ops_ok(ops)?;
        self.call(
            doc,
            "form designer change",
            |doc| {
                RequestBody::Author(AuthorRequest::Apply {
                    doc,
                    ops: ops.to_vec(),
                })
            },
            accept_changed,
        )
    }
}
