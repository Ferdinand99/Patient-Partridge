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

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use links::KnownLinks;
use partridge_engine_client::{ClientError, EngineConfig, EnginePool, LinkTarget};
use positions::{Position, Positions};
use serde::Serialize;
use tauri::webview::PageLoadEvent;
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

    /// The page is starting again, so nobody is listening: paths wait until it asks for them.
    fn reset(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).ready = false;
    }
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
async fn close_pdf(
    engine: State<'_, Engine>,
    known: State<'_, LinkMemory>,
    doc: u64,
) -> Result<(), CommandError> {
    known
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .forget(doc);
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

    let app = tauri::Builder::default()
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
            open_link,
            print_webview,
            search_page,
            take_pending_paths,
            measure_mode,
            frontend_error,
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
