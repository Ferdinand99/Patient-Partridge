//! The annotation commands: what the window may ask about, and change in, the annotations of a
//! document. The window sends numbers, colours and short text; the app process checks the shape of
//! each request again, and the engine process makes the annotation. Nothing the window sends is a
//! path, a file name or a piece of PDF, and nothing an annotation holds is sent on as anything but
//! text and numbers.

use partridge_engine_client::{ability, AnnotInfo, AnnotKind, ClientError, StampKind};
use partridge_protocol::ErrorCode;
use serde::Serialize;
use tauri::State;

use crate::updates::{self, UpdateState};
use crate::{blocking, round2, CommandError, Engine};

/// What can be done with an annotation.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Abilities {
    edit_text: bool,
    recolour: bool,
    #[serde(rename = "move")]
    movable: bool,
    remove: bool,
    restore: bool,
    /// The file or the document forbids changing it.
    locked: bool,
    /// A picture of a signature can be made bigger or smaller.
    resize: bool,
}

/// An annotation of a page as the window sees it. `rect` is `[left, top, right, bottom]` in points
/// from the top left of the page as it is drawn; `text` and `author` come from the file and are to
/// be shown as text only.
#[derive(Serialize, Debug, PartialEq)]
pub struct AnnotationInfo {
    id: u32,
    kind: &'static str,
    can: Abilities,
    rect: [f32; 4],
    color: [u8; 4],
    text: String,
    author: String,
}

impl From<AnnotInfo> for AnnotationInfo {
    fn from(item: AnnotInfo) -> Self {
        AnnotationInfo {
            id: item.id,
            kind: item.kind.name(),
            can: Abilities {
                edit_text: item.abilities & ability::EDIT_TEXT != 0,
                recolour: item.abilities & ability::RECOLOUR != 0,
                movable: item.abilities & ability::MOVE != 0,
                remove: item.abilities & ability::REMOVE != 0,
                restore: item.abilities & ability::RESTORE != 0,
                locked: item.abilities & ability::LOCKED != 0,
                resize: item.abilities & ability::RESIZE != 0,
            },
            rect: item.rect.map(round2),
            color: item.color,
            text: item.text,
            author: item.author,
        }
    }
}

fn bad(message: &str) -> CommandError {
    CommandError::from(ClientError::Engine {
        code: ErrorCode::InvalidRequest,
        message: message.into(),
    })
}

/// The kind of markup a name stands for.
pub fn markup_kind(name: &str) -> Result<AnnotKind, CommandError> {
    match name {
        "highlight" => Ok(AnnotKind::Highlight),
        "underline" => Ok(AnnotKind::Underline),
        "strikeout" => Ok(AnnotKind::Strikeout),
        _ => Err(bad("that is not a kind of markup")),
    }
}

/// The stamp a name stands for.
pub fn stamp_kind(name: &str) -> Result<StampKind, CommandError> {
    StampKind::ALL
        .iter()
        .copied()
        .find(|s| s.name() == name)
        .ok_or_else(|| bad("that is not a stamp"))
}

/// The annotations of a page that the reader can show and change.
#[tauri::command]
pub async fn list_annotations(
    engine: State<'_, Engine>,
    doc: u64,
    page: u32,
) -> Result<Vec<AnnotationInfo>, CommandError> {
    let pool = engine.0.clone();
    let items = blocking(move || pool.list_annots(doc, page)).await?;
    Ok(items.into_iter().map(AnnotationInfo::from).collect())
}

