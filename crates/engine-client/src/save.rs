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
//! 3. The new file is given the access rights of the file it will replace (permissions, owner,
//!    group, ACL), or the save stops and a copy is offered. It is flushed (`fsync`), then read back
//!    and compared with the bytes that were received. The read-back is served by the system's
//!    cache as a rule, so it shows that what was written is what was sent, not that the disk
//!    stored it: a fault of the media itself is beyond what any program can see this way.
//! 4. The bytes are untrusted: a fresh, sandboxed engine opens the new file. It must open (with the
//!    same password), have the pages the document has, be protected the way the document is, carry
//!    the same signatures, and agree on a fingerprint of every page (size, text, number of objects
//!    and annotations). A file that does not pass is never put in place.
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
    /// How many signatures the engine found, in the field list and in the widgets of the pages.
    pub signatures: u32,
    /// The bytes of the file mention something that belongs to a signature.
    pub maybe_signed: bool,
}

impl DocInfo {
    /// A document is treated as signed when either way of looking says so. A warning that was not
    /// needed is cheap; a signature broken without a word is not.
    pub fn is_signed(&self) -> bool {
        self.signatures > 0 || self.maybe_signed
    }
}

/// A digest of what a reader sees of the document (see the engine's `Fingerprint`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint {
    pub pages: u32,
    /// Pages whose size, text or objects could not be read.
    pub failed: u32,
    pub digest: u64,
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
    /// The file the document is read from was changed in place after the document was opened, so
    /// what the engine has of it may be a mix of two versions. Nothing can be saved from it.
    SourceChanged,
    /// A read of the file the document comes from failed, so the document may be missing parts.
    SourceUnreadable,
    /// The file is open in a way that does not let it be replaced here.
    Locked,
    /// The file has more than one name (a hard link); a save would leave the other names with the
    /// old file.
    HardLink,
    /// The file belongs to another user or group, and the new file would not.
    OwnerWouldChange,
    /// The access rights of the file could not be given to the new file.
    AccessNotKept,
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
            SaveError::SourceChanged => "save_source_changed",
            SaveError::SourceUnreadable => "save_source_unreadable",
            SaveError::Locked => "save_locked",
            SaveError::HardLink => "save_hard_link",
            SaveError::OwnerWouldChange => "save_owner_changes",
            SaveError::AccessNotKept => "save_access_kept",
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
            SaveError::SourceChanged => write!(
                f,
                "the file was changed while it was open, so what is open is no longer safe to save"
            ),
            SaveError::SourceUnreadable => write!(
                f,
                "a part of the file could not be read, so the document may be missing parts"
            ),
            SaveError::Locked => write!(
                f,
                "the file is open in a way that does not let it be replaced here"
            ),
            SaveError::HardLink => write!(
                f,
                "the file has more than one name, and a save would split them"
            ),
            SaveError::OwnerWouldChange => write!(
                f,
                "the file belongs to someone else, and the new file would not"
            ),
            SaveError::AccessNotKept => write!(
                f,
                "the access rights of the file could not be given to the new file"
            ),
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

    /// For a file that could not be looked at when the document was opened: it equals no real
    /// state of a file, so the first save asks, and the answer makes it known.
    pub fn unknown() -> FileStamp {
        FileStamp {
            len: u64::MAX,
            modified: None,
            id: Some((u64::MAX, u64::MAX)),
        }
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

/// True for an error that says the file is in use in a way that blocks the step: a sharing or
/// lock violation on Windows.
fn is_locked(e: &io::Error) -> bool {
    #[cfg(windows)]
    {
        // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION
        matches!(e.raw_os_error(), Some(32 | 33))
    }
    #[cfg(not(windows))]
    {
        let _ = e;
        false
    }
}

/// An error of the last step, the replacing of the file. The folder and the file have been tried
/// for write access before this, so on Windows a refusal here means the file is held open in a way
/// that blocks the swap (FAT, exFAT and network drives cannot replace a file that is open), not
/// that it is read-only.
fn replace_error(e: &io::Error) -> SaveError {
    if cfg!(windows) && e.kind() == io::ErrorKind::PermissionDenied {
        return SaveError::Locked;
    }
    from_io(e, "the file")
}

/// Sorts an error of the file system into what the user can act on.
fn from_io(e: &io::Error, what: &str) -> SaveError {
    if is_disk_full(e) {
        return SaveError::DiskFull;
    }
    if is_locked(e) {
        return SaveError::Locked;
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

/// The part of a file name that the name of the new file carries: at most 100 bytes, cut on a
/// character boundary, so that the whole name stays below the limit of 255 bytes of most file
/// systems.
fn short_name(name: &str) -> &str {
    let mut end = name.len().min(100);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

fn temp_prefix(name: &str) -> String {
    format!(".{}.partridge-", short_name(name))
}

/// Creates a new file next to `target`, with a name that cannot clash with anything. `private`:
/// nobody but the owner may read it while it is written (the file replaces one that was private,
/// and gets that file's permissions at the end).
fn create_temp(target: &Path, private: bool) -> Result<(Temp, File), SaveError> {
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or(SaveError::NotAFile)?;
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
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        #[cfg(not(unix))]
        let _ = private;
        match options.open(&path) {
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
    let prefix = temp_prefix(&name.to_string_lossy());
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
        Ok(meta) => {
            // The rename needs rights on the folder, not on the file, so a file the user may not
            // write would be replaced all the same. Whether they may is found out by trying to
            // open it for writing (nothing is written or cut off).
            let handle = match OpenOptions::new().write(true).open(target) {
                Ok(handle) => handle,
                Err(e) => {
                    return Err(match e.kind() {
                        io::ErrorKind::PermissionDenied if !is_locked(&e) => SaveError::ReadOnly(
                            "you may not change this file; save a copy under another name instead"
                                .into(),
                        ),
                        _ => from_io(&e, "the file"),
                    });
                }
            };
            // A file with a second name would be split from it by the replacement: the other
            // name would keep the old content.
            if link_count(&handle, &meta).map_err(|e| from_io(&e, "the file"))? > 1 {
                return Err(SaveError::HardLink);
            }
            Ok(Some(meta))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(from_io(&e, "the file")),
    }
}

/// How many names the file has (hard links, the name used to open it included).
#[cfg(unix)]
fn link_count(_file: &File, meta: &fs::Metadata) -> io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(meta.nlink())
}

/// How many names the file has (hard links, the name used to open it included). The standard
/// library does not give the number on Windows; the system does, for an open handle.
#[cfg(windows)]
fn link_count(file: &File, _meta: &fs::Metadata) -> io::Result<u64> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    // SAFETY: the handle is open for the whole call, and the structure is plain data that the
    // call fills in.
    unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        if GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(u64::from(info.nNumberOfLinks))
    }
}

/// A link is followed, so that the file it points to is replaced and the link stays a link.
fn resolve(target: &Path) -> PathBuf {
    let mut path = target.to_path_buf();
    // A link that points at nothing is followed by hand, so that the file it names is made and the
    // link stays a link. A loop of links stops after a few steps.
    for _ in 0..16 {
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => {
                if let Ok(real) = fs::canonicalize(&path) {
                    return real;
                }
                let Ok(next) = fs::read_link(&path) else {
                    return path;
                };
                path = match path.parent() {
                    Some(dir) => dir.join(next),
                    None => next,
                };
            }
            _ => return path,
        }
    }
    path
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
    print: Option<&Fingerprint>,
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
        if let Some(print) = print {
            let seen = client
                .fingerprint(opened.doc)
                .map_err(|e| verify(format!("its pages could not be looked at ({e})")))?;
            if seen.failed > 0 {
                return Err(verify(format!(
                    "{} of its pages could not be read",
                    seen.failed
                )));
            }
            if seen.pages != print.pages || seen.digest != print.digest {
                return Err(verify(
                    "its pages (their size, text or contents) are not those of the document".into(),
                ));
            }
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
    verify: &dyn Fn(&Path, DocInfo, &Fingerprint) -> Result<(), SaveError>,
    still_open: &dyn Fn() -> bool,
) -> Result<SaveOutcome, ClientError> {
    let info = client.doc_info(doc).map_err(from_engine)?;
    if info.is_signed() && !options.allow_signed {
        return Err(SaveError::Signed {
            signatures: info.signatures.max(1),
        }
        .into());
    }
    // What the pages are like now, as the engine that holds the document sees them. The file
    // that is written must give the same.
    let print = client.fingerprint(doc).map_err(from_engine)?;
    if print.failed > 0 {
        return Err(SaveError::SourceUnreadable.into());
    }
    let requested = target;
    let target = resolve(target);
    let existing = check_target(&target)?;
    // Ask early: there is no point in writing a file that will not be used.
    check_unchanged(&target, expected, options)?;
    remove_stale_temps(&target);

    let (mut temp, mut file) = create_temp(&target, existing.is_some())?;
    if let Some(meta) = &existing {
        // The new file belongs to the same user and group as the old one, and can be reached by
        // the same people. Where that cannot be done the save stops: a file that quietly changes
        // hands, or opens to others, is worse than a save that asks for a copy.
        keep_owner(&file, meta)?;
        keep_access(&target, &temp.path)?;
    }
    let mut written: u64 = 0;
    let mut sum = Fnv::new();
    let total = client.save_to(doc, &mut |piece| {
        file.write_all(piece)?;
        sum.update(piece);
        written += piece.len() as u64;
        Ok(())
    });
    let total = match total {
        Ok(total) => total,
        Err(SaveStreamError::Sink(e)) => return Err(from_io(&e, "the new file").into()),
        Err(SaveStreamError::Client(e)) => return Err(from_engine(e)),
    };
    if written != total {
        return Err(SaveError::Write("fewer bytes were written than were sent".into()).into());
    }
    if let Some(meta) = &existing {
        // The file keeps the permissions it had.
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

    // Read the file back and compare it with what was received. The bytes were written a moment
    // ago, so the system serves them from its cache and not from the disk: this catches a short
    // or damaged write on the way to the cache, and it cannot show that the disk stored them.
    let back = digest_of_file(&temp.path).map_err(|e| from_io(&e, "the new file"))?;
    if back != sum.finish() {
        return Err(SaveError::Write("the new file is not what was written to it".into()).into());
    }

    verify(&temp.path, info, &print)?;

    // The document may have been closed while the file was checked (the user gave the changes up):
    // then nothing is replaced.
    if !still_open() {
        return Err(ClientError::Engine {
            code: ErrorCode::UnknownDocument,
            message: "the document was closed before the file was replaced".into(),
        });
    }
    // The last look at the target, as close to the replacement as it gets.
    check_unchanged(&target, expected, options)?;
    fs::rename(&temp.path, &target).map_err(|e| replace_error(&e))?;
    temp.keep = true;
    sync_folder(&target);
    Ok(SaveOutcome {
        path: requested.to_path_buf(),
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

// ---- small helpers of the save ------------------------------------------------------------------------------------

/// FNV-1a over 64 bits, to see that the bytes in the new file are the bytes that were received.
/// It is a check against a faulty write, not against an adversary.
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

fn digest_of_file(path: &Path) -> io::Result<u64> {
    use std::io::Read;
    let mut file = File::open(path)?;
    let mut sum = Fnv::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            return Ok(sum.finish());
        }
        sum.update(&buffer[..n]);
    }
}

/// An error of the engine while saving, as the save error it stands for.
fn from_engine(e: ClientError) -> ClientError {
    match e {
        ClientError::Engine {
            code: ErrorCode::SourceChanged,
            ..
        } => SaveError::SourceChanged.into(),
        ClientError::Engine {
            code: ErrorCode::SourceUnreadable,
            ..
        } => SaveError::SourceUnreadable.into(),
        other => other,
    }
}

/// What to do about the owner and the group of the file that is replaced, given the owner and the
/// group of the new file and the ones we may give it.
#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OwnerPlan {
    /// The new file is the old file's already.
    Same,
    /// Give the new file this owner (when different) and group (when different).
    Chown { uid: Option<u32>, gid: Option<u32> },
}

#[cfg(unix)]
pub(crate) fn plan_owner(old: (u32, u32), new: (u32, u32)) -> OwnerPlan {
    if old == new {
        return OwnerPlan::Same;
    }
    OwnerPlan::Chown {
        uid: (old.0 != new.0).then_some(old.0),
        gid: (old.1 != new.1).then_some(old.1),
    }
}

/// Gives the new file the owner and group of the old one, or says it cannot.
#[cfg(unix)]
fn keep_owner(file: &File, old: &fs::Metadata) -> Result<(), SaveError> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;
    let new = file.metadata().map_err(|e| from_io(&e, "the new file"))?;
    let OwnerPlan::Chown { uid, gid } = plan_owner((old.uid(), old.gid()), (new.uid(), new.gid()))
    else {
        return Ok(());
    };
    // SAFETY: a plain system call on a descriptor this function borrows; (uid_t)-1 means "leave
    // as it is".
    let done = unsafe {
        libc::fchown(
            file.as_raw_fd(),
            uid.unwrap_or(u32::MAX),
            gid.unwrap_or(u32::MAX),
        )
    };
    if done == 0 {
        Ok(())
    } else {
        Err(SaveError::OwnerWouldChange)
    }
}

#[cfg(not(unix))]
fn keep_owner(_file: &File, _old: &fs::Metadata) -> Result<(), SaveError> {
    Ok(())
}

/// A security descriptor may have no owner; such an owner is unknown, and an unknown owner cannot
/// be kept.
#[cfg(windows)]
fn owner_known(owner: *const std::ffi::c_void) -> Result<(), SaveError> {
    if owner.is_null() {
        Err(SaveError::OwnerWouldChange)
    } else {
        Ok(())
    }
}

/// Gives the new file the access rights of the old one. A new file in a folder gets the rights of
/// the folder (and its owner is the user who saves), which for a file that was shared with fewer
/// people or more is not the same.
///
/// - Windows: the owner, and the DACL with its protection from inheritance. When the owner cannot
///   be given to the new file (it takes a privilege to make someone else the owner), the save stops
///   with [`SaveError::OwnerWouldChange`].
/// - Linux: the POSIX ACL (`system.posix_acl_access`).
/// - macOS: the extended ACL.
///
/// Where the rights cannot be given, the save stops with [`SaveError::AccessNotKept`]: the caller
/// offers a copy, and nobody gains or loses access without being told.
#[cfg(windows)]
fn keep_access(old: &Path, new: &Path) -> Result<(), SaveError> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS};
    use windows_sys::Win32::Security::Authorization::{
        GetNamedSecurityInfoW, SetNamedSecurityInfoW, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        EqualSid, GetSecurityDescriptorControl, ACL, DACL_SECURITY_INFORMATION,
        OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        PSID, SE_DACL_PROTECTED, UNPROTECTED_DACL_SECURITY_INFORMATION,
    };
    let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain(Some(0)).collect() };
    let (from, to) = (wide(old), wide(new));
    let mut dacl: *mut ACL = null_mut();
    let mut owner: PSID = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let mut new_owner: PSID = null_mut();
    let mut new_descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: documented Win32 calls on paths that are valid, zero ended and live for the call;
    // the descriptors that the calls allocate are freed with LocalFree before the function ends,
    // and the owners point into them, so they are used only before that.
    unsafe {
        let status = GetNamedSecurityInfoW(
            from.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        );
        if status != ERROR_SUCCESS {
            return Err(SaveError::AccessNotKept);
        }
        let result = (|| {
            // The owner first: with it in place the user has the right to change the DACL.
            let status = GetNamedSecurityInfoW(
                to.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut new_owner,
                null_mut(),
                null_mut(),
                null_mut(),
                &mut new_descriptor,
            );
            if status != ERROR_SUCCESS {
                return Err(SaveError::AccessNotKept);
            }
            // Win32 allows a descriptor without an owner. Comparing with a null owner would
            // crash, and what cannot be told is treated as an owner that cannot be kept.
            owner_known(owner)?;
            owner_known(new_owner)?;
            if EqualSid(owner, new_owner) == 0 {
                let status = SetNamedSecurityInfoW(
                    to.as_ptr(),
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION,
                    owner,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                );
                if status != ERROR_SUCCESS {
                    return Err(SaveError::OwnerWouldChange);
                }
            }
            let mut control: u16 = 0;
            let mut revision: u32 = 0;
            let protected = GetSecurityDescriptorControl(descriptor, &mut control, &mut revision)
                != 0
                && control & SE_DACL_PROTECTED != 0;
            let flags = DACL_SECURITY_INFORMATION
                | if protected {
                    PROTECTED_DACL_SECURITY_INFORMATION
                } else {
                    UNPROTECTED_DACL_SECURITY_INFORMATION
                };
            let status = SetNamedSecurityInfoW(
                to.as_ptr(),
                SE_FILE_OBJECT,
                flags,
                null_mut(),
                null_mut(),
                dacl,
                null_mut(),
            );
            if status != ERROR_SUCCESS {
                return Err(SaveError::AccessNotKept);
            }
            Ok(())
        })();
        LocalFree(new_descriptor);
        LocalFree(descriptor);
        result
    }
}

