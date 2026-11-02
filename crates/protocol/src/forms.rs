//! Form fields on the wire: the kinds, the limits and the pieces of the messages that are more than
//! a few numbers.
//!
//! Everything a field carries that came from a PDF (its name, its tooltip, its value, the labels of
//! its choices) is untrusted. The caps below are held by the encoder and by the decoder, on both
//! sides of the connection, so that neither a damaged file nor a damaged engine can make the app
//! hold, or put in the window, more than a fixed amount. The window shows all of it as text and
//! nothing else.

use super::{put_f32, put_str_max, put_u32, ProtocolError, Reader};

/// Most fields the engine lists for one page. A page with more shows the first ones.
pub const MAX_FORM_FIELDS_PER_PAGE: usize = 1_000;

/// Longest field name (the full name, with its parents), in bytes. Longer names are cut on a
/// character boundary when listed; a request that names a field is compared with the cut name.
pub const MAX_FORM_NAME: usize = 512;

/// Longest tooltip of a field, in bytes.
pub const MAX_FORM_TOOLTIP: usize = 512;

/// Longest value of a field, in bytes. A field whose value is longer is listed with the start of it
/// and marked [`field_flag::VALUE_CUT`]; the window does not let the reader replace it, because
/// what the reader would see is not what the file holds.
pub const MAX_FORM_VALUE: usize = 8 * 1024;

/// Most choices of a combo box or a list box that are listed, and most that a request selects.
pub const MAX_FORM_OPTIONS: usize = 500;

/// Longest label of a choice, in bytes.
pub const MAX_FORM_OPTION_LABEL: usize = 256;

/// Biggest answer to a request for the fields of a page, in bytes. The engine leaves out the fields
/// that do not fit, and the app refuses a longer one before it reads it.
pub const MAX_FORM_ANSWER_BYTES: usize = 2 * 1024 * 1024;

/// The fewest bytes one listed field takes: its place, kind, flags, box, font size, the lengths of
/// the three strings, the longest value and the number of choices.
pub const FORM_FIELD_MIN: usize = 4 + 1 + 2 + 1 + 16 + 4 + 4 + 4 + 4 + 4 + 4;

/// The fewest bytes one choice takes: whether it is selected, and the length of its label.
const FORM_OPTION_MIN: usize = 1 + 4;

/// What kind of form a document has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FormKind {
    /// No form, or a form with no field.
    None = 0,
    /// A form of fields in the PDF itself (an AcroForm). The reader can fill it in.
    Acro = 1,
    /// A form that carries XFA data, whole or next to ordinary fields. A reader with XFA support
    /// shows the XFA part and ignores the ordinary fields, so a value written to them would not show: the reader does
    /// not fill these in.
    Xfa = 2,
}

impl FormKind {
    pub fn from_u8(v: u8) -> Result<FormKind, ProtocolError> {
        Ok(match v {
            0 => FormKind::None,
            1 => FormKind::Acro,
            2 => FormKind::Xfa,
            other => return Err(ProtocolError::UnknownValue(other)),
        })
    }

    /// A short stable name.
    pub fn name(self) -> &'static str {
        match self {
            FormKind::None => "none",
            FormKind::Acro => "acro",
            FormKind::Xfa => "xfa",
        }
    }
}

/// The kinds of field the reader fills in. Buttons that only run an action and signature fields
/// are not listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FormFieldKind {
    /// A line or a box of text.
    Text = 1,
    Checkbox = 2,
    /// One button of a group of which at most one is on.
    Radio = 3,
    /// A drop-down list, with or without a box to type in.
    Combo = 4,
    List = 5,
}

impl FormFieldKind {
    pub fn from_u8(v: u8) -> Result<FormFieldKind, ProtocolError> {
        Ok(match v {
            1 => FormFieldKind::Text,
            2 => FormFieldKind::Checkbox,
            3 => FormFieldKind::Radio,
            4 => FormFieldKind::Combo,
            5 => FormFieldKind::List,
            other => return Err(ProtocolError::UnknownValue(other)),
        })
    }

    /// A short stable name.
    pub fn name(self) -> &'static str {
        match self {
            FormFieldKind::Text => "text",
            FormFieldKind::Checkbox => "checkbox",
            FormFieldKind::Radio => "radio",
            FormFieldKind::Combo => "combo",
            FormFieldKind::List => "list",
        }
    }
}

