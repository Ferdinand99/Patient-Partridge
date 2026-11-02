//! Remembers which files were open, so that the next start can open them again.
//!
//! The list is one small JSON file (`session.json`) in the app's data folder, next to the remembered
//! positions. It holds the paths of the open files in tab order and which tab was in view, and
//! nothing from inside any document. It is written whenever a tab is opened, closed or brought to
//! the front, by writing a temporary file and moving it into place, so a crash cannot leave half a
//! file. While the setting "Reopen tabs at start" is off, no list is kept: turning the setting off
//! deletes the file, and nothing is written afterwards.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::updates::{read_small_file, write_json, UpdateState};
use crate::CommandError;

/// Most tabs remembered: the same as the most documents that can be open.
pub const MAX_TABS: usize = crate::MAX_DOCUMENTS;
const MAX_PATH_CHARS: usize = 4096;

/// The open files in tab order, and the index of the one in view.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TabList {
    pub paths: Vec<String>,
    pub active: Option<usize>,
    /// The paths (a part of `paths`) that could not be reached at the last start. They are tried
    /// once more, and forgotten if they fail again.
    #[serde(default)]
    pub failed: Vec<String>,
}

impl TabList {
    /// The list with everything that makes no sense taken out: empty or very long paths, repeats, and
    /// more tabs than the app can have open. The tab in view stays in view while it is in the list.
    pub fn cleaned(self) -> TabList {
        let active_path = self.active.and_then(|i| self.paths.get(i)).cloned();
        let mut paths: Vec<String> = Vec::new();
        for path in self.paths {
            if path.is_empty() || path.chars().count() > MAX_PATH_CHARS || paths.contains(&path) {
                continue;
            }
            paths.push(path);
        }
        // When the list is too long the tab in view is kept, and the last of the others go.
        if paths.len() > MAX_TABS {
            let keep_active = active_path
                .as_ref()
                .and_then(|a| paths.iter().position(|p| p == a));
            let mut room = MAX_TABS - usize::from(keep_active.is_some());
            let mut kept = Vec::new();
            for (i, path) in paths.into_iter().enumerate() {
                if Some(i) == keep_active {
                    kept.push(path);
                } else if room > 0 {
                    room -= 1;
                    kept.push(path);
                }
            }
            paths = kept;
        }
        let active = active_path.and_then(|a| paths.iter().position(|p| *p == a));
        let mut failed: Vec<String> = Vec::new();
        for path in self.failed {
            if paths.contains(&path) && !failed.contains(&path) {
                failed.push(path);
            }
        }
        TabList {
            paths,
            active,
            failed,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Stored {
    version: u32,
    paths: Vec<String>,
    active: Option<usize>,
    #[serde(default)]
    failed: Vec<String>,
}

/// The remembered tabs: what the last run left, and what the current run has now.
pub struct Session {
    file: Option<PathBuf>,
    /// What the last run left. Handed to the window once, at the start.
    startup: TabList,
}

impl Session {
    /// Reads the list of the last run. With the setting off nothing is read and a file that is left
    /// over is deleted, so that no list stays on the disk. A damaged or oversized file is ignored.
    pub fn load(file: Option<PathBuf>, enabled: bool) -> Session {
        let mut session = Session {
            file,
            startup: TabList::default(),
        };
        if !enabled {
            let _ = session.clear();
            return session;
        }
        session.startup = session
            .file
            .as_deref()
            .and_then(read_small_file)
            .and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok())
            .filter(|s| s.version == 1)
            .map(|s| {
                TabList {
                    paths: s.paths,
                    active: s.active,
                    failed: s.failed,
                }
                .cleaned()
            })
            .unwrap_or_default();
        session
    }

    /// The list of the last run, once. The window asks for it when it has started.
    pub fn take_startup(&mut self) -> TabList {
        std::mem::take(&mut self.startup)
    }

    /// Writes the current tabs. With the setting off nothing is kept: the file is deleted instead.
    pub fn save(&mut self, enabled: bool, list: TabList) -> io::Result<()> {
        if !enabled {
            return self.clear();
        }
        let Some(file) = &self.file else {
            return Ok(());
        };
        let list = list.cleaned();
        write_json(
            file,
            &Stored {
                version: 1,
                paths: list.paths,
                active: list.active,
                failed: list.failed,
            },
        )
    }

    /// Forgets the list, in the app and on the disk, also a temporary file that a crash or a failed
    /// move left beside it. Both are tried even when the first cannot be deleted.
    pub fn clear(&mut self) -> io::Result<()> {
        self.startup = TabList::default();
        let Some(file) = &self.file else {
            return Ok(());
        };
        let remove = |path: &std::path::Path| match fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
        let list = remove(file);
        let temp = remove(&temp_file(file));
        list.and(temp)
    }
}

/// The temporary file beside the list, the one `write_json` writes before it moves it into place.
fn temp_file(file: &std::path::Path) -> PathBuf {
    file.with_extension("json.tmp")
}

/// The managed state: the remembered tabs.
pub struct SessionStore(pub std::sync::Mutex<Session>);

impl SessionStore {
    pub fn clear(&self) {
        let _ = self.0.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

fn internal(error: io::Error) -> CommandError {
    CommandError {
        code: "internal".into(),
        message: error.to_string(),
    }
}

/// Whether the tabs of this run are reopened at the next start.
#[tauri::command]
pub fn get_reopen_tabs(state: State<'_, UpdateState>) -> bool {
    state.reopen_tabs()
}

/// Turns "Reopen tabs at start" on or off. Turning it off deletes the list at once. The setting
/// holds for this run even when it could not be written to the disk (the error says so).
pub fn change_reopen_tabs(
    state: &UpdateState,
    session: &SessionStore,
    enabled: bool,
) -> io::Result<()> {
    let saved = state.set_reopen_tabs(enabled);
    if !enabled {
        session.clear();
    }
    saved
}

#[tauri::command]
pub fn set_reopen_tabs(
    state: State<'_, UpdateState>,
    session: State<'_, SessionStore>,
    enabled: bool,
) -> Result<(), CommandError> {
    change_reopen_tabs(&state, &session, enabled).map_err(internal)
}

/// Remembers the open tabs. Does nothing, and keeps no list, while the setting is off.
#[tauri::command]
pub fn save_session(
    state: State<'_, UpdateState>,
    session: State<'_, SessionStore>,
    paths: Vec<String>,
    active: Option<usize>,
    failed: Vec<String>,
) {
    let enabled = state.reopen_tabs();
    let _ = session.0.lock().unwrap_or_else(|e| e.into_inner()).save(
        enabled,
        TabList {
            paths,
            active,
            failed,
        },
    );
}

/// The tabs the last run left, once. Empty while the setting is off.
#[tauri::command]
pub fn take_session(session: State<'_, SessionStore>) -> TabList {
    session
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take_startup()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> TempDir {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "partridge-session-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn file(&self) -> PathBuf {
            self.0.join("session.json")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn list(paths: &[&str], active: Option<usize>) -> TabList {
        TabList {
            paths: paths.iter().map(|p| p.to_string()).collect(),
            active,
            failed: Vec::new(),
        }
    }

    #[test]
    fn the_tabs_come_back_in_order_with_the_one_in_view() {
        let dir = TempDir::new();
        let mut session = Session::load(Some(dir.file()), true);
        assert_eq!(session.take_startup(), TabList::default());
        session
            .save(true, list(&["/a.pdf", "/b.pdf", "/c.pdf"], Some(1)))
            .unwrap();

        let mut again = Session::load(Some(dir.file()), true);
        assert_eq!(
            again.take_startup(),
            list(&["/a.pdf", "/b.pdf", "/c.pdf"], Some(1))
        );
        // Handed over once.
        assert_eq!(again.take_startup(), TabList::default());
    }

    #[test]
    fn with_the_setting_off_no_list_is_kept_and_a_list_that_is_left_is_deleted() {
        let dir = TempDir::new();
        let mut session = Session::load(Some(dir.file()), true);
        session.save(true, list(&["/a.pdf"], Some(0))).unwrap();
        assert!(dir.file().exists());

        // Turned off: deleted at once, and a later save writes nothing.
        session.clear().unwrap();
        assert!(!dir.file().exists());
        session.save(false, list(&["/a.pdf"], Some(0))).unwrap();
        assert!(!dir.file().exists());

        // Started with the setting off while a file was left over: the file goes and is not read.
        fs::write(
            dir.file(),
            br#"{"version":1,"paths":["/old.pdf"],"active":0}"#,
        )
        .unwrap();
        let mut off = Session::load(Some(dir.file()), false);
        assert!(!dir.file().exists());
        assert_eq!(off.take_startup(), TabList::default());
        // Clearing what is not there is fine.
        off.clear().unwrap();
    }

    #[test]
    fn the_file_leaves_no_temporary_file_and_holds_only_paths_and_the_active_tab() {
        let dir = TempDir::new();
        let mut session = Session::load(Some(dir.file()), true);
        session.save(true, list(&["/a.pdf"], Some(0))).unwrap();
        // No temporary file is left beside it.
        let names: Vec<_> = fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["session.json".to_string()]);
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.file()).unwrap()).unwrap();
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, vec!["active", "failed", "paths", "version"]);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_for_the_user_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let file = dir.0.join("data").join("session.json");
        let mut session = Session::load(Some(file.clone()), true);
        session.save(true, list(&["/a.pdf"], None)).unwrap();
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn a_damaged_foreign_or_oversized_file_starts_with_no_tabs() {
        let dir = TempDir::new();
        for content in [
            &b"not json"[..],
            &b"{\"version\":2,\"paths\":[\"/a.pdf\"],\"active\":0}"[..],
            &b"{\"version\":1}"[..],
            &[0xff, 0xfe][..],
            &b""[..],
        ] {
            fs::write(dir.file(), content).unwrap();
            assert_eq!(
                Session::load(Some(dir.file()), true).take_startup(),
                TabList::default()
            );
        }
        fs::write(dir.file(), vec![b' '; 70 * 1024]).unwrap();
        assert_eq!(
            Session::load(Some(dir.file()), true).take_startup(),
            TabList::default()
        );
    }

    #[test]
    fn nonsense_is_taken_out_and_the_list_keeps_to_the_document_limit() {
        // Empty, repeated and very long paths go; the tab in view is found again by its path.
        let long = "x".repeat(MAX_PATH_CHARS + 1);
        let cleaned = list(&["", "/a.pdf", "/a.pdf", &long, "/b.pdf"], Some(4)).cleaned();
        assert_eq!(cleaned, list(&["/a.pdf", "/b.pdf"], Some(1)));
        // An index that points nowhere is no tab in view.
        assert_eq!(list(&["/a.pdf"], Some(5)).cleaned().active, None);

        // More than the limit: the first ones stay, and so does the one in view.
        let many: Vec<String> = (0..MAX_TABS + 5).map(|i| format!("/{i}.pdf")).collect();
        let last = many.len() - 1;
        let cleaned = TabList {
            paths: many.clone(),
            active: Some(last),
            failed: Vec::new(),
        }
        .cleaned();
        assert_eq!(cleaned.paths.len(), MAX_TABS);
        assert_eq!(cleaned.paths[MAX_TABS - 1], many[last]);
        assert_eq!(cleaned.active, Some(MAX_TABS - 1));
        assert_eq!(cleaned.paths[0], "/0.pdf");
    }

    #[test]
    fn files_that_could_not_be_reached_are_listed_and_only_while_they_are_in_the_list() {
        let dir = TempDir::new();
        let mut session = Session::load(Some(dir.file()), true);
        let mut tabs = list(&["/a.pdf", "/b.pdf"], Some(0));
        tabs.failed = vec![
            "/b.pdf".into(),
            "/b.pdf".into(),
            "/not-in-the-list.pdf".into(),
        ];
        session.save(true, tabs).unwrap();
        let back = Session::load(Some(dir.file()), true).take_startup();
        assert_eq!(back.failed, vec!["/b.pdf".to_string()]);
        // A file from before the field existed has none.
        fs::write(
            dir.file(),
            br#"{"version":1,"paths":["/a.pdf"],"active":0}"#,
        )
        .unwrap();
        assert!(Session::load(Some(dir.file()), true)
            .take_startup()
            .failed
            .is_empty());
    }

    #[test]
    fn clearing_deletes_a_temporary_file_that_was_left_behind() {
        let dir = TempDir::new();
        let temp = dir.0.join("session.json.tmp");
        let mut session = Session::load(Some(dir.file()), true);
        session.save(true, list(&["/a.pdf"], Some(0))).unwrap();
        fs::write(&temp, br#"{"version":1,"paths":["/left.pdf"]}"#).unwrap();
        session.clear().unwrap();
        assert!(!dir.file().exists() && !temp.exists());
        // Turning the setting off at the start deletes it too.
        fs::write(&temp, b"x").unwrap();
        let _ = Session::load(Some(dir.file()), false);
        assert!(!temp.exists());
    }

    #[test]
    fn turning_the_setting_off_deletes_the_list() {
        use crate::updates::InstallMethod;
        let dir = TempDir::new();
        let state = UpdateState::new(Some(dir.0.clone()), InstallMethod::Manual);
        let store = SessionStore(std::sync::Mutex::new(Session::load(Some(dir.file()), true)));
        store
            .0
            .lock()
            .unwrap()
            .save(true, list(&["/a.pdf"], Some(0)))
            .unwrap();
        assert!(dir.file().exists());

        change_reopen_tabs(&state, &store, false).unwrap();
        assert!(!state.reopen_tabs());
        assert!(!dir.file().exists(), "the list must go at once");
        // Nothing is kept while it is off, even if the window sends a list.
        store
            .0
            .lock()
            .unwrap()
            .save(state.reopen_tabs(), list(&["/a.pdf"], Some(0)))
            .unwrap();
        assert!(!dir.file().exists());

        change_reopen_tabs(&state, &store, true).unwrap();
        assert!(state.reopen_tabs());
        assert!(!dir.file().exists());
    }

    #[test]
    fn nothing_is_written_without_a_file() {
        let mut session = Session::load(None, true);
        session.save(true, list(&["/a.pdf"], Some(0))).unwrap();
        session.clear().unwrap();
    }
}
