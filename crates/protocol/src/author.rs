//! The messages of the form designer: making fields, changing the ones that were made, taking
//! them away, and listing what the designer can work on.
//!
//! Places on a page are in points from the top left of the page as it is drawn (the same as the
//! boxes of the form fields): the engine turns them into the page's own coordinates and writes the
//! turn of a page into the field. Every list and string has a cap that the encoder and the decoder
//! both hold, so a damaged peer cannot make the other allocate or loop for ever.
//!
//! A new field never carries an action or a script, and there is nothing on the wire to ask for one.

use super::{
    put_f32, put_str_max, put_u32, ProtocolError, Reader, MAX_BOX_VALUE, MAX_FORM_NAME,
    MAX_FORM_OPTIONS, MAX_FORM_OPTION_LABEL, MAX_FORM_TOOLTIP, MAX_FORM_VALUE,
};

/// Most changes in one `Apply`. One reload of the document serves all of them.
pub const MAX_AUTHOR_OPS: usize = 100;

/// Most widgets (places on the pages) that one field may have.
pub const MAX_AUTHOR_WIDGETS: usize = 64;

/// Most names that `Check` hands back.
pub const MAX_AUTHOR_NAMES: usize = 5_000;

/// Longest export value of a check box or a radio button, in bytes.
pub const MAX_EXPORT_VALUE: usize = 128;

/// Largest number of characters a field may be limited to.
pub const MAX_FIELD_LENGTH: u32 = 32_767;

/// Most characters a comb field may have boxes for.
pub const MAX_COMB_LENGTH: u32 = 200;

/// Biggest answer to a request of the designer that lists things, in bytes. The engine leaves out
/// what does not fit, and the app refuses a longer one before it reads it.
pub const MAX_AUTHOR_ANSWER_BYTES: usize = 4 * 1024 * 1024;

/// Smallest side of a widget, in points.
pub const MIN_WIDGET_SIDE: f32 = 4.0;

/// Most widgets one `List` answer holds.
pub const MAX_LISTED_WIDGETS: usize = 1_000;

pub(crate) const FIRST_REQUEST_TAG: u8 = 100;
pub(crate) const LAST_REQUEST_TAG: u8 = 102;
pub(crate) const FIRST_RESPONSE_TAG: u8 = 210;
pub(crate) const LAST_RESPONSE_TAG: u8 = 211;

const TAG_CHECK: u8 = 100;
const TAG_LIST: u8 = 101;
const TAG_APPLY: u8 = 102;

const TAG_READY: u8 = 210;
const TAG_WIDGETS: u8 = 211;

// The tags of the designer are not the tags of the page tools.
const _: () = assert!(
    LAST_REQUEST_TAG < crate::pages::FIRST_REQUEST_TAG
        || FIRST_REQUEST_TAG > crate::pages::LAST_REQUEST_TAG
);
const _: () = assert!(
    LAST_RESPONSE_TAG < crate::pages::FIRST_RESPONSE_TAG
        || FIRST_RESPONSE_TAG > crate::pages::LAST_RESPONSE_TAG
);

const OP_ADD: u8 = 1;
const OP_REMOVE: u8 = 2;
const OP_REMOVE_WIDGET: u8 = 3;
const OP_ADD_WIDGET: u8 = 4;
const OP_MOVE: u8 = 5;
const OP_UPDATE: u8 = 6;
const OP_REORDER: u8 = 7;

/// The kinds of field the designer makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DesignKind {
    Text = 1,
    Checkbox = 2,
    /// One button of a group; a group is one field with several widgets.
    Radio = 3,
    Combo = 4,
    List = 5,
    /// A place to sign that nobody has signed.
    Signature = 6,
}

impl DesignKind {
    pub fn from_u8(v: u8) -> Result<DesignKind, ProtocolError> {
        Ok(match v {
            1 => DesignKind::Text,
            2 => DesignKind::Checkbox,
            3 => DesignKind::Radio,
            4 => DesignKind::Combo,
            5 => DesignKind::List,
            6 => DesignKind::Signature,
            other => return Err(ProtocolError::UnknownValue(other)),
        })
    }