#[cfg(target_os = "linux")]
const ACL_NAME: &[u8] = b"system.posix_acl_access\0";

/// The POSIX ACL of a file on Linux, as the system stores it; `None` for a file with only the
/// plain mode bits (or on a file system that has no ACLs).
#[cfg(target_os = "linux")]
pub(crate) fn read_acl(path: &Path) -> Result<Option<Vec<u8>>, SaveError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| SaveError::AccessNotKept)?;
    for _ in 0..4 {
        // SAFETY: a plain system call on a zero ended string; with size 0 it only tells how big
        // the value is.
        let size = unsafe {
            libc::getxattr(
                path.as_ptr(),
                ACL_NAME.as_ptr().cast(),
                std::ptr::null_mut(),
                0,
            )
        };
        if size < 0 {
            return match io::Error::last_os_error().raw_os_error() {
                Some(libc::ENODATA | libc::ENOTSUP) => Ok(None),
                _ => Err(SaveError::AccessNotKept),
            };
        }
        let mut value = vec![0u8; size as usize];
        // SAFETY: as above, with a buffer of the size the system gave.
        let got = unsafe {
            libc::getxattr(
                path.as_ptr(),
                ACL_NAME.as_ptr().cast(),
                value.as_mut_ptr().cast(),
                value.len(),
            )
        };
        if got >= 0 {
            value.truncate(got as usize);
            return Ok(Some(value));
        }
        // The value grew between the two calls: ask again.
        if io::Error::last_os_error().raw_os_error() != Some(libc::ERANGE) {
            return Err(SaveError::AccessNotKept);
        }
    }
    Err(SaveError::AccessNotKept)
}

