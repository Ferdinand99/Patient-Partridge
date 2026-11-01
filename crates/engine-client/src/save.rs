//! Saving a document without ever risking the file that is on disk.
//!
//! The engine hands the app the bytes of the document (see `EngineClient::save_to`); this module
//! writes them. The rules, in the order they are applied:
//!
//! 1. The document's signatures and protection are asked for first. A signed document is not saved
//!    unless the caller says so, because PDFium rewrites the whole file and that breaks a
//!    signature.
//! 2. The bytes go to a new file next to the target, in the same folder (so the last step is a
//!    rename within one file system). The target is not touched. If anything fails from here on
//!    (a full disk, the engine dying, an unreadable result), the new file is removed and the
//!    target is as it was.
//! 3. The new file is flushed to the disk (`fsync`), and given the permissions of the file it will
//!    replace.
//! 4. The bytes are untrusted: a fresh, sandboxed engine opens the new file. It must open (with the
//!    same password), have the pages the document has, be protected the way the document is, and
//!    carry the same signatures. A file that does not pass is never put in place.
//! 5. If the target was changed by someone else since it was opened, the caller is asked first
//!    (`SaveError::Changed`), unless it said to overwrite.
//! 6. The new file replaces the target in one rename, and the folder is flushed.
//!
//! A crash at any point before step 6 leaves the target as it was, and at most one stray file
//! named `.<name>.partridge-<number>.tmp`, which a later save of the same file removes once it is
//! a day old.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use partridge_protocol::ErrorCode;

use crate::{ClientError, EngineClient, EngineConfig, SaveStreamError};

/// What the app needs to know about a document before it saves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocInfo {
    pub pages: u32,
    /// The revision of the security handler (2 to 4), 255 for a newer one, 0 for none.
    pub security: u8,
    pub signatures: u32,
}

/// What the caller has agreed to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SaveOptions {
    /// Save a signed document although the rewrite breaks its signatures.
    pub allow_signed: bool,
    /// Replace the file although someone else changed it since it was opened.
    pub overwrite_changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveOutcome {
    /// The file that was written.
    pub path: PathBuf,
    pub bytes: u64,
}

/// Why a save did not happen. Whatever the reason, the file it was meant for is as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    /// The document holds digital signatures and a save would break them.
    Signed { signatures: u32 },
    /// The file was changed (or deleted) by someone else since it was opened.
    Changed,
    /// The file or its folder may not be written.
    ReadOnly(String),
    /// There is no room left on the disk.
    DiskFull,
    /// The path is a folder or some other thing that is not a plain file.
    NotAFile,
    /// The bytes could not be written or put in place.
    Write(String),
    /// The new file was written, but it did not pass the check, so it was not used.
    Verify(String),
    /// The new file is not protected the way the document is.
    ProtectionLost,
    /// Another save of the same document is under way.
    Busy,
}

impl SaveError {
    /// A short stable name, the error code the window sees.
    pub fn code(&self) -> &'static str {
        match self {
            SaveError::Signed { .. } => "save_signed",
            SaveError::Changed => "save_changed",
            SaveError::ReadOnly(_) => "save_read_only",
            SaveError::DiskFull => "save_disk_full",
            SaveError::NotAFile => "save_not_a_file",
            SaveError::Write(_) => "save_write_failed",
            SaveError::Verify(_) => "save_verify_failed",
            SaveError::ProtectionLost => "save_protection_lost",
            SaveError::Busy => "save_busy",
        }
    }
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SaveError::Signed { signatures } => write!(
                f,
                "the document has {signatures} digital signature{}, and saving it would break {}",
                if *signatures == 1 { "" } else { "s" },
                if *signatures == 1 { "it" } else { "them" }
            ),
            SaveError::Changed => write!(
                f,
                "the file was changed by something else since it was opened"
            ),
            SaveError::ReadOnly(why) => write!(f, "{why}"),
            SaveError::DiskFull => write!(f, "there is no room left on the disk"),
            SaveError::NotAFile => write!(f, "the path is not a plain file"),
            SaveError::Write(why) => write!(f, "the file could not be written: {why}"),
            SaveError::Verify(why) => write!(f, "the new file did not pass the check: {why}"),
            SaveError::ProtectionLost => write!(
                f,
                "the new file would not be protected by the password like the document is"
            ),
            SaveError::Busy => write!(f, "another save of this document is under way"),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<SaveError> for ClientError {
    fn from(e: SaveError) -> Self {
        ClientError::Save(e)
    }
}