    /// A short stable name.
    pub fn name(self) -> &'static str {
        match self {
            DesignKind::Text => "text",
            DesignKind::Checkbox => "checkbox",
            DesignKind::Radio => "radio",
            DesignKind::Combo => "combo",
            DesignKind::List => "list",
            DesignKind::Signature => "signature",
        }
    }
}

/// What a field is, apart from its name, its kind and where it sits.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DesignProps {
    pub tooltip: String,
    pub required: bool,
    pub read_only: bool,
    /// Text: more than one line.
    pub multiline: bool,
    /// Text: what is typed is hidden.
    pub password: bool,
    /// Text: one box for each character (needs `max_len`).
    pub comb: bool,
    /// Text: the most characters, or 0 for no limit.
    pub max_len: u32,
    /// Drop-down: the reader may type a value that is not in the list.
    pub editable: bool,
    /// List: more than one choice may be picked.
    pub multiselect: bool,
    /// Points, or 0 to fit the text to the height of the box.
    pub font_size: f32,
    /// Text: the text the field starts with. Choices: the labels picked at the start, one on each
    /// line. Check box: "on" for a box that starts checked. Radio: the export value of the button
    /// that starts on.
    pub default_value: String,
    /// Drop-down and list: the labels of the choices, in order.
    pub choices: Vec<String>,
    /// Check box: the word the box stands for when it is on. Empty is "Yes".
    pub export_value: String,
}

pub mod props_flag {
    pub const REQUIRED: u16 = 1;
    pub const READ_ONLY: u16 = 2;
    pub const MULTILINE: u16 = 4;
    pub const PASSWORD: u16 = 8;
    pub const COMB: u16 = 16;
    pub const EDITABLE: u16 = 32;
    pub const MULTISELECT: u16 = 64;
    pub const ALL: u16 = 127;
}

/// One place of a field on a page.
#[derive(Debug, Clone, PartialEq)]
pub struct DesignWidget {
    pub page: u32,
    /// `[left, top, right, bottom]` in points on the page as it is drawn.
    pub rect: [f32; 4],
    /// Radio button: the word the button stands for. Empty for the other kinds.
    pub export: String,
}

/// A field as the designer makes it.
#[derive(Debug, Clone, PartialEq)]
pub struct DesignField {
    pub name: String,
    pub kind: DesignKind,
    pub props: DesignProps,
    pub widgets: Vec<DesignWidget>,
}

/// One widget of a field, named by the name of the field and its place among the widgets of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetRef {
    pub name: String,
    pub widget: u32,
}

/// One change of a batch.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthorOp {
    /// Makes a field.
    Add(DesignField),
    /// Takes a whole field away.
    Remove { name: String },
    /// Takes one widget of a field away; the last one takes the field with it.
    RemoveWidget { name: String, widget: u32 },
    /// Adds a widget to a field (another button of a radio group).
    AddWidget { name: String, widget: DesignWidget },
    /// Moves or resizes a widget, to the same page or to another.
    Move {
        name: String,
        widget: u32,
        page: u32,
        rect: [f32; 4],
    },
    /// Changes what a field is. `new_name` is the name from now on (the same name keeps it).
    /// `exports` are the export values of the radio buttons in order, or empty to keep them.
    Update {
        name: String,
        new_name: String,
        props: DesignProps,
        exports: Vec<String>,
    },
    /// Puts the widgets of a page in a new order, which is the order Tab walks them. Widgets of
    /// the page that are not named keep the places they have among the other annotations.
    Reorder { page: u32, order: Vec<WidgetRef> },
}

/// A request of the form designer.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthorRequest {
    /// Asks whether the document can take new fields, and for the names of the fields it has. A
    /// document that is encrypted, or has an XFA form, or whose file the designer cannot read, is
    /// refused with `AuthorUnsupported` and a reason.
    Check { doc: u32 },
    /// The widgets of a page that the designer knows.
    List { doc: u32, page: u32 },
    /// Makes the changes, in order, and reloads the document once. All or nothing. The answer is
    /// `Pages(Changed)` with the number of the copy of the document before the change.
    Apply { doc: u32, ops: Vec<AuthorOp> },
}