#[cfg(target_os = "linux")]
fn keep_access(old: &Path, new: &Path) -> Result<(), SaveError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let Some(value) = read_acl(old)? else {
        return Ok(());
    };
    let to = CString::new(new.as_os_str().as_bytes()).map_err(|_| SaveError::AccessNotKept)?;
    // SAFETY: a plain system call on a zero ended string and a buffer that lives for the call.
    let done = unsafe {
        libc::setxattr(
            to.as_ptr(),
            ACL_NAME.as_ptr().cast(),
            value.as_ptr().cast(),
            value.len(),
            0,
        )
    };
    if done == 0 {
        Ok(())
    } else {
        Err(SaveError::AccessNotKept)
    }
}

/// The calls of macOS for access control lists (`<sys/acl.h>`).
#[cfg(target_os = "macos")]
mod macos_acl {
    use std::ffi::{c_char, c_int, c_void};
    /// The list that macOS shows with `ls -e`.
    pub const ACL_TYPE_EXTENDED: c_int = 0x100;
    extern "C" {
        pub fn acl_get_file(path: *const c_char, kind: c_int) -> *mut c_void;
        pub fn acl_set_file(path: *const c_char, kind: c_int, acl: *mut c_void) -> c_int;
        pub fn acl_free(object: *mut c_void) -> c_int;
    }
}

