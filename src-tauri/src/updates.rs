//! Updates of the app itself.
//!
//! This is the only code in the project that talks to the internet. It runs in the app process:
//! the window loads nothing from the network (its content security policy allows none), and the
//! engine has no network code at all. The window asks this module to check, download and install,
//! and gets short answers and progress back over the same channel as every other command.
//!
//! What is sent: one request for `latest.json` of the project's GitHub releases (when the
//! setting is on, at the start and every 6 hours, and when the user asks), and, when the user
//! agrees, one download of the installer. No document, file name or identifier is part of either.
//! The download is checked against the public key in `tauri.conf.json` before anything is
//! installed.
//!
//! Before an update is installed every engine is stopped and no new one can start, because a
//! program that is running cannot be replaced on Windows. See [`install_with_engines_stopped`].

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use partridge_engine_client::EnginePool;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tauri_plugin_updater::UpdaterExt;

use crate::positions::{create_private_dir, write_private_file};
use crate::{links, locate, now_seconds, CommandError, Engine};

/// Where a person goes to install an update by hand.
pub const RELEASES_PAGE: &str = "https://github.com/Ferdinand99/Patient-Partridge/releases/latest";

/// Longest release note passed to the window, in characters.
const MAX_NOTES_CHARS: usize = 20_000;

/// How long a stopping install waits for documents that were being opened, in seconds.
const OPEN_DRAIN_SECS: u64 = 10;

// ---- how this copy of the app can be updated ------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallMethod {
    /// The app can replace itself: the Windows installers, the macOS app bundle, a Linux AppImage.
    Automatic,
    /// Someone has to install the new version by hand: a Linux package (deb), or anything that
    /// is not clearly one of the three above.
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Mac,
    Linux,
    Other,
}

/// What is known about the running copy. Gathered by [`detect`]; a plain value so that the rule
/// can be tested for every system on any system.
#[derive(Debug, Clone)]
pub struct Facts {
    pub os: Os,
    /// Path of the running program.
    pub exe: Option<PathBuf>,
    /// `$APPIMAGE`: the file the AppImage runtime was started from.
    pub appimage: Option<PathBuf>,
    /// `$APPDIR`: where the runtime put the contents of the AppImage.
    pub appdir: Option<PathBuf>,
    /// Whether `appimage` is a file that exists.
    pub appimage_is_file: bool,
}

/// True for `.../Something.app/Contents/MacOS/program`.
fn inside_app_bundle(exe: &Path) -> bool {
    let name_of = |p: Option<&Path>| p.and_then(Path::file_name).map(|n| n.to_owned());
    let macos = exe.parent();
    let contents = macos.and_then(Path::parent);
    let app = contents.and_then(Path::parent);
    name_of(macos).is_some_and(|n| n == "MacOS")
        && name_of(contents).is_some_and(|n| n == "Contents")
        && app.and_then(Path::extension).is_some_and(|e| e == "app")
}

/// Decides how this copy gets updated. Errs on the side of [`InstallMethod::Manual`]: an update is
/// installed by the app only where the way the app was installed is positively recognised.
///
/// On Linux that means an AppImage, and the proof is three things together: `$APPIMAGE` names an
/// existing file, `$APPDIR` is set, and the running program is inside that `$APPDIR`. The last part
/// matters because the variables are inherited: a deb package started from a terminal that is
/// itself an AppImage sees `$APPIMAGE` of the terminal, but its program is in `/usr/bin`.
pub fn install_method(facts: &Facts) -> InstallMethod {
    let automatic = match facts.os {
        Os::Windows => facts.exe.is_some(),
        Os::Mac => facts.exe.as_deref().is_some_and(inside_app_bundle),
        Os::Linux => match (&facts.exe, &facts.appimage, &facts.appdir) {
            (Some(exe), Some(appimage), Some(appdir)) => {
                appimage.has_root()
                    && facts.appimage_is_file
                    && appdir.has_root()
                    && appdir.parent().is_some()
                    && exe.starts_with(appdir)
            }
            _ => false,
        },
        Os::Other => false,
    };
    if automatic {
        InstallMethod::Automatic
    } else {
        InstallMethod::Manual
    }
}

fn current_os() -> Os {
    if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::Mac
    } else if cfg!(target_os = "linux") {
        Os::Linux
    } else {
        Os::Other
    }
}

