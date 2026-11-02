//! Patient Partridge, the app process.
//!
//! This process shows the window and decides what to open. It never parses a PDF: all of that is
//! done by `partridge-engine`, a separate process that this app starts and supervises. If the
//! engine crashes or hangs on a bad file, the command that was waiting returns an error and the
//! window stays up.
//!
//! Every open document has its own engine process (see `EnginePool`), so a file that crashes its
//! engine closes only its own tab. The documents belong to the page in the window: when the page
//! starts again (a reload, whatever the cause), every engine is ended and the window starts clean.

mod links;
mod locate;
mod positions;
mod session;
mod unsaved;
mod updates;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use links::KnownLinks;
use partridge_engine_client::{
    ClientError, EngineConfig, EnginePool, LinkTarget, SaveOptions, SaveOutcome,
};
use positions::{Position, Positions};
use serde::Serialize;
use session::SessionStore;
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use updates::UpdateState;

static START: OnceLock<Instant> = OnceLock::new();

/// The event that asks the window to put the question "save the changes?" before the app closes.
const QUIT_REQUESTED: &str = "quit-requested";

/// Paths handed to us before the frontend is listening (command line, file association).
/// `ready` and the queue share one lock, so a path can never slip in between "is the frontend
/// ready?" and "take the queue".
#[derive(Default)]
struct Pending(Mutex<PendingInner>);

#[derive(Default)]
struct PendingInner {
    ready: bool,
    paths: Vec<String>,
}

impl Pending {
    /// Returns the paths to emit now when the frontend is ready, otherwise queues them.
    fn deliver(&self, paths: Vec<String>) -> Option<Vec<String>> {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if inner.ready {
            Some(paths)
        } else {
            inner.paths.extend(paths);
            None
        }
    }

    /// Marks the frontend ready and returns everything queued so far.
    fn take(&self) -> Vec<String> {
        let mut inner = self.0.lock().unwrap_or_else(|e| e.into_inner());
        inner.ready = true;
        std::mem::take(&mut inner.paths)
    }

    /// The page is starting again, so nobody is listening: paths wait until it asks for them.
    fn reset(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).ready = false;
    }
}

/// What a request to close the window or to quit leads to.
#[derive(Debug, PartialEq, Eq)]
enum ExitAction {
    /// Nothing is in the way.
    Go,
    /// An update is replacing the program: closing now would leave a half file.
    Refuse,
    /// A document has changes that are not saved: the window asks the user first.
    Ask,
}

fn exit_action(state: Option<&UpdateState>) -> ExitAction {
    match state {
        Some(state) if state.is_close_blocked() => ExitAction::Refuse,
        Some(state) if state.unsaved().holds_exit() => ExitAction::Ask,
        _ => ExitAction::Go,
    }
}

/// Where a save as goes, and whether the user has to be asked before it replaces something.
#[derive(Debug, PartialEq, Eq)]
struct SaveTarget {
    path: PathBuf,
    ask_to_replace: bool,
}

/// A name typed without an ending gets `.pdf`. The dialog of the system asks about replacing the
/// name as it was typed, not the name with the ending (GTK, for one, does not look further), so
/// when the name with the ending is a file that exists, the app asks.
fn plan_save_target(chosen: PathBuf, exists: impl Fn(&Path) -> bool) -> SaveTarget {
    if chosen.extension().is_some() {
        // The dialog has asked already, if the file exists.
        return SaveTarget {
            path: chosen,
            ask_to_replace: false,
        };
    }
    let mut path = chosen;
    path.set_extension("pdf");
    let ask_to_replace = exists(&path);
    SaveTarget {
        path,
        ask_to_replace,
    }
}

/// The id of the menu entry that quits the app.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const QUIT_MENU_ID: &str = "partridge-quit";

/// What the entry "Quit" of the menu does: the same as a request to close the window.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn quit_from_menu<R: tauri::Runtime>(app: &AppHandle<R>) {
    match exit_action(app.try_state::<UpdateState>().as_deref()) {
        ExitAction::Go => app.exit(0),
        ExitAction::Refuse => {}
        ExitAction::Ask => {
            let _ = app.emit(QUIT_REQUESTED, ());
        }
    }
}

/// The menu of the app. On macOS the standard menu has a Quit entry that ends the program
/// through the system (`terminate:`) without asking anyone, so that entry (and its Cmd+Q) is
/// replaced by one of the app's own, which goes through [`quit_from_menu`]. Quitting from the
/// Dock's menu, and a log out or shut down, also go through the system and cannot be held back
/// this way.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn app_menu<R: tauri::Runtime>(app: &AppHandle<R>) -> tauri::Result<tauri::menu::Menu<R>> {
    let menu = tauri::menu::Menu::default(app)?;
    use_own_quit(app, &menu)?;
    Ok(menu)
}

/// Replaces the last entry of the first submenu (the application menu, where macOS puts Quit)
/// by an entry of the app's own with the same key.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn use_own_quit<R: tauri::Runtime>(
    app: &AppHandle<R>,
    menu: &tauri::menu::Menu<R>,
) -> tauri::Result<bool> {
    use tauri::menu::{MenuItem, MenuItemKind};
    let Some(MenuItemKind::Submenu(first)) = menu.items()?.into_iter().next() else {
        return Ok(false);
    };
    let Some(last) = first.items()?.into_iter().last() else {
        return Ok(false);
    };
    if !matches!(last, MenuItemKind::Predefined(_)) {
        return Ok(false);
    }
    first.remove(&last)?;
    let quit = MenuItem::with_id(
        app,
        QUIT_MENU_ID,
        format!("Quit {}", app.package_info().name),
        true,
        Some("CmdOrCtrl+Q"),
    )?;
    first.append(&quit)?;
    Ok(true)
}