/// True when the file has an extended ACL (an entry beyond the mode bits) on macOS.
#[cfg(all(target_os = "macos", test))]
pub(crate) fn has_acl(path: &Path) -> Result<bool, SaveError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| SaveError::AccessNotKept)?;
    // SAFETY: a plain call on a zero ended string; the list it returns is freed again.
    unsafe {
        let acl = macos_acl::acl_get_file(path.as_ptr(), macos_acl::ACL_TYPE_EXTENDED);
        if acl.is_null() {
            return match io::Error::last_os_error().raw_os_error() {
                // No list on this file, or a file system that has none.
                Some(libc::ENOENT | libc::ENOTSUP) => Ok(false),
                _ => Err(SaveError::AccessNotKept),
            };
        }
        macos_acl::acl_free(acl);
        Ok(true)
    }
}

#[cfg(target_os = "macos")]
fn keep_access(old: &Path, new: &Path) -> Result<(), SaveError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let from = CString::new(old.as_os_str().as_bytes()).map_err(|_| SaveError::AccessNotKept)?;
    let to = CString::new(new.as_os_str().as_bytes()).map_err(|_| SaveError::AccessNotKept)?;
    // SAFETY: plain calls on zero ended strings; the list is freed again on every path.
    unsafe {
        let acl = macos_acl::acl_get_file(from.as_ptr(), macos_acl::ACL_TYPE_EXTENDED);
        if acl.is_null() {
            return match io::Error::last_os_error().raw_os_error() {
                Some(libc::ENOENT | libc::ENOTSUP) => Ok(()),
                _ => Err(SaveError::AccessNotKept),
            };
        }
        let done = macos_acl::acl_set_file(to.as_ptr(), macos_acl::ACL_TYPE_EXTENDED, acl);
        macos_acl::acl_free(acl);
        if done == 0 {
            Ok(())
        } else {
            Err(SaveError::AccessNotKept)
        }
    }
}