/// A widget the designer lists.
#[derive(Debug, Clone, PartialEq)]
pub struct DesignedWidget {
    pub name: String,
    pub kind: DesignKind,
    /// The place of the widget among the widgets of its field, in the order of the file.
    pub widget: u32,
    /// How many widgets the field has.
    pub widgets: u32,
    /// `[left, top, right, bottom]` in points on the page as it is drawn.
    pub rect: [f32; 4],
    pub export: String,
    /// The designer can change this field. A field with an action, or inside a tree of fields, or
    /// of a kind it does not know, is shown but left alone.
    pub editable: bool,
    pub props: DesignProps,
}

/// An answer of the form designer.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthorResponse {
    /// The document can take fields. These are the full names of the fields it has now.
    Ready { names: Vec<String> },
    /// The widgets of a page, in the order of the page's annotations.
    Widgets { items: Vec<DesignedWidget> },
}

fn put_flag(out: &mut Vec<u8>, v: bool) {
    out.push(u8::from(v));
}

fn get_flag(r: &mut Reader<'_>) -> Result<bool, ProtocolError> {
    match r.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(ProtocolError::UnknownValue(other)),
    }
}

/// A box is written inside the range that the reader of it accepts, so that nothing that is
/// encoded is refused when it is decoded: a place no one can use is cut to the edge of the range.
fn put_box(out: &mut Vec<u8>, b: &[f32; 4]) {
    for v in b {
        put_f32(
            out,
            if v.is_finite() {
                v.clamp(-MAX_BOX_VALUE, MAX_BOX_VALUE)
            } else {
                0.0
            },
        );
    }
}

fn get_box(r: &mut Reader<'_>) -> Result<[f32; 4], ProtocolError> {
    let mut out = [0.0f32; 4];
    for v in &mut out {
        let x = r.f32()?;
        if !x.is_finite() || x.abs() > MAX_BOX_VALUE {
            return Err(ProtocolError::InvalidLength);
        }
        *v = x;
    }
    Ok(out)
}

fn put_props(out: &mut Vec<u8>, p: &DesignProps) {
    put_str_max(out, &p.tooltip, MAX_FORM_TOOLTIP);
    let mut flags = 0u16;
    for (on, bit) in [
        (p.required, props_flag::REQUIRED),
        (p.read_only, props_flag::READ_ONLY),
        (p.multiline, props_flag::MULTILINE),
        (p.password, props_flag::PASSWORD),
        (p.comb, props_flag::COMB),
        (p.editable, props_flag::EDITABLE),
        (p.multiselect, props_flag::MULTISELECT),
    ] {
        if on {
            flags |= bit;
        }
    }
    out.extend_from_slice(&flags.to_le_bytes());
    put_u32(out, p.max_len);
    put_f32(
        out,
        if p.font_size.is_finite() && p.font_size >= 0.0 {
            p.font_size
        } else {
            0.0
        },
    );
    put_str_max(out, &p.default_value, MAX_FORM_VALUE);
    let choices = &p.choices[..p.choices.len().min(MAX_FORM_OPTIONS)];
    put_u32(out, choices.len() as u32);
    for c in choices {
        put_str_max(out, c, MAX_FORM_OPTION_LABEL);
    }
    put_str_max(out, &p.export_value, MAX_EXPORT_VALUE);
}

/// The fewest bytes a list of choices needs for each choice.
const CHOICE_MIN: usize = 4;