/// The bits of [`FormField::flags`].
pub mod field_flag {
    /// The reader may not change the field.
    pub const READ_ONLY: u16 = 1;
    /// The form wants a value in the field.
    pub const REQUIRED: u16 = 2;
    /// A text field that takes more than one line.
    pub const MULTILINE: u16 = 4;
    /// A text field that hides what is typed.
    pub const PASSWORD: u16 = 8;
    /// A combo box with a box to type in.
    pub const EDITABLE: u16 = 16;
    /// A list box that takes more than one choice.
    pub const MULTISELECT: u16 = 32;
    /// A check box or a radio button that is on.
    pub const CHECKED: u16 = 64;
    /// The value is longer than [`super::MAX_FORM_VALUE`] and is listed cut.
    pub const VALUE_CUT: u16 = 128;
    /// A radio button that cannot be turned off by pressing it again.
    pub const NO_TOGGLE_OFF: u16 = 256;
    /// A text field that is a row of boxes with one character each.
    pub const COMB: u16 = 512;
    /// The field has an action of its own (a script, a format, a button action). The reader never
    /// runs it.
    pub const ACTION: u16 = 1024;
    /// The field has an action of its own that could change the document (a reset or a hide are in
    /// the document), so it is left alone: it is also marked read only.
    pub const ACTION_BLOCKED: u16 = 2048;
    /// All the bits that mean something.
    pub const ALL: u16 = READ_ONLY
        | REQUIRED
        | MULTILINE
        | PASSWORD
        | EDITABLE
        | MULTISELECT
        | CHECKED
        | VALUE_CUT
        | NO_TOGGLE_OFF
        | COMB
        | ACTION
        | ACTION_BLOCKED;
}

/// One choice of a combo box or a list box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormOption {
    pub selected: bool,
    /// The words the reader sees. From the file: show it as text, nothing else.
    pub label: String,
}

/// One field of a page.
#[derive(Debug, Clone, PartialEq)]
pub struct FormField {
    /// Names the field on its page: the place of its widget among the widgets of the page. It is
    /// only good while the page is not changed; a request that uses it also names the field.
    pub index: u32,
    pub kind: FormFieldKind,
    /// A sum of the bits of [`field_flag`].
    pub flags: u16,
    /// Quarter turns clockwise (0 to 3) that the field is drawn turned on the page as it is shown.
    pub turns: u8,
    /// `[left, top, right, bottom]` in points from the top left of the page as it is drawn.
    pub rect: [f32; 4],
    /// The size of the text in points, or 0 when the field fits it to its height.
    pub font_size: f32,
    /// The full name of the field. From the file: show it as text, nothing else.
    pub name: String,
    /// The tooltip of the field, if the file has one. From the file too.
    pub tooltip: String,
    /// The text of a text field or of an editable combo box. Empty for the other kinds. Line breaks
    /// are `\n`.
    pub value: String,
    /// The most characters the file allows, or 0 when it says nothing (or says it where the engine
    /// does not look). The engine holds the limit either way.
    pub max_len: u32,
    /// The choices of a combo box or a list box, in the order of the file.
    pub options: Vec<FormOption>,
}

impl FormField {
    pub fn has(&self, flag: u16) -> bool {
        self.flags & flag != 0
    }
}

/// What a request sets a field to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormValue {
    /// The text of a text field or of an editable combo box. Line breaks are `\n`.
    Text(String),
    /// A check box or a radio button on or off.
    Checked(bool),
    /// The choices of a combo box or a list box that are selected, as places in the list.
    Choice(Vec<u32>),
}

const VALUE_TEXT: u8 = 1;
const VALUE_CHECKED: u8 = 2;
const VALUE_CHOICE: u8 = 3;

pub(crate) fn put_form_value(out: &mut Vec<u8>, value: &FormValue) {
    match value {
        FormValue::Text(text) => {
            out.push(VALUE_TEXT);
            put_str_max(out, text, MAX_FORM_VALUE);
        }
        FormValue::Checked(on) => {
            out.push(VALUE_CHECKED);
            out.push(u8::from(*on));
        }
        FormValue::Choice(picked) => {
            out.push(VALUE_CHOICE);
            let picked = &picked[..picked.len().min(MAX_FORM_OPTIONS)];
            put_u32(out, picked.len() as u32);
            for p in picked {
                put_u32(out, *p);
            }
        }
    }
}

pub(crate) fn read_form_value(r: &mut Reader<'_>) -> Result<FormValue, ProtocolError> {
    Ok(match r.u8()? {
        VALUE_TEXT => FormValue::Text(r.string_max(MAX_FORM_VALUE)?),
        VALUE_CHECKED => FormValue::Checked(match r.u8()? {
            0 => false,
            1 => true,
            other => return Err(ProtocolError::UnknownValue(other)),
        }),
        VALUE_CHOICE => {
            let count = r.u32()? as usize;
            if count > MAX_FORM_OPTIONS || count > r.remaining() / 4 {
                return Err(ProtocolError::InvalidLength);
            }
            let mut picked = Vec::with_capacity(count);
            for _ in 0..count {
                picked.push(r.u32()?);
            }
            FormValue::Choice(picked)
        }
        other => return Err(ProtocolError::UnknownValue(other)),
    })
}