// ---- telling whether a file changed -----------------------------------------------------------

/// What identifies the state of a file well enough to notice that someone else changed it: its
/// size, its time of last change and, where the system has one, which file it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    id: Option<(u64, u64)>,
}

impl FileStamp {
    pub fn of(meta: &fs::Metadata) -> FileStamp {
        #[cfg(unix)]
        let id = {
            use std::os::unix::fs::MetadataExt;
            Some((meta.dev(), meta.ino()))
        };
        #[cfg(not(unix))]
        let id = None;
        FileStamp {
            len: meta.len(),
            modified: meta.modified().ok(),
            id,
        }
    }

    pub fn read(path: &Path) -> io::Result<FileStamp> {
        fs::metadata(path).map(|m| FileStamp::of(&m))
    }
}

// ---- errors of the disk ---------------------------------------------------------------------------

fn is_disk_full(e: &io::Error) -> bool {
    if matches!(
        e.kind(),
        io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded
    ) {
        return true;
    }
    match e.raw_os_error() {
        #[cfg(unix)]
        Some(code) => code == libc::ENOSPC || code == libc::EDQUOT,
        // ERROR_HANDLE_DISK_FULL, ERROR_DISK_FULL
        #[cfg(windows)]
        Some(code) => code == 39 || code == 112,
        _ => false,
    }
}

/// Sorts an error of the file system into what the user can act on.
fn from_io(e: &io::Error, what: &str) -> SaveError {
    if is_disk_full(e) {
        return SaveError::DiskFull;
    }
    match e.kind() {
        io::ErrorKind::PermissionDenied => {
            SaveError::ReadOnly(format!("{what} may not be written ({e})"))
        }
        io::ErrorKind::ReadOnlyFilesystem => {
            SaveError::ReadOnly(format!("{what} is on a read-only disk"))
        }
        _ => SaveError::Write(format!("{what}: {e}")),
    }
}

// ---- the new file -----------------------------------------------------------------------------------

/// The new file. It is removed again unless it was moved into place.
struct Temp {
    path: PathBuf,
    keep: bool,
}