/// Holds back a request to close the window while a document has changes that are not saved, and
/// asks the window to put the question. (An update that is replacing the program holds it back
/// too.)
fn exit_guard<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::<R>::new("exit-guard")
        .on_event(|app, event| {
            if let tauri::RunEvent::WindowEvent {
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } = event
            {
                match exit_action(app.try_state::<UpdateState>().as_deref()) {
                    ExitAction::Go => {}
                    ExitAction::Refuse => api.prevent_close(),
                    ExitAction::Ask => {
                        // Changes that are not saved: the window asks the user what to do.
                        api.prevent_close();
                        let _ = app.emit(QUIT_REQUESTED, ());
                    }
                }
            }
        })
        .build()
}

/// True for the pages of the app itself: the built window (`tauri://localhost` on macOS and Linux,
/// `http://tauri.localhost` on Windows) and, in a development build, the dev server. The window
/// never goes anywhere else, so any other address is refused instead of being loaded in a window
/// that can talk to the app.
fn is_app_page(url: &url::Url, development: bool) -> bool {
    match url.scheme() {
        "tauri" => url.host_str() == Some("localhost"),
        "http" | "https" => {
            url.host_str() == Some("tauri.localhost")
                || (development
                    && matches!(url.host_str(), Some("localhost" | "127.0.0.1"))
                    && url.port() == Some(1421))
        }
        "about" => url.as_str() == "about:blank",
        _ => false,
    }
}

/// Everything the app keeps for the page that was in the window: the engines of its documents, the
/// addresses they reported, and the wait for the page to ask for the files it was started with.
/// The new page knows nothing of the old documents, so nobody could close them.
fn release_documents(pool: &EnginePool, known: &Mutex<KnownLinks>, pending: &Pending) {
    pool.kill_all();
    known.lock().unwrap_or_else(|e| e.into_inner()).clear();
    pending.reset();
}

/// Called when the page in the window starts to load, the first time and after every reload.
fn page_started<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let (Some(engine), Some(known)) = (app.try_state::<Engine>(), app.try_state::<LinkMemory>())
    {
        release_documents(&engine.0, &known.0, &app.state::<Pending>());
        // The page that knew the documents is gone, so they are not "open" for an update either.
        if let Some(update_state) = app.try_state::<UpdateState>() {
            update_state.forget_all_docs();
        }
    }
}

/// The checks before a web address from a document is opened: the engine reported it for that
/// document, and it is a plain `http` or `https` address. Gives the address to open.
fn authorize_link(known: &Mutex<KnownLinks>, doc: u64, url: &str) -> Result<String, CommandError> {
    let refuse = |message: &str| CommandError {
        code: "link_refused".into(),
        message: message.into(),
    };
    if !known
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .allows(doc, url)
    {
        return Err(refuse("this address is not a link of the document"));
    }
    links::check_web_address(url)
        .map_err(|_| refuse("only web addresses (http and https) are opened"))
}

/// Most documents open at once. Each one has an engine process of its own, about 4 MiB when idle.
const MAX_DOCUMENTS: usize = 16;

/// The supervised engine processes, one per open document.
struct Engine(Arc<EnginePool>);

/// The web addresses the engine has reported for each open document. Only these can be opened.
struct LinkMemory(Mutex<KnownLinks>);

/// Where the reader was in each file, kept in a file in the app's data folder.
struct PositionStore(Mutex<Positions>);

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What the frontend sees when a command fails: a stable code and a short detail.
#[derive(Serialize, Debug, PartialEq)]
struct CommandError {
    code: String,
    message: String,
}

impl From<ClientError> for CommandError {
    fn from(err: ClientError) -> Self {
        match err {
            ClientError::EngineStopped(why) => CommandError {
                code: "engine_stopped".into(),
                message: why,
            },
            ClientError::Engine { code, message } => CommandError {
                code: code.name().into(),
                message,
            },
            ClientError::Save(why) => CommandError {
                code: why.code().into(),
                message: why.to_string(),
            },
        }
    }
}

#[derive(Serialize)]
struct OpenedInfo {
    /// Handle of the document. Counted up from 1 and never reused, so a handle that outlives its
    /// document cannot reach another one. It is far below 2^53, so the window reads it exactly.
    doc: u64,
    /// Page sizes in PDF points, `[width, height]`.
    pages: Vec<[f32; 2]>,
    name: String,
    /// Where this file was left the last time, if it was.
    position: Option<Position>,
}

/// Runs a blocking engine call off the thread that handles the window.
async fn blocking<T, F>(f: F) -> Result<T, CommandError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ClientError> + Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(f).await {
        Ok(result) => result.map_err(CommandError::from),
        Err(e) => Err(CommandError {
            code: "internal".into(),
            message: e.to_string(),
        }),
    }
}

#[tauri::command]
async fn open_pdf(
    engine: State<'_, Engine>,
    positions: State<'_, PositionStore>,
    update_state: State<'_, UpdateState>,
    path: String,
    password: Option<String>,
) -> Result<OpenedInfo, CommandError> {
    // Not while an update is being installed: an engine started now would keep the installer from
    // replacing the program. The permit is held until the document is open.
    let _permit = update_state
        .gate()
        .enter()
        .map_err(|_| updates::updating_error())?;
    let pool = engine.0.clone();
    let name = Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());
    let key = positions::key_for(&path);
    let opened_path = path.clone();
    let (doc, pages) = blocking(move || pool.open(&path, password.as_deref())).await?;
    // The app knows which files are open, so that an update can bring them back.
    update_state.record_open(doc, &opened_path);
    let position = positions
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key);
    Ok(OpenedInfo {
        doc,
        pages: pages.iter().map(|p| [p.width, p.height]).collect(),
        name,
        position,
    })
}

