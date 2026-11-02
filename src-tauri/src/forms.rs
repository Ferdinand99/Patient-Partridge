//! The commands that list and fill in the fields of a form.
//!
//! The window never sees a field as the engine made it: the name, the tooltip, the value and the
//! labels of the choices come from the PDF, so they travel as plain strings (already cut to the
//! caps of the protocol by the engine and checked by the client), and the window puts them on the
//! page as text and nothing else. A value that comes back from the window is checked again by the
//! client before it goes to the engine, and the engine checks it against the field.

use partridge_engine_client::{field_flag, FormField, FormFieldKind, FormKind, FormValue};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::updates::{self, UpdateState};
use crate::{blocking, round2, CommandError, Engine};

/// What kind of form a document has, and whether it has actions that never run.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FormInfoOut {
    /// "none", "acro" or "xfa".
    kind: &'static str,
    actions: bool,
}

/// One field, as the window gets it.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FieldOut {
    index: u32,
    kind: &'static str,
    read_only: bool,
    required: bool,
    multiline: bool,
    password: bool,
    editable: bool,
    multiselect: bool,
    checked: bool,
    /// The value is longer than the window may hold: the field is shown but not changed.
    value_cut: bool,
    no_toggle_off: bool,
    comb: bool,
    /// The field has an action that is never run.
    action: bool,
    /// The field has an action that could change the form, so it is left alone (and read only).
    action_blocked: bool,
    /// Quarter turns clockwise.
    turns: u8,
    /// Left, top, right and bottom in points from the top left of the page as it is drawn.
    rect: [f32; 4],
    font_size: f32,
    name: String,
    tooltip: String,
    value: String,
    max_len: u32,
    options: Vec<OptionOut>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct OptionOut {
    selected: bool,
    label: String,
}

/// What the window asks a field to be set to.
#[derive(Deserialize, Debug)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ValueIn {
    Text { text: String },
    Checked { on: bool },
    Choice { picked: Vec<u32> },
}

pub fn kind_name(kind: FormKind) -> &'static str {
    match kind {
        FormKind::None => "none",
        FormKind::Acro => "acro",
        FormKind::Xfa => "xfa",
    }
}

pub fn field_out(field: FormField) -> FieldOut {
    let has = |flag| field.has(flag);
    FieldOut {
        index: field.index,
        kind: match field.kind {
            FormFieldKind::Text => "text",
            FormFieldKind::Checkbox => "checkbox",
            FormFieldKind::Radio => "radio",
            FormFieldKind::Combo => "combo",
            FormFieldKind::List => "list",
        },
        read_only: has(field_flag::READ_ONLY),
        required: has(field_flag::REQUIRED),
        multiline: has(field_flag::MULTILINE),
        password: has(field_flag::PASSWORD),
        editable: has(field_flag::EDITABLE),
        multiselect: has(field_flag::MULTISELECT),
        checked: has(field_flag::CHECKED),
        value_cut: has(field_flag::VALUE_CUT),
        no_toggle_off: has(field_flag::NO_TOGGLE_OFF),
        comb: has(field_flag::COMB),
        action: has(field_flag::ACTION),
        action_blocked: has(field_flag::ACTION_BLOCKED),
        turns: field.turns,
        rect: field.rect.map(round2),
        font_size: round2(field.font_size),
        max_len: field.max_len,
        name: field.name,
        tooltip: field.tooltip,
        value: field.value,
        options: field
            .options
            .into_iter()
            .map(|o| OptionOut {
                selected: o.selected,
                label: o.label,
            })
            .collect(),
    }
}

#[tauri::command]
pub async fn form_info(engine: State<'_, Engine>, doc: u64) -> Result<FormInfoOut, CommandError> {
    let pool = engine.0.clone();
    let (kind, actions) = blocking(move || pool.form_info(doc)).await?;
    Ok(FormInfoOut {
        kind: kind_name(kind),
        actions,
    })
}

