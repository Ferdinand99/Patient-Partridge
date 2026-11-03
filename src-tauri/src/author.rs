//! The commands of the form designer: the fields of a page as the designer sees them, and a batch of
//! changes (make, change, move and take away fields) that the engine writes with one reload of the
//! document.
//!
//! The window never names a file or an object of the PDF: it names fields by their name and their
//! place among the widgets of the field. Nothing here can ask for an action or a script; the
//! shapes below have no place for one, and a request with a key that is not listed is refused.

use partridge_protocol::{
    AuthorOp, DesignField, DesignKind, DesignProps, DesignWidget, DesignedWidget, WidgetRef,
};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::pages::{permit, unsaved, ChangedInfo, Changing};
use crate::{blocking, round2, CommandError, Engine, UpdateState};

#[derive(Deserialize, Serialize, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PropsWire {
    #[serde(default)]
    tooltip: String,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    read_only: bool,
    #[serde(default)]
    multiline: bool,
    #[serde(default)]
    password: bool,
    #[serde(default)]
    comb: bool,
    #[serde(default)]
    max_len: u32,
    #[serde(default)]
    editable: bool,
    #[serde(default)]
    multiselect: bool,
    #[serde(default)]
    font_size: f32,
    #[serde(default)]
    default_value: String,
    #[serde(default)]
    choices: Vec<String>,
    #[serde(default)]
    export_value: String,
}

impl From<PropsWire> for DesignProps {
    fn from(p: PropsWire) -> Self {
        DesignProps {
            tooltip: p.tooltip,
            required: p.required,
            read_only: p.read_only,
            multiline: p.multiline,
            password: p.password,
            comb: p.comb,
            max_len: p.max_len,
            editable: p.editable,
            multiselect: p.multiselect,
            font_size: if p.font_size.is_finite() && p.font_size >= 0.0 {
                p.font_size
            } else {
                0.0
            },
            default_value: p.default_value,
            choices: p.choices,
            export_value: p.export_value,
        }
    }
}

impl From<DesignProps> for PropsWire {
    fn from(p: DesignProps) -> Self {
        PropsWire {
            tooltip: p.tooltip,
            required: p.required,
            read_only: p.read_only,
            multiline: p.multiline,
            password: p.password,
            comb: p.comb,
            max_len: p.max_len,
            editable: p.editable,
            multiselect: p.multiselect,
            font_size: round2(p.font_size),
            default_value: p.default_value,
            choices: p.choices,
            export_value: p.export_value,
        }
    }
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WidgetIn {
    page: u32,
    /// Left, top, right and bottom in points from the top left of the page as it is drawn.
    rect: [f32; 4],
    #[serde(default)]
    export: String,
}

impl From<WidgetIn> for DesignWidget {
    fn from(w: WidgetIn) -> Self {
        DesignWidget {
            page: w.page,
            rect: w.rect,
            export: w.export,
        }
    }
}

#[derive(Deserialize, Debug, PartialEq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum KindIn {
    Text,
    Checkbox,
    Radio,
    Combo,
    List,
    Signature,
}