/// The method for the running copy. A development build never replaces itself.
pub fn detect() -> InstallMethod {
    if cfg!(debug_assertions) {
        return InstallMethod::Manual;
    }
    let appimage = std::env::var_os("APPIMAGE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let appdir = std::env::var_os("APPDIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    install_method(&Facts {
        os: current_os(),
        exe: std::env::current_exe().ok(),
        appimage_is_file: appimage.as_deref().is_some_and(Path::is_file),
        appimage,
        appdir,
    })
}

// ---- no new engine while an update is installed --------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub struct GateClosed;

#[derive(Default)]
struct GateState {
    closed: bool,
    /// Documents that are being opened right now.
    in_flight: usize,
}

/// Lets documents be opened, until an update is installed. Opening a document starts an engine, and
/// an engine that starts after the others were stopped would block the installer just the same.
/// Every open takes a permit first. Once the gate is closed no permit is given, and
/// [`OpenGate::wait_idle`] waits for the opens that already hold one.
#[derive(Default)]
pub struct OpenGate {
    state: Mutex<GateState>,
    idle: Condvar,
}

pub struct OpenPermit {
    gate: Arc<OpenGate>,
}

impl Drop for OpenPermit {
    fn drop(&mut self) {
        let mut st = self.gate.lock();
        st.in_flight -= 1;
        if st.in_flight == 0 {
            self.gate.idle.notify_all();
        }
    }
}

impl OpenGate {
    fn lock(&self) -> MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Asks to open a document. Fails once the gate is closed.
    pub fn enter(self: &Arc<Self>) -> Result<OpenPermit, GateClosed> {
        let mut st = self.lock();
        if st.closed {
            return Err(GateClosed);
        }
        st.in_flight += 1;
        Ok(OpenPermit { gate: self.clone() })
    }

    pub fn close(&self) {
        self.lock().closed = true;
    }

    pub fn reopen(&self) {
        self.lock().closed = false;
    }

    pub fn is_closed(&self) -> bool {
        self.lock().closed
    }

    /// Waits until no open is in progress. False when that did not happen within `timeout`.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        let mut st = self.lock();
        while st.in_flight > 0 {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return false;
            }
            st = self
                .idle
                .wait_timeout(st, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        true
    }
}

/// Something that runs engines and can stop all of them.
pub trait Engines {
    fn stop_all(&self);
}

impl Engines for EnginePool {
    fn stop_all(&self) {
        self.kill_all();
    }
}

/// Why [`install_with_engines_stopped`] did not return the result of `install`.
#[derive(Debug, PartialEq, Eq)]
pub enum Stopped<X> {
    /// A document was still being opened when the wait for it ended, so an engine may exist that
    /// the pool does not know yet. Nothing was installed.
    OpensStuck,
    /// `install` ran and failed.
    Install(X),
}

/// Runs `install` with no engine alive and no way for one to start.
///
/// 1. The gate is closed, so no further document can be opened.
/// 2. Every engine is stopped.
/// 3. The opens that were already under way are waited for (at most `drain`). When one of them is
///    still running after that, nothing is installed: its engine would keep the installer from
///    replacing the program.
/// 4. Every engine is stopped again: one of those opens may have started an engine after the first
///    stop.
/// 5. `prepare` runs (the app writes down which documents to bring back: the list is taken here,
///    after nothing can be opened or closed by an open any more, and not before the download).
/// 6. `install` runs.
///
/// When the wait or `install` fails the gate is opened again; the documents are gone either way.
pub fn install_with_engines_stopped<E, T, X>(
    gate: &OpenGate,
    engines: &E,
    drain: Duration,
    prepare: impl FnOnce(),
    install: impl FnOnce() -> Result<T, X>,
) -> Result<T, Stopped<X>>
where
    E: Engines + ?Sized,
{
    gate.close();
    engines.stop_all();
    if !gate.wait_idle(drain) {
        gate.reopen();
        return Err(Stopped::OpensStuck);
    }
    engines.stop_all();
    prepare();
    let result = install();
    if result.is_err() {
        gate.reopen();
    }
    result.map_err(Stopped::Install)
}

/// The documents to bring back after an update, in the order they were opened, except that the one
/// in view (`active`) comes first. A path appears once.
pub fn restore_list(docs: &[(u64, String)], active: Option<&str>) -> Vec<String> {
    let mut list: Vec<String> = Vec::new();
    for (_, path) in docs {
        if !list.contains(path) {
            list.push(path.clone());
        }
    }
    if let Some(at) = active.and_then(|a| list.iter().position(|p| p == a)) {
        let first = list.remove(at);
        list.insert(0, first);
    }
    list
}

/// True when an update file may be fetched from this address. Only `https`. A test build (the
/// `update-driver` feature) may also fetch from this computer, for the update test.
pub fn download_allowed(url: &url::Url) -> bool {
    url.scheme() == "https"
        || (cfg!(feature = "update-driver")
            && url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost")))
}

// ---- settings --------------------------------------------------------------------------------

const MAX_SMALL_FILE: u64 = 64 * 1024;

#[derive(Debug, Serialize, Deserialize)]
struct StoredSettings {
    version: u32,
    #[serde(rename = "autoUpdate")]
    auto_update: bool,
}

/// The one setting: whether the app checks for updates by itself. On unless the user turned it
/// off. Kept in a small file in the app's data folder. A missing or damaged file means "on".
pub struct Settings {
    file: Option<PathBuf>,
    auto_update: bool,
}

impl Settings {
    pub fn load(file: Option<PathBuf>) -> Settings {
        let auto_update = file
            .as_deref()
            .and_then(read_small_file)
            .and_then(|bytes| serde_json::from_slice::<StoredSettings>(&bytes).ok())
            .filter(|s| s.version == 1)
            .map(|s| s.auto_update)
            .unwrap_or(true);
        Settings { file, auto_update }
    }

    pub fn auto_update(&self) -> bool {
        self.auto_update
    }

    /// Changes the setting. The change holds for this run even when the file cannot be written.
    pub fn set_auto_update(&mut self, on: bool) -> io::Result<()> {
        self.auto_update = on;
        let Some(file) = &self.file else {
            return Ok(());
        };
        write_json(
            file,
            &StoredSettings {
                version: 1,
                auto_update: on,
            },
        )
    }
}

fn read_small_file(file: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(file).ok()?;
    if meta.len() > MAX_SMALL_FILE {
        return None;
    }
    std::fs::read(file).ok()
}

fn write_json<T: Serialize>(file: &Path, value: &T) -> io::Result<()> {
    if let Some(dir) = file.parent() {
        create_private_dir(dir)?;
    }
    let json = serde_json::to_vec(value).map_err(io::Error::other)?;
    let temp = file.with_extension("json.tmp");
    write_private_file(&temp, &json)?;
    std::fs::rename(&temp, file)
}

// ---- bringing the open documents back after the restart -------------------------------------

/// How old the list of documents may be and still be used, in seconds.
const RESTORE_MAX_AGE_SECS: u64 = 3600;
/// Most documents brought back (the same limit as open documents).
const RESTORE_MAX_PATHS: usize = crate::MAX_DOCUMENTS;
const RESTORE_MAX_PATH_CHARS: usize = 4096;

#[derive(Debug, Serialize, Deserialize)]
struct Restore {
    version: u32,
    saved: u64,
    paths: Vec<String>,
}

/// Writes down which documents are open, just before an update is installed. The file is for the
/// next start only: [`take_restore`] reads it once and deletes it.
pub fn save_restore(file: &Path, paths: &[String], now: u64) -> io::Result<()> {
    let paths = paths
        .iter()
        .filter(|p| !p.is_empty() && p.chars().count() <= RESTORE_MAX_PATH_CHARS)
        .take(RESTORE_MAX_PATHS)
        .cloned()
        .collect();
    write_json(
        file,
        &Restore {
            version: 1,
            saved: now,
            paths,
        },
    )
}

/// Reads and deletes the list written by [`save_restore`]. Gives the files that are still there,
/// and nothing when the list is damaged or older than an hour (an old list is not what the
/// reader expects to see).
pub fn take_restore(file: &Path, now: u64) -> Vec<String> {
    let bytes = read_small_file(file);
    let _ = std::fs::remove_file(file);
    let Some(restore) = bytes.and_then(|b| serde_json::from_slice::<Restore>(&b).ok()) else {
        return Vec::new();
    };
    if restore.version != 1 || restore.saved > now || now - restore.saved > RESTORE_MAX_AGE_SECS {
        return Vec::new();
    }
    let paths = restore
        .paths
        .into_iter()
        .filter(|p| p.chars().count() <= RESTORE_MAX_PATH_CHARS)
        .take(RESTORE_MAX_PATHS);
    locate::existing_files(paths, None)
}

// ---- what the window sees -------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    /// The update server could not be reached, or answered with an error.
    Offline,
    /// The download does not match the public key of the app, or the version it was signed for.
    InvalidSignature,
    /// The updater is not set up (no endpoint).
    NotConfigured,
    /// `latest.json` is missing, unreadable, or has nothing for this system.
    BadManifest,
    /// An automatic check, while the setting is off.
    Disabled,
    /// An update is being installed already.
    Busy,
    /// This copy cannot install by itself (a deb package).
    Manual,
    Other,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFailure {
    kind: FailureKind,
    message: String,
    /// True when the engines were stopped for the install and it then failed. The open documents
    /// are not usable any more; a restart brings them back.
    engines_stopped: bool,
}

fn failure(kind: FailureKind, message: impl Into<String>) -> UpdateFailure {
    UpdateFailure {
        kind,
        message: message.into(),
        engines_stopped: false,
    }
}

/// Sorts an error of the updater into a kind the window has a message for.
pub fn kind_of(error: &tauri_plugin_updater::Error) -> FailureKind {
    use tauri_plugin_updater::Error as E;
    match error {
        E::Minisign(_)
        | E::Base64(_)
        | E::SignatureUtf8(_)
        | E::SignedVersionMismatch { .. }
        | E::MissingSignedVersion => FailureKind::InvalidSignature,
        E::Reqwest(_) | E::Network(_) => FailureKind::Offline,
        E::ReleaseNotFound
        | E::Serialization(_)
        | E::Semver(_)
        | E::TargetNotFound(_)
        | E::TargetsNotFound(_) => FailureKind::BadManifest,
        E::EmptyEndpoints | E::UrlParse(_) | E::InsecureTransportProtocol => {
            FailureKind::NotConfigured
        }
        _ => FailureKind::Other,
    }
}

fn from_updater(error: tauri_plugin_updater::Error) -> UpdateFailure {
    failure(kind_of(&error), error.to_string())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    version: String,
    current_version: String,
    notes: String,
    method: InstallMethod,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum UpdateEvent {
    /// Bytes of the download that arrived (`total` when the server said how many there are).
    Progress { chunk: u64, total: Option<u64> },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSettings {
    auto_update: bool,
    method: InstallMethod,
    current_version: String,
}

fn cut_notes(notes: Option<String>) -> String {
    let notes = notes.unwrap_or_default();
    match notes.char_indices().nth(MAX_NOTES_CHARS) {
        Some((end, _)) => notes[..end].to_string(),
        None => notes,
    }
}

// ---- the state and the commands ---------------------------------------------------------------

pub struct UpdateState {
    gate: Arc<OpenGate>,
    settings: Mutex<Settings>,
    /// The update the last check found, until it is installed.
    pending: Mutex<Option<tauri_plugin_updater::Update>>,
    /// The update that was downloaded and checked, until it is installed.
    downloaded: Mutex<Option<(tauri_plugin_updater::Update, Vec<u8>)>>,
    /// A download or an install is under way (or done, and the app is to restart).
    installing: AtomicBool,
    /// True while the program is being replaced: closing the window then would leave a half file.
    close_blocked: Arc<AtomicBool>,
    /// The documents that are open, by handle: kept here so that the list for the restart is taken
    /// at the moment of the install, from the app's own knowledge, not from the window's.
    open_docs: Arc<Mutex<Vec<(u64, String)>>>,
    /// The documents of the last run, to be handed to the window once.
    restored: Mutex<Vec<String>>,
    restore_file: Option<PathBuf>,
    method: InstallMethod,
}

impl UpdateState {
    pub fn new(data_dir: Option<PathBuf>, method: InstallMethod) -> UpdateState {
        UpdateState {
            gate: Arc::new(OpenGate::default()),
            settings: Mutex::new(Settings::load(
                data_dir.as_ref().map(|d| d.join("settings.json")),
            )),
            pending: Mutex::new(None),
            downloaded: Mutex::new(None),
            installing: AtomicBool::new(false),
            close_blocked: Arc::new(AtomicBool::new(false)),
            open_docs: Arc::new(Mutex::new(Vec::new())),
            restored: Mutex::new(Vec::new()),
            restore_file: data_dir.map(|d| d.join("restore-after-update.json")),
            method,
        }
    }

    pub fn gate(&self) -> &Arc<OpenGate> {
        &self.gate
    }

    /// Reads the documents that were open before an update (the file is deleted by this) and keeps
    /// them for [`UpdateState::take_restored`]. Returns how many there are. Empty if there was no
    /// update.
    pub fn load_restored(&self) -> usize {
        let list = self
            .restore_file
            .as_deref()
            .map(|f| take_restore(f, now_seconds()))
            .unwrap_or_default();
        let n = list.len();
        *self.restored.lock().unwrap_or_else(|e| e.into_inner()) = list;
        n
    }

    /// The documents to bring back, once: the window asks when it has started.
    pub fn take_restored(&self) -> Vec<String> {
        std::mem::take(&mut *self.restored.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// A document was opened.
    pub fn record_open(&self, doc: u64, path: &str) {
        self.open_docs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((doc, path.to_string()));
    }

    /// A document was closed.
    pub fn forget_doc(&self, doc: u64) {
        self.open_docs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(d, _)| *d != doc);
    }

    /// The window that knew the documents is gone (a reload).
    pub fn forget_all_docs(&self) {
        self.open_docs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// True while an update is being installed and the window must stay open.
    pub fn is_close_blocked(&self) -> bool {
        self.close_blocked.load(Ordering::SeqCst)
    }

    fn auto_update(&self) -> bool {
        self.settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .auto_update()
    }
}

/// The error a document gets when it is opened while an update is being installed.
pub fn updating_error() -> CommandError {
    CommandError {
        code: "updating".into(),
        message: "an update is being installed; restart the app to continue".into(),
    }
}

pub async fn check(
    app: &AppHandle,
    state: &UpdateState,
    manual: bool,
) -> Result<Option<UpdateInfo>, UpdateFailure> {
    if !manual && !state.auto_update() {
        return Err(failure(FailureKind::Disabled, "automatic checks are off"));
    }
    if state.installing.load(Ordering::SeqCst) || state.gate.is_closed() {
        return Err(failure(FailureKind::Busy, "an update is being installed"));
    }
    let found = app
        .updater()
        .map_err(from_updater)?
        .check()
        .await
        .map_err(from_updater)?;
    let Some(update) = found else {
        *state.pending.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return Ok(None);
    };
    let info = UpdateInfo {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        notes: cut_notes(update.body.clone()),
        method: state.method,
    };
    *state.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(update);
    Ok(Some(info))
}

/// Downloads the update found by the last check and keeps it, checked against the public key, for
/// [`install`]. Nothing about the running app changes: the engines and the open files are not
/// touched, so a failure here costs nothing.
pub async fn download(
    state: &UpdateState,
    report: impl Fn(UpdateEvent) + Send + 'static,
) -> Result<(), UpdateFailure> {
    if state.method != InstallMethod::Automatic {
        return Err(failure(
            FailureKind::Manual,
            "this copy of the app cannot install an update by itself",
        ));
    }
    let update = state
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or_else(|| failure(FailureKind::Other, "no update has been checked for"))?;
    if state.installing.swap(true, Ordering::SeqCst) {
        return Err(failure(FailureKind::Busy, "an update is being installed"));
    }
    let result = fetch(&update, &report).await;
    match result {
        Ok(bytes) => {
            *state.downloaded.lock().unwrap_or_else(|e| e.into_inner()) = Some((update, bytes));
            Ok(())
        }
        Err(e) => {
            state.installing.store(false, Ordering::SeqCst);
            Err(e)
        }
    }
}

async fn fetch(
    update: &tauri_plugin_updater::Update,
    report: &(impl Fn(UpdateEvent) + Send + 'static),
) -> Result<Vec<u8>, UpdateFailure> {
    // The manifest is not signed, so the address of the file in it is not trusted either: only
    // https. (The signature protects the bytes; this keeps the download itself private.)
    if !download_allowed(&update.download_url) {
        return Err(failure(
            FailureKind::BadManifest,
            "the update is not offered over https, so it is not downloaded",
        ));
    }
    // The download is checked against the public key here.
    update
        .download(
            |chunk, total| {
                report(UpdateEvent::Progress {
                    chunk: chunk as u64,
                    total,
                });
            },
            || {},
        )
        .await
        .map_err(from_updater)
}

/// Stops the engines and installs the update that [`download`] kept. The documents to bring back
/// are taken here, from what the app has open at this moment (a file opened while the download ran
/// is in the list, one closed is not); `active` is the one in view, which comes back first. Where
/// the installer replaces the running app (Windows) this does not return: the app ends.
pub async fn install(
    state: &UpdateState,
    pool: Arc<EnginePool>,
    active: Option<String>,
) -> Result<(), UpdateFailure> {
    let Some((update, bytes)) = state
        .downloaded
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    else {
        return Err(failure(FailureKind::Other, "no update has been downloaded"));
    };
    let gate = state.gate.clone();
    let restore_file = state.restore_file.clone();
    let docs = state.open_docs.clone();
    let close_blocked = state.close_blocked.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        #[cfg(feature = "update-driver")]
        let open_before = pool.len();
        close_blocked.store(true, Ordering::SeqCst);
        let result = install_with_engines_stopped(
            &gate,
            &*pool,
            Duration::from_secs(OPEN_DRAIN_SECS),
            || {
                if let Some(file) = &restore_file {
                    let paths = restore_list(
                        &docs.lock().unwrap_or_else(|e| e.into_inner()),
                        active.as_deref(),
                    );
                    let _ = save_restore(file, &paths, now_seconds());
                }
            },
            || {
                #[cfg(feature = "update-driver")]
                driver::note(&format!(
                    "documents open before the stop={open_before}, when the install starts={}",
                    pool.len()
                ));
                update.install(&bytes)
            },
        );
        close_blocked.store(false, Ordering::SeqCst);
        result
    })
    .await;
    let result = match outcome {
        Ok(Ok(())) => return Ok(()),
        Ok(Err(Stopped::Install(e))) => from_updater(e),
        Ok(Err(Stopped::OpensStuck)) => failure(
            FailureKind::Other,
            "a file was still being opened, so nothing was installed",
        ),
        Err(e) => failure(FailureKind::Other, e.to_string()),
    };
    // The engines were stopped, and the program was not replaced: the app can be used again after
    // a restart (which brings the documents back).
    state.installing.store(false, Ordering::SeqCst);
    Err(UpdateFailure {
        engines_stopped: true,
        ..result
    })
}

fn command_error(error: io::Error) -> CommandError {
    CommandError {
        code: "internal".into(),
        message: error.to_string(),
    }
}

#[tauri::command]
pub fn get_update_settings(app: AppHandle, state: State<'_, UpdateState>) -> UpdateSettings {
    UpdateSettings {
        auto_update: state.auto_update(),
        method: state.method,
        current_version: app.package_info().version.to_string(),
    }
}

#[tauri::command]
pub fn set_auto_update(state: State<'_, UpdateState>, enabled: bool) -> Result<(), CommandError> {
    state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_auto_update(enabled)
        .map_err(command_error)
}

/// Asks GitHub whether a newer version exists. `Ok(None)` means there is none. An automatic check
/// (`manual` false) is refused while the setting is off.
#[tauri::command]
pub async fn check_update(
    app: AppHandle,
    state: State<'_, UpdateState>,
    manual: bool,
) -> Result<Option<UpdateInfo>, UpdateFailure> {
    check(&app, &state, manual).await
}

/// The documents that were open before the last update, once: the window opens them again. They
/// do not go through the rules for files named at the start (no limit of eight, and a file that
/// does not end in .pdf is kept), because they are what the reader had open.
#[tauri::command]
pub fn take_restored_paths(state: State<'_, UpdateState>) -> Vec<String> {
    state.take_restored()
}

/// Downloads the update that the last check found, with progress, and checks it against the public
/// key. Nothing about the running app changes.
#[tauri::command]
pub async fn download_update(
    state: State<'_, UpdateState>,
    on_event: Channel<UpdateEvent>,
) -> Result<(), UpdateFailure> {
    download(&state, move |event| {
        let _ = on_event.send(event);
    })
    .await
}

/// Installs the update that was downloaded: stops the engines, writes down the documents that are
/// open at this moment, and replaces the program. `active` is the document in view; it comes back
/// first.
#[tauri::command]
pub async fn install_update(
    state: State<'_, UpdateState>,
    engine: State<'_, Engine>,
    active: Option<String>,
) -> Result<(), UpdateFailure> {
    let pool = engine.0.clone();
    install(&state, pool, active).await
}

/// Starts the app again. The engines are already stopped when this follows an install.
#[tauri::command]
pub fn restart_app(app: AppHandle, engine: State<'_, Engine>) {
    engine.0.kill_all();
    app.restart();
}

/// Opens the page with the releases of the app in the user's browser. The address is fixed here;
/// the window cannot choose another one.
#[tauri::command]
pub fn open_releases_page() -> Result<(), CommandError> {
    links::open_in_browser(RELEASES_PAGE).map_err(|_| CommandError {
        code: "link_refused".into(),
        message: "the web browser could not be started".into(),
    })
}

/// Test builds only: drives a whole update without a person, so a test can run the real install.
#[cfg(feature = "update-driver")]
pub mod driver {
    use super::*;
    use std::io::Write;
    use tauri::Manager;

    static LOG: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

    /// Writes a line to the log of the driver, if there is one (called from the install).
    pub fn note(line: &str) {
        if let Some(file) = LOG.get() {
            log(file, line);
        }
    }

    fn log(file: &Path, line: &str) {
        eprintln!("update-driver: {line}");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
        {
            let _ = writeln!(f, "{line}");
        }
    }

    pub fn start(app: AppHandle, restored: usize) {
        // The Windows installer starts the new version without the environment of the old one, so
        // the log can also be named by a file in the app's data folder.
        let named_by_file = || {
            let dir = app.path().app_data_dir().ok()?;
            let text = std::fs::read_to_string(dir.join("update-driver.path")).ok()?;
            Some(PathBuf::from(text.trim()))
        };
        let Some(file) = std::env::var_os("PARTRIDGE_UPDATE_DRIVER")
            .map(PathBuf::from)
            .or_else(named_by_file)
        else {
            return;
        };
        let _ = LOG.set(file.clone());
        log(
            &file,
            &format!(
                "started version={} method={:?} restored={}",
                app.package_info().version,
                detect(),
                restored
            ),
        );
        tauri::async_runtime::spawn(async move {
            // Time for the window to come up and open the documents it was started with.
            wait_secs(8).await;
            let state = app.state::<UpdateState>();
            let engine = app.state::<Engine>();
            match check(&app, &state, true).await {
                Ok(None) => {
                    log(
                        &file,
                        &format!("up-to-date version={}", app.package_info().version),
                    );
                    app.exit(0);
                }
                Ok(Some(info)) => {
                    log(
                        &file,
                        &format!(
                            "found version={} from={}",
                            info.version, info.current_version
                        ),
                    );
                    if info.method == InstallMethod::Manual {
                        log(
                            &file,
                            &format!("manual-notice version={} (not installing)", info.version),
                        );
                        app.exit(0);
                        return;
                    }
                    log(&file, &format!("documents open={}", engine.0.len()));
                    let result = match download(&state, |_| {}).await {
                        Ok(()) => {
                            log(&file, "installing: engines are stopped next");
                            install(&state, engine.0.clone(), None).await
                        }
                        Err(e) => Err(e),
                    };
                    match result {
                        Ok(()) => {
                            log(&file, "installed; restarting");
                            app.restart();
                        }
                        Err(e) => {
                            log(
                                &file,
                                &format!("install-failed kind={:?} {}", e.kind, e.message),
                            );
                            app.exit(3);
                        }
                    }
                }
                Err(e) => {
                    log(
                        &file,
                        &format!("check-failed kind={:?} {}", e.kind, e.message),
                    );
                    app.exit(2);
                }
            }
        });
    }

    async fn wait_secs(secs: u64) {
        let _ = tauri::async_runtime::spawn_blocking(move || {
            std::thread::sleep(Duration::from_secs(secs));
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use partridge_engine_client::EngineConfig;
    use std::cell::RefCell;
    use std::sync::atomic::AtomicUsize;

    fn facts(os: Os, exe: &str) -> Facts {
        Facts {
            os,
            exe: Some(PathBuf::from(exe)),
            appimage: None,
            appdir: None,
            appimage_is_file: false,
        }
    }

    fn appimage_facts(exe: &str, appimage: &str, appdir: &str, is_file: bool) -> Facts {
        Facts {
            os: Os::Linux,
            exe: Some(PathBuf::from(exe)),
            appimage: Some(PathBuf::from(appimage)),
            appdir: Some(PathBuf::from(appdir)),
            appimage_is_file: is_file,
        }
    }

    #[test]
    fn windows_and_a_macos_app_bundle_update_themselves() {
        let m = |f| install_method(&f);
        assert_eq!(
            m(facts(
                Os::Windows,
                "C:/Program Files/Patient Partridge/patient-partridge.exe"
            )),
            InstallMethod::Automatic
        );
        assert_eq!(
            m(facts(
                Os::Mac,
                "/Applications/Patient Partridge.app/Contents/MacOS/patient-partridge"
            )),
            InstallMethod::Automatic
        );
        // A program that is not inside an app bundle is not something the bundle update can replace.
        assert_eq!(
            m(facts(
                Os::Mac,
                "/Users/me/code/target/release/patient-partridge"
            )),
            InstallMethod::Manual
        );
        assert_eq!(
            m(facts(
                Os::Mac,
                "/Applications/Thing.app/Contents/Resources/patient-partridge"
            )),
            InstallMethod::Manual
        );
        assert_eq!(
            m(Facts {
                exe: None,
                ..facts(Os::Windows, "")
            }),
            InstallMethod::Manual
        );
        assert_eq!(m(facts(Os::Other, "/x/y")), InstallMethod::Manual);
    }

    #[test]
    fn on_linux_only_a_real_appimage_updates_itself() {
        let m = |f| install_method(&f);
        let run = "/tmp/.mount_PatienXyz";
        let exe = "/tmp/.mount_PatienXyz/usr/bin/patient-partridge";
        assert_eq!(
            m(appimage_facts(
                exe,
                "/home/me/Apps/Partridge.AppImage",
                run,
                true
            )),
            InstallMethod::Automatic
        );
        // A deb package: /usr/bin, and no AppImage variables.
        assert_eq!(
            m(facts(Os::Linux, "/usr/bin/patient-partridge")),
            InstallMethod::Manual
        );
        // A deb package started from a terminal that is an AppImage: the variables are inherited,
        // but the program is not inside that AppImage.
        assert_eq!(
            m(appimage_facts(
                "/usr/bin/patient-partridge",
                "/home/me/Apps/Terminal.AppImage",
                "/tmp/.mount_TerminXyz",
                true
            )),
            InstallMethod::Manual
        );
        // The file named by $APPIMAGE is gone, or the paths are not absolute, or $APPDIR is the root.
        assert_eq!(
            m(appimage_facts(
                exe,
                "/home/me/Apps/Partridge.AppImage",
                run,
                false
            )),
            InstallMethod::Manual
        );
        assert_eq!(
            m(appimage_facts(exe, "Partridge.AppImage", run, true)),
            InstallMethod::Manual
        );
        assert_eq!(
            m(appimage_facts(
                exe,
                "/home/me/Apps/Partridge.AppImage",
                "tmp/.mount_x",
                true
            )),
            InstallMethod::Manual
        );
        assert_eq!(
            m(appimage_facts(
                "/usr/bin/patient-partridge",
                "/home/me/a.AppImage",
                "/",
                true
            )),
            InstallMethod::Manual
        );
        // A path that only starts with the same letters is not inside the folder.
        assert_eq!(
            m(appimage_facts(
                "/tmp/.mount_PatienXyz2/usr/bin/patient-partridge",
                "/home/me/Apps/Partridge.AppImage",
                run,
                true
            )),
            InstallMethod::Manual
        );
        // Only one of the two variables.
        let mut only_image = appimage_facts(exe, "/home/me/a.AppImage", run, true);
        only_image.appdir = None;
        assert_eq!(m(only_image), InstallMethod::Manual);
    }

    #[test]
    fn the_gate_gives_permits_until_it_is_closed() {
        let gate = Arc::new(OpenGate::default());
        let permit = gate.enter().unwrap();
        gate.close();
        assert!(gate.enter().is_err());
        assert!(
            !gate.wait_idle(Duration::from_millis(30)),
            "a permit is out"
        );
        drop(permit);
        assert!(gate.wait_idle(Duration::from_millis(30)));
        gate.reopen();
        assert!(gate.enter().is_ok());
    }

    /// Engines that are only counted. An "open" that was under way when the install began starts
    /// its engine late, after the first stop.
    struct Counting {
        live: AtomicUsize,
        stops: AtomicUsize,
    }

    impl Engines for Counting {
        fn stop_all(&self) {
            self.stops.fetch_add(1, Ordering::SeqCst);
            self.live.store(0, Ordering::SeqCst);
        }
    }

    #[test]
    fn engines_are_stopped_before_the_install_even_when_an_open_is_late() {
        let gate = Arc::new(OpenGate::default());
        let engines = Arc::new(Counting {
            live: AtomicUsize::new(1),
            stops: AtomicUsize::new(0),
        });
        // An open that holds its permit and registers its engine only after the first stop.
        let permit = gate.enter().unwrap();
        let late = {
            let engines = engines.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(150));
                engines.live.fetch_add(1, Ordering::SeqCst);
                drop(permit);
            })
        };
        let seen = RefCell::new(Vec::new());
        let result: Result<(), Stopped<()>> = install_with_engines_stopped(
            &gate,
            &*engines,
            Duration::from_secs(5),
            || seen.borrow_mut().push("prepare"),
            || {
                seen.borrow_mut().push("install");
                assert_eq!(engines.live.load(Ordering::SeqCst), 0, "an engine is alive");
                assert!(gate.enter().is_err(), "a document can still be opened");
                Ok(())
            },
        );
        late.join().unwrap();
        assert!(result.is_ok());
        assert_eq!(*seen.borrow(), vec!["prepare", "install"]);
        assert_eq!(engines.stops.load(Ordering::SeqCst), 2);
        // After a good install the gate stays closed: the app is about to restart.
        assert!(gate.is_closed());
    }

    #[test]
    fn a_failed_install_opens_the_gate_again() {
        let gate = OpenGate::default();
        let engines = Counting {
            live: AtomicUsize::new(0),
            stops: AtomicUsize::new(0),
        };
        let result: Result<(), Stopped<&str>> = install_with_engines_stopped(
            &gate,
            &engines,
            Duration::from_secs(5),
            || {},
            || Err("the installer failed"),
        );
        assert_eq!(result, Err(Stopped::Install("the installer failed")));
        assert!(!gate.is_closed());
    }

    #[test]
    fn an_open_that_does_not_finish_in_time_stops_the_install() {
        let gate = Arc::new(OpenGate::default());
        let engines = Counting {
            live: AtomicUsize::new(0),
            stops: AtomicUsize::new(0),
        };
        let permit = gate.enter().unwrap();
        let installed = RefCell::new(false);
        let result: Result<(), Stopped<()>> = install_with_engines_stopped(
            &gate,
            &engines,
            Duration::from_millis(50),
            || {},
            || {
                *installed.borrow_mut() = true;
                Ok(())
            },
        );
        assert_eq!(result, Err(Stopped::OpensStuck));
        assert!(
            !*installed.borrow(),
            "nothing may be installed with an open still running"
        );
        assert!(!gate.is_closed(), "the app is usable again");
        drop(permit);
    }

    #[test]
    fn the_list_of_documents_is_taken_after_the_stop_and_has_the_one_in_view_first() {
        let docs = vec![
            (1, "/a.pdf".to_string()),
            (2, "/b".to_string()),
            (3, "/c.pdf".to_string()),
            (4, "/a.pdf".to_string()),
        ];
        assert_eq!(restore_list(&docs, None), vec!["/a.pdf", "/b", "/c.pdf"]);
        assert_eq!(
            restore_list(&docs, Some("/c.pdf")),
            vec!["/c.pdf", "/a.pdf", "/b"]
        );
        // A view that is not in the list changes nothing.
        assert_eq!(
            restore_list(&docs, Some("/nope")),
            vec!["/a.pdf", "/b", "/c.pdf"]
        );
        assert!(restore_list(&[], Some("/x")).is_empty());

        // The prepare step sees a document that was opened while the download was running.
        let gate = OpenGate::default();
        let live = Arc::new(Mutex::new(vec![(1u64, "/first.pdf".to_string())]));
        let engines = Counting {
            live: AtomicUsize::new(0),
            stops: AtomicUsize::new(0),
        };
        live.lock()
            .unwrap()
            .push((2, "/opened-during-the-download.pdf".to_string()));
        let written = RefCell::new(Vec::new());
        let _: Result<(), Stopped<()>> = install_with_engines_stopped(
            &gate,
            &engines,
            Duration::from_secs(1),
            || *written.borrow_mut() = restore_list(&live.lock().unwrap(), None),
            || Ok(()),
        );
        assert_eq!(
            *written.borrow(),
            vec!["/first.pdf", "/opened-during-the-download.pdf"]
        );
    }

    #[test]
    fn documents_are_recorded_when_opened_and_forgotten_when_closed_or_when_the_page_restarts() {
        let state = UpdateState::new(None, InstallMethod::Manual);
        state.record_open(1, "/a.pdf");
        state.record_open(2, "/b.pdf");
        state.forget_doc(1);
        let docs = state.open_docs.lock().unwrap().clone();
        assert_eq!(restore_list(&docs, None), vec!["/b.pdf"]);
        state.forget_all_docs();
        assert!(state.open_docs.lock().unwrap().is_empty());
    }

    #[test]
    fn an_update_is_downloaded_only_over_https() {
        let url = |s: &str| url::Url::parse(s).unwrap();
        assert!(download_allowed(&url(
            "https://api.github.com/repos/o/r/releases/assets/1"
        )));
        assert!(!download_allowed(&url(
            "http://api.github.com/repos/o/r/releases/assets/1"
        )));
        assert!(!download_allowed(&url("ftp://example.com/file")));
        // This computer, over http, is for the update test only: it is allowed in a build with the
        // update-driver feature and refused in every other.
        assert_eq!(
            download_allowed(&url("http://127.0.0.1:18099/x")),
            cfg!(feature = "update-driver")
        );
        assert!(!download_allowed(&url("http://example.com/x")));
    }

    #[test]
    fn the_window_is_kept_open_only_while_the_program_is_replaced() {
        let state = UpdateState::new(None, InstallMethod::Manual);
        assert!(!state.is_close_blocked());
        state.close_blocked.store(true, Ordering::SeqCst);
        assert!(state.is_close_blocked());
    }

    #[test]
    fn the_steps_of_an_install_come_in_this_order() {
        // Stop, wait, stop again, write down the documents, install: kept in one place.
        let gate = OpenGate::default();
        let order = RefCell::new(Vec::new());
        struct Log<'a>(&'a RefCell<Vec<&'static str>>);
        impl Engines for Log<'_> {
            fn stop_all(&self) {
                self.0.borrow_mut().push("stop");
            }
        }
        let _: Result<(), Stopped<()>> = install_with_engines_stopped(
            &gate,
            &Log(&order),
            Duration::from_secs(1),
            || order.borrow_mut().push("prepare"),
            || {
                order.borrow_mut().push("install");
                Ok(())
            },
        );
        assert_eq!(*order.borrow(), vec!["stop", "stop", "prepare", "install"]);
    }

    fn fake_engine() -> PathBuf {
        let exe = std::env::current_exe().unwrap();
        let dir = exe.parent().unwrap().parent().unwrap().join("examples");
        dir.join(format!("fake_engine{}", std::env::consts::EXE_SUFFIX))
    }

    #[cfg(unix)]
    fn alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    #[test]
    fn real_engine_processes_are_gone_when_the_install_runs() {
        let fake = fake_engine();
        assert!(
            fake.is_file(),
            "build the examples first: {}",
            fake.display()
        );
        let mut config = EngineConfig::new(fake);
        config.args = vec!["good".into()];
        let pool = EnginePool::new(config, 4);
        // The app opens the file itself and hands it to the engine, so the file has to exist.
        let dir =
            std::env::temp_dir().join(format!("partridge-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let handles: Vec<u64> = (0..3)
            .map(|i| {
                let doc = dir.join(format!("doc{i}.pdf"));
                std::fs::write(&doc, partridge_testkit::simple_pdf(1)).unwrap();
                pool.open(&doc.to_string_lossy(), None).unwrap().0
            })
            .collect();
        let _ = std::fs::remove_dir_all(&dir);
        let pids: Vec<u32> = handles.iter().filter_map(|h| pool.pid(*h)).collect();
        assert_eq!(pids.len(), 3, "three engines should be running");
        #[cfg(unix)]
        assert!(pids.iter().all(|p| alive(*p)));

        let gate = OpenGate::default();
        let result: Result<(), Stopped<()>> = install_with_engines_stopped(
            &gate,
            &pool,
            Duration::from_secs(5),
            || {},
            || {
                assert!(pool.is_empty(), "documents are still open");
                #[cfg(unix)]
                for pid in &pids {
                    assert!(!alive(*pid), "engine {pid} is still running");
                }
                Ok(())
            },
        );
        assert!(result.is_ok());
        assert!(gate.is_closed());
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("partridge-updates-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn automatic_checks_are_on_until_the_user_turns_them_off() {
        let dir = temp_dir("settings");
        let file = dir.join("settings.json");
        assert!(
            Settings::load(Some(file.clone())).auto_update(),
            "default is on"
        );
        assert!(Settings::load(None).auto_update());

        let mut s = Settings::load(Some(file.clone()));
        s.set_auto_update(false).unwrap();
        assert!(!s.auto_update());
        assert!(
            !Settings::load(Some(file.clone())).auto_update(),
            "kept on disk"
        );
        s.set_auto_update(true).unwrap();
        assert!(Settings::load(Some(file.clone())).auto_update());

        // A damaged or foreign file means the default, not a crash.
        std::fs::write(&file, b"{ not json").unwrap();
        assert!(Settings::load(Some(file.clone())).auto_update());
        std::fs::write(&file, br#"{"version":9,"autoUpdate":false}"#).unwrap();
        assert!(Settings::load(Some(file)).auto_update());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn documents_come_back_once_and_only_when_the_list_is_fresh() {
        let dir = temp_dir("restore");
        let file = dir.join("restore.json");
        let a = dir.join("a.pdf");
        let b = dir.join("b.pdf");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"x").unwrap();
        let gone = dir.join("gone.pdf");
        let list: Vec<String> = [&a, &gone, &b]
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();

        save_restore(&file, &list, 1000).unwrap();
        let back = take_restore(&file, 1100);
        assert_eq!(
            back,
            vec![
                a.to_string_lossy().into_owned(),
                b.to_string_lossy().into_owned()
            ],
            "order is kept, a file that is gone is left out"
        );
        assert!(!file.exists(), "the list is used once");
        assert!(take_restore(&file, 1100).is_empty());

        // Too old, from the future, or damaged: nothing, and the file is removed all the same.
        save_restore(&file, &list, 1000).unwrap();
        assert!(take_restore(&file, 1000 + RESTORE_MAX_AGE_SECS + 1).is_empty());
        assert!(!file.exists());
        save_restore(&file, &list, 5000).unwrap();
        assert!(take_restore(&file, 4000).is_empty());
        std::fs::write(&file, b"garbage").unwrap();
        assert!(take_restore(&file, 1).is_empty());
        assert!(!file.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_list_of_documents_is_limited() {
        let dir = temp_dir("restore-limit");
        let file = dir.join("restore.json");
        let many: Vec<String> = (0..40)
            .map(|i| format!("{}/f{i}.pdf", dir.display()))
            .collect();
        for p in &many {
            std::fs::write(p, b"x").unwrap();
        }
        let mut with_junk = many.clone();
        with_junk.insert(0, String::new());
        with_junk.insert(1, "x".repeat(RESTORE_MAX_PATH_CHARS + 1));
        save_restore(&file, &with_junk, 10).unwrap();
        let back = take_restore(&file, 10);
        assert_eq!(back.len(), RESTORE_MAX_PATHS);
        assert_eq!(back[0], many[0]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn errors_of_the_updater_get_a_kind_the_window_can_explain() {
        use tauri_plugin_updater::Error as E;
        assert_eq!(kind_of(&E::Network("boom".into())), FailureKind::Offline);
        assert_eq!(kind_of(&E::ReleaseNotFound), FailureKind::BadManifest);
        assert_eq!(
            kind_of(&E::TargetsNotFound(vec!["linux-x86_64".into()])),
            FailureKind::BadManifest
        );
        assert_eq!(kind_of(&E::EmptyEndpoints), FailureKind::NotConfigured);
        assert_eq!(
            kind_of(&E::SignedVersionMismatch {
                signed: "1.0.0".into(),
                announced: "9.9.9".into()
            }),
            FailureKind::InvalidSignature
        );
        assert_eq!(
            kind_of(&E::MissingSignedVersion),
            FailureKind::InvalidSignature
        );
        assert_eq!(
            kind_of(&E::SignatureUtf8("x".into())),
            FailureKind::InvalidSignature
        );
        assert_eq!(kind_of(&E::InvalidUpdaterFormat), FailureKind::Other);
    }

    #[test]
    fn long_release_notes_are_cut_on_a_character_boundary() {
        assert_eq!(cut_notes(None), "");
        assert_eq!(cut_notes(Some("short".into())), "short");
        let long = "é".repeat(MAX_NOTES_CHARS + 50);
        let cut = cut_notes(Some(long));
        assert_eq!(cut.chars().count(), MAX_NOTES_CHARS);
    }

    #[test]
    fn the_releases_page_is_a_plain_web_address_of_the_project() {
        assert_eq!(
            links::check_web_address(RELEASES_PAGE).unwrap(),
            RELEASES_PAGE
        );
        assert!(RELEASES_PAGE.starts_with("https://github.com/Ferdinand99/Patient-Partridge/"));
    }
}