#[tauri::command]
pub async fn get_form_fields(
    engine: State<'_, Engine>,
    doc: u64,
    page: u32,
) -> Result<Vec<FieldOut>, CommandError> {
    let pool = engine.0.clone();
    let fields = blocking(move || pool.form_fields(doc, page)).await?;
    Ok(fields.into_iter().map(field_out).collect())
}

/// Sets one field and returns the fields of the page as they are then. Like every change to a
/// document it holds a permit of the update gate while it runs, so that an install cannot stop the
/// engine in the middle of a change.
#[tauri::command]
pub async fn set_form_field(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    page: u32,
    index: u32,
    name: String,
    value: ValueIn,
) -> Result<Vec<FieldOut>, CommandError> {
    let _permit = update_state
        .gate()
        .enter()
        .map_err(|_| updates::updating_error())?;
    let pool = engine.0.clone();
    let value = match value {
        ValueIn::Text { text } => FormValue::Text(text),
        ValueIn::Checked { on } => FormValue::Checked(on),
        ValueIn::Choice { picked } => FormValue::Choice(picked),
    };
    let fields = blocking(move || pool.set_form_field(doc, page, index, &name, &value)).await?;
    Ok(fields.into_iter().map(field_out).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use partridge_engine_client::FormOption;

    fn field() -> FormField {
        FormField {
            index: 3,
            kind: FormFieldKind::Combo,
            flags: field_flag::REQUIRED | field_flag::EDITABLE | field_flag::ACTION,
            turns: 1,
            rect: [1.234, 2.0, 3.0, 4.0],
            font_size: 0.0,
            name: "a <b>\"c\"</b>".into(),
            tooltip: "t".into(),
            value: "v".into(),
            max_len: 0,
            options: vec![FormOption {
                selected: true,
                label: "<script>".into(),
            }],
        }
    }

    #[test]
    fn a_field_reaches_the_window_with_its_flags_as_plain_data_and_its_text_untouched() {
        let out = field_out(field());
        assert!(out.required && out.editable && out.action);
        assert!(!out.read_only && !out.checked && !out.value_cut);
        assert_eq!(out.kind, "combo");
        assert_eq!(out.rect, [1.23, 2.0, 3.0, 4.0]);
        // The text goes through exactly as the PDF had it: the window shows it as text.
        assert_eq!(out.name, "a <b>\"c\"</b>");
        assert_eq!(out.options[0].label, "<script>");
        let json = serde_json::to_value(&out).unwrap();
        assert_eq!(json["readOnly"], false);
        assert_eq!(json["options"][0]["selected"], true);
    }

    #[test]
    fn the_window_can_ask_for_three_kinds_of_value_and_nothing_else() {
        let text: ValueIn = serde_json::from_str(r#"{"kind":"text","text":"x"}"#).unwrap();
        assert!(matches!(text, ValueIn::Text { text } if text == "x"));
        let on: ValueIn = serde_json::from_str(r#"{"kind":"checked","on":true}"#).unwrap();
        assert!(matches!(on, ValueIn::Checked { on: true }));
        let pick: ValueIn = serde_json::from_str(r#"{"kind":"choice","picked":[1,2]}"#).unwrap();
        assert!(matches!(pick, ValueIn::Choice { picked } if picked == vec![1, 2]));
        for bad in [
            r#"{"kind":"script","code":"x"}"#,
            r#"{"kind":"checked","on":"yes"}"#,
            r#"{"kind":"choice","picked":[-1]}"#,
            r#"{"kind":"text"}"#,
            r#"{}"#,
        ] {
            assert!(serde_json::from_str::<ValueIn>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_kind_of_form_has_a_stable_name() {
        assert_eq!(kind_name(FormKind::None), "none");
        assert_eq!(kind_name(FormKind::Acro), "acro");
        assert_eq!(kind_name(FormKind::Xfa), "xfa");
    }
}