fn get_props(r: &mut Reader<'_>) -> Result<DesignProps, ProtocolError> {
    let tooltip = r.string_max(MAX_FORM_TOOLTIP)?;
    let flags = {
        let low = u16::from(r.u8()?);
        let high = u16::from(r.u8()?);
        low | (high << 8)
    };
    if flags & !props_flag::ALL != 0 {
        return Err(ProtocolError::UnknownValue((flags >> 8) as u8));
    }
    let max_len = r.u32()?;
    // A request has exactly one encoding, so a size that the encoder would change is refused here
    // instead of being quietly changed.
    let font_size = r.f32()?;
    if !font_size.is_finite() || font_size.is_sign_negative() && font_size != 0.0 {
        return Err(ProtocolError::InvalidLength);
    }
    let default_value = r.string_max(MAX_FORM_VALUE)?;
    let count = r.u32()? as usize;
    if count > MAX_FORM_OPTIONS || count > r.remaining() / CHOICE_MIN {
        return Err(ProtocolError::InvalidLength);
    }
    let mut choices = Vec::with_capacity(count);
    for _ in 0..count {
        choices.push(r.string_max(MAX_FORM_OPTION_LABEL)?);
    }
    let export_value = r.string_max(MAX_EXPORT_VALUE)?;
    Ok(DesignProps {
        tooltip,
        required: flags & props_flag::REQUIRED != 0,
        read_only: flags & props_flag::READ_ONLY != 0,
        multiline: flags & props_flag::MULTILINE != 0,
        password: flags & props_flag::PASSWORD != 0,
        comb: flags & props_flag::COMB != 0,
        editable: flags & props_flag::EDITABLE != 0,
        multiselect: flags & props_flag::MULTISELECT != 0,
        max_len,
        font_size,
        default_value,
        choices,
        export_value,
    })
}

/// The fewest bytes one widget takes.
const WIDGET_MIN: usize = 4 + 16 + 4;

fn put_widget(out: &mut Vec<u8>, w: &DesignWidget) {
    put_u32(out, w.page);
    put_box(out, &w.rect);
    put_str_max(out, &w.export, MAX_EXPORT_VALUE);
}

fn get_widget(r: &mut Reader<'_>) -> Result<DesignWidget, ProtocolError> {
    Ok(DesignWidget {
        page: r.u32()?,
        rect: get_box(r)?,
        export: r.string_max(MAX_EXPORT_VALUE)?,
    })
}

fn put_field(out: &mut Vec<u8>, f: &DesignField) {
    put_str_max(out, &f.name, MAX_FORM_NAME);
    out.push(f.kind as u8);
    put_props(out, &f.props);
    let widgets = &f.widgets[..f.widgets.len().min(MAX_AUTHOR_WIDGETS)];
    put_u32(out, widgets.len() as u32);
    for w in widgets {
        put_widget(out, w);
    }
}

fn get_field(r: &mut Reader<'_>) -> Result<DesignField, ProtocolError> {
    let name = r.string_max(MAX_FORM_NAME)?;
    let kind = DesignKind::from_u8(r.u8()?)?;
    let props = get_props(r)?;
    let count = r.u32()? as usize;
    if count > MAX_AUTHOR_WIDGETS || count > r.remaining() / WIDGET_MIN {
        return Err(ProtocolError::InvalidLength);
    }
    let mut widgets = Vec::with_capacity(count);
    for _ in 0..count {
        widgets.push(get_widget(r)?);
    }
    Ok(DesignField {
        name,
        kind,
        props,
        widgets,
    })
}

fn put_ref(out: &mut Vec<u8>, w: &WidgetRef) {
    put_str_max(out, &w.name, MAX_FORM_NAME);
    put_u32(out, w.widget);
}

fn get_ref(r: &mut Reader<'_>) -> Result<WidgetRef, ProtocolError> {
    Ok(WidgetRef {
        name: r.string_max(MAX_FORM_NAME)?,
        widget: r.u32()?,
    })
}

/// The fewest bytes one named widget takes.
const REF_MIN: usize = 4 + 4;

