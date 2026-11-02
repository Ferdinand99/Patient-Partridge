//! Remembers where the reader was in each file, so that opening a file again puts the reader back.
//!
//! The memory is one small JSON file in the app's data folder on this computer. Nothing is sent
//! anywhere, and "Forget remembered positions" in the settings menu empties it. A damaged or
//! oversized file is ignored (the memory starts empty), and at most 500 files are remembered: the
//! one not opened for the longest time is forgotten first.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Most files remembered.
pub const MAX_ENTRIES: usize = 500;

/// Largest file read back. A real one is a few tens of kilobytes.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Lowest and highest zoom accepted, as a factor (1 = 100 %). Matches the viewer.
const ZOOM_RANGE: (f32, f32) = (0.1, 8.0);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Position {
    /// Page, counted from 0.
    pub page: u32,
    /// How far down the page: 0 at the top, 1 at the bottom.
    pub offset: f32,
    /// The zoom, or none for fit width.
    pub zoom: Option<f32>,
}

impl Position {
    /// A position with every number brought into its range, or none when a number is not a number.
    fn checked(self) -> Option<Position> {
        if !self.offset.is_finite() {
            return None;
        }
        let zoom = match self.zoom {
            Some(z) if z.is_finite() => Some(z.clamp(ZOOM_RANGE.0, ZOOM_RANGE.1)),
            Some(_) => return None,
            None => None,
        };
        Some(Position {
            page: self.page,
            offset: self.offset.clamp(0.0, 1.0),
            zoom,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    path: String,
    #[serde(flatten)]
    position: Position,
    /// When the file was last opened or left, in seconds since 1970. Only used to order entries.
    updated: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Stored {
    version: u32,
    entries: Vec<Entry>,
}

/// The key a path is remembered under: the real path when it can be found, and in lower case on
/// Windows, where file names do not care about case.
pub fn key_for(path: &str) -> String {
    let real = fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string());
    if cfg!(windows) {
        real.to_lowercase()
    } else {
        real
    }
}

pub struct Positions {
    file: Option<PathBuf>,
    entries: Vec<Entry>,
}

impl Positions {
    /// Loads the memory from `file`. With no file the memory lives only as long as the app.
    pub fn load(file: Option<PathBuf>) -> Positions {
        let entries = file.as_deref().map(read_entries).unwrap_or_default();
        Positions { file, entries }
    }

    pub fn get(&self, key: &str) -> Option<Position> {
        self.entries
            .iter()
            .find(|e| e.path == key)
            .map(|e| e.position)
    }

    /// Remembers a position. A position that makes no sense is ignored. Returns an error only when
    /// the file could not be written; the memory in the app is updated either way.
    pub fn set(&mut self, key: &str, position: Position, now: u64) -> io::Result<()> {
        let Some(position) = position.checked() else {
            return Ok(());
        };
        self.entries.retain(|e| e.path != key);
        self.entries.push(Entry {
            path: key.to_string(),
            position,
            updated: now,
        });
        if self.entries.len() > MAX_ENTRIES {
            self.entries.sort_by_key(|e| std::cmp::Reverse(e.updated));
            self.entries.truncate(MAX_ENTRIES);
        }
        self.save()
    }

    /// Forgets everything, in the app and in the file.
    pub fn clear(&mut self) -> io::Result<()> {
        self.entries.clear();
        self.save()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn save(&self) -> io::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        if let Some(dir) = file.parent() {
            create_private_dir(dir)?;
        }
        let stored = Stored {
            version: 1,
            entries: self.entries.clone(),
        };
        let json = serde_json::to_vec(&stored).map_err(io::Error::other)?;
        // Write beside the file and move it into place, so a crash cannot leave half a file.
        let temp = file.with_extension("json.tmp");
        write_private_file(&temp, &json)?;
        fs::rename(&temp, file)
    }
}

/// Makes the folder if it is missing. On Unix it is for the user alone (the file holds the paths of
/// everything that was read).
pub(crate) fn create_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)
    }
}

/// Writes a file that, on Unix, only the user can read.
pub(crate) fn write_private_file(file: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(file)?.write_all(bytes)
}

