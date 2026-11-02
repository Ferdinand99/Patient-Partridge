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

/// A change that was made counts as an unsaved change at once, in the app process, before the answer
/// is on its way to the window: an update that waits for the commands in flight and then looks again
/// at the documents with unsaved changes (it does, see `install_with_engines_stopped`) must not miss
/// the one that has just been made.
fn unsaved(update_state: &UpdateState, doc: u64) {
    update_state.unsaved().set(doc, true);
}

/// Held while a page tool changes a document: the document counts as having unsaved changes from
/// the start, so that a quit, a close or an update that looks at the documents while the tool works
/// does not find a clean one and lose the change.
struct Changing<'a> {
    state: &'a UpdateState,
    doc: u64,
}

impl<'a> Changing<'a> {
    fn begin(state: &'a UpdateState, doc: u64) -> Changing<'a> {
        state.unsaved().begin_change(doc);
        Changing { state, doc }
    }
}

impl Drop for Changing<'_> {
    fn drop(&mut self) {
        self.state.unsaved().end_change(self.doc);
    }
}

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
    let _changing = Changing::begin(&update_state, doc);
    let pool = engine.0.clone();
    let changed = blocking(move || pool.pages_delete(doc, &pages)).await?;
    unsaved(&update_state, doc);
    Ok(changed.into())
}

#[tauri::command]
pub async fn pages_reorder(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    order: Vec<u32>,
) -> Result<ChangedInfo, CommandError> {
    let _permit = permit(&update_state)?;
    let _changing = Changing::begin(&update_state, doc);
    let pool = engine.0.clone();
    let changed = blocking(move || pool.pages_reorder(doc, &order)).await?;
    unsaved(&update_state, doc);
    Ok(changed.into())
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
    let _changing = Changing::begin(&update_state, doc);
    let pool = engine.0.clone();
    let changed = blocking(move || pool.pages_rotate(doc, &pages, turns)).await?;
    unsaved(&update_state, doc);
    Ok(changed.into())
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
    let _changing = Changing::begin(&update_state, doc);
    let pool = engine.0.clone();
    let cropped = blocking(move || pool.pages_crop(doc, &pages, margins)).await?;
    unsaved(&update_state, doc);
    Ok(cropped.into())
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
    let _changing = Changing::begin(&update_state, doc);
    let pool = engine.0.clone();
    let changed = blocking(move || pool.pages_set_boxes(doc, &pages, &boxes)).await?;
    unsaved(&update_state, doc);
    Ok(changed.into())
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
    let _changing = Changing::begin(&update_state, doc);
    let pool = engine.0.clone();
    let changed = blocking(move || pool.pages_insert_blank(doc, at, width, height)).await?;
    unsaved(&update_state, doc);
    Ok(changed.into())
}

#[tauri::command]
pub async fn pages_restore(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    snapshot: u32,
) -> Result<ChangedInfo, CommandError> {
    let _permit = permit(&update_state)?;
    let _changing = Changing::begin(&update_state, doc);
    let pool = engine.0.clone();
    let changed = blocking(move || pool.pages_restore(doc, snapshot)).await?;
    unsaved(&update_state, doc);
    Ok(changed.into())
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
    let _changing = Changing::begin(&update_state, doc);
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
    unsaved(&update_state, doc);
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
/// as it was) or `"working"` (the engine is in the middle of a change, which cannot be interrupted
/// without losing the unsaved changes of the document: nothing was stopped, and the change
/// finishes).
#[tauri::command]
pub fn pages_cancel(engine: State<'_, Engine>, doc: u64) -> &'static str {
    cancel_word(engine.0.cancel_pages(doc))
}

/// The word the window knows for what a stop did.
fn cancel_word(done: Cancelled) -> &'static str {
    match done {
        Cancelled::Idle => "idle",
        Cancelled::Handover => "handover",
        Cancelled::Working => "working",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updates::InstallMethod;
    use partridge_protocol::PageSize;

    #[test]
    fn a_page_change_is_refused_while_an_update_is_being_installed_and_allowed_otherwise() {
        let state = UpdateState::new(None, InstallMethod::Automatic);
        // Every page command takes a permit first, like opening a document.
        let permit_now = permit(&state);
        assert!(permit_now.is_ok());
        drop(permit_now);
        state.gate().close();
        match permit(&state) {
            Err(e) => assert_eq!(e.code, "updating"),
            Ok(_) => panic!("a page change was allowed while an update was installing"),
        }
        state.gate().reopen();
        assert!(permit(&state).is_ok());
    }

    #[test]
    fn a_change_that_was_made_counts_as_unsaved_before_the_window_has_the_answer() {
        let state = UpdateState::new(None, InstallMethod::Automatic);
        assert!(!state.unsaved().any());
        unsaved(&state, 7);
        // An update that waits for the commands in flight and then asks must find it.
        assert!(state.unsaved().any());
        state.unsaved().set(7, false);
        assert!(!state.unsaved().any());
        // Another document is not touched by it.
        unsaved(&state, 3);
        state.forget_doc(3);
        assert!(!state.unsaved().any());
    }

    #[test]
    fn a_document_counts_as_unsaved_while_a_page_tool_works_on_it_and_not_after_a_failure() {
        let state = UpdateState::new(None, InstallMethod::Automatic);
        {
            let _changing = Changing::begin(&state, 5);
            // The window has not been told of any change, and the tool has not answered.
            assert!(state.unsaved().any());
            assert!(state.unsaved().holds_exit());
        }
        // The tool failed: nothing was changed, and nothing is held.
        assert!(!state.unsaved().any());
        {
            let _changing = Changing::begin(&state, 5);
            unsaved(&state, 5);
        }
        assert!(state.unsaved().any(), "a change that was made stays");
    }

    #[test]
    fn what_a_stop_did_is_told_in_the_words_the_window_knows() {
        assert_eq!(cancel_word(Cancelled::Idle), "idle");
        assert_eq!(cancel_word(Cancelled::Handover), "handover");
        // A change under way is not ended: the window is told it is too late, nothing more.
        assert_eq!(cancel_word(Cancelled::Working), "working");
    }

    #[test]
    fn the_sizes_of_a_change_reach_the_window_as_width_and_height_in_points() {
        let info = ChangedInfo::from(PagesChanged {
            pages: vec![
                PageSize {
                    width: 612.0,
                    height: 792.0,
                },
                PageSize {
                    width: 300.5,
                    height: 400.0,
                },
            ],
            snapshot: 4,
            count: 2,
            form_fields: 1,
        });
        assert_eq!(info.pages, vec![[612.0, 792.0], [300.5, 400.0]]);
        assert_eq!((info.snapshot, info.count, info.form_fields), (4, 2, 1));
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["formFields"], 1);
        let crop = CroppedInfo::from(PagesCropped {
            pages: vec![PageSize {
                width: 100.0,
                height: 200.0,
            }],
            before: vec![[0.0, 0.0, 100.0, 200.0]],
            after: vec![[1.0, 2.0, 99.0, 198.0]],
        });
        assert_eq!(crop.after, vec![[1.0, 2.0, 99.0, 198.0]]);
    }
}