fn put_op(out: &mut Vec<u8>, op: &AuthorOp) {
    match op {
        AuthorOp::Add(field) => {
            out.push(OP_ADD);
            put_field(out, field);
        }
        AuthorOp::Remove { name } => {
            out.push(OP_REMOVE);
            put_str_max(out, name, MAX_FORM_NAME);
        }
        AuthorOp::RemoveWidget { name, widget } => {
            out.push(OP_REMOVE_WIDGET);
            put_str_max(out, name, MAX_FORM_NAME);
            put_u32(out, *widget);
        }
        AuthorOp::AddWidget { name, widget } => {
            out.push(OP_ADD_WIDGET);
            put_str_max(out, name, MAX_FORM_NAME);
            put_widget(out, widget);
        }
        AuthorOp::Move {
            name,
            widget,
            page,
            rect,
        } => {
            out.push(OP_MOVE);
            put_str_max(out, name, MAX_FORM_NAME);
            put_u32(out, *widget);
            put_u32(out, *page);
            put_box(out, rect);
        }
        AuthorOp::Update {
            name,
            new_name,
            props,
            exports,
        } => {
            out.push(OP_UPDATE);
            put_str_max(out, name, MAX_FORM_NAME);
            put_str_max(out, new_name, MAX_FORM_NAME);
            put_props(out, props);
            let exports = &exports[..exports.len().min(MAX_AUTHOR_WIDGETS)];
            put_u32(out, exports.len() as u32);
            for e in exports {
                put_str_max(out, e, MAX_EXPORT_VALUE);
            }
        }
        AuthorOp::Reorder { page, order } => {
            out.push(OP_REORDER);
            put_u32(out, *page);
            let order = &order[..order.len().min(super::MAX_FORM_FIELDS_PER_PAGE)];
            put_u32(out, order.len() as u32);
            for w in order {
                put_ref(out, w);
            }
        }
    }
}

fn get_op(r: &mut Reader<'_>) -> Result<AuthorOp, ProtocolError> {
    Ok(match r.u8()? {
        OP_ADD => AuthorOp::Add(get_field(r)?),
        OP_REMOVE => AuthorOp::Remove {
            name: r.string_max(MAX_FORM_NAME)?,
        },
        OP_REMOVE_WIDGET => AuthorOp::RemoveWidget {
            name: r.string_max(MAX_FORM_NAME)?,
            widget: r.u32()?,
        },
        OP_ADD_WIDGET => AuthorOp::AddWidget {
            name: r.string_max(MAX_FORM_NAME)?,
            widget: get_widget(r)?,
        },
        OP_MOVE => AuthorOp::Move {
            name: r.string_max(MAX_FORM_NAME)?,
            widget: r.u32()?,
            page: r.u32()?,
            rect: get_box(r)?,
        },
        OP_UPDATE => {
            let name = r.string_max(MAX_FORM_NAME)?;
            let new_name = r.string_max(MAX_FORM_NAME)?;
            let props = get_props(r)?;
            let count = r.u32()? as usize;
            if count > MAX_AUTHOR_WIDGETS || count > r.remaining() / CHOICE_MIN {
                return Err(ProtocolError::InvalidLength);
            }
            let mut exports = Vec::with_capacity(count);
            for _ in 0..count {
                exports.push(r.string_max(MAX_EXPORT_VALUE)?);
            }
            AuthorOp::Update {
                name,
                new_name,
                props,
                exports,
            }
        }
        OP_REORDER => {
            let page = r.u32()?;
            let count = r.u32()? as usize;
            if count > super::MAX_FORM_FIELDS_PER_PAGE || count > r.remaining() / REF_MIN {
                return Err(ProtocolError::InvalidLength);
            }
            let mut order = Vec::with_capacity(count);
            for _ in 0..count {
                order.push(get_ref(r)?);
            }
            AuthorOp::Reorder { page, order }
        }
        other => return Err(ProtocolError::UnknownValue(other)),
    })
}

impl AuthorRequest {
    pub(crate) fn encode(&self, id: u32, out: &mut Vec<u8>) {
        match self {
            AuthorRequest::Check { doc } => {
                out.push(TAG_CHECK);
                put_u32(out, id);
                put_u32(out, *doc);
            }
            AuthorRequest::List { doc, page } => {
                out.push(TAG_LIST);
                put_u32(out, id);
                put_u32(out, *doc);
                put_u32(out, *page);
            }
            AuthorRequest::Apply { doc, ops } => {
                out.push(TAG_APPLY);
                put_u32(out, id);
                put_u32(out, *doc);
                let ops = &ops[..ops.len().min(MAX_AUTHOR_OPS)];
                put_u32(out, ops.len() as u32);
                for op in ops {
                    put_op(out, op);
                }
            }
        }
    }