/// Other systems have no list that this code knows how to read, so there is nothing to keep.
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn keep_access(_old: &Path, _new: &Path) -> Result<(), SaveError> {
    Ok(())
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
            SaveError::SourceChanged,
            SaveError::SourceUnreadable,
            SaveError::Locked,
            SaveError::HardLink,
            SaveError::OwnerWouldChange,
            SaveError::AccessNotKept,
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

    fn accept(_: &Path, _: DocInfo, _: &Fingerprint) -> Result<(), SaveError> {
        Ok(())
    }

    fn save(
        client: &EngineClient,
        doc: u64,
        target: &Path,
        expected: Option<&FileStamp>,
        options: SaveOptions,
    ) -> Result<SaveOutcome, ClientError> {
        save_document(client, doc, target, expected, options, &accept, &|| true)
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
                &|file, _, _| {
                    // The new file exists and holds all the bytes when it is checked.
                    assert_eq!(fs::metadata(file).unwrap().len() as usize, FAKE_SIZE);
                    Err(refusal.clone())
                },
                &|| true,
            );
            assert_eq!(save_error(result), refusal);
            assert_untouched(&dir, &path);
        }
    }

    #[test]
    fn a_document_that_was_closed_during_the_check_is_not_saved() {
        let dir = Dir::new("closed");
        let path = original_in(&dir);
        let (client, doc) = client("save-good");
        let result = save_document(
            &client,
            doc,
            &path,
            None,
            SaveOptions::default(),
            &accept,
            &|| false,
        );
        match result {
            Err(ClientError::Engine { code, .. }) => assert_eq!(code, ErrorCode::UnknownDocument),
            other => panic!("{other:?}"),
        }
        assert_untouched(&dir, &path);
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
            &|_, info, print| {
                assert_eq!(
                    info,
                    DocInfo {
                        pages: 1,
                        security: 0,
                        signatures: 0,
                        maybe_signed: false
                    }
                );
                assert_eq!(print.pages, 1);
                Ok(())
            },
            &|| true,
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
            &move |_, _, _| {
                fs::write(&during, b"changed during the check, and longer").unwrap();
                Ok(())
            },
            &|| true,
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

    #[cfg(unix)]
    #[test]
    fn the_owner_and_group_of_the_old_file_are_asked_for_only_where_they_differ() {
        assert_eq!(plan_owner((10, 20), (10, 20)), OwnerPlan::Same);
        assert_eq!(
            plan_owner((10, 20), (10, 99)),
            OwnerPlan::Chown {
                uid: None,
                gid: Some(20)
            }
        );
        assert_eq!(
            plan_owner((10, 20), (11, 20)),
            OwnerPlan::Chown {
                uid: Some(10),
                gid: None
            }
        );
        assert_eq!(
            plan_owner((10, 20), (11, 21)),
            OwnerPlan::Chown {
                uid: Some(10),
                gid: Some(20)
            }
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_descriptor_without_an_owner_is_an_owner_that_cannot_be_kept() {
        assert_eq!(
            owner_known(std::ptr::null()),
            Err(SaveError::OwnerWouldChange)
        );
        let sid = 1u8;
        assert_eq!(owner_known((&sid as *const u8).cast()), Ok(()));
    }

    #[cfg(unix)]
    #[test]
    fn the_new_file_can_be_read_by_nobody_else_while_it_is_written() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Dir::new("private");
        let path = original_in(&dir);
        let (temp, _file) = create_temp(&path, true).unwrap();
        let mode = fs::metadata(&temp.path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode & 0o077, 0, "mode {mode:o}");
    }

    #[test]
    fn a_long_name_with_wide_characters_still_makes_a_name_the_file_system_takes() {
        let dir = Dir::new("long");
        // 120 characters of 2 bytes each: 240 bytes, near the limit of most file systems.
        let name = format!("{}.pdf", "\u{e9}".repeat(120));
        let path = dir.file(&name);
        fs::write(&path, ORIGINAL).unwrap();
        assert!(short_name(&name).len() <= 100);
        assert!(name.starts_with(short_name(&name)));
        let (client, doc) = client("save-good");
        save(&client, doc, &path, None, SaveOptions::default()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), fake_bytes());
        assert_eq!(dir.names(), vec![name]);
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
                &|_, _, _| std::process::abort(),
                &|| true,
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
    /// room for less than the 2.5 MB the stand-in saves. It is ignored by default (the default run
    /// has no such disk) and run on purpose with `--ignored`: CI mounts a small tmpfs for it on
    /// Linux, and fails if the variable is missing.
    #[test]
    #[ignore = "needs a small file system: PARTRIDGE_TEST_SMALL_DISK (run in CI with --ignored)"]
    fn a_full_disk_stops_the_save_and_leaves_the_file_as_it_was() {
        let small = std::env::var_os("PARTRIDGE_TEST_SMALL_DISK")
            .expect("PARTRIDGE_TEST_SMALL_DISK names a folder on a small file system");
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

    // ---- who may open the file (N3) -------------------------------------------------------

    #[test]
    fn a_file_with_two_names_is_not_saved_in_place_on_any_system() {
        let dir = Dir::new("twonames");
        let path = original_in(&dir);
        let other = dir.file("other-name.pdf");
        fs::hard_link(&path, &other).unwrap();
        let (client, doc) = client("save-good");
        let err = save_error(save(&client, doc, &path, None, SaveOptions::default()));
        assert_eq!(err, SaveError::HardLink);
        // Both names still have the file as it was, and nothing was left behind.
        assert_eq!(fs::read(&path).unwrap(), ORIGINAL);
        assert_eq!(fs::read(&other).unwrap(), ORIGINAL);
        assert_eq!(
            dir.names(),
            vec!["other-name.pdf".to_string(), "report.pdf".to_string()]
        );
    }

    /// The bytes of a POSIX ACL as Linux stores them: version 2, then entries of tag, permission
    /// and id. User 1234 may read and write, the group and the mask may read and write.
    #[cfg(target_os = "linux")]
    fn sample_acl() -> Vec<u8> {
        let mut value = 2u32.to_le_bytes().to_vec();
        for (tag, perm, id) in [
            (0x01u16, 6u16, u32::MAX), // the owner: read, write
            (0x02, 6, 1234),           // user 1234: read, write
            (0x04, 4, u32::MAX),       // the group: read
            (0x10, 6, u32::MAX),       // the mask
            (0x20, 0, u32::MAX),       // others: nothing
        ] {
            value.extend_from_slice(&tag.to_le_bytes());
            value.extend_from_slice(&perm.to_le_bytes());
            value.extend_from_slice(&id.to_le_bytes());
        }
        value
    }

    /// Puts the ACL on a file. False when the file system of the test folder has none.
    #[cfg(target_os = "linux")]
    fn set_acl(path: &Path, value: &[u8]) -> bool {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let path = CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: a plain system call on a zero ended string and a buffer that lives for the call.
        let done = unsafe {
            libc::setxattr(
                path.as_ptr(),
                ACL_NAME.as_ptr().cast(),
                value.as_ptr().cast(),
                value.len(),
                0,
            )
        };
        done == 0
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn n3_a_posix_acl_is_kept_by_a_save() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Dir::new("acl");
        let path = original_in(&dir);
        if !set_acl(&path, &sample_acl()) {
            // The file system of the temporary folder has no ACLs; nothing to see here.
            eprintln!("skipped: no ACLs on this file system");
            return;
        }
        let before = read_acl(&path).unwrap().expect("the ACL was set");
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let (client, doc) = client("save-good");
        save(&client, doc, &path, None, SaveOptions::default()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), fake_bytes());
        assert_eq!(
            read_acl(&path).unwrap().as_deref(),
            Some(&before[..]),
            "carol lost her access"
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            mode
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn n3_a_file_with_only_mode_bits_gets_no_acl() {
        let dir = Dir::new("noacl");
        let path = original_in(&dir);
        let (client, doc) = client("save-good");
        save(&client, doc, &path, None, SaveOptions::default()).unwrap();
        assert_eq!(read_acl(&path).unwrap(), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn n3_an_extended_acl_is_kept_by_a_save() {
        let dir = Dir::new("macacl");
        let path = original_in(&dir);
        let added = std::process::Command::new("chmod")
            .args(["+a", "everyone allow read"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(added.success(), "chmod +a failed");
        assert!(has_acl(&path).unwrap());
        let (client, doc) = client("save-good");
        save(&client, doc, &path, None, SaveOptions::default()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), fake_bytes());
        assert!(has_acl(&path).unwrap(), "the ACL was lost");
    }

    /// What `icacls` says about a file, without the path and sorted, so that two files can be
    /// compared. `(I)` marks a right that comes from the folder.
    #[cfg(windows)]
    fn rights_of(path: &Path) -> Vec<String> {
        let out = std::process::Command::new("icacls")
            .arg(path)
            .output()
            .unwrap();
        assert!(out.status.success(), "icacls failed");
        let shown = path.to_string_lossy().into_owned();
        let mut lines: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.replace(&shown, "").trim().to_string())
            .filter(|l| !l.is_empty() && !l.starts_with("Successfully processed"))
            .collect();
        lines.sort();
        lines
    }

    /// A folder whose rights pass to the files made in it (all users may read them), like the
    /// folders of a shared drive. Without it the folder of a test run hands down nothing, and a
    /// file made there has only the default rights of the user's token, whether or not the rights
    /// are protected from inheritance.
    #[cfg(windows)]
    fn share_with_users(dir: &Dir) {
        assert!(
            icacls(&dir.0, &["/grant", "*S-1-5-32-545:(OI)(CI)(R)"]),
            "grant on the folder"
        );
    }

    #[cfg(windows)]
    fn icacls(path: &Path, args: &[&str]) -> bool {
        std::process::Command::new("icacls")
            .arg(path)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// The owner of a file as `dir /q` prints it.
    #[cfg(windows)]
    fn owner_line(path: &Path) -> String {
        let out = std::process::Command::new("cmd")
            .args(["/c", "dir", "/q"])
            .arg(path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .find(|l| l.contains("report.pdf"))
            .unwrap_or("")
            .to_string()
    }

    #[cfg(windows)]
    #[test]
    fn n3_a_protected_dacl_is_kept_with_its_protection() {
        let dir = Dir::new("dacl");
        share_with_users(&dir);
        let path = original_in(&dir);
        let user = std::env::var("USERNAME").expect("the user's name");
        // The folder hands its rights down: a new file there starts with them.
        let probe = dir.file("probe.pdf");
        fs::write(&probe, ORIGINAL).unwrap();
        assert!(
            rights_of(&probe).iter().any(|l| l.contains("(I)")),
            "the folder passes nothing down: {:?}",
            rights_of(&probe)
        );
        fs::remove_file(&probe).unwrap();
        // Cut off from the folder's rights, with a short list of its own: the user, the system,
        // and the group of all users (read only).
        assert!(icacls(&path, &["/inheritance:r"]), "inheritance:r");
        assert!(
            icacls(
                &path,
                &[
                    "/grant",
                    &format!("{user}:(F)"),
                    "/grant",
                    "*S-1-5-18:(R)",
                    "/grant",
                    "*S-1-5-32-545:(R)",
                ]
            ),
            "grant"
        );
        let before = rights_of(&path);
        assert!(
            before.iter().all(|l| !l.contains("(I)")),
            "the test file inherits: {before:?}"
        );
        assert!(before.len() >= 3, "{before:?}");
        let (client, doc) = client("save-good");
        save(&client, doc, &path, None, SaveOptions::default()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), fake_bytes());
        // The new file has the same list, and none of the rights the folder would have given it.
        assert_eq!(rights_of(&path), before);
    }

    #[cfg(windows)]
    #[test]
    fn n3_an_unprotected_dacl_still_follows_the_folder() {
        let dir = Dir::new("dacl-open");
        share_with_users(&dir);
        let path = original_in(&dir);
        let before = rights_of(&path);
        assert!(
            before.iter().any(|l| l.contains("(I)")),
            "the test file does not inherit: {before:?}"
        );
        let (client, doc) = client("save-good");
        save(&client, doc, &path, None, SaveOptions::default()).unwrap();
        assert_eq!(rights_of(&path), before);
    }

    #[cfg(windows)]
    #[test]
    fn n3_the_owner_of_a_file_is_kept_or_the_save_stops() {
        let dir = Dir::new("owner");
        let path = original_in(&dir);
        let user = std::env::var("USERNAME")
            .expect("the user's name")
            .to_lowercase();
        let owned_by_user = |p: &Path| owner_line(p).to_lowercase().contains(&user);
        // A new file is owned by whoever the system makes the owner by default. If that is the
        // user already, or the system does not let the user be made the owner, there is no change
        // of owner to look at.
        if owned_by_user(&path) {
            eprintln!("skipped: the user owns new files already");
            return;
        }
        if !icacls(&path, &["/setowner", &user]) || !owned_by_user(&path) {
            eprintln!("skipped: the owner could not be set");
            return;
        }
        let (client, doc) = client("save-good");
        match save(&client, doc, &path, None, SaveOptions::default()) {
            Ok(_) => {
                // The file was replaced: the new file has the owner of the old one.
                assert_eq!(fs::read(&path).unwrap(), fake_bytes());
                assert!(owned_by_user(&path), "the file changed hands");
            }
            Err(e) => {
                // The owner could not be given to the new file: the save stopped before it wrote
                // anything, and says why.
                assert_eq!(
                    save_error(Err::<SaveOutcome, _>(e)),
                    SaveError::OwnerWouldChange
                );
                assert_eq!(fs::read(&path).unwrap(), ORIGINAL);
                assert_eq!(dir.names(), vec!["report.pdf".to_string()]);
            }
        }
    }
}