/// How many bytes a field takes on the wire, as [`put_field`] writes it.
fn field_len(field: &FormField) -> usize {
    let cut = |s: &str, max: usize| -> usize {
        let mut end = s.len().min(max);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        end
    };
    let mut n = FORM_FIELD_MIN
        + cut(&field.name, MAX_FORM_NAME)
        + cut(&field.tooltip, MAX_FORM_TOOLTIP)
        + cut(&field.value, MAX_FORM_VALUE);
    for option in field.options.iter().take(MAX_FORM_OPTIONS) {
        n += FORM_OPTION_MIN + cut(&option.label, MAX_FORM_OPTION_LABEL);
    }
    n
}

pub(crate) fn put_field(out: &mut Vec<u8>, field: &FormField) {
    put_u32(out, field.index);
    out.push(field.kind as u8);
    out.extend_from_slice(&(field.flags & field_flag::ALL).to_le_bytes());
    out.push(field.turns & 3);
    for v in field.rect {
        put_f32(out, v);
    }
    put_f32(out, field.font_size);
    put_str_max(out, &field.name, MAX_FORM_NAME);
    put_str_max(out, &field.tooltip, MAX_FORM_TOOLTIP);
    put_str_max(out, &field.value, MAX_FORM_VALUE);
    put_u32(out, field.max_len);
    let options = &field.options[..field.options.len().min(MAX_FORM_OPTIONS)];
    put_u32(out, options.len() as u32);
    for option in options {
        out.push(u8::from(option.selected));
        put_str_max(out, &option.label, MAX_FORM_OPTION_LABEL);
    }
}

pub(crate) fn read_field(r: &mut Reader<'_>) -> Result<FormField, ProtocolError> {
    let index = r.u32()?;
    let kind = FormFieldKind::from_u8(r.u8()?)?;
    let flags = {
        let low = u16::from(r.u8()?);
        let high = u16::from(r.u8()?);
        low | (high << 8)
    };
    if flags & !field_flag::ALL != 0 {
        return Err(ProtocolError::UnknownValue((flags >> 8) as u8));
    }
    let turns = r.u8()?;
    if turns > 3 {
        return Err(ProtocolError::UnknownValue(turns));
    }
    let rect = r.rect()?;
    let font_size = {
        let v = r.f32()?;
        if v.is_finite() && v >= 0.0 {
            v
        } else {
            0.0
        }
    };
    let name = r.string_max(MAX_FORM_NAME)?;
    let tooltip = r.string_max(MAX_FORM_TOOLTIP)?;
    let value = r.string_max(MAX_FORM_VALUE)?;
    let max_len = r.u32()?;
    let count = r.u32()? as usize;
    if count > MAX_FORM_OPTIONS || count > r.remaining() / FORM_OPTION_MIN {
        return Err(ProtocolError::InvalidLength);
    }
    let mut options = Vec::with_capacity(count);
    for _ in 0..count {
        let selected = match r.u8()? {
            0 => false,
            1 => true,
            other => return Err(ProtocolError::UnknownValue(other)),
        };
        options.push(FormOption {
            selected,
            label: r.string_max(MAX_FORM_OPTION_LABEL)?,
        });
    }
    Ok(FormField {
        index,
        kind,
        flags,
        turns,
        rect,
        font_size,
        name,
        tooltip,
        value,
        max_len,
        options,
    })
}

/// Writes a list of fields: the number of them, then each. Whatever is over the caps (the number of
/// fields, the bytes of the answer) is left out, so the result always decodes.
pub(crate) fn put_fields(out: &mut Vec<u8>, fields: &[FormField]) {
    let start = out.len();
    put_u32(out, 0);
    let mut kept = 0u32;
    let mut used = 4usize;
    for field in fields.iter().take(MAX_FORM_FIELDS_PER_PAGE) {
        let len = field_len(field);
        // The tag and the id of the message are 5 more bytes.
        if used + len + 5 > MAX_FORM_ANSWER_BYTES {
            break;
        }
        used += len;
        put_field(out, field);
        kept += 1;
    }
    out[start..start + 4].copy_from_slice(&kept.to_le_bytes());
}

pub(crate) fn read_fields(r: &mut Reader<'_>) -> Result<Vec<FormField>, ProtocolError> {
    let count = r.u32()? as usize;
    if count > MAX_FORM_FIELDS_PER_PAGE || count > r.remaining() / FORM_FIELD_MIN {
        return Err(ProtocolError::InvalidLength);
    }
    let mut fields = Vec::with_capacity(count);
    for _ in 0..count {
        fields.push(read_field(r)?);
    }
    Ok(fields)
}

#[cfg(test)]
#[path = "forms_tests.rs"]
mod tests;