/// Remembers where the reader is in a file. A failure to write the file is not an error for the
/// reader: the position is kept in the app until it closes.
#[tauri::command]
fn set_position(
    positions: State<'_, PositionStore>,
    path: String,
    page: u32,
    offset: f32,
    zoom: Option<f32>,
) {
    let key = positions::key_for(&path);
    let _ = positions.0.lock().unwrap_or_else(|e| e.into_inner()).set(
        &key,
        Position { page, offset, zoom },
        now_seconds(),
    );
}

/// Forgets every remembered position, and the list of tabs to reopen with them.
fn forget_positions(positions: &Mutex<Positions>, session: &SessionStore) {
    let _ = positions.lock().unwrap_or_else(|e| e.into_inner()).clear();
    session.clear();
}

#[tauri::command]
fn clear_positions(positions: State<'_, PositionStore>, session: State<'_, SessionStore>) {
    forget_positions(&positions.0, &session);
}

/// Draws a page. The answer is the raw bitmap, `width * height * 4` bytes of RGBA, sent as binary
/// (not JSON) so a full page does not turn into text on the way to the window.
#[tauri::command]
async fn render_page(
    engine: State<'_, Engine>,
    doc: u64,
    page: u32,
    width: u32,
    height: u32,
) -> Result<tauri::ipc::Response, CommandError> {
    let pool = engine.0.clone();
    let bitmap = blocking(move || pool.render(doc, page, width, height)).await?;
    Ok(tauri::ipc::Response::new(bitmap.rgba))
}

#[tauri::command]
async fn close_pdf(
    engine: State<'_, Engine>,
    known: State<'_, LinkMemory>,
    update_state: State<'_, UpdateState>,
    doc: u64,
) -> Result<(), CommandError> {
    known
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .forget(doc);
    update_state.forget_doc(doc);
    let pool = engine.0.clone();
    // Closing a document that is gone is fine: the end state is what the caller wants.
    let _ = blocking(move || {
        pool.close(doc);
        Ok(())
    })
    .await;
    Ok(())
}

/// The window says whether a document has changes that are not saved. The app then holds back
/// closing, quitting and installing an update until the user has decided.
#[tauri::command]
fn set_dirty(update_state: State<'_, UpdateState>, doc: u64, dirty: bool) {
    update_state.unsaved().set(doc, dirty);
}

/// The file a save wrote. The window shows the name, and uses the path as the tab's file.
#[derive(Serialize)]
struct SavedInfo {
    path: String,
    name: String,
}

impl From<SaveOutcome> for SavedInfo {
    fn from(outcome: SaveOutcome) -> Self {
        SavedInfo {
            name: outcome
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path: outcome.path.to_string_lossy().into_owned(),
        }
    }
}

/// Saves a document to the file it was opened from. The window never names a file: the app knows
/// which one a document belongs to. `overwrite_changed` and `allow_signed` are the answers to the
/// two questions a save can ask (the file changed on disk; the document is signed).
#[tauri::command]
async fn save_pdf(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    overwrite_changed: bool,
    allow_signed: bool,
) -> Result<SavedInfo, CommandError> {
    // Not while an update is being installed: it would end the engine in the middle of the save.
    let _permit = update_state
        .gate()
        .enter()
        .map_err(|_| updates::updating_error())?;
    let pool = engine.0.clone();
    let options = SaveOptions {
        allow_signed,
        overwrite_changed,
    };
    let outcome = blocking(move || pool.save(doc, options)).await?;
    // The dirty state is not touched here. A save writes the document as it was when the save
    // began; an edit may have come since, and only the window knows. It tells the app process
    // through `set_dirty` when the document is clean.
    Ok(outcome.into())
}

/// Asks the user where to save a copy (with the dialog of the system, which the app opens itself:
/// the window cannot name a file to write), and saves the document there. The document belongs to
/// the new file afterwards. `None` when the user cancels the dialog.
#[tauri::command]
async fn save_pdf_as(
    app: AppHandle,
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    allow_signed: bool,
) -> Result<Option<SavedInfo>, CommandError> {
    let pool = engine.0.clone();
    let Some(current) = pool.source_path(doc) else {
        return Err(CommandError::from(ClientError::Engine {
            code: partridge_protocol::ErrorCode::UnknownDocument,
            message: "the document is not open".into(),
        }));
    };
    let dialog_app = app.clone();
    // The dialog is the user's to answer for as long as they like; the gate (which an update
    // install closes) is entered only afterwards, so the dialog never holds an update back.
    let chosen = tauri::async_runtime::spawn_blocking(move || {
        let mut start = current;
        // A name typed without an ending gets `.pdf`. The dialog asked about replacing the name as
        // it was typed; when the name with the ending is another file that exists, the dialog is
        // shown again with that name, so the system asks the question about the right file and
        // nothing is replaced without one. (The message boxes of the system are not used: they
        // need a manifest that the test programs of Windows do not have.)
        for _ in 0..3 {
            let mut dialog = dialog_app
                .dialog()
                .file()
                .add_filter("PDF documents", &["pdf"]);
            if let Some(window) = dialog_app.get_webview_window("main") {
                dialog = dialog.set_parent(&window);
            }
            if let Some(dir) = start.parent() {
                dialog = dialog.set_directory(dir);
            }
            if let Some(name) = start.file_name() {
                dialog = dialog.set_file_name(name.to_string_lossy());
            }
            let chosen = dialog.blocking_save_file()?;
            let Ok(chosen) = chosen.into_path() else {
                return Some(Err(()));
            };
            let plan = plan_save_target(chosen, |p| p.exists());
            if !plan.ask_to_replace {
                return Some(Ok(plan.path));
            }
            start = plan.path;
        }
        None
    })
    .await
    .map_err(|e| CommandError {
        code: "internal".into(),
        message: e.to_string(),
    })?;
    let target = match chosen {
        None => return Ok(None),
        Some(Ok(path)) => path,
        Some(Err(())) => {
            return Err(CommandError {
                code: "save_write_failed".into(),
                message: "the place that was chosen is not a file on this computer".into(),
            })
        }
    };
    // Not while an update is being installed: it would end the engine in the middle of the save.
    let _permit = update_state
        .gate()
        .enter()
        .map_err(|_| updates::updating_error())?;
    let options = SaveOptions {
        allow_signed,
        overwrite_changed: false,
    };
    let outcome = blocking(move || pool.save_as(doc, &target, options)).await?;
    let info = SavedInfo::from(outcome);
    update_state.move_doc(doc, &info.path);
    Ok(Some(info))
}

