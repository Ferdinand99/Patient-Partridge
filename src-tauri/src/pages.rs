//! The commands of the page tools: delete, move, turn, crop, insert and copy out pages.
//!
//! The window names pages and numbers; the app process does the rest. It never names a file to
//! write (the dialog is opened here), and a file to read comes from the window the way it does for
//! opening a document. Every change is made in the engine of the document and is saved with the
//! same commands as any other change.

use partridge_engine_client::{
    Cancelled, ExtractOptions, InsertOptions, PagesChanged, PagesCropped,
};
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, State};

use crate::{blocking, choose_save_target, updates, CommandError, Engine, SavedInfo, UpdateState};

/// The document after a change, for the window.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedInfo {
    /// The size of every page in points, `[width, height]`, as it is drawn.
    pages: Vec<[f32; 2]>,
    /// The number of the copy of the document before the change, to go back to it (0: none).
    snapshot: u32,
    /// How many pages the change touched.
    count: u32,
    /// How many form fields sat on the pages that were deleted.
    form_fields: u32,
}

impl From<PagesChanged> for ChangedInfo {
    fn from(c: PagesChanged) -> Self {
        ChangedInfo {
            pages: c.pages.iter().map(|p| [p.width, p.height]).collect(),
            snapshot: c.snapshot,
            count: c.count,
            form_fields: c.form_fields,
        }
    }
}

/// The document after a crop: the crop box of each page before and after (`[left, bottom, right,
/// top]` in the page's own coordinates).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CroppedInfo {
    pages: Vec<[f32; 2]>,
    before: Vec<[f32; 4]>,
    after: Vec<[f32; 4]>,
}

impl From<PagesCropped> for CroppedInfo {
    fn from(c: PagesCropped) -> Self {
        CroppedInfo {
            pages: c.pages.iter().map(|p| [p.width, p.height]).collect(),
            before: c.before,
            after: c.after,
        }
    }
}

/// What an insert did.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InsertedInfo {
    changed: ChangedInfo,
    /// How many pages the other file had.
    inserted: u32,
}

/// How far the pages of an insert or a copy have come, in bytes: `[done, total]`.
type Progress = Channel<[u64; 2]>;

/// Page changes are not made while an update is being installed: it would end the engine in the
/// middle of the change.
fn permit(update_state: &UpdateState) -> Result<updates::OpenPermit, CommandError> {
    update_state
        .gate()
        .enter()
        .map_err(|_| updates::updating_error())
}

#[tauri::command]
pub async fn pages_delete(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    pages: Vec<u32>,
) -> Result<ChangedInfo, CommandError> {
    let _permit = permit(&update_state)?;
    let pool = engine.0.clone();
    blocking(move || pool.pages_delete(doc, &pages))
        .await
        .map(Into::into)
}

#[tauri::command]
pub async fn pages_reorder(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    order: Vec<u32>,
) -> Result<ChangedInfo, CommandError> {
    let _permit = permit(&update_state)?;
    let pool = engine.0.clone();
    blocking(move || pool.pages_reorder(doc, &order))
        .await
        .map(Into::into)
}

#[tauri::command]
pub async fn pages_rotate(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    pages: Vec<u32>,
    turns: u8,
) -> Result<ChangedInfo, CommandError> {
    let _permit = permit(&update_state)?;
    let pool = engine.0.clone();
    blocking(move || pool.pages_rotate(doc, &pages, turns))
        .await
        .map(Into::into)
}

/// `margins` are `[left, top, right, bottom]` in points on the page as it is drawn.
#[tauri::command]
pub async fn pages_crop(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    pages: Vec<u32>,
    margins: [f32; 4],
) -> Result<CroppedInfo, CommandError> {
    let _permit = permit(&update_state)?;
    let pool = engine.0.clone();
    blocking(move || pool.pages_crop(doc, &pages, margins))
        .await
        .map(Into::into)
}

