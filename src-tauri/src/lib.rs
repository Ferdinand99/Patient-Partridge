//! Patient Partridge, the app process.
//!
//! This process shows the window and decides what to open. It never parses a PDF: all of that is
//! done by `partridge-engine`, a separate process that this app starts and supervises. If the
//! engine crashes or hangs on a bad file, the command that was waiting returns an error, the
//! window stays up, and the next command starts a new engine.
//!
//! Every open document has its own engine process (see `EnginePool`), so a file that crashes its
//! engine closes only its own tab.

mod locate;
mod positions;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use partridge_engine_client::{ClientError, EngineConfig, EnginePool, LinkTarget};
use positions::{Position, Positions};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

static START: OnceLock<Instant> = OnceLock::new();

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
}

/// Most documents open at once. Each one has an engine process of its own, about 4 MiB when idle.
const MAX_DOCUMENTS: usize = 16;

/// The supervised engine processes, one per open document.
struct Engine(Arc<EnginePool>);

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
    path: String,
    password: Option<String>,
) -> Result<OpenedInfo, CommandError> {
    let pool = engine.0.clone();
    let name = Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());
    let key = positions::key_for(&path);
    let (doc, pages) = blocking(move || pool.open(&path, password.as_deref())).await?;
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

/// Forgets every remembered position.
#[tauri::command]
fn clear_positions(positions: State<'_, PositionStore>) {
    let _ = positions
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
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
async fn close_pdf(engine: State<'_, Engine>, doc: u64) -> Result<(), CommandError> {
    let pool = engine.0.clone();
    // Closing a document that is gone is fine: the end state is what the caller wants.
    let _ = blocking(move || {
        pool.close(doc);
        Ok(())
    })
    .await;
    Ok(())
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

#[tauri::command]
async fn get_page_links(
    engine: State<'_, Engine>,
    doc: u64,
    page: u32,
) -> Result<Vec<LinkInfo>, CommandError> {
    let pool = engine.0.clone();
    let links = blocking(move || pool.page_links(doc, page)).await?;
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

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            deliver_paths(
                app,
                locate::existing_files(args.into_iter().skip(1), Some(Path::new(&cwd))),
            );
            focus_main(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(Pending::default())
        .setup(|app| {
            let positions_file = app
                .path()
                .app_data_dir()
                .ok()
                .map(|dir| dir.join("positions.json"));
            app.manage(PositionStore(Mutex::new(Positions::load(positions_file))));
            app.manage(Engine(Arc::new(EnginePool::new(
                engine_config(app),
                MAX_DOCUMENTS,
            ))));
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
            set_position,
            clear_positions,
            get_outline,
            get_page_text,
            get_page_links,
            search_page,
            take_pending_paths,
            measure_mode,
            measure_report,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the app");

    app.run(|handle, event| {
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