/// Quits the app after the user has decided what happens to the changes that are not saved.
#[tauri::command]
fn quit_app(app: AppHandle, update_state: State<'_, UpdateState>) {
    update_state.unsaved().allow_exit();
    app.exit(0);
}

/// The user answered "stay" to the question before closing: the next request to close asks again.
#[tauri::command]
fn stay_open(update_state: State<'_, UpdateState>) {
    update_state.unsaved().disallow_exit();
}

/// One entry of the table of contents.
#[derive(Serialize)]
struct OutlineEntry {
    depth: u8,
    /// The page the entry leads to, counted from 0, or null when it leads nowhere in the document.
    page: Option<u32>,
    title: String,
}

#[tauri::command]
async fn get_outline(
    engine: State<'_, Engine>,
    doc: u64,
) -> Result<Vec<OutlineEntry>, CommandError> {
    let pool = engine.0.clone();
    let items = blocking(move || pool.outline(doc)).await?;
    Ok(items
        .into_iter()
        .map(|i| OutlineEntry {
            depth: i.depth,
            page: i.page,
            title: i.title,
        })
        .collect())
}

/// Rounds a coordinate to a hundredth of a point, which is far finer than a pixel and keeps the
/// text of a page small when it is sent to the window.
fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

/// The text of a page. `rects` holds four numbers for every character of `text`, in order:
/// left, top, right and bottom, in points from the top left of the page as it is drawn.
#[derive(Serialize)]
struct PageTextInfo {
    text: String,
    rects: Vec<f32>,
}

#[tauri::command]
async fn get_page_text(
    engine: State<'_, Engine>,
    doc: u64,
    page: u32,
) -> Result<PageTextInfo, CommandError> {
    let pool = engine.0.clone();
    let text = blocking(move || pool.page_text(doc, page)).await?;
    Ok(PageTextInfo {
        text: text.text,
        rects: text.rects.iter().flatten().map(|v| round2(*v)).collect(),
    })
}

/// A link on a page, with the box it covers (left, top, right, bottom, in points from the top
/// left of the page as it is drawn). Exactly one of `uri` and `page` is set.
#[derive(Serialize)]
struct LinkInfo {
    rect: [f32; 4],
    uri: Option<String>,
    page: Option<u32>,
}

/// The scripts for which the engine of a document was asked for a font and had none to give
/// (`partridge_engine_client::system_fonts` says which fonts it looks for). Text in those scripts,
/// in a font the PDF does not carry, is drawn as blanks; the window tells the reader.
#[tauri::command]
async fn missed_fonts(engine: State<'_, Engine>, doc: u64) -> Result<Vec<String>, CommandError> {
    let pool = engine.0.clone();
    let scripts = blocking(move || pool.font_report(doc)).await?;
    Ok(scripts.into_iter().map(|s| s.name().to_string()).collect())
}

#[tauri::command]
async fn get_page_links(
    engine: State<'_, Engine>,
    known: State<'_, LinkMemory>,
    doc: u64,
    page: u32,
) -> Result<Vec<LinkInfo>, CommandError> {
    let pool = engine.0.clone();
    let links = blocking(move || pool.page_links(doc, page)).await?;
    {
        let mut memory = known.0.lock().unwrap_or_else(|e| e.into_inner());
        for link in &links {
            if let LinkTarget::Uri(uri) = &link.target {
                memory.remember(doc, uri);
            }
        }
    }
    Ok(links
        .into_iter()
        .map(|l| {
            let (uri, page) = match l.target {
                LinkTarget::Uri(u) => (Some(u), None),
                LinkTarget::Page(p) => (None, Some(p)),
            };
            LinkInfo {
                rect: l.rect.map(round2),
                uri,
                page,
            }
        })
        .collect())
}

/// Opens a web address from a document in the user's browser. The window has asked the user first;
/// this is the second check. Only `http` and `https` addresses are opened, and only ones that the
/// engine reported for this document.
#[tauri::command]
fn open_link(known: State<'_, LinkMemory>, doc: u64, url: String) -> Result<(), CommandError> {
    let address = authorize_link(&known.0, doc, &url)?;
    links::open_in_browser(&address).map_err(|_| CommandError {
        code: "link_refused".into(),
        message: "the web browser could not be started".into(),
    })
}

/// Finds a text on one page. Each match is `[start, length]` in characters of the page text.
#[tauri::command]
async fn search_page(
    engine: State<'_, Engine>,
    doc: u64,
    page: u32,
    query: String,
) -> Result<Vec<[u32; 2]>, CommandError> {
    let pool = engine.0.clone();
    let found = blocking(move || pool.search_page(doc, page, &query)).await?;
    Ok(found.into_iter().map(|m| [m.start, m.len]).collect())
}