impl Drop for Temp {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn temp_prefix(name: &str) -> String {
    format!(".{name}.partridge-")
}

/// Creates a new file next to `target`, with a name that cannot clash with anything.
fn create_temp(target: &Path) -> Result<(Temp, File), SaveError> {
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or(SaveError::NotAFile)?;
    // Names are cut so that the temporary name stays below the limit of 255 bytes.
    let name: String = name.chars().take(100).collect();
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    for attempt in 0..8u32 {
        let path = dir.join(format!(
            "{}{}-{}.tmp",
            temp_prefix(&name),
            std::process::id(),
            nanos.wrapping_add(attempt)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((Temp { path, keep: false }, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(from_io(&e, "the folder")),
        }
    }
    Err(SaveError::Write(
        "no name for a new file could be found".into(),
    ))
}

/// Removes files that an earlier save of this file left behind (the app was killed in the middle
/// of it). Only files that carry the name of this save, and only when they are a day old, so
/// that a save that runs at this moment in another copy of the app is left alone.
fn remove_stale_temps(target: &Path) {
    let (Some(dir), Some(name)) = (target.parent(), target.file_name()) else {
        return;
    };
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let prefix = temp_prefix(&name.to_string_lossy().chars().take(100).collect::<String>());
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if !(file_name.starts_with(&prefix) && file_name.ends_with(".tmp")) {
            continue;
        }
        let old = entry
            .metadata()
            .ok()
            .filter(|m| m.is_file())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(24 * 60 * 60));
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Looks at the file that will be replaced. `None` when there is none yet.
fn check_target(target: &Path) -> Result<Option<fs::Metadata>, SaveError> {
    match fs::metadata(target) {
        Ok(meta) if !meta.is_file() => Err(SaveError::NotAFile),
        // A file that was made read-only is the user saying "not this one". A copy elsewhere is
        // what Save As is for.
        Ok(meta) if meta.permissions().readonly() => Err(SaveError::ReadOnly(
            "the file is read-only; save a copy under another name instead".into(),
        )),
        Ok(meta) => Ok(Some(meta)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(from_io(&e, "the file")),
    }
}

/// A link is followed, so that the file it points to is replaced and the link stays a link.
fn resolve(target: &Path) -> PathBuf {
    match fs::symlink_metadata(target) {
        Ok(m) if m.file_type().is_symlink() => {
            fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf())
        }
        _ => target.to_path_buf(),
    }
}

// ---- checking what was written ---------------------------------------------------------------------------

/// Opens `path` in a fresh, sandboxed engine and checks that it is the document it should be:
/// it opens (with `password`), has `expected.pages` pages, is protected like the document and has
/// the same signatures. When a password was needed, the file must refuse to open without one.
pub fn verify_written(
    config: &EngineConfig,
    path: &Path,
    password: Option<&str>,
    expected: DocInfo,
) -> Result<(), SaveError> {
    let verify = |why: String| SaveError::Verify(why);
    let text = path.to_string_lossy();
    let client = EngineClient::new(config.clone());
    let result = (|| {
        let opened = client
            .open(&text, password)
            .map_err(|e| verify(format!("it does not open ({e})")))?;
        if opened.pages.len() as u32 != expected.pages {
            return Err(verify(format!(
                "it has {} pages, the document has {}",
                opened.pages.len(),
                expected.pages
            )));
        }
        let got = client
            .doc_info(opened.doc)
            .map_err(|e| verify(format!("its description could not be read ({e})")))?;
        if expected.security != 0 && got.security == 0 {
            return Err(SaveError::ProtectionLost);
        }
        if got.security != expected.security {
            return Err(verify(format!(
                "its protection is of another kind ({} instead of {})",
                got.security, expected.security
            )));
        }
        if got.signatures != expected.signatures {
            return Err(verify(format!(
                "it holds {} signatures, the document holds {}",
                got.signatures, expected.signatures
            )));
        }
        Ok(())
    })();
    client.kill();
    result?;
    if password.is_some() {
        // A password was needed to open the document: the file must still need one.
        let second = EngineClient::new(config.clone());
        let outcome = second.open(&text, None);
        second.kill();
        match outcome {
            Err(ClientError::Engine {
                code: ErrorCode::PasswordRequired,
                ..
            }) => {}
            Ok(_) => return Err(SaveError::ProtectionLost),
            Err(e) => return Err(verify(format!("it was refused in a strange way ({e})"))),
        }
    }
    Ok(())
}

// ---- the whole save ---------------------------------------------------------------------------------------------

/// Saves the document `doc` of `client` to `target`. `expected` is the state the target was in when
/// the document was opened (or chosen), `None` when it did not exist then. `verify` is given the
/// new file and the description of the document (see [`verify_written`]); a file it refuses is
/// never put in place.
pub(crate) fn save_document(
    client: &EngineClient,
    doc: u64,
    target: &Path,
    expected: Option<&FileStamp>,
    options: SaveOptions,
    verify: &dyn Fn(&Path, DocInfo) -> Result<(), SaveError>,
) -> Result<SaveOutcome, ClientError> {
    let info = client.doc_info(doc)?;
    if info.signatures > 0 && !options.allow_signed {
        return Err(SaveError::Signed {
            signatures: info.signatures,
        }
        .into());
    }
    let target = resolve(target);
    let existing = check_target(&target)?;
    // Ask early: there is no point in writing a file that will not be used.
    check_unchanged(&target, expected, options)?;
    remove_stale_temps(&target);

    let (mut temp, mut file) = create_temp(&target)?;
    let mut written: u64 = 0;
    let total = client.save_to(doc, &mut |piece| {
        file.write_all(piece)?;
        written += piece.len() as u64;
        Ok(())
    });
    let total = match total {
        Ok(total) => total,
        Err(SaveStreamError::Sink(e)) => return Err(from_io(&e, "the new file").into()),
        Err(SaveStreamError::Client(e)) => return Err(e),
    };
    if written != total {
        return Err(SaveError::Write("fewer bytes were written than were sent".into()).into());
    }
    if let Some(meta) = &existing {
        // The file keeps the permissions it had. (Where the owner is another user, the system
        // decides who owns the new file.)
        file.set_permissions(meta.permissions())
            .map_err(|e| from_io(&e, "the new file"))?;
    }
    // The bytes must be on the disk, not in a cache, before the old file goes away.
    file.sync_all().map_err(|e| from_io(&e, "the new file"))?;
    drop(file);
    let on_disk = fs::metadata(&temp.path)
        .map_err(|e| from_io(&e, "the new file"))?
        .len();
    if on_disk != total {
        return Err(SaveError::Write(format!(
            "the new file has {on_disk} bytes on the disk, not {total}"
        ))
        .into());
    }

    verify(&temp.path, info)?;

    // The last look at the target, as close to the replacement as it gets.
    check_unchanged(&target, expected, options)?;
    fs::rename(&temp.path, &target).map_err(|e| from_io(&e, "the file"))?;
    temp.keep = true;
    sync_folder(&target);
    Ok(SaveOutcome {
        path: target,
        bytes: total,
    })
}

fn check_unchanged(
    target: &Path,
    expected: Option<&FileStamp>,
    options: SaveOptions,
) -> Result<(), SaveError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    if options.overwrite_changed {
        return Ok(());
    }
    match FileStamp::read(target) {
        Ok(now) if &now == expected => Ok(()),
        Ok(_) => Err(SaveError::Changed),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(SaveError::Changed),
        Err(e) => Err(from_io(&e, "the file")),
    }
}

/// Makes the rename itself durable. Not every file system can do this; that is not an error.
fn sync_folder(target: &Path) {
    #[cfg(unix)]
    if let Some(dir) = target.parent() {
        let dir = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        if let Ok(dir) = File::open(dir) {
            let _ = dir.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = target;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    // ---- the pieces ----------------------------------------------------------------------

    #[test]
    fn a_full_disk_is_told_from_other_write_errors() {
        #[cfg(unix)]
        {
            let full = io::Error::from_raw_os_error(libc::ENOSPC);
            assert_eq!(from_io(&full, "x"), SaveError::DiskFull);
            let quota = io::Error::from_raw_os_error(libc::EDQUOT);
            assert_eq!(from_io(&quota, "x"), SaveError::DiskFull);
        }
        #[cfg(windows)]
        {
            assert_eq!(
                from_io(&io::Error::from_raw_os_error(112), "x"),
                SaveError::DiskFull
            );
        }
        assert!(matches!(
            from_io(
                &io::Error::from(io::ErrorKind::PermissionDenied),
                "the folder"
            ),
            SaveError::ReadOnly(_)
        ));
        assert!(matches!(
            from_io(&io::Error::other("boom"), "the folder"),
            SaveError::Write(_)
        ));
    }

    #[test]
    fn every_error_has_its_own_code_and_a_message() {
        let all = [
            SaveError::Signed { signatures: 2 },
            SaveError::Changed,
            SaveError::ReadOnly("r".into()),
            SaveError::DiskFull,
            SaveError::NotAFile,
            SaveError::Write("w".into()),
            SaveError::Verify("v".into()),
            SaveError::ProtectionLost,
            SaveError::Busy,
        ];
        let mut codes: Vec<&str> = all.iter().map(SaveError::code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), all.len());
        assert!(all.iter().all(|e| !e.to_string().is_empty()));
        assert!(SaveError::Signed { signatures: 1 }
            .to_string()
            .contains("1 digital signature,"));
    }

    // ---- a save against a stand-in engine --------------------------------------------------------

    /// What the stand-in engine saves: byte `i` is `i % 251`.
    const FAKE_SIZE: usize = 2_500_000;

    fn fake_bytes() -> Vec<u8> {
        (0..FAKE_SIZE).map(|i| (i % 251) as u8).collect()
    }

    fn fake_engine() -> PathBuf {
        let exe = std::env::current_exe().unwrap();
        exe.parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("examples")
            .join(format!("fake_engine{}", std::env::consts::EXE_SUFFIX))
    }

    /// A folder of its own for one test, removed again at the end.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "partridge-save-{name}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            Dir(dir)
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }

        /// Names of everything in the folder.
        fn names(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.0)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            // A folder made read-only by a test must be removable.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
            }
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A client with an open document (the stand-in's, which ignores the file it is given).
    fn client(mode: &str) -> (EngineClient, u64) {
        let mut config = EngineConfig::new(fake_engine());
        config.args = vec![mode.into()];
        config.timeout = Duration::from_secs(20);
        let client = EngineClient::new(config);
        let probe = partridge_testkit::TempFile::new("probe.pdf", b"%PDF-1.7\n");
        let doc = client.open(&probe.path_str(), None).unwrap().doc;
        (client, doc)
    }

    fn accept(_: &Path, _: DocInfo) -> Result<(), SaveError> {
        Ok(())
    }

    fn save(
        client: &EngineClient,
        doc: u64,
        target: &Path,
        expected: Option<&FileStamp>,
        options: SaveOptions,
    ) -> Result<SaveOutcome, ClientError> {
        save_document(client, doc, target, expected, options, &accept)
    }

    fn save_error(result: Result<SaveOutcome, ClientError>) -> SaveError {
        match result {
            Err(ClientError::Save(e)) => e,
            other => panic!("expected a save error, got {other:?}"),
        }
    }

    const ORIGINAL: &[u8] = b"the file as the user has it";

    fn original_in(dir: &Dir) -> PathBuf {
        let path = dir.file("report.pdf");
        fs::write(&path, ORIGINAL).unwrap();
        path
    }

    fn assert_untouched(dir: &Dir, path: &Path) {
        assert_eq!(fs::read(path).unwrap(), ORIGINAL, "the original changed");
        assert_eq!(dir.names(), vec!["report.pdf".to_string()], "stray files");
    }

    #[test]
    fn the_bytes_of_the_engine_replace_the_file_and_nothing_is_left_behind() {
        let dir = Dir::new("ok");
        let path = original_in(&dir);
        let (client, doc) = client("save-good");
        let stamp = FileStamp::read(&path).unwrap();
        let out = save(&client, doc, &path, Some(&stamp), SaveOptions::default()).unwrap();
        assert_eq!(out.bytes as usize, FAKE_SIZE);
        assert_eq!(fs::read(&path).unwrap(), fake_bytes());
        assert_eq!(dir.names(), vec!["report.pdf".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Dir::new("mode");
        let path = original_in(&dir);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let (client, doc) = client("save-good");
        save(&client, doc, &path, None, SaveOptions::default()).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
        assert_eq!(fs::read(&path).unwrap(), fake_bytes());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_stays_a_link_and_the_file_it_points_to_is_replaced() {
        let dir = Dir::new("link");
        let real = dir.file("real.pdf");
        fs::write(&real, ORIGINAL).unwrap();
        let link = dir.file("link.pdf");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let (client, doc) = client("save-good");
        save(&client, doc, &link, None, SaveOptions::default()).unwrap();
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read(&real).unwrap(), fake_bytes());
    }

    #[test]
    fn a_new_file_can_be_written_where_nothing_was() {
        let dir = Dir::new("new");
        let path = dir.file("copy.pdf");
        let (client, doc) = client("save-good");
        save(&client, doc, &path, None, SaveOptions::default()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), fake_bytes());
        assert_eq!(dir.names(), vec!["copy.pdf".to_string()]);
    }

    #[test]
    fn a_file_the_check_refuses_is_never_put_in_place() {
        let dir = Dir::new("verify");
        let path = original_in(&dir);
        let (client, doc) = client("save-good");
        for refusal in [
            SaveError::Verify("it does not open".into()),
            SaveError::ProtectionLost,
        ] {
            let result = save_document(
                &client,
                doc,
                &path,
                None,
                SaveOptions::default(),
                &|file, _| {
                    // The new file exists and holds all the bytes when it is checked.
                    assert_eq!(fs::metadata(file).unwrap().len() as usize, FAKE_SIZE);
                    Err(refusal.clone())
                },
            );
            assert_eq!(save_error(result), refusal);
            assert_untouched(&dir, &path);
        }
    }

    #[test]
    fn the_check_is_told_what_the_document_is() {
        let dir = Dir::new("info");
        let path = original_in(&dir);
        let (client, doc) = client("save-good");
        save_document(
            &client,
            doc,
            &path,
            None,
            SaveOptions::default(),
            &|_, info| {
                assert_eq!(
                    info,
                    DocInfo {
                        pages: 1,
                        security: 0,
                        signatures: 0
                    }
                );
                Ok(())
            },
        )
        .unwrap();
    }

    #[test]
    fn an_engine_that_misbehaves_while_saving_costs_nothing() {
        for (mode, needle) in [
            ("save-short", "unexpected"),
            ("save-wrong-offset", "unexpected"),
            ("save-big", "unreadable"),
            ("save-lies", "the end is missing"),
            ("save-error", "cannot save"),
        ] {
            let dir = Dir::new(mode);
            let path = original_in(&dir);
            let (client, doc) = client(mode);
            let err = save(&client, doc, &path, None, SaveOptions::default()).unwrap_err();
            assert!(err.to_string().contains(needle), "{mode}: {err}");
            assert_untouched(&dir, &path);
        }
    }

    #[test]
    fn a_signed_document_is_saved_only_when_the_caller_says_so() {
        let dir = Dir::new("signed");
        let path = original_in(&dir);
        let (client, doc) = client("save-signed");
        let err = save_error(save(&client, doc, &path, None, SaveOptions::default()));
        assert_eq!(err, SaveError::Signed { signatures: 2 });
        assert_untouched(&dir, &path);
        let allowed = SaveOptions {
            allow_signed: true,
            ..SaveOptions::default()
        };
        save(&client, doc, &path, None, allowed).unwrap();
        assert_eq!(fs::read(&path).unwrap(), fake_bytes());
    }

    #[test]
    fn a_file_that_someone_else_changed_is_not_overwritten_unless_the_caller_says_so() {
        let dir = Dir::new("changed");
        let path = original_in(&dir);
        let (client, doc) = client("save-good");
        let stamp = FileStamp::read(&path).unwrap();
        // Same size, a later time: still a different file as far as the app can tell.
        let changed = b"the file as someone changed";
        assert_eq!(changed.len(), ORIGINAL.len());
        fs::write(&path, changed).unwrap();
        let file = File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        drop(file);

        let err = save_error(save(
            &client,
            doc,
            &path,
            Some(&stamp),
            SaveOptions::default(),
        ));
        assert_eq!(err, SaveError::Changed);
        assert_eq!(
            fs::read(&path).unwrap(),
            changed,
            "the other change was lost"
        );
        assert_eq!(dir.names(), vec!["report.pdf".to_string()]);

        let overwrite = SaveOptions {
            overwrite_changed: true,
            ..SaveOptions::default()
        };
        save(&client, doc, &path, Some(&stamp), overwrite).unwrap();
        assert_eq!(fs::read(&path).unwrap(), fake_bytes());
    }

    #[test]
    fn a_file_that_was_deleted_since_it_was_opened_is_asked_about() {
        let dir = Dir::new("deleted");
        let path = original_in(&dir);
        let stamp = FileStamp::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        let (client, doc) = client("save-good");
        let err = save_error(save(
            &client,
            doc,
            &path,
            Some(&stamp),
            SaveOptions::default(),
        ));
        assert_eq!(err, SaveError::Changed);
        assert!(dir.names().is_empty());
    }

    #[test]
    fn the_target_is_looked_at_again_after_the_check() {
        // Someone changes the file while the new one is being checked: the save stops.
        let dir = Dir::new("late");
        let path = original_in(&dir);
        let (client, doc) = client("save-good");
        let stamp = FileStamp::read(&path).unwrap();
        let during = path.clone();
        let result = save_document(
            &client,
            doc,
            &path,
            Some(&stamp),
            SaveOptions::default(),
            &move |_, _| {
                fs::write(&during, b"changed during the check, and longer").unwrap();
                Ok(())
            },
        );
        assert_eq!(save_error(result), SaveError::Changed);
        assert_eq!(
            fs::read(&path).unwrap(),
            b"changed during the check, and longer"
        );
        assert_eq!(dir.names(), vec!["report.pdf".to_string()]);
    }

    #[test]
    fn a_read_only_file_a_folder_and_a_missing_folder_are_refused() {
        let dir = Dir::new("refused");
        let path = original_in(&dir);
        let (client, doc) = client("save-good");

        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&path, perms.clone()).unwrap();
        let err = save_error(save(&client, doc, &path, None, SaveOptions::default()));
        assert!(matches!(err, SaveError::ReadOnly(_)), "{err:?}");
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(&path, perms).unwrap();
        assert_untouched(&dir, &path);

        let err = save_error(save(&client, doc, &dir.0, None, SaveOptions::default()));
        assert_eq!(err, SaveError::NotAFile);

        let err = save_error(save(
            &client,
            doc,
            &dir.file("no-such-folder").join("x.pdf"),
            None,
            SaveOptions::default(),
        ));
        assert!(matches!(err, SaveError::Write(_)), "{err:?}");
        assert_untouched(&dir, &path);
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_may_not_be_written_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Dir::new("noperm");
        let path = original_in(&dir);
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o555)).unwrap();
        // The super user ignores the mode, so there is nothing to see in that case.
        if fs::File::create(dir.file("probe")).is_ok() {
            return;
        }
        let (client, doc) = client("save-good");
        let err = save_error(save(&client, doc, &path, None, SaveOptions::default()));
        assert!(matches!(err, SaveError::ReadOnly(_)), "{err:?}");
        assert_eq!(fs::read(&path).unwrap(), ORIGINAL);
    }

    /// The test program starts itself with this variable to be killed in the middle of a save.
    const CRASH_ENV: &str = "PARTRIDGE_TEST_SAVE_CRASH_DIR";

    #[test]
    fn a_crash_in_the_middle_of_a_save_leaves_the_file_as_it_was() {
        if let Some(dir) = std::env::var_os(CRASH_ENV) {
            // The child: save, and die while the new file is written and checked.
            let path = PathBuf::from(dir).join("report.pdf");
            let (client, doc) = client("save-good");
            let _ = save_document(
                &client,
                doc,
                &path,
                None,
                SaveOptions::default(),
                &|_, _| std::process::abort(),
            );
            return;
        }
        let dir = Dir::new("crash");
        let path = original_in(&dir);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "save::tests::a_crash_in_the_middle_of_a_save_leaves_the_file_as_it_was",
                "--nocapture",
            ])
            .env(CRASH_ENV, &dir.0)
            .status()
            .unwrap();
        assert!(!status.success(), "the child was meant to die");
        assert_eq!(fs::read(&path).unwrap(), ORIGINAL, "the original changed");
        // The new file is still there, complete: the crash came before the replacement.
        let strays: Vec<String> = dir
            .names()
            .into_iter()
            .filter(|n| n != "report.pdf")
            .collect();
        assert_eq!(strays.len(), 1, "{strays:?}");
        assert!(strays[0].starts_with(".report.pdf.partridge-") && strays[0].ends_with(".tmp"));
        let stray = dir.file(&strays[0]);
        assert_eq!(fs::metadata(&stray).unwrap().len() as usize, FAKE_SIZE);