/// Reads a small file of the app's own data, or gives nothing. Only a plain file is read, after
/// links are followed: a named pipe would block the start, and a link to a device such as
/// `/dev/zero` would fill the memory. A file longer than `cap` bytes is not read, and a file that
/// grows while it is read is cut at the limit and counts as too long.
pub(crate) fn read_plain_file(file: &Path, cap: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let meta = fs::metadata(file).ok()?;
    if !meta.is_file() || meta.len() > cap {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(file).ok()?.take(cap + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= cap).then_some(bytes)
}

fn read_entries(file: &Path) -> Vec<Entry> {
    let Some(bytes) = read_plain_file(file, MAX_FILE_BYTES) else {
        return Vec::new();
    };
    let Ok(stored) = serde_json::from_slice::<Stored>(&bytes) else {
        return Vec::new();
    };
    if stored.version != 1 {
        return Vec::new();
    }
    let mut entries: Vec<Entry> = stored
        .entries
        .into_iter()
        .filter_map(|mut e| {
            e.position = e.position.checked()?;
            Some(e)
        })
        .collect();
    entries.truncate(MAX_ENTRIES);
    entries
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
                "partridge-positions-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn file(&self) -> PathBuf {
            self.0.join("positions.json")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn pos(page: u32) -> Position {
        Position {
            page,
            offset: 0.25,
            zoom: Some(1.5),
        }
    }

    #[test]
    fn a_position_is_remembered_and_comes_back_after_a_restart() {
        let dir = TempDir::new();
        let mut positions = Positions::load(Some(dir.file()));
        assert!(positions.get("a.pdf").is_none());
        positions.set("a.pdf", pos(7), 100).unwrap();
        positions
            .set(
                "b.pdf",
                Position {
                    page: 0,
                    offset: 0.0,
                    zoom: None,
                },
                101,
            )
            .unwrap();
        assert_eq!(positions.get("a.pdf"), Some(pos(7)));

        let again = Positions::load(Some(dir.file()));
        assert_eq!(again.len(), 2);
        assert_eq!(again.get("a.pdf"), Some(pos(7)));
        assert_eq!(again.get("b.pdf").unwrap().zoom, None);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_and_its_folder_are_for_the_user_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let inner = dir.0.join("data");
        let file = inner.join("positions.json");
        let mut positions = Positions::load(Some(file.clone()));
        positions.set("a.pdf", pos(1), 1).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(&inner), 0o700);
        // Writing again keeps it so.
        positions.set("b.pdf", pos(2), 2).unwrap();
        assert_eq!(mode(&file), 0o600);
    }

    #[test]
    fn setting_a_file_again_replaces_it() {
        let mut positions = Positions::load(None);
        positions.set("a.pdf", pos(1), 1).unwrap();
        positions.set("a.pdf", pos(9), 2).unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions.get("a.pdf").unwrap().page, 9);
    }

    #[test]
    fn numbers_are_brought_into_range_and_nonsense_is_ignored() {
        let mut positions = Positions::load(None);
        positions
            .set(
                "a.pdf",
                Position {
                    page: 3,
                    offset: 7.0,
                    zoom: Some(100.0),
                },
                1,
            )
            .unwrap();
        let got = positions.get("a.pdf").unwrap();
        assert_eq!((got.offset, got.zoom), (1.0, Some(8.0)));
        positions
            .set(
                "b.pdf",
                Position {
                    page: 0,
                    offset: f32::NAN,
                    zoom: None,
                },
                2,
            )
            .unwrap();
        positions
            .set(
                "c.pdf",
                Position {
                    page: 0,
                    offset: 0.0,
                    zoom: Some(f32::INFINITY),
                },
                3,
            )
            .unwrap();
        assert!(positions.get("b.pdf").is_none() && positions.get("c.pdf").is_none());
    }

    #[test]
    fn only_the_newest_entries_are_kept() {
        let mut positions = Positions::load(None);
        for i in 0..(MAX_ENTRIES as u32 + 20) {
            positions
                .set(&format!("{i}.pdf"), pos(i), u64::from(i))
                .unwrap();
        }
        assert_eq!(positions.len(), MAX_ENTRIES);
        assert!(positions.get("0.pdf").is_none());
        assert!(positions
            .get(&format!("{}.pdf", MAX_ENTRIES + 19))
            .is_some());
    }

    #[test]
    fn clearing_empties_the_file_too() {
        let dir = TempDir::new();
        let mut positions = Positions::load(Some(dir.file()));
        positions.set("a.pdf", pos(1), 1).unwrap();
        positions.clear().unwrap();
        assert!(positions.is_empty());
        assert!(Positions::load(Some(dir.file())).is_empty());
    }

    #[test]
    fn a_damaged_or_foreign_file_starts_the_memory_empty() {
        let dir = TempDir::new();
        for content in [
            &b"not json at all"[..],
            &b"{\"version\":2,\"entries\":[]}"[..],
            &b"{\"version\":1}"[..],
            &[0xff, 0xfe, 0x00][..],
            &b""[..],
        ] {
            fs::write(dir.file(), content).unwrap();
            assert!(Positions::load(Some(dir.file())).is_empty());
        }
        // And it can be written over afterwards.
        let mut positions = Positions::load(Some(dir.file()));
        positions.set("a.pdf", pos(2), 5).unwrap();
        assert_eq!(Positions::load(Some(dir.file())).len(), 1);
    }

    #[test]
    fn only_a_plain_file_of_a_fitting_size_is_read() {
        let dir = TempDir::new();
        let file = dir.file();
        fs::write(&file, b"12345").unwrap();
        assert_eq!(read_plain_file(&file, 5).as_deref(), Some(&b"12345"[..]));
        assert_eq!(read_plain_file(&file, 4), None);
        assert_eq!(read_plain_file(&dir.file().with_extension("missing"), 5), None);
        // A folder is not a file.
        assert_eq!(read_plain_file(file.parent().unwrap(), 1 << 20), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_named_pipe_and_a_link_to_a_device_are_not_read() {
        let dir = TempDir::new();
        let pipe = dir.file().with_extension("pipe");
        let made = std::process::Command::new("mkfifo").arg(&pipe).status();
        if made.is_ok_and(|s| s.success()) {
            // Opening a pipe for reading would wait for ever: the answer must come at once.
            assert_eq!(read_plain_file(&pipe, 1 << 20), None);
        }
        let link = dir.file().with_extension("link");
        if std::os::unix::fs::symlink("/dev/zero", &link).is_ok() {
            assert_eq!(read_plain_file(&link, 1 << 20), None);
        }
    }

    #[test]
    fn a_file_that_is_too_big_is_not_read() {
        let dir = TempDir::new();
        fs::write(dir.file(), vec![b' '; MAX_FILE_BYTES as usize + 1]).unwrap();
        assert!(Positions::load(Some(dir.file())).is_empty());
    }

    #[test]
    fn entries_with_nonsense_numbers_are_dropped_on_load() {
        let dir = TempDir::new();
        fs::write(
            dir.file(),
            br#"{"version":1,"entries":[
                {"path":"ok.pdf","page":1,"offset":0.5,"zoom":null,"updated":1},
                {"path":"big.pdf","page":1,"offset":9.0,"zoom":50.0,"updated":2},
                {"path":"bad.pdf","page":-1,"offset":0.5,"zoom":null,"updated":3}
            ]}"#,
        )
        .unwrap();
        // The entry with a negative page does not fit the type: the whole file is refused rather
        // than half trusted.
        assert!(Positions::load(Some(dir.file())).is_empty());
        fs::write(
            dir.file(),
            br#"{"version":1,"entries":[
                {"path":"ok.pdf","page":1,"offset":0.5,"zoom":null,"updated":1},
                {"path":"big.pdf","page":1,"offset":9.0,"zoom":50.0,"updated":2}
            ]}"#,
        )
        .unwrap();
        let loaded = Positions::load(Some(dir.file()));
        assert_eq!(loaded.len(), 2);
        let big = loaded.get("big.pdf").unwrap();
        assert_eq!((big.offset, big.zoom), (1.0, Some(8.0)));
    }

    #[test]
    fn the_same_file_has_the_same_key_however_it_is_named() {
        let dir = TempDir::new();
        let file = dir.0.join("a.pdf");
        fs::write(&file, b"x").unwrap();
        let direct = key_for(&file.to_string_lossy());
        let roundabout = key_for(&dir.0.join(".").join("a.pdf").to_string_lossy());
        assert_eq!(direct, roundabout);
        // A path that does not exist is used as it is.
        assert_eq!(key_for("/no/such/file.pdf"), "/no/such/file.pdf");
    }
}