/// Marks the characters `start..end` of the page text.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn add_markup(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    page: u32,
    kind: String,
    color: [u8; 4],
    start: u32,
    end: u32,
) -> Result<AnnotationInfo, CommandError> {
    let kind = markup_kind(&kind)?;
    // Not while an update is being installed: the engine would be stopped under the change.
    let _permit = update_state
        .gate()
        .enter()
        .map_err(|_| updates::updating_error())?;
    let pool = engine.0.clone();
    let item = blocking(move || pool.add_markup(doc, page, kind, color, start, end)).await?;
    Ok(item.into())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn add_note(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    page: u32,
    x: f32,
    y: f32,
    color: [u8; 4],
    text: String,
) -> Result<AnnotationInfo, CommandError> {
    let _permit = update_state
        .gate()
        .enter()
        .map_err(|_| updates::updating_error())?;
    let pool = engine.0.clone();
    let item = blocking(move || pool.add_note(doc, page, x, y, color, &text)).await?;
    Ok(item.into())
}

#[tauri::command]
pub async fn add_ink(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    page: u32,
    color: [u8; 4],
    width: f32,
    strokes: Vec<Vec<[f32; 2]>>,
) -> Result<AnnotationInfo, CommandError> {
    let _permit = update_state
        .gate()
        .enter()
        .map_err(|_| updates::updating_error())?;
    let pool = engine.0.clone();
    let item = blocking(move || pool.add_ink(doc, page, color, width, strokes)).await?;
    Ok(item.into())
}

#[tauri::command]
pub async fn add_stamp(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    page: u32,
    x: f32,
    y: f32,
    stamp: String,
) -> Result<AnnotationInfo, CommandError> {
    let stamp = stamp_kind(&stamp)?;
    let _permit = update_state
        .gate()
        .enter()
        .map_err(|_| updates::updating_error())?;
    let pool = engine.0.clone();
    let item = blocking(move || pool.add_stamp(doc, page, x, y, stamp)).await?;
    Ok(item.into())
}

/// Changes the text, the colour or the place of an annotation. `dx` and `dy` are in points on the
/// page as it is drawn.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn update_annotation(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    page: u32,
    id: u32,
    text: Option<String>,
    color: Option<[u8; 4]>,
    dx: f32,
    dy: f32,
) -> Result<AnnotationInfo, CommandError> {
    let _permit = update_state
        .gate()
        .enter()
        .map_err(|_| updates::updating_error())?;
    let pool = engine.0.clone();
    let item =
        blocking(move || pool.update_annot(doc, page, id, text.as_deref(), color, dx, dy)).await?;
    Ok(item.into())
}

#[tauri::command]
pub async fn remove_annotation(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    page: u32,
    id: u32,
) -> Result<(), CommandError> {
    let _permit = update_state
        .gate()
        .enter()
        .map_err(|_| updates::updating_error())?;
    let pool = engine.0.clone();
    blocking(move || pool.remove_annot(doc, page, id)).await
}

/// Puts back an annotation that was removed; it keeps its id.
#[tauri::command]
pub async fn restore_annotation(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    page: u32,
    id: u32,
) -> Result<AnnotationInfo, CommandError> {
    let _permit = update_state
        .gate()
        .enter()
        .map_err(|_| updates::updating_error())?;
    let pool = engine.0.clone();
    let item = blocking(move || pool.restore_annot(doc, page, id)).await?;
    Ok(item.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_three_kinds_are_markup_and_only_known_stamps_are_stamps() {
        assert_eq!(markup_kind("highlight").unwrap(), AnnotKind::Highlight);
        assert_eq!(markup_kind("underline").unwrap(), AnnotKind::Underline);
        assert_eq!(markup_kind("strikeout").unwrap(), AnnotKind::Strikeout);
        for name in [
            "note",
            "ink",
            "stamp",
            "other",
            "",
            "Highlight",
            "highlight ",
        ] {
            assert!(markup_kind(name).is_err(), "{name:?}");
        }
        for stamp in StampKind::ALL {
            assert_eq!(stamp_kind(stamp.name()).unwrap(), stamp);
        }
        for name in ["", "APPROVED", "secret", "approved "] {
            assert!(stamp_kind(name).is_err(), "{name:?}");
        }
        assert_eq!(markup_kind("note").unwrap_err().code, "invalid_request");
    }

    #[test]
    fn an_annotation_reaches_the_window_as_plain_data() {
        let item = AnnotInfo {
            id: 7,
            kind: AnnotKind::Note,
            abilities: ability::EDIT_TEXT | ability::MOVE | ability::REMOVE,
            rect: [10.504, 20.0, 30.25, 40.999],
            color: [1, 2, 3, 255],
            text: "<img src=x onerror=alert(1)>".into(),
            author: "Someone".into(),
        };
        let json = serde_json::to_value(AnnotationInfo::from(item)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "id": 7,
                "kind": "note",
                "can": {
                    "editText": true,
                    "recolour": false,
                    "move": true,
                    "remove": true,
                    "restore": false,
                    "locked": false,
                    "resize": false
                },
                "rect": [10.5, 20.0, 30.25, 41.0],
                "color": [1, 2, 3, 255],
                "text": "<img src=x onerror=alert(1)>",
                "author": "Someone"
            })
        );
    }
}