/// The paths the app was started with. Also tells the app that the frontend is now listening.
#[tauri::command]
fn take_pending_paths(pending: State<'_, Pending>) -> Vec<String> {
    pending.take()
}

/// Writes an error of the window to the standard error stream of the app, so that a failure in the
/// window shows up in a terminal or in the log of a test run. It goes nowhere else. The text is cut
/// to a short length.
#[tauri::command]
fn frontend_error(message: String) {
    let short: String = message.chars().take(2000).collect();
    eprintln!("window error: {short}");
}

/// Opens the print dialog of the web view. Only macOS needs this, because `window.print()` does
/// nothing in its web view; everywhere else the window calls `window.print()` itself.
#[tauri::command]
fn print_webview(window: tauri::WebviewWindow) -> Result<(), CommandError> {
    window.print().map_err(|e| CommandError {
        code: "internal".into(),
        message: e.to_string(),
    })
}

/// True when a measurement run asked the app to report its timings (see `scripts/measure.sh`).
#[tauri::command]
fn measure_mode() -> bool {
    std::env::var_os("PARTRIDGE_MEASURE_REPORT").is_some()
}

/// Writes the timings of a measurement run to the file named in `PARTRIDGE_MEASURE_REPORT`, then
/// closes the app after a short pause that leaves time to read the memory use of its processes.
#[tauri::command]
fn measure_report(app: AppHandle, open_ms: f64, first_page_ms: f64, pages: u32) {
    let Some(path) = std::env::var_os("PARTRIDGE_MEASURE_REPORT") else {
        return;
    };
    let since_start = START.get().map(|s| s.elapsed().as_secs_f64() * 1000.0);
    let json = serde_json::json!({
        "open_ms": open_ms,
        "first_page_ms": first_page_ms,
        "pages": pages,
        "since_process_start_ms": since_start,
    });
    let _ = std::fs::write(path, json.to_string());
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(4));
        app.exit(0);
    });
}

fn deliver_paths(app: &AppHandle, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    if let Some(now) = app.state::<Pending>().deliver(paths) {
        let _ = app.emit("open-paths", now);
    }
}

fn focus_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn engine_config(app: &tauri::App) -> EngineConfig {
    let resource_dir = app.path().resource_dir().ok();
    let pdfium_dir = locate::pdfium_dir(resource_dir.as_deref());
    let mut config = EngineConfig::new(locate::engine_program());
    config.args = vec!["--pdfium-dir".into(), pdfium_dir.into_os_string()];
    config
}