#[tauri::command]
pub async fn pages_set_boxes(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    pages: Vec<u32>,
    boxes: Vec<[f32; 4]>,
) -> Result<ChangedInfo, CommandError> {
    let _permit = permit(&update_state)?;
    let pool = engine.0.clone();
    blocking(move || pool.pages_set_boxes(doc, &pages, &boxes))
        .await
        .map(Into::into)
}

#[tauri::command]
pub async fn pages_insert_blank(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    at: u32,
    width: f32,
    height: f32,
) -> Result<ChangedInfo, CommandError> {
    let _permit = permit(&update_state)?;
    let pool = engine.0.clone();
    blocking(move || pool.pages_insert_blank(doc, at, width, height))
        .await
        .map(Into::into)
}

#[tauri::command]
pub async fn pages_restore(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    snapshot: u32,
) -> Result<ChangedInfo, CommandError> {
    let _permit = permit(&update_state)?;
    let pool = engine.0.clone();
    blocking(move || pool.pages_restore(doc, snapshot))
        .await
        .map(Into::into)
}

/// Inserts all pages of the file at `path` so that the first becomes page `at`. The file needs the
/// `password` if it is protected, and `allow_protected` and `allow_signed` say that its pages may
/// join this document without the protection and the signature (the command fails with
/// `insert_protected` or `insert_signed` until they do). `progress` is told `[done, total]` bytes.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn pages_insert_pdf(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    at: u32,
    path: String,
    password: Option<String>,
    allow_protected: bool,
    allow_signed: bool,
    progress: Progress,
) -> Result<InsertedInfo, CommandError> {
    let _permit = permit(&update_state)?;
    let pool = engine.0.clone();
    let options = InsertOptions {
        allow_protected,
        allow_signed,
    };
    let outcome = blocking(move || {
        pool.insert_pdf(
            doc,
            at,
            &path,
            password.as_deref(),
            options,
            &|done, total| {
                let _ = progress.send([done, total]);
            },
        )
    })
    .await?;
    Ok(InsertedInfo {
        changed: outcome.changed.into(),
        inserted: outcome.inserted,
    })
}

/// Asks where to write the new file (with the dialog of the system), and copies the pages
/// (counted from 0, in the order given) into it. `None` when the dialog is cancelled. The new file
/// has no password and no signature; the command fails with `save_unprotected` or `save_signed`
/// until `allow_unprotected` and `allow_signed` say that is all right.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn pages_extract(
    app: AppHandle,
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    pages: Vec<u32>,
    allow_signed: bool,
    allow_unprotected: bool,
    progress: Progress,
) -> Result<Option<SavedInfo>, CommandError> {
    let pool = engine.0.clone();
    let Some(current) = pool.source_path(doc) else {
        return Err(CommandError::from(
            partridge_engine_client::ClientError::Engine {
                code: partridge_protocol::ErrorCode::UnknownDocument,
                message: "the document is not open".into(),
            },
        ));
    };
    // The dialog starts from the document's folder with a name of its own, never the document.
    let stem = current
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "pages".into());
    let start = current.with_file_name(format!("{stem}-pages.pdf"));
    let Some(target) = choose_save_target(&app, start).await? else {
        return Ok(None);
    };
    let _permit = permit(&update_state)?;
    let options = ExtractOptions {
        allow_signed,
        allow_unprotected,
    };
    let outcome = blocking(move || {
        pool.extract_pages(doc, &pages, &target, options, &|done, total| {
            let _ = progress.send([done, total]);
        })
    })
    .await?;
    Ok(Some(SavedInfo::from(outcome)))
}

/// Stops the work a page tool is doing on a document. Answers what that did: `"idle"` (nothing was
/// under way), `"handover"` (the pages being brought in or copied out were stopped, the document is
/// as it was) or `"stopped"` (the engine was in the middle of a change, which cannot be
/// interrupted, so the engine was stopped and the document is gone with its unsaved changes; the
/// file on the disk is untouched).
#[tauri::command]
pub fn pages_cancel(engine: State<'_, Engine>, doc: u64) -> &'static str {
    match engine.0.cancel_pages(doc) {
        Cancelled::Idle => "idle",
        Cancelled::Handover => "handover",
        Cancelled::EngineStopped => "stopped",
    }
}