        // The next save leaves a fresh stray file alone (it may belong to a save that is running)
        // and removes one that is a day old.
        let (client, doc) = client("save-good");
        save(&client, doc, &path, None, SaveOptions::default()).unwrap();
        assert!(stray.exists(), "a new file was removed");
        let file = File::options().write(true).open(&stray).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60))
            .unwrap();
        drop(file);
        save(&client, doc, &path, None, SaveOptions::default()).unwrap();
        assert!(!stray.exists(), "an old stray file stayed");
        assert_eq!(dir.names(), vec!["report.pdf".to_string()]);
    }

    /// Needs a small file system: `PARTRIDGE_TEST_SMALL_DISK` names a folder on one that has
    /// room for less than the 2.5 MB the stand-in saves. The CI runners have none, so this runs
    /// where somebody mounts one (a small tmpfs).
    #[test]
    fn a_full_disk_stops_the_save_and_leaves_the_file_as_it_was() {
        let Some(small) = std::env::var_os("PARTRIDGE_TEST_SMALL_DISK") else {
            eprintln!("skipped: PARTRIDGE_TEST_SMALL_DISK is not set");
            return;
        };
        let dir = PathBuf::from(small).join(format!("full-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("report.pdf");
        fs::write(&path, ORIGINAL).unwrap();
        let (client, doc) = client("save-good");
        let err = save_error(save(&client, doc, &path, None, SaveOptions::default()));
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        let original = fs::read(&path).unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(err, SaveError::DiskFull);
        assert_eq!(original, ORIGINAL);
        assert_eq!(names, vec!["report.pdf".to_string()]);
    }
}