pub fn run() {
    let _ = START.set(Instant::now());

    let builder = tauri::Builder::default();
    // On macOS the application menu has a Quit entry that would end the app without a word: it is
    // replaced by one that goes through the same check as closing the window. (Windows and Linux
    // have no menu bar and keep none.)
    #[cfg(target_os = "macos")]
    let builder = builder.menu(app_menu).on_menu_event(|app, event| {
        if event.id().as_ref() == QUIT_MENU_ID {
            quit_from_menu(app);
        }
    });
    let app = builder
        // While an update replaces the program, or a document has changes that are not saved,
        // a request to close the window is held back.
        .plugin(exit_guard())
        .on_page_load(|webview, payload| {
            if matches!(payload.event(), PageLoadEvent::Started) {
                page_started(webview.app_handle());
            }
        })
        .plugin(
            tauri::plugin::Builder::<tauri::Wry>::new("navigation-guard")
                .on_navigation(|_webview, url| is_app_page(url, cfg!(debug_assertions)))
                .build(),
        )
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            deliver_paths(
                app,
                locate::existing_files(args.into_iter().skip(1), Some(Path::new(&cwd))),
            );
            focus_main(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Pending::default())
        .setup(|app| {
            let positions_file = app
                .path()
                .app_data_dir()
                .ok()
                .map(|dir| dir.join("positions.json"));
            app.manage(LinkMemory(Mutex::new(KnownLinks::default())));
            app.manage(PositionStore(Mutex::new(Positions::load(positions_file))));
            app.manage(Engine(Arc::new(EnginePool::new(
                engine_config(app),
                MAX_DOCUMENTS,
            ))));
            let update_state = UpdateState::new(app.path().app_data_dir().ok(), updates::detect());
            // The tabs of the last run, for the window to reopen. Nothing is read, and a list that is
            // left over is deleted, while the setting is off.
            app.manage(SessionStore(Mutex::new(session::Session::load(
                app.path()
                    .app_data_dir()
                    .ok()
                    .map(|dir| dir.join("session.json")),
                update_state.reopen_tabs(),
            ))));
            // After an update the documents that were open come back, once: the window asks for them
            // (`take_restored_paths`) and opens them without the rules for files named at the start.
            #[cfg_attr(not(feature = "update-driver"), allow(unused_variables))]
            let restored = update_state.load_restored();
            app.manage(update_state);
            #[cfg(feature = "update-driver")]
            updates::driver::start(app.handle().clone(), restored);
            let args: Vec<PathBuf> = locate::existing_files(
                std::env::args().skip(1),
                std::env::current_dir().ok().as_deref(),
            )
            .into_iter()
            .map(PathBuf::from)
            .collect();
            app.state::<Pending>().deliver(
                args.iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect(),
            );
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_pdf,
            render_page,
            close_pdf,
            set_dirty,
            save_pdf,
            save_pdf_as,
            quit_app,
            stay_open,
            set_position,
            clear_positions,
            session::get_reopen_tabs,
            session::set_reopen_tabs,
            session::save_session,
            session::take_session,
            get_outline,
            get_page_text,
            get_page_links,
            missed_fonts,
            open_link,
            print_webview,
            search_page,
            take_pending_paths,
            measure_mode,
            frontend_error,
            measure_report,
            updates::get_update_settings,
            updates::set_auto_update,
            updates::check_update,
            updates::download_update,
            updates::install_update,
            updates::restart_app,
            updates::open_releases_page,
            updates::take_restored_paths,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the app");

    app.run(|handle, event| {
        // Quitting from the menu or the keyboard (Cmd+Q) does not go through the window's close
        // request: changes that are not saved hold it back here too.
        if let tauri::RunEvent::ExitRequested {
            api, code: None, ..
        } = &event
        {
            match exit_action(handle.try_state::<UpdateState>().as_deref()) {
                ExitAction::Go => {}
                ExitAction::Refuse => api.prevent_exit(),
                ExitAction::Ask => {
                    api.prevent_exit();
                    let _ = handle.emit(QUIT_REQUESTED, ());
                }
            }
        }
        // The engine must not outlive the app. A healthy engine ends by itself when its input
        // closes, but one that is stuck inside PDFium never looks at its input, and a request that
        // waits for it would hold the quit back. Killing it does not wait for any request.
        if let tauri::RunEvent::Exit = &event {
            if let Some(engine) = handle.try_state::<Engine>() {
                engine.0.kill_all();
            }
        }
        // A file opened from the dock or Finder on macOS arrives as an event, not as an argument.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Opened { urls } = &event {
            let paths = urls
                .iter()
                .filter_map(|u| u.to_file_path().ok())
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            deliver_paths(handle, paths);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use partridge_protocol::ErrorCode;

    #[test]
    fn forgetting_the_positions_forgets_the_list_of_tabs_too() {
        let dir = std::env::temp_dir().join(format!("partridge-forget-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let positions_file = dir.join("positions.json");
        let session_file = dir.join("session.json");
        let mut positions = Positions::load(Some(positions_file.clone()));
        positions
            .set(
                "a.pdf",
                Position {
                    page: 2,
                    offset: 0.5,
                    zoom: None,
                },
                1,
            )
            .unwrap();
        let positions = Mutex::new(positions);
        let mut session = session::Session::load(Some(session_file.clone()), true);
        session
            .save(
                true,
                session::TabList {
                    paths: vec!["a.pdf".into()],
                    active: Some(0),
                    failed: Vec::new(),
                },
            )
            .unwrap();
        let store = SessionStore(Mutex::new(session));
        assert!(session_file.exists());

        forget_positions(&positions, &store);

        assert!(
            !session_file.exists(),
            "the list of tabs must go with the positions"
        );
        assert!(positions.lock().unwrap().get("a.pdf").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn closing_waits_for_the_user_while_a_document_has_unsaved_changes() {
        let state = UpdateState::new(None, updates::InstallMethod::Automatic);
        assert_eq!(exit_action(Some(&state)), ExitAction::Go);
        assert_eq!(exit_action(None), ExitAction::Go);

        state.unsaved().set(4, true);
        assert_eq!(exit_action(Some(&state)), ExitAction::Ask);
        // The user chose to give the changes up: closing goes on. "Stay" asks again next time.
        state.unsaved().allow_exit();
        assert_eq!(exit_action(Some(&state)), ExitAction::Go);
        state.unsaved().disallow_exit();
        assert_eq!(exit_action(Some(&state)), ExitAction::Ask);
        // Saved: nothing to ask.
        state.unsaved().set(4, false);
        assert_eq!(exit_action(Some(&state)), ExitAction::Go);
        // A document that is closed takes its changes with it.
        state.unsaved().set(5, true);
        state.forget_doc(5);
        assert_eq!(exit_action(Some(&state)), ExitAction::Go);
        // The page starting again forgets them too.
        state.unsaved().set(6, true);
        state.forget_all_docs();
        assert_eq!(exit_action(Some(&state)), ExitAction::Go);
    }

    #[test]
    fn a_save_reports_the_file_it_wrote_by_name_and_path() {
        let info = SavedInfo::from(SaveOutcome {
            path: PathBuf::from("/docs/report.pdf"),
            bytes: 10,
        });
        assert_eq!(info.name, "report.pdf");
        assert_eq!(info.path, "/docs/report.pdf");
    }

    #[test]
    fn a_failed_save_reaches_the_window_with_a_code_of_its_own() {
        let changed = CommandError::from(ClientError::Save(
            partridge_engine_client::SaveError::Changed,
        ));
        assert_eq!(changed.code, "save_changed");
        let signed = CommandError::from(ClientError::Save(
            partridge_engine_client::SaveError::Signed { signatures: 2 },
        ));
        assert_eq!(signed.code, "save_signed");
        assert!(signed.message.contains("2 digital signatures"));
    }

    #[test]
    fn a_name_typed_without_an_ending_gets_pdf_and_asks_before_it_replaces_a_file() {
        let exists = |names: &'static [&'static str]| {
            move |p: &Path| names.iter().any(|n| p == Path::new(n))
        };
        // "report" and report.pdf exists: the dialog asked about "report" only.
        assert_eq!(
            plan_save_target(PathBuf::from("/docs/report"), exists(&["/docs/report.pdf"])),
            SaveTarget {
                path: PathBuf::from("/docs/report.pdf"),
                ask_to_replace: true
            }
        );
        // Nothing there: no question.
        assert_eq!(
            plan_save_target(PathBuf::from("/docs/report"), exists(&[])),
            SaveTarget {
                path: PathBuf::from("/docs/report.pdf"),
                ask_to_replace: false
            }
        );
        // A name with an ending is what the dialog asked about: it is not asked again.
        assert_eq!(
            plan_save_target(
                PathBuf::from("/docs/report.pdf"),
                exists(&["/docs/report.pdf"])
            ),
            SaveTarget {
                path: PathBuf::from("/docs/report.pdf"),
                ask_to_replace: false
            }
        );
        assert_eq!(
            plan_save_target(
                PathBuf::from("/docs/notes.txt"),
                exists(&["/docs/notes.txt"])
            )
            .path,
            PathBuf::from("/docs/notes.txt")
        );
    }

    #[cfg(not(windows))]
    /// The app of a test: the real guard on a window, with the mock runtime of Tauri.
    fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_builder()
            .plugin(exit_guard())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(UpdateState::new(None, updates::InstallMethod::Automatic));
        app
    }

    #[cfg(not(windows))]
    /// Runs the mock app through its event loop (one turn a second), with `step` called at the
    /// start and after every turn, with the number of the turn. The loop ends when the last window
    /// is closed and nothing holds the exit back.
    fn run_mock(
        app: tauri::App<tauri::test::MockRuntime>,
        mut step: impl FnMut(&tauri::AppHandle<tauri::test::MockRuntime>, usize) + 'static,
    ) {
        let mut turn = 0usize;
        app.run(move |handle, event| match event {
            tauri::RunEvent::Ready => {
                step(handle, 0);
            }
            tauri::RunEvent::MainEventsCleared => {
                turn += 1;
                if turn > 6 {
                    panic!("the app did not end: a close request was held back for good");
                }
                step(handle, turn);
            }
            _ => {}
        });
    }

    #[cfg(not(windows))]
    #[test]
    fn closing_the_window_asks_first_and_keeps_the_window_while_a_document_has_unsaved_changes() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tauri::Listener;
        let app = mock_app();
        let asked = Arc::new(AtomicUsize::new(0));
        let counter = asked.clone();
        app.listen(QUIT_REQUESTED, move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let seen = asked.clone();
        let reached = Arc::new(AtomicUsize::new(0));
        let turns = reached.clone();
        run_mock(app, move |handle, turn| match turn {
            0 => {
                let window =
                    tauri::WebviewWindowBuilder::new(handle, "main", tauri::WebviewUrl::default())
                        .build()
                        .unwrap();
                // A document with changes: the close request is held back, and the window code
                // is asked.
                handle.state::<UpdateState>().unsaved().set(1, true);
                window.close().unwrap();
            }
            1 => {
                turns.fetch_add(1, Ordering::SeqCst);
                assert_eq!(seen.load(Ordering::SeqCst), 1, "the window was not asked");
                assert!(
                    handle.get_webview_window("main").is_some(),
                    "the window was closed over unsaved changes"
                );
                // The user chose to quit without saving: the next close request goes through.
                handle.state::<UpdateState>().unsaved().allow_exit();
                handle.get_webview_window("main").unwrap().close().unwrap();
            }
            _ => {
                assert_eq!(
                    seen.load(Ordering::SeqCst),
                    1,
                    "asked again after the answer"
                );
                assert!(handle.get_webview_window("main").is_none());
            }
        });
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        // The app was still running after the first close request: the window had been kept.
        assert_eq!(
            reached.load(Ordering::SeqCst),
            1,
            "the window was closed at once"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn closing_a_window_without_unsaved_changes_asks_nothing() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tauri::Listener;
        let app = mock_app();
        let asked = Arc::new(AtomicUsize::new(0));
        let counter = asked.clone();
        app.listen(QUIT_REQUESTED, move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        run_mock(app, |handle, turn| {
            if turn == 0 {
                tauri::WebviewWindowBuilder::new(handle, "main", tauri::WebviewUrl::default())
                    .build()
                    .unwrap()
                    .close()
                    .unwrap();
            }
        });
        assert_eq!(asked.load(Ordering::SeqCst), 0);
    }

    // The menus of macOS may be made on the main thread only, and tests run on others, so this
    // runs on the other systems; the function is the same code on all of them.
    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn the_quit_entry_of_the_application_menu_is_replaced_by_one_that_goes_through_the_guard() {
        use tauri::menu::{Menu, MenuItemKind, PredefinedMenuItem, Submenu};
        let app = mock_app();
        let handle = app.handle();
        let quit = PredefinedMenuItem::quit(handle, None).unwrap();
        let about = PredefinedMenuItem::separator(handle).unwrap();
        let application = Submenu::with_items(handle, "App", true, &[&about, &quit]).unwrap();
        let edit = Submenu::with_items(
            handle,
            "Edit",
            true,
            &[&PredefinedMenuItem::copy(handle, None).unwrap()],
        )
        .unwrap();
        let menu = Menu::with_items(handle, &[&application, &edit]).unwrap();

        assert!(use_own_quit(handle, &menu).unwrap());

        let items = application.items().unwrap();
        assert_eq!(items.len(), 2);
        match items.last().unwrap() {
            MenuItemKind::MenuItem(item) => assert_eq!(item.id().as_ref(), QUIT_MENU_ID),
            _ => panic!("the last entry is not an entry of the app"),
        }
        // Other menus are left alone.
        assert_eq!(edit.items().unwrap().len(), 1);
        // A menu without a predefined last entry is not touched.
        assert!(!use_own_quit(handle, &menu).unwrap());
    }

    #[cfg(not(windows))]
    #[test]
    fn the_quit_entry_asks_while_a_document_has_unsaved_changes() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tauri::Listener;
        let app = mock_app();
        let asked = Arc::new(AtomicUsize::new(0));
        let counter = asked.clone();
        app.listen(QUIT_REQUESTED, move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        app.state::<UpdateState>().unsaved().set(3, true);
        quit_from_menu(app.handle());
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        // While the program is being replaced it does nothing at all.
        let blocked = mock_app();
        blocked.state::<UpdateState>().unsaved().set(3, true);
        quit_from_menu(blocked.handle());
    }

    #[test]
    fn pending_paths_wait_for_the_frontend() {
        let pending = Pending::default();
        assert!(pending.deliver(vec!["a.pdf".into()]).is_none());
        assert!(pending.deliver(vec!["b.pdf".into()]).is_none());
        assert_eq!(
            pending.take(),
            vec!["a.pdf".to_string(), "b.pdf".to_string()]
        );
        // Once the frontend is ready, paths are handed over at once and nothing is queued.
        assert_eq!(
            pending.deliver(vec!["c.pdf".into()]),
            Some(vec!["c.pdf".to_string()])
        );
        assert!(pending.take().is_empty());
    }

    /// The real engine program, built by the same `cargo test --workspace` run.
    fn built_engine() -> PathBuf {
        let exe = std::env::current_exe().unwrap();
        let engine = exe
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(format!("partridge-engine{}", std::env::consts::EXE_SUFFIX));
        assert!(
            engine.exists(),
            "{} is missing: run the tests with `cargo test --workspace`",
            engine.display()
        );
        engine
    }

    #[cfg(unix)]
    fn alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// What the app does when the page in the window starts again, with real engines: all of them
    /// end, the addresses are forgotten, and the pool takes new documents. (Only the lookup of the
    /// managed state by `page_started` is left out; the launch of the app in CI covers that.)
    #[test]
    fn a_page_that_starts_again_ends_the_engines_of_the_old_one() {
        use partridge_testkit as kit;
        let mut config = EngineConfig::new(built_engine());
        config.args = vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
        let pool = EnginePool::new(config, 4);
        let file = kit::TempFile::new("reload.pdf", &kit::simple_pdf(1));
        let (one, _) = pool.open(&file.path_str(), None).unwrap();
        let (two, _) = pool.open(&file.path_str(), None).unwrap();
        #[cfg(unix)]
        let pids = [pool.pid(one).unwrap(), pool.pid(two).unwrap()];
        assert_eq!(pool.len(), 2);
        let known = Mutex::new(KnownLinks::default());
        known.lock().unwrap().remember(one, "https://example.com/a");
        let pending = Pending::default();
        assert!(pending.take().is_empty());

        release_documents(&pool, &known, &pending);

        assert!(pool.is_empty());
        #[cfg(unix)]
        assert!(
            pids.iter().all(|pid| !alive(*pid)),
            "an engine is still running"
        );
        assert!(!known.lock().unwrap().allows(one, "https://example.com/a"));
        assert!(pending.deliver(vec!["x.pdf".into()]).is_none());
        // The old handles are dead, and the pool takes new documents.
        assert!(pool.render(one, 0, 20, 20).is_err());
        assert!(pool.open(&file.path_str(), None).is_ok());
    }

    #[test]
    fn only_the_pages_of_the_app_may_be_loaded_in_the_window() {
        let url = |s: &str| url::Url::parse(s).unwrap();
        for ok in [
            "tauri://localhost/",
            "tauri://localhost/index.html",
            "http://tauri.localhost/",
            "https://tauri.localhost/index.html",
            "about:blank",
        ] {
            assert!(is_app_page(&url(ok), false), "{ok}");
        }
        for bad in [
            "https://example.com/",
            "http://tauri.localhost.example.com/",
            "tauri://example/",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,hi",
            "about:srcdoc",
            "http://localhost:1421/",
            "ipc://localhost/",
        ] {
            assert!(!is_app_page(&url(bad), false), "{bad}");
        }
        // The dev server is allowed in a development build, on its own port only.
        assert!(is_app_page(&url("http://localhost:1421/"), true));
        assert!(!is_app_page(&url("http://localhost:8080/"), true));
        assert!(!is_app_page(&url("http://example.com:1421/"), true));
    }

    #[test]
    fn a_link_is_opened_only_when_the_document_holds_it_and_it_is_a_web_address() {
        let known = Mutex::new(KnownLinks::default());
        {
            let mut k = known.lock().unwrap();
            k.remember(1, "https://example.com/a");
            k.remember(1, "file:///etc/passwd");
            k.remember(1, "https://user:pw@example.com/");
        }
        // The cleaned up address is what gets opened.
        assert_eq!(
            authorize_link(&known, 1, "https://example.com/a").unwrap(),
            "https://example.com/a"
        );
        let refused = |doc, url: &str| authorize_link(&known, doc, url).unwrap_err().code;
        // Not an address of that document, whether another document holds it or none does.
        assert_eq!(refused(2, "https://example.com/a"), "link_refused");
        assert_eq!(refused(1, "https://example.com/other"), "link_refused");
        // An address the document holds but that is not a plain web address.
        assert_eq!(refused(1, "file:///etc/passwd"), "link_refused");
        assert_eq!(refused(1, "https://user:pw@example.com/"), "link_refused");
        assert!(authorize_link(&known, 1, "file:///etc/passwd")
            .unwrap_err()
            .message
            .contains("web addresses"));
    }

    #[test]
    fn engine_errors_reach_the_frontend_with_stable_codes() {
        let stopped = CommandError::from(ClientError::EngineStopped("it exited".into()));
        assert_eq!(stopped.code, "engine_stopped");
        let password = CommandError::from(ClientError::Engine {
            code: ErrorCode::PasswordRequired,
            message: "locked".into(),
        });
        assert_eq!(password.code, "password_required");
        assert_eq!(password.message, "locked");
    }
}