impl From<KindIn> for DesignKind {
    fn from(k: KindIn) -> Self {
        match k {
            KindIn::Text => DesignKind::Text,
            KindIn::Checkbox => DesignKind::Checkbox,
            KindIn::Radio => DesignKind::Radio,
            KindIn::Combo => DesignKind::Combo,
            KindIn::List => DesignKind::List,
            KindIn::Signature => DesignKind::Signature,
        }
    }
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FieldIn {
    name: String,
    kind: KindIn,
    #[serde(default)]
    props: PropsWire,
    widgets: Vec<WidgetIn>,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RefIn {
    name: String,
    widget: u32,
}

/// One change, as the window names it.
#[derive(Deserialize, Debug, PartialEq)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
pub enum OpIn {
    Add {
        field: FieldIn,
    },
    Remove {
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    RemoveWidget {
        name: String,
        widget: u32,
    },
    #[serde(rename_all = "camelCase")]
    AddWidget {
        name: String,
        widget: WidgetIn,
    },
    #[serde(rename_all = "camelCase")]
    Move {
        name: String,
        widget: u32,
        page: u32,
        rect: [f32; 4],
    },
    #[serde(rename_all = "camelCase")]
    Update {
        name: String,
        new_name: String,
        props: PropsWire,
        #[serde(default)]
        exports: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    Reorder {
        page: u32,
        order: Vec<RefIn>,
    },
}

impl From<OpIn> for AuthorOp {
    fn from(op: OpIn) -> Self {
        match op {
            OpIn::Add { field } => AuthorOp::Add(DesignField {
                name: field.name,
                kind: field.kind.into(),
                props: field.props.into(),
                widgets: field.widgets.into_iter().map(Into::into).collect(),
            }),
            OpIn::Remove { name } => AuthorOp::Remove { name },
            OpIn::RemoveWidget { name, widget } => AuthorOp::RemoveWidget { name, widget },
            OpIn::AddWidget { name, widget } => AuthorOp::AddWidget {
                name,
                widget: widget.into(),
            },
            OpIn::Move {
                name,
                widget,
                page,
                rect,
            } => AuthorOp::Move {
                name,
                widget,
                page,
                rect,
            },
            OpIn::Update {
                name,
                new_name,
                props,
                exports,
            } => AuthorOp::Update {
                name,
                new_name,
                props: props.into(),
                exports,
            },
            OpIn::Reorder { page, order } => AuthorOp::Reorder {
                page,
                order: order
                    .into_iter()
                    .map(|r| WidgetRef {
                        name: r.name,
                        widget: r.widget,
                    })
                    .collect(),
            },
        }
    }
}

/// A widget, as the window gets it.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WidgetOut {
    name: String,
    kind: &'static str,
    widget: u32,
    widgets: u32,
    rect: [f32; 4],
    export: String,
    /// The designer can change this field.
    editable: bool,
    props: PropsWire,
}

pub fn widget_out(w: DesignedWidget) -> WidgetOut {
    WidgetOut {
        name: w.name,
        kind: w.kind.name(),
        widget: w.widget,
        widgets: w.widgets,
        rect: w.rect.map(round2),
        export: w.export,
        editable: w.editable,
        props: w.props.into(),
    }
}

/// Whether the designer can work on the document, and the names of the fields it has.
#[tauri::command]
pub async fn author_check(
    engine: State<'_, Engine>,
    doc: u64,
) -> Result<Vec<String>, CommandError> {
    let pool = engine.0.clone();
    blocking(move || pool.author_check(doc)).await
}

/// The widgets of a page that the designer lists.
#[tauri::command]
pub async fn author_list(
    engine: State<'_, Engine>,
    doc: u64,
    page: u32,
) -> Result<Vec<WidgetOut>, CommandError> {
    let pool = engine.0.clone();
    let items = blocking(move || pool.author_list(doc, page)).await?;
    Ok(items.into_iter().map(widget_out).collect())
}

/// Makes a batch of changes with one reload of the document. Like every change to a document it
/// holds a permit of the update gate while it runs, and the document counts as changed from the start.
#[tauri::command]
pub async fn author_apply(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    ops: Vec<OpIn>,
) -> Result<ChangedInfo, CommandError> {
    let _permit = permit(&update_state)?;
    let _changing = Changing::begin(&update_state, doc);
    let pool = engine.0.clone();
    let ops: Vec<AuthorOp> = ops.into_iter().map(Into::into).collect();
    let changed = blocking(move || pool.author_apply(doc, &ops)).await?;
    unsaved(&update_state, doc);
    Ok(changed.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_can_ask_for_the_changes_of_the_designer_and_nothing_else() {
        let add: OpIn = serde_json::from_str(
            r#"{"op":"add","field":{"name":"a","kind":"text","props":{"maxLen":5},"widgets":[{"page":0,"rect":[1,2,30,40]}]}}"#,
        )
        .unwrap();
        let AuthorOp::Add(field) = AuthorOp::from(add) else {
            panic!("not an add");
        };
        assert_eq!(field.kind, DesignKind::Text);
        assert_eq!(field.props.max_len, 5);
        assert_eq!(field.widgets[0].rect, [1.0, 2.0, 30.0, 40.0]);
        let update: OpIn = serde_json::from_str(
            r#"{"op":"update","name":"a","newName":"b","props":{},"exports":["x"]}"#,
        )
        .unwrap();
        assert!(
            matches!(AuthorOp::from(update), AuthorOp::Update { new_name, .. } if new_name == "b")
        );
        for bad in [
            // No place for an action or a script anywhere.
            r#"{"op":"add","field":{"name":"a","kind":"text","props":{"action":"x"},"widgets":[]}}"#,
            r#"{"op":"add","field":{"name":"a","kind":"button","widgets":[]}}"#,
            r#"{"op":"add","field":{"name":"a","kind":"text","js":"1","widgets":[]}}"#,
            r#"{"op":"script","code":"x"}"#,
            r#"{"op":"remove"}"#,
            r#"{"op":"move","name":"a","widget":-1,"page":0,"rect":[0,0,1,1]}"#,
            r#"{"op":"remove","name":"a","extra":true}"#,
            r#"{}"#,
        ] {
            assert!(serde_json::from_str::<OpIn>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_widget_reaches_the_window_as_plain_data_and_its_text_untouched() {
        let w = DesignedWidget {
            name: "a <b>".into(),
            kind: DesignKind::Radio,
            widget: 1,
            widgets: 3,
            rect: [1.234, 2.0, 3.0, 4.0],
            export: "<script>".into(),
            editable: true,
            props: DesignProps {
                choices: vec!["\"x\"".into()],
                font_size: 11.567,
                ..DesignProps::default()
            },
        };
        // As the window gets it: text, read again.
        let text = serde_json::to_string(&widget_out(w)).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["kind"], "radio");
        assert_eq!(json["name"], "a <b>");
        assert_eq!(json["export"], "<script>");
        assert_eq!(json["rect"][0], 1.23);
        assert_eq!(json["props"]["fontSize"], 11.57);
        assert_eq!(json["props"]["choices"][0], "\"x\"");
        assert_eq!(json["editable"], true);
    }
}