    /// Reads the fields after the tag and the id.
    pub(crate) fn decode(tag: u8, r: &mut Reader<'_>) -> Result<AuthorRequest, ProtocolError> {
        Ok(match tag {
            TAG_CHECK => AuthorRequest::Check { doc: r.u32()? },
            TAG_LIST => AuthorRequest::List {
                doc: r.u32()?,
                page: r.u32()?,
            },
            TAG_APPLY => {
                let doc = r.u32()?;
                let count = r.u32()? as usize;
                // An operation takes at least its tag and one number.
                if count > MAX_AUTHOR_OPS || count > r.remaining() {
                    return Err(ProtocolError::InvalidLength);
                }
                let mut ops = Vec::with_capacity(count);
                for _ in 0..count {
                    ops.push(get_op(r)?);
                }
                AuthorRequest::Apply { doc, ops }
            }
            other => return Err(ProtocolError::UnknownTag(other)),
        })
    }
}

/// The fewest bytes one listed widget takes.
const DESIGNED_MIN: usize = 4 + 1 + 4 + 4 + 16 + 4 + 1 + PROPS_MIN;

/// The fewest bytes a set of properties takes: the tooltip, the flags, the length limit, the font
/// size, the default value, the number of choices and the export value.
const PROPS_MIN: usize = 4 + 2 + 4 + 4 + 4 + 4 + 4;

fn put_designed(out: &mut Vec<u8>, w: &DesignedWidget) {
    put_str_max(out, &w.name, MAX_FORM_NAME);
    out.push(w.kind as u8);
    put_u32(out, w.widget);
    put_u32(out, w.widgets);
    put_box(out, &w.rect);
    put_str_max(out, &w.export, MAX_EXPORT_VALUE);
    put_flag(out, w.editable);
    put_props(out, &w.props);
}

fn get_designed(r: &mut Reader<'_>) -> Result<DesignedWidget, ProtocolError> {
    let name = r.string_max(MAX_FORM_NAME)?;
    let kind = DesignKind::from_u8(r.u8()?)?;
    let widget = r.u32()?;
    let widgets = r.u32()?;
    let rect = get_box(r)?;
    let export = r.string_max(MAX_EXPORT_VALUE)?;
    let editable = get_flag(r)?;
    let props = get_props(r)?;
    Ok(DesignedWidget {
        name,
        kind,
        widget,
        widgets,
        rect,
        export,
        editable,
        props,
    })
}

impl AuthorResponse {
    pub(crate) fn encode(&self, id: u32, out: &mut Vec<u8>) {
        match self {
            AuthorResponse::Ready { names } => {
                out.push(TAG_READY);
                put_u32(out, id);
                let names = &names[..names.len().min(MAX_AUTHOR_NAMES)];
                put_u32(out, names.len() as u32);
                for n in names {
                    put_str_max(out, n, MAX_FORM_NAME);
                }
            }
            AuthorResponse::Widgets { items } => {
                out.push(TAG_WIDGETS);
                put_u32(out, id);
                let items = &items[..items.len().min(MAX_LISTED_WIDGETS)];
                put_u32(out, items.len() as u32);
                for w in items {
                    put_designed(out, w);
                }
            }
        }
    }

    pub(crate) fn decode(tag: u8, r: &mut Reader<'_>) -> Result<AuthorResponse, ProtocolError> {
        Ok(match tag {
            TAG_READY => {
                let count = r.u32()? as usize;
                if count > MAX_AUTHOR_NAMES || count > r.remaining() / CHOICE_MIN {
                    return Err(ProtocolError::InvalidLength);
                }
                let mut names = Vec::with_capacity(count);
                for _ in 0..count {
                    names.push(r.string_max(MAX_FORM_NAME)?);
                }
                AuthorResponse::Ready { names }
            }
            TAG_WIDGETS => {
                let count = r.u32()? as usize;
                if count > MAX_LISTED_WIDGETS || count > r.remaining() / DESIGNED_MIN {
                    return Err(ProtocolError::InvalidLength);
                }
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    items.push(get_designed(r)?);
                }
                AuthorResponse::Widgets { items }
            }
            other => return Err(ProtocolError::UnknownTag(other)),
        })
    }
}

#[cfg(test)]
#[path = "author_tests.rs"]
mod tests;
