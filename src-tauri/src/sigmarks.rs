//! Pictures of signatures that the reader keeps to use again, and the commands that put a picture
//! on a page.
//!
//! The pictures live in one small JSON file in the app's local data folder on this computer (on
//! Windows that is Local AppData, which does not roam with a profile and is not redirected to a
//! server the way Roaming AppData can be), kept much the way `positions.rs` keeps its file: the
//! folder and the file are for the user alone on Unix, and "Forget saved signatures" in the
//! settings menu empties it. Nothing is sent anywhere. Anyone who can read the folder can use the
//! pictures: they are pictures, and prove nothing about who placed them.
//!
//! An earlier version kept the file in the app's data folder (Roaming AppData on Windows). At every
//! start the old file is moved to the new place; when the new place has a file already, the
//! pictures of the old file that the new one lacks are added to it, and the old file is removed
//! only when all of its pictures are in the new one. An old file that cannot be read, or whose
//! pictures do not all fit, stays where it is and is tried again at the next start; while the new
//! place cannot be used, the pictures of the old file are the list for the run (and are written to
//! the new place with the first change). "Forget" deletes the old file too.
//!
//! Another program may change the file while the app runs. Before every change the app looks at
//! the file again: when its size or its time has changed since the app last read or wrote it, the
//! list is read from the file first, so a change made elsewhere is built on and not overwritten.
//! A file that cannot be read as a list (damaged, cut short, too big, from another version) is
//! never written over: the list starts empty, or stays as it was when the file went bad while the
//! app ran, and the first change moves the bad file aside as `signatures.json.bad` (or the next
//! free `.bad.1` to `.bad.9`, never over one that is there; when all are taken nothing is written)
//! before it writes the new one. "Forget" deletes those files too.
//!
//! The file is user data on a disk that other programs can write, so everything in it is checked
//! again when it is read, with the same caps as a picture that is sent to the engine. An imported
//! image is kept as the PNG the window made of it, as text; this process never decodes it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use partridge_engine_client::{ClientError, SignatureFont, SignatureMark};
use partridge_protocol::{mark_image_size_ok, mark_text_ok, ErrorCode};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::annotations::AnnotationInfo;
use crate::positions::{create_private_dir, read_plain_file};
use crate::updates::{self, UpdateState};
use crate::{blocking, CommandError, Engine};

/// Most pictures kept.
pub const MAX_MARKS: usize = 20;

/// Most characters of the name of a kept picture.
pub const MAX_NAME_CHARS: usize = 60;

/// Most characters of the text (base64) of a kept image: about 400 KB of PNG.
pub const MAX_PNG_BASE64: usize = 540_000;

/// Largest file read back. Twenty pictures at the most, each far below the cap of an image.
const MAX_FILE_BYTES: u64 = (MAX_MARKS * (MAX_PNG_BASE64 + 4_096) + 65_536) as u64;

/// The part of a PNG file that holds its size: the eight byte signature, the length and the name of
/// the header chunk, and then the width and the height. In base64 these 24 bytes are the first 32
/// characters.
const PNG_HEAD_BYTES: usize = 24;
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// The value of one character of the base64 alphabet.
fn base64_value(c: u8) -> Option<u32> {
    match c {
        b'A'..=b'Z' => Some(u32::from(c - b'A')),
        b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
        b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// The width and the height a PNG declares in its header chunk, from the start of the base64 text
/// of the file. `None` when the text does not start like a PNG. Nothing is decoded: the window
/// decodes the picture, and it must not be asked to make one that is far bigger than the size that
/// was stored beside it.
pub fn png_text_size(png: &str) -> Option<(u32, u32)> {
    let text = png.as_bytes().get(..PNG_HEAD_BYTES / 3 * 4)?;
    let mut head = [0u8; PNG_HEAD_BYTES];
    for (group, out) in text.chunks(4).zip(head.chunks_mut(3)) {
        let mut value = 0u32;
        for c in group {
            value = (value << 6) | base64_value(*c)?;
        }
        out.copy_from_slice(&value.to_be_bytes()[1..]);
    }
    if head[..8] != PNG_SIGNATURE || head[8..12] != [0, 0, 0, 13] || &head[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes([head[16], head[17], head[18], head[19]]);
    let height = u32::from_be_bytes([head[20], head[21], head[22], head[23]]);
    Some((width, height))
}

/// What a kept picture is made of. The window makes these three, and the same shapes go between
/// the window and this file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MarkBody {
    /// Lines in a box of `width` by `height` units, drawn with a line of `line` units.
    Draw {
        width: f32,
        height: f32,
        line: f32,
        strokes: Vec<Vec<[f32; 2]>>,
    },
    /// Text set in one of the bundled fonts, by the name of the font.
    Type { font: String, text: String },
    /// A picture the person imported, as a PNG in base64.
    Image {
        width: u32,
        height: u32,
        png: String,
    },
}

impl MarkBody {
    /// The same picture when it is within every cap, and nothing otherwise.
    fn checked(self) -> Option<MarkBody> {
        let ok = match &self {
            MarkBody::Draw {
                width,
                height,
                line,
                strokes,
            } => SignatureMark::Strokes {
                width: *width,
                height: *height,
                line: *line,
                strokes: strokes.clone(),
            }
            .within_caps(),
            MarkBody::Type { font, text } => {
                SignatureFont::from_name(font).is_some() && mark_text_ok(text)
            }
            MarkBody::Image { width, height, png } => {
                mark_image_size_ok(*width, *height)
                    && png.len() <= MAX_PNG_BASE64
                    && is_base64(png)
                    // The size in the header of the PNG is the size stored beside it.
                    && png_text_size(png) == Some((*width, *height))
            }
        };
        ok.then_some(self)
    }

    /// The picture as the engine takes it, for a drawn or a typed one. An image goes to the engine
    /// as pixels, through `add_signature_image`.
    pub fn to_mark(&self) -> Option<SignatureMark> {
        match self {
            MarkBody::Draw {
                width,
                height,
                line,
                strokes,
            } => Some(SignatureMark::Strokes {
                width: *width,
                height: *height,
                line: *line,
                strokes: strokes.clone(),
            }),
            MarkBody::Type { font, text } => Some(SignatureMark::Typed {
                font: SignatureFont::from_name(font)?,
                text: text.clone(),
            }),
            MarkBody::Image { .. } => None,
        }
    }
}

/// True for text of the base64 alphabet, with the padding only at the end.
fn is_base64(text: &str) -> bool {
    let body = text.trim_end_matches('=');
    text.len() - body.len() <= 2
        && text.len().is_multiple_of(4)
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

/// A kept picture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredMark {
    pub id: String,
    pub name: String,
    /// When it was kept, in seconds since 1970. Only used to order the list.
    pub created: u64,
    #[serde(flatten)]
    pub body: MarkBody,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Stored {
    version: u32,
    marks: Vec<StoredMark>,
}

/// Why the list could not be changed.
#[derive(Debug)]
pub enum StoreError {
    /// The name is empty, too long, or holds control characters.
    Name,
    /// The picture is outside the caps.
    Picture,
    /// There are as many pictures as the list holds.
    Full,
    /// There is no picture with that id.
    Missing,
    /// The file could not be written; the list is as it was.
    Write(io::Error),
}

/// A name made fit to show: trimmed, one line, within the cap. `None` for an empty name.
pub fn clean_name(name: &str) -> Option<String> {
    let name = name.trim();
    let count = name.chars().count();
    if count == 0 || count > MAX_NAME_CHARS || name.chars().any(|c| c.is_control()) {
        return None;
    }
    Some(name.to_string())
}

fn id_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The pictures kept for reuse.
pub struct SignatureMarks {
    file: Option<PathBuf>,
    marks: Vec<StoredMark>,
    /// The size and the time of change of the file when the app last read or wrote it.
    seen: Option<FileStamp>,
    /// Where an earlier version kept the file, when that is another place.
    earlier: Option<PathBuf>,
    /// The file is there and could not be read as a list, so it is moved aside before it is replaced.
    unreadable: bool,
    /// The list is that of the earlier file, which has not been written to the file yet. Once it is,
    /// the earlier file goes, so that a picture the person deletes meanwhile does not come back.
    adopted: bool,
}

/// What reading the file gave.
enum Loaded {
    /// A list (empty for a file that is not there).
    List(Vec<StoredMark>),
    /// A file that could not be read as a list.
    Unreadable,
}

/// What tells that a file was changed by someone else: its size and its time of change.
type FileStamp = (u64, SystemTime);

fn stamp_of(file: &Path) -> Option<FileStamp> {
    let meta = fs::metadata(file).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

impl SignatureMarks {
    /// Loads the list from `file`. With no file the list lives only as long as the app.
    pub fn load(file: Option<PathBuf>) -> SignatureMarks {
        let seen = file.as_deref().and_then(stamp_of);
        let (marks, unreadable) = match file.as_deref().map(read_marks) {
            Some(Loaded::List(list)) => (list, false),
            Some(Loaded::Unreadable) => (Vec::new(), true),
            None => (Vec::new(), false),
        };
        SignatureMarks {
            file,
            marks,
            seen,
            earlier: None,
            unreadable,
            adopted: false,
        }
    }

    /// As [`SignatureMarks::load`], after the file of an earlier version at `earlier` has been
    /// moved to `file` (see [`move_earlier_file`]). When that move did not work, the pictures of
    /// the old file are the list for this run, and the first change writes them to `file`.
    pub fn load_moving(file: Option<PathBuf>, earlier: Option<PathBuf>) -> SignatureMarks {
        let earlier = earlier.filter(|old| Some(old) != file.as_ref());
        if let (Some(to), Some(from)) = (&file, &earlier) {
            move_earlier_file(from, to);
        }
        let mut store = SignatureMarks::load(file);
        store.earlier = earlier;
        store.take_in_earlier();
        store
    }

    /// Brings in the pictures of the earlier file, when there is one that can be read. Where the new
    /// file cannot be used (it is not there because the move did not work, or it cannot be read as
    /// a list) they are the list for this run. Otherwise those the new file lacks are added to it,
    /// and the earlier file is removed once all of its pictures are in the new one. A file that
    /// cannot be read, or whose pictures do not all fit, is left where it is for the next start.
    fn take_in_earlier(&mut self) {
        let (Some(to), Some(from)) = (self.file.clone(), self.earlier.clone()) else {
            return;
        };
        if fs::symlink_metadata(&from).is_err() {
            return;
        }
        let Loaded::List(old) = read_marks(&from) else {
            return;
        };
        if self.unreadable || fs::symlink_metadata(&to).is_err() {
            self.marks = old;
            self.adopted = true;
            return;
        }
        let missing: Vec<StoredMark> = old
            .into_iter()
            .filter(|m| self.marks.iter().all(|k| k.id != m.id))
            .collect();
        if !missing.is_empty() {
            if self.marks.len() + missing.len() > MAX_MARKS {
                return;
            }
            let before = self.marks.clone();
            let mut merged = missing;
            merged.append(&mut self.marks);
            self.marks = merged;
            if self.save().is_err() {
                self.marks = before;
                return;
            }
        }
        let _ = fs::remove_file(&from);
    }

    /// Reads the list again when the file is not the one the app last saw. A file that is gone
    /// leaves the list as it is: the next change writes it again. A file that cannot be read as a
    /// list leaves the list as it is too, and is not written over (see [`SignatureMarks::save`]).
    fn refresh(&mut self) {
        let Some(file) = &self.file else {
            return;
        };
        let now = stamp_of(file);
        if now.is_some() && now != self.seen {
            match read_marks(file) {
                Loaded::List(list) => {
                    self.marks = list;
                    self.unreadable = false;
                }
                Loaded::Unreadable => self.unreadable = true,
            }
            self.seen = now;
        }
    }

    pub fn list(&mut self) -> Vec<StoredMark> {
        self.refresh();
        self.marks.clone()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.marks.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    /// Keeps a new picture under a name of the person's choosing.
    pub fn add(&mut self, name: &str, body: MarkBody, now: u64) -> Result<StoredMark, StoreError> {
        let name = clean_name(name).ok_or(StoreError::Name)?;
        let body = body.checked().ok_or(StoreError::Picture)?;
        self.refresh();
        if self.marks.len() >= MAX_MARKS {
            return Err(StoreError::Full);
        }
        let mut n = self.marks.len() as u64;
        let id = loop {
            let id = format!("m{now:x}-{n:x}");
            if self.marks.iter().all(|m| m.id != id) {
                break id;
            }
            n += 1;
        };
        let mark = StoredMark {
            id,
            name,
            created: now,
            body,
        };
        let before = self.marks.clone();
        self.marks.push(mark.clone());
        self.keep(before)?;
        Ok(mark)
    }

    pub fn rename(&mut self, id: &str, name: &str) -> Result<StoredMark, StoreError> {
        let name = clean_name(name).ok_or(StoreError::Name)?;
        self.refresh();
        let before = self.marks.clone();
        let mark = self
            .marks
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or(StoreError::Missing)?;
        mark.name = name;
        let renamed = mark.clone();
        self.keep(before)?;
        Ok(renamed)
    }

    pub fn delete(&mut self, id: &str) -> Result<(), StoreError> {
        self.refresh();
        let before = self.marks.clone();
        let at = self
            .marks
            .iter()
            .position(|m| m.id == id)
            .ok_or(StoreError::Missing)?;
        self.marks.remove(at);
        self.keep(before)
    }

    /// Forgets every picture, in the app and in the file, and deletes the file of an earlier
    /// version and a file that was moved aside because it could not be read.
    pub fn clear(&mut self) -> Result<(), StoreError> {
        let before = std::mem::take(&mut self.marks);
        // The bad file is deleted below, so it need not be kept aside first.
        let was_unreadable = std::mem::replace(&mut self.unreadable, false);
        if let Err(e) = self.keep(before) {
            self.unreadable = was_unreadable;
            return Err(e);
        }
        let Some(file) = &self.file else {
            return Ok(());
        };
        let mut gone: Vec<PathBuf> = (0..ASIDE_SLOTS)
            .map(|slot| aside_name(file, slot))
            .collect();
        gone.extend(self.earlier.clone());
        for old in gone {
            match fs::remove_file(&old) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(StoreError::Write(e)),
            }
        }
        Ok(())
    }

    /// Writes the list; when that fails the list goes back to `before`.
    fn keep(&mut self, before: Vec<StoredMark>) -> Result<(), StoreError> {
        match self.save() {
            Ok(()) => Ok(()),
            Err(e) => {
                self.marks = before;
                Err(StoreError::Write(e))
            }
        }
    }

    fn save(&mut self) -> io::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        if let Some(dir) = file.parent() {
            create_private_dir(dir)?;
        }
        let stored = Stored {
            version: 1,
            marks: self.marks.clone(),
        };
        let json = serde_json::to_vec(&stored).map_err(io::Error::other)?;
        // A file that could not be read as a list is not written over: it is moved aside first, and
        // when that cannot be done nothing is written.
        if self.unreadable && fs::symlink_metadata(file).is_ok_and(|m| m.is_file()) {
            let aside = (0..ASIDE_SLOTS)
                .map(|slot| aside_name(file, slot))
                .find(|name| fs::symlink_metadata(name).is_err())
                .ok_or_else(|| io::Error::other("too many unreadable files are set aside"))?;
            fs::rename(file, aside)?;
        }
        write_replacing(file, &json)?;
        self.unreadable = false;
        self.seen = stamp_of(file);
        // The list now holds what the earlier file held, with the changes made since: the earlier
        // file has nothing more to give, and a picture that was deleted must not come back from it.
        if self.adopted {
            let gone = match &self.earlier {
                Some(old) => match fs::remove_file(old) {
                    Ok(()) => true,
                    Err(e) => e.kind() == io::ErrorKind::NotFound,
                },
                None => true,
            };
            self.adopted = !gone;
        }
        Ok(())
    }
}

/// How many files that could not be read are kept at most (see [`aside_name`]).
const ASIDE_SLOTS: u32 = 10;

/// Where a file that could not be read is kept when the list is written anew: `.bad`, then
/// `.bad.1` and so on. An earlier one is never written over.
fn aside_name(file: &Path, slot: u32) -> PathBuf {
    if slot == 0 {
        file.with_extension("json.bad")
    } else {
        file.with_extension(format!("json.bad.{slot}"))
    }
}

#[cfg(test)]
fn aside_of(file: &Path) -> PathBuf {
    aside_name(file, 0)
}

/// Writes `bytes` to `file` so that a crash or a failure leaves either the old file or the new
/// one: they go to a new file beside it, which is flushed to the disk before it is moved into
/// place, and which is removed again when anything fails. A temporary file left over from an
/// earlier crash is removed first, so the new one is made fresh (for the user alone on Unix).
fn write_replacing(file: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = file.with_extension("json.tmp");
    match fs::remove_file(&temp) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let done = write_flushed(&temp, bytes).and_then(|()| fs::rename(&temp, file));
    if done.is_err() {
        let _ = fs::remove_file(&temp);
        return done;
    }
    // The move itself is flushed too, where a folder can be.
    #[cfg(unix)]
    if let Some(dir) = file.parent() {
        if let Ok(dir) = fs::File::open(dir) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

/// Makes a new file that, on Unix, only the user can read, writes `bytes` to it and flushes it to
/// the disk. It fails when the file is there already.
fn write_flushed(file: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut out = options.open(file)?;
    out.write_all(bytes)?;
    out.sync_all()
}

/// Moves the file of an earlier version to its place in the local data folder. Before, the file
/// lived in the app's data folder, which on Windows is Roaming AppData and can be copied to a
/// server with the profile. It is tried at every start, so a move that did not work is made later.
/// Nothing happens when `from` and `to` are the same file, when there is no earlier file, or when
/// the new place has something already (see [`SignatureMarks::load_moving`] for that case). The
/// old file is removed only after the new one is written and flushed.
pub fn move_earlier_file(from: &Path, to: &Path) {
    if from == to || fs::symlink_metadata(to).is_ok() {
        return;
    }
    let Some(bytes) = read_plain_file(from, MAX_FILE_BYTES) else {
        return;
    };
    let Some(dir) = to.parent() else {
        return;
    };
    if create_private_dir(dir).is_err() || write_replacing(to, &bytes).is_err() {
        return;
    }
    let _ = fs::remove_file(from);
}

/// Reads the list from the file. A file that is not there is an empty list; a file that is there
/// and cannot be read as a list of the right version is unreadable.
fn read_marks(file: &std::path::Path) -> Loaded {
    if fs::symlink_metadata(file).is_err() {
        return Loaded::List(Vec::new());
    }
    let Some(bytes) = read_plain_file(file, MAX_FILE_BYTES) else {
        return Loaded::Unreadable;
    };
    let Ok(stored) = serde_json::from_slice::<Stored>(&bytes) else {
        return Loaded::Unreadable;
    };
    if stored.version != 1 {
        return Loaded::Unreadable;
    }
    let mut out: Vec<StoredMark> = Vec::new();
    for mut mark in stored.marks {
        // What is not a picture within the caps is left out, one by one.
        let Some(name) = clean_name(&mark.name) else {
            continue;
        };
        if !id_ok(&mark.id) || out.iter().any(|m| m.id == mark.id) {
            continue;
        }
        let Some(body) = mark.body.clone().checked() else {
            continue;
        };
        mark.name = name;
        mark.body = body;
        out.push(mark);
        if out.len() >= MAX_MARKS {
            break;
        }
    }
    Loaded::List(out)
}

/// The pictures kept, behind a lock.
pub struct SignatureMarkStore(pub Mutex<SignatureMarks>);

fn error(code: &str, message: &str) -> CommandError {
    CommandError {
        code: code.into(),
        message: message.into(),
    }
}

impl From<StoreError> for CommandError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Name => error(
                "invalid_request",
                "A name has one to sixty characters on one line.",
            ),
            StoreError::Picture => error(
                "invalid_request",
                "That picture is outside the limits for a saved signature.",
            ),
            StoreError::Full => error(
                "signature_store_full",
                "As many signatures are saved as can be. Delete one first.",
            ),
            StoreError::Missing => error(
                "invalid_request",
                "That saved signature is not there any more.",
            ),
            StoreError::Write(e) => error(
                "signature_store_write",
                &format!(
                    "The saved signatures could not be written to this computer ({}).",
                    e.kind()
                ),
            ),
        }
    }
}

fn lock(store: &SignatureMarkStore) -> std::sync::MutexGuard<'_, SignatureMarks> {
    store.0.lock().unwrap_or_else(|e| e.into_inner())
}

/// The saved signatures, oldest first.
#[tauri::command]
pub fn list_signature_marks(store: State<'_, SignatureMarkStore>) -> Vec<StoredMark> {
    lock(&store).list()
}

/// Saves a picture for use again.
#[tauri::command]
pub fn add_signature_mark(
    store: State<'_, SignatureMarkStore>,
    name: String,
    mark: MarkBody,
) -> Result<StoredMark, CommandError> {
    Ok(lock(&store).add(&name, mark, crate::now_seconds())?)
}

#[tauri::command]
pub fn rename_signature_mark(
    store: State<'_, SignatureMarkStore>,
    id: String,
    name: String,
) -> Result<StoredMark, CommandError> {
    Ok(lock(&store).rename(&id, &name)?)
}

#[tauri::command]
pub fn delete_signature_mark(
    store: State<'_, SignatureMarkStore>,
    id: String,
) -> Result<(), CommandError> {
    Ok(lock(&store).delete(&id)?)
}

/// Forgets every saved signature.
#[tauri::command]
pub fn clear_signature_marks(store: State<'_, SignatureMarkStore>) -> Result<(), CommandError> {
    Ok(lock(&store).clear()?)
}

// ---- putting a picture on a page -----------------------------------------------------------

fn bad(message: &str) -> CommandError {
    CommandError::from(ClientError::Engine {
        code: ErrorCode::InvalidRequest,
        message: message.into(),
    })
}

/// Puts a drawn or a typed picture on a page, fitted whole into `rect`, which is
/// `[left, top, right, bottom]` in points from the top left of the page as it is drawn. The
/// picture is checked again here and by the engine.
#[tauri::command]
pub async fn add_signature(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    page: u32,
    rect: [f32; 4],
    color: [u8; 4],
    mark: MarkBody,
) -> Result<AnnotationInfo, CommandError> {
    let mark = mark
        .checked()
        .and_then(|m| m.to_mark())
        .ok_or_else(|| bad("that is not a picture to draw or to type"))?;
    // Not while an update is being installed: the engine would be stopped under the change.
    let _permit = update_state
        .gate()
        .enter()
        .map_err(|_| updates::updating_error())?;
    let pool = engine.0.clone();
    let item = blocking(move || pool.add_signature(doc, page, rect, color, mark)).await?;
    Ok(item.into())
}

/// What the window says about an imported picture, in the header of the request. The pixels are
/// the body: `width * height * 4` bytes, red, green, blue and alpha.
#[derive(Debug, Deserialize)]
struct ImageArgs {
    doc: u64,
    page: u32,
    rect: [f32; 4],
    color: [u8; 4],
    width: u32,
    height: u32,
}

/// The size of an imported picture, the part of the header that every use of one needs.
#[derive(Debug, Deserialize)]
struct ImageSize {
    width: u32,
    height: u32,
}

/// The picture of a request that carries one in the way `add_signature_image` does: the size in the
/// header and the pixels as the body. Used by the signature with a certificate, which keeps the
/// picture until the file is signed.
pub fn image_mark_of_request(
    request: &tauri::ipc::Request<'_>,
) -> Result<SignatureMark, CommandError> {
    let size: ImageSize = request
        .headers()
        .get(IMAGE_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| serde_json::from_str(v).ok())
        .ok_or_else(|| bad("the description of the image is missing"))?;
    if !mark_image_size_ok(size.width, size.height) {
        return Err(bad("that image is bigger than a signature picture may be"));
    }
    let pixels = pixels_of(
        request.body(),
        size.width as usize * size.height as usize * 4,
    )?;
    if pixels.len() != size.width as usize * size.height as usize * 4 {
        return Err(bad("the pixels do not fit the size of the image"));
    }
    Ok(SignatureMark::Image {
        width: size.width,
        height: size.height,
        rgba: pixels.into_owned(),
    })
}

/// The name of the header that carries [`ImageArgs`].
const IMAGE_HEADER: &str = "x-signature-image";

/// The pixels of the request. The window sends them as the raw body of the request. If the webview
/// cannot use its custom protocol for the call it falls back to a message, and Tauri then delivers
/// the same bytes as a JSON array of numbers; those are taken too, after their number is checked
/// against the size the window declared and before anything is made of them.
fn pixels_of(
    body: &tauri::ipc::InvokeBody,
    expected: usize,
) -> Result<std::borrow::Cow<'_, [u8]>, CommandError> {
    match body {
        tauri::ipc::InvokeBody::Raw(bytes) => Ok(std::borrow::Cow::Borrowed(bytes.as_slice())),
        tauri::ipc::InvokeBody::Json(serde_json::Value::Array(items)) => {
            if items.len() != expected {
                return Err(bad("the pixels do not fit the size of the image"));
            }
            let mut out = Vec::with_capacity(expected);
            for item in items {
                let byte = item
                    .as_u64()
                    .and_then(|n| u8::try_from(n).ok())
                    .ok_or_else(|| bad("the pixels of the image are not bytes"))?;
                out.push(byte);
            }
            Ok(std::borrow::Cow::Owned(out))
        }
        tauri::ipc::InvokeBody::Json(_) => Err(bad("the pixels of the image are missing")),
    }
}

fn image_mark(args: &ImageArgs, body: &[u8]) -> Result<SignatureMark, CommandError> {
    if !mark_image_size_ok(args.width, args.height) {
        return Err(bad("that image is bigger than a signature picture may be"));
    }
    if body.len() != args.width as usize * args.height as usize * 4 {
        return Err(bad("the pixels do not fit the size of the image"));
    }
    Ok(SignatureMark::Image {
        width: args.width,
        height: args.height,
        rgba: body.to_vec(),
    })
}

/// Puts an imported picture on a page. The window decoded the image and sends raw pixels.
#[tauri::command]
pub async fn add_signature_image(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    request: tauri::ipc::Request<'_>,
) -> Result<AnnotationInfo, CommandError> {
    let args: ImageArgs = request
        .headers()
        .get(IMAGE_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| serde_json::from_str(v).ok())
        .ok_or_else(|| bad("the description of the image is missing"))?;
    if !mark_image_size_ok(args.width, args.height) {
        return Err(bad("that image is bigger than a signature picture may be"));
    }
    let pixels = pixels_of(
        request.body(),
        args.width as usize * args.height as usize * 4,
    )?;
    let mark = image_mark(&args, &pixels)?;
    let _permit = update_state
        .gate()
        .enter()
        .map_err(|_| updates::updating_error())?;
    let pool = engine.0.clone();
    let (doc, page, rect, color) = (args.doc, args.page, args.rect, args.color);
    let item = blocking(move || pool.add_signature(doc, page, rect, color, mark)).await?;
    Ok(item.into())
}

/// Moves and resizes a picture of a signature: it takes the top left corner and the width of
/// `rect`, and keeps its proportions.
#[tauri::command]
pub async fn place_signature(
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    doc: u64,
    page: u32,
    id: u32,
    rect: [f32; 4],
) -> Result<AnnotationInfo, CommandError> {
    let _permit = update_state
        .gate()
        .enter()
        .map_err(|_| updates::updating_error())?;
    let pool = engine.0.clone();
    let item = blocking(move || pool.place_signature(doc, page, id, rect)).await?;
    Ok(item.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use partridge_protocol::{MAX_MARK_IMAGE_PIXELS, MAX_MARK_IMAGE_SIDE};
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> TempDir {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "partridge-sigmarks-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn file(&self) -> PathBuf {
            self.0.join("signatures.json")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn draw() -> MarkBody {
        MarkBody::Draw {
            width: 300.0,
            height: 100.0,
            line: 3.0,
            strokes: vec![vec![[1.0, 2.0], [30.0, 40.0]], vec![[5.0, 5.0]]],
        }
    }

    fn typed() -> MarkBody {
        MarkBody::Type {
            font: "allura".into(),
            text: "Anna Alvorlig".into(),
        }
    }

    /// Base64 text.
    fn base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for group in bytes.chunks(3) {
            let value = group
                .iter()
                .enumerate()
                .fold(0u32, |v, (i, b)| v | (u32::from(*b) << (16 - 8 * i)));
            for i in 0..4 {
                if i <= group.len() {
                    out.push(ALPHABET[((value >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// The start of a PNG that declares the size `width` by `height`, as base64 text (the rest of a
    /// file is not looked at). 24 bytes make 32 characters, so nothing is padded.
    fn png_text(width: u32, height: u32) -> String {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 13]);
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        base64(&bytes)
    }

    /// As [`png_text`], followed by `more` characters of base64 as the rest of the file.
    fn png_text_with(width: u32, height: u32, more: usize) -> String {
        format!("{}{}", png_text(width, height), "A".repeat(more))
    }

    fn image() -> MarkBody {
        MarkBody::Image {
            width: 4,
            height: 2,
            png: png_text_with(4, 2, 8),
        }
    }

    #[test]
    fn a_picture_is_kept_and_comes_back_after_a_restart() {
        let dir = TempDir::new();
        let mut marks = SignatureMarks::load(Some(dir.file()));
        assert!(marks.is_empty());
        let a = marks.add("My name", draw(), 100).unwrap();
        let b = marks.add("Typed", typed(), 101).unwrap();
        let c = marks.add("Stamp image", image(), 102).unwrap();
        assert_eq!(marks.len(), 3);
        assert_ne!(a.id, b.id);
        let mut again = SignatureMarks::load(Some(dir.file()));
        assert_eq!(again.list(), vec![a, b, c]);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_and_its_folder_are_for_the_user_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let inner = dir.0.join("data");
        let file = inner.join("signatures.json");
        let mut marks = SignatureMarks::load(Some(file.clone()));
        marks.add("One", draw(), 1).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(&inner), 0o700);
        // Writing again keeps it so, also when the file is made wide open in the meantime, and
        // when a temporary file that is wide open was left behind by a crash.
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        let stale = file.with_extension("json.tmp");
        fs::write(&stale, b"left over").unwrap();
        fs::set_permissions(&stale, fs::Permissions::from_mode(0o644)).unwrap();
        marks.add("Two", typed(), 2).unwrap();
        assert_eq!(mode(&file), 0o600);
        assert!(!stale.exists());
        assert_eq!(SignatureMarks::load(Some(file)).len(), 2);
    }

    #[test]
    fn a_picture_is_renamed_and_deleted() {
        let dir = TempDir::new();
        let mut marks = SignatureMarks::load(Some(dir.file()));
        let a = marks.add("First", draw(), 1).unwrap();
        let b = marks.add("Second", typed(), 2).unwrap();
        let renamed = marks.rename(&a.id, "  Formal  ").unwrap();
        assert_eq!(renamed.name, "Formal");
        marks.delete(&b.id).unwrap();
        let mut again = SignatureMarks::load(Some(dir.file()));
        assert_eq!(again.list().len(), 1);
        assert_eq!(again.list()[0].name, "Formal");
        assert!(matches!(marks.delete(&b.id), Err(StoreError::Missing)));
        assert!(matches!(
            marks.rename("nobody", "x"),
            Err(StoreError::Missing)
        ));
        assert!(matches!(marks.rename(&a.id, ""), Err(StoreError::Name)));
    }

    #[test]
    fn forgetting_empties_the_file_too() {
        let dir = TempDir::new();
        let mut marks = SignatureMarks::load(Some(dir.file()));
        marks.add("One", draw(), 1).unwrap();
        marks.clear().unwrap();
        assert!(marks.is_empty());
        assert!(SignatureMarks::load(Some(dir.file())).is_empty());
    }

    #[test]
    fn names_are_one_line_and_within_their_cap() {
        let dir = TempDir::new();
        let mut marks = SignatureMarks::load(Some(dir.file()));
        for bad in [
            "".to_string(),
            "   ".to_string(),
            "two\nlines".to_string(),
            "tab\there".to_string(),
            "x".repeat(MAX_NAME_CHARS + 1),
        ] {
            assert!(
                matches!(marks.add(&bad, draw(), 1), Err(StoreError::Name)),
                "{bad:?}"
            );
        }
        marks.add(&"x".repeat(MAX_NAME_CHARS), draw(), 1).unwrap();
        marks
            .add(&"\u{e5}".repeat(MAX_NAME_CHARS), typed(), 2)
            .unwrap();
        assert_eq!(marks.len(), 2);
    }

    #[test]
    fn only_so_many_pictures_are_kept() {
        let mut marks = SignatureMarks::load(None);
        for i in 0..MAX_MARKS {
            marks.add(&format!("n{i}"), typed(), 1).unwrap();
        }
        assert!(matches!(
            marks.add("one more", typed(), 2),
            Err(StoreError::Full)
        ));
        assert_eq!(marks.len(), MAX_MARKS);
        // Ids stay unique when many are made in the same second.
        let mut ids: Vec<String> = marks.list().into_iter().map(|m| m.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), MAX_MARKS);
    }

    #[test]
    fn a_picture_outside_the_caps_is_refused() {
        let mut marks = SignatureMarks::load(None);
        let strokes = |n: usize, p: usize| MarkBody::Draw {
            width: 100.0,
            height: 50.0,
            line: 2.0,
            strokes: vec![vec![[1.0, 1.0]; p]; n],
        };
        assert!(marks.add("ok", strokes(64, 64), 1).is_ok());
        for (what, body) in [
            ("a stroke too many", strokes(65, 1)),
            ("a point too many", strokes(1, 4_097)),
            ("no strokes", strokes(0, 1)),
            (
                "a box that is not a number",
                MarkBody::Draw {
                    width: f32::NAN,
                    height: 50.0,
                    line: 2.0,
                    strokes: vec![vec![[1.0, 1.0]]],
                },
            ),
            (
                "a line too thin",
                MarkBody::Draw {
                    width: 100.0,
                    height: 50.0,
                    line: 0.0,
                    strokes: vec![vec![[1.0, 1.0]]],
                },
            ),
            (
                "text of 41 characters",
                MarkBody::Type {
                    font: "allura".into(),
                    text: "a".repeat(41),
                },
            ),
            (
                "text that is only spaces",
                MarkBody::Type {
                    font: "allura".into(),
                    text: "   ".into(),
                },
            ),
            (
                "a font nobody bundled",
                MarkBody::Type {
                    font: "comic-sans".into(),
                    text: "Anna".into(),
                },
            ),
            (
                "an image one pixel too wide",
                MarkBody::Image {
                    width: MAX_MARK_IMAGE_SIDE + 1,
                    height: 1,
                    png: png_text_with(MAX_MARK_IMAGE_SIDE + 1, 1, 8),
                },
            ),
            (
                "an image with too many pixels",
                MarkBody::Image {
                    width: MAX_MARK_IMAGE_SIDE,
                    height: MAX_MARK_IMAGE_PIXELS / MAX_MARK_IMAGE_SIDE + 1,
                    png: png_text_with(
                        MAX_MARK_IMAGE_SIDE,
                        MAX_MARK_IMAGE_PIXELS / MAX_MARK_IMAGE_SIDE + 1,
                        8,
                    ),
                },
            ),
            (
                "an image that is not a PNG",
                MarkBody::Image {
                    width: 4,
                    height: 4,
                    png: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
                },
            ),
            (
                "an image that is not base64",
                MarkBody::Image {
                    width: 4,
                    height: 4,
                    png: format!("{}<script>x</script>", png_text(4, 4)),
                },
            ),
            (
                "an image whose header says another size than the one stored",
                MarkBody::Image {
                    width: 4,
                    height: 4,
                    png: png_text_with(4, 5, 8),
                },
            ),
            (
                "an image whose header says it is huge",
                MarkBody::Image {
                    width: 4,
                    height: 4,
                    png: png_text_with(30_000, 30_000, 8),
                },
            ),
            (
                "an image that is only the start of a signature",
                MarkBody::Image {
                    width: 4,
                    height: 4,
                    png: png_text(4, 4)[..28].to_string(),
                },
            ),
            (
                "an image that is too big",
                MarkBody::Image {
                    width: 4,
                    height: 4,
                    png: png_text_with(4, 4, MAX_PNG_BASE64),
                },
            ),
        ] {
            assert!(
                matches!(marks.add("n", body, 1), Err(StoreError::Picture)),
                "{what}"
            );
        }
        // At the caps it works: the biggest image text, the longest typed text.
        let big = png_text_with(
            MAX_MARK_IMAGE_SIDE,
            MAX_MARK_IMAGE_PIXELS / MAX_MARK_IMAGE_SIDE,
            MAX_PNG_BASE64 - 32,
        );
        assert!(marks
            .add(
                "big",
                MarkBody::Image {
                    width: MAX_MARK_IMAGE_SIDE,
                    height: MAX_MARK_IMAGE_PIXELS / MAX_MARK_IMAGE_SIDE,
                    png: big,
                },
                1
            )
            .is_ok());
        assert!(marks
            .add(
                "long",
                MarkBody::Type {
                    font: "kristi".into(),
                    text: "a".repeat(40)
                },
                1
            )
            .is_ok());
    }

    #[test]
    fn a_failed_write_leaves_the_list_as_it_was() {
        let dir = TempDir::new();
        // The folder of the file is a file: nothing can be written.
        let blocker = dir.0.join("blocker");
        fs::write(&blocker, b"x").unwrap();
        let mut marks = SignatureMarks::load(Some(blocker.join("signatures.json")));
        assert!(matches!(
            marks.add("One", draw(), 1),
            Err(StoreError::Write(_))
        ));
        assert!(marks.is_empty());
    }

    #[test]
    fn a_damaged_or_foreign_file_starts_the_list_empty() {
        let dir = TempDir::new();
        for content in [
            &b"not json at all"[..],
            &b"{\"version\":2,\"marks\":[]}"[..],
            &b"{\"version\":1}"[..],
            &[0xff, 0xfe, 0x00][..],
            &b""[..],
        ] {
            fs::write(dir.file(), content).unwrap();
            assert!(SignatureMarks::load(Some(dir.file())).is_empty());
        }
        let mut marks = SignatureMarks::load(Some(dir.file()));
        marks.add("One", draw(), 5).unwrap();
        assert_eq!(SignatureMarks::load(Some(dir.file())).len(), 1);
    }

    #[test]
    fn a_file_that_is_too_big_is_not_read() {
        let dir = TempDir::new();
        fs::write(dir.file(), vec![b' '; MAX_FILE_BYTES as usize + 1]).unwrap();
        assert!(SignatureMarks::load(Some(dir.file())).is_empty());
    }

    #[test]
    fn entries_that_are_not_pictures_within_the_caps_are_dropped_on_load() {
        let dir = TempDir::new();
        let many: Vec<String> = (0..MAX_MARKS + 5)
            .map(|i| {
                format!(
                    r#"{{"id":"x{i}","name":"n{i}","created":1,"kind":"type","font":"allura","text":"Ann"}}"#
                )
            })
            .collect();
        fs::write(
            dir.file(),
            format!(
                r#"{{"version":1,"marks":[
                {{"id":"good","name":"Good","created":1,"kind":"type","font":"allura","text":"Anna"}},
                {{"id":"good","name":"Same id","created":2,"kind":"type","font":"allura","text":"Anna"}},
                {{"id":"bad id!","name":"Odd id","created":2,"kind":"type","font":"allura","text":"Anna"}},
                {{"id":"nofont","name":"No font","created":3,"kind":"type","font":"papyrus","text":"Anna"}},
                {{"id":"longtext","name":"Long","created":3,"kind":"type","font":"allura","text":"{}"}},
                {{"id":"noname","name":"  ","created":3,"kind":"type","font":"allura","text":"Anna"}},
                {{"id":"thin","name":"Thin","created":3,"kind":"draw","width":100,"height":50,"line":0.001,"strokes":[[[1,1]]]}},
                {{"id":"fine","name":"  Fine  ","created":4,"kind":"draw","width":100,"height":50,"line":2,"strokes":[[[1,1],[2,2]]]}}
                ]}}"#,
                "a".repeat(41)
            ),
        )
        .unwrap();
        let mut loaded = SignatureMarks::load(Some(dir.file()));
        let ids: Vec<String> = loaded.list().into_iter().map(|m| m.id).collect();
        assert_eq!(ids, vec!["good".to_string(), "fine".to_string()]);
        assert_eq!(loaded.list()[1].name, "Fine");
        // A file with more than the list holds is cut.
        fs::write(
            dir.file(),
            format!(r#"{{"version":1,"marks":[{}]}}"#, many.join(",")),
        )
        .unwrap();
        assert_eq!(SignatureMarks::load(Some(dir.file())).len(), MAX_MARKS);
        // An unknown kind of picture makes the whole file unreadable rather than half trusted.
        fs::write(
            dir.file(),
            br#"{"version":1,"marks":[{"id":"a","name":"A","created":1,"kind":"video","src":"x"}]}"#,
        )
        .unwrap();
        assert!(SignatureMarks::load(Some(dir.file())).is_empty());
    }

    #[test]
    fn the_size_in_the_header_of_a_png_is_read_from_its_text() {
        assert_eq!(png_text_size(&png_text(640, 480)), Some((640, 480)));
        assert_eq!(png_text_size(&png_text_with(1, 2, 100)), Some((1, 2)));
        assert_eq!(png_text_size(&png_text(u32::MAX, 7)), Some((u32::MAX, 7)));
        // Too short, not a PNG, a header chunk that is not the header, not base64.
        assert_eq!(png_text_size(""), None);
        assert_eq!(png_text_size(&png_text(1, 1)[..31]), None);
        assert_eq!(png_text_size(&"A".repeat(40)), None);
        let mut other = PNG_SIGNATURE.to_vec();
        other.extend_from_slice(&[0, 0, 0, 13]);
        other.extend_from_slice(b"IDAT");
        other.extend_from_slice(&[0; 8]);
        assert_eq!(png_text_size(&base64(&other)), None);
        assert_eq!(png_text_size(&format!("!{}", &png_text(1, 1)[1..])), None);
    }

    #[test]
    fn a_saved_image_whose_header_disagrees_with_its_size_is_dropped_on_load_and_on_add() {
        let dir = TempDir::new();
        let entry = |id: &str, w: u32, h: u32, header: (u32, u32)| {
            format!(
                r#"{{"id":"{id}","name":"{id}","created":1,"kind":"image","width":{w},"height":{h},"png":"{}"}}"#,
                png_text_with(header.0, header.1, 8)
            )
        };
        fs::write(
            dir.file(),
            format!(
                r#"{{"version":1,"marks":[{},{},{}]}}"#,
                entry("agrees", 4, 2, (4, 2)),
                entry("wider", 4, 2, (40_000, 2)),
                entry("bomb", 4, 2, (30_000, 30_000)),
            ),
        )
        .unwrap();
        let mut loaded = SignatureMarks::load(Some(dir.file()));
        let ids: Vec<String> = loaded.list().into_iter().map(|m| m.id).collect();
        assert_eq!(ids, vec!["agrees".to_string()]);
        let disagree = MarkBody::Image {
            width: 4,
            height: 2,
            png: png_text_with(30_000, 30_000, 8),
        };
        assert!(matches!(
            loaded.add("Bomb", disagree, 5),
            Err(StoreError::Picture)
        ));
    }

    #[test]
    fn a_change_made_to_the_file_by_another_program_is_built_on_not_overwritten() {
        let dir = TempDir::new();
        let mut marks = SignatureMarks::load(Some(dir.file()));
        marks.add("Mine", draw(), 1).unwrap();
        // Another program writes a longer list.
        let mut other = SignatureMarks::load(Some(dir.file()));
        other.add("Theirs", typed(), 2).unwrap();
        other.add("Theirs too", typed(), 3).unwrap();
        // The app lists what is in the file now, and a change keeps the others.
        assert_eq!(marks.list().len(), 3);
        other.add("Later", typed(), 4).unwrap();
        let added = marks.add("Mine again", draw(), 5).unwrap();
        let names: Vec<String> = SignatureMarks::load(Some(dir.file()))
            .list()
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert_eq!(
            names,
            vec!["Mine", "Theirs", "Theirs too", "Later", "Mine again"]
        );
        // The same for a rename after the other program added something.
        let mut third = SignatureMarks::load(Some(dir.file()));
        third.add("Third", typed(), 6).unwrap();
        marks.rename(&added.id, "Renamed").unwrap();
        let names: Vec<String> = marks.list().into_iter().map(|m| m.name).collect();
        assert!(names.contains(&"Third".to_string()) && names.contains(&"Renamed".to_string()));
    }

    #[test]
    fn a_failed_move_into_place_leaves_no_temporary_file_and_the_old_file_stays() {
        let dir = TempDir::new();
        // The place of the file is a folder with something in it, so the move cannot replace it.
        fs::create_dir(dir.file()).unwrap();
        fs::write(dir.file().join("inside"), b"x").unwrap();
        assert!(write_replacing(&dir.file(), b"{}").is_err());
        assert!(!dir.file().with_extension("json.tmp").exists());
        assert!(dir.file().join("inside").exists());
        // The same through the list: the change is refused and the list is as it was.
        let mut marks = SignatureMarks::load(Some(dir.file()));
        assert!(matches!(
            marks.add("Two", typed(), 2),
            Err(StoreError::Write(_))
        ));
        assert!(marks.is_empty());
        assert!(!dir.file().with_extension("json.tmp").exists());
    }

    #[test]
    fn a_file_from_the_roaming_folder_is_moved_once_to_the_local_one() {
        let dir = TempDir::new();
        let from = dir.0.join("roaming").join("signatures.json");
        let to = dir.0.join("local").join("signatures.json");
        let mut old = SignatureMarks::load(Some(from.clone()));
        old.add("Kept", draw(), 1).unwrap();
        move_earlier_file(&from, &to);
        assert!(!from.exists(), "the old file is gone");
        let marks = SignatureMarks::load(Some(to.clone()));
        assert_eq!(marks.len(), 1);
        // A new place that has a file already is not touched by the move, and the old file stays.
        let mut again = SignatureMarks::load(Some(from.clone()));
        again.add("Second", typed(), 2).unwrap();
        move_earlier_file(&from, &to);
        assert!(from.exists());
        assert_eq!(SignatureMarks::load(Some(to.clone())).len(), 1);
        // The same place, or no earlier file, is nothing to do.
        move_earlier_file(&to, &to);
        move_earlier_file(&dir.0.join("nothing.json"), &dir.0.join("elsewhere.json"));
        assert!(!dir.0.join("elsewhere.json").exists());
        assert_eq!(SignatureMarks::load(Some(to)).len(), 1);
    }

    #[test]
    fn a_move_that_did_not_work_is_tried_again_at_the_next_start() {
        let dir = TempDir::new();
        let from = dir.0.join("roaming").join("signatures.json");
        let to = dir.0.join("local").join("signatures.json");
        SignatureMarks::load(Some(from.clone()))
            .add("Kept", draw(), 1)
            .unwrap();
        // The folder of the new file cannot be made: a file is where it should be.
        fs::write(dir.0.join("local"), b"in the way").unwrap();
        let mut first = SignatureMarks::load_moving(Some(to.clone()), Some(from.clone()));
        assert!(
            from.exists(),
            "the old file stays when the move did not work"
        );
        // The pictures of the old file are what the person sees meanwhile.
        assert_eq!(first.list().len(), 1);
        // The next start, with the way free, moves it.
        fs::remove_file(dir.0.join("local")).unwrap();
        let mut second = SignatureMarks::load_moving(Some(to.clone()), Some(from.clone()));
        assert!(!from.exists());
        assert!(to.exists());
        assert_eq!(second.list().len(), 1);
    }

    #[test]
    fn the_pictures_of_the_old_file_are_kept_when_the_first_change_comes_before_a_move_works() {
        let dir = TempDir::new();
        let from = dir.0.join("roaming").join("signatures.json");
        let to = dir.0.join("local").join("signatures.json");
        SignatureMarks::load(Some(from.clone()))
            .add("Kept", draw(), 1)
            .unwrap();
        // The new place cannot be written at first, and can be when the first change comes.
        fs::write(dir.0.join("local"), b"in the way").unwrap();
        let mut marks = SignatureMarks::load_moving(Some(to.clone()), Some(from.clone()));
        assert_eq!(marks.len(), 1);
        fs::remove_file(dir.0.join("local")).unwrap();
        marks.add("New", typed(), 2).unwrap();
        // Both files are there now. At the next start the old one goes, and nothing is lost.
        let again = SignatureMarks::load_moving(Some(to.clone()), Some(from.clone()));
        assert!(!from.exists());
        assert_eq!(again.len(), 2);
    }

    #[test]
    fn an_old_file_that_is_still_there_next_to_the_new_one_is_merged_and_then_removed_at_the_start()
    {
        let dir = TempDir::new();
        let from = dir.0.join("roaming").join("signatures.json");
        let to = dir.0.join("local").join("signatures.json");
        SignatureMarks::load(Some(from.clone()))
            .add("Old", draw(), 1)
            .unwrap();
        SignatureMarks::load(Some(to.clone()))
            .add("New", typed(), 2)
            .unwrap();
        let mut marks = SignatureMarks::load_moving(Some(to.clone()), Some(from.clone()));
        assert!(!from.exists());
        let names: Vec<String> = marks.list().into_iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["Old", "New"]);
        // It is in the new file, not only in memory.
        assert_eq!(SignatureMarks::load(Some(to)).len(), 2);
    }

    #[test]
    fn an_old_file_that_could_not_be_read_at_the_first_start_is_not_removed_by_the_next() {
        let dir = TempDir::new();
        let from = dir.0.join("roaming").join("signatures.json");
        let to = dir.0.join("local").join("signatures.json");
        SignatureMarks::load(Some(from.clone()))
            .add("Old", draw(), 1)
            .unwrap();
        let good = fs::read(&from).unwrap();
        // At the first start the old file cannot be read (here it is too big; a file held by
        // another program is the same to the app).
        fs::write(&from, vec![b' '; MAX_FILE_BYTES as usize + 1]).unwrap();
        let mut first = SignatureMarks::load_moving(Some(to.clone()), Some(from.clone()));
        assert!(first.list().is_empty());
        assert!(from.exists());
        // One picture is saved, which makes the new file.
        first.add("New", typed(), 2).unwrap();
        // The next start finds the old file unreadable still, and leaves it.
        let second = SignatureMarks::load_moving(Some(to.clone()), Some(from.clone()));
        assert!(from.exists());
        assert_eq!(second.len(), 1);
        // Once it can be read, its picture is brought in and it is removed.
        fs::write(&from, good).unwrap();
        let mut third = SignatureMarks::load_moving(Some(to.clone()), Some(from.clone()));
        assert!(!from.exists());
        let names: Vec<String> = third.list().into_iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["Old", "New"]);
    }

    #[test]
    fn the_old_file_stays_while_the_new_place_cannot_be_used_and_its_pictures_are_the_list() {
        let dir = TempDir::new();
        let from = dir.0.join("roaming").join("signatures.json");
        let to = dir.0.join("local").join("signatures.json");
        SignatureMarks::load(Some(from.clone()))
            .add("Old", draw(), 1)
            .unwrap();
        // The new place holds a file from a later version.
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::write(&to, b"{\"version\":2,\"marks\":[]}").unwrap();
        let mut marks = SignatureMarks::load_moving(Some(to.clone()), Some(from.clone()));
        assert!(from.exists());
        assert_eq!(marks.list().len(), 1);
        // The new place is a folder.
        fs::remove_file(&to).unwrap();
        fs::create_dir(&to).unwrap();
        let mut marks = SignatureMarks::load_moving(Some(to.clone()), Some(from.clone()));
        assert!(from.exists());
        assert_eq!(marks.list().len(), 1);
    }

    #[test]
    fn a_picture_deleted_while_the_old_list_is_in_use_does_not_come_back() {
        // The new place cannot be made at the first start, and can be later.
        let dir = TempDir::new();
        let from = dir.0.join("roaming").join("signatures.json");
        let to = dir.0.join("local").join("signatures.json");
        let mut old = SignatureMarks::load(Some(from.clone()));
        let first = old.add("Old", draw(), 1).unwrap();
        old.add("Old two", typed(), 2).unwrap();
        fs::write(dir.0.join("local"), b"in the way").unwrap();
        let mut marks = SignatureMarks::load_moving(Some(to.clone()), Some(from.clone()));
        assert_eq!(marks.len(), 2);
        fs::remove_file(dir.0.join("local")).unwrap();
        marks.delete(&first.id).unwrap();
        assert!(!from.exists(), "the earlier file goes with the first write");
        let mut next = SignatureMarks::load_moving(Some(to.clone()), Some(from.clone()));
        let names: Vec<String> = next.list().into_iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["Old two"]);

        // The new place holds a file of a later version: the same.
        let dir = TempDir::new();
        let from = dir.0.join("roaming").join("signatures.json");
        let to = dir.0.join("local").join("signatures.json");
        let mut old = SignatureMarks::load(Some(from.clone()));
        let first = old.add("Old", draw(), 1).unwrap();
        old.add("Old two", typed(), 2).unwrap();
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::write(&to, b"{\"version\":2,\"marks\":[]}").unwrap();
        let mut marks = SignatureMarks::load_moving(Some(to.clone()), Some(from.clone()));
        assert_eq!(marks.len(), 2);
        assert!(from.exists(), "nothing is removed before a write");
        marks.delete(&first.id).unwrap();
        assert!(!from.exists());
        let mut next = SignatureMarks::load_moving(Some(to.clone()), Some(from.clone()));
        let names: Vec<String> = next.list().into_iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["Old two"]);
        // The version 2 file is kept aside, not written over.
        assert!(aside_name(&to, 0).exists());
    }

    #[test]
    fn the_old_file_is_kept_when_the_first_write_fails() {
        let dir = TempDir::new();
        let from = dir.0.join("roaming").join("signatures.json");
        let to = dir.0.join("local").join("signatures.json");
        SignatureMarks::load(Some(from.clone()))
            .add("Old", draw(), 1)
            .unwrap();
        fs::write(dir.0.join("local"), b"in the way").unwrap();
        let mut marks = SignatureMarks::load_moving(Some(to), Some(from.clone()));
        // The folder is still not there to be made: the change fails, and the old file stays.
        assert!(marks.add("New", typed(), 2).is_err());
        assert!(from.exists());
        assert_eq!(marks.len(), 1);
    }

    #[test]
    fn an_old_file_whose_pictures_do_not_all_fit_is_left_for_later() {
        let dir = TempDir::new();
        let from = dir.0.join("roaming").join("signatures.json");
        let to = dir.0.join("local").join("signatures.json");
        SignatureMarks::load(Some(from.clone()))
            .add("Old", draw(), 1)
            .unwrap();
        let mut full = SignatureMarks::load(Some(to.clone()));
        for n in 0..MAX_MARKS as u64 {
            full.add(&format!("New {n}"), typed(), 100 + n).unwrap();
        }
        let before = fs::read(&to).unwrap();
        let marks = SignatureMarks::load_moving(Some(to.clone()), Some(from.clone()));
        assert!(from.exists());
        assert_eq!(marks.len(), MAX_MARKS);
        assert_eq!(fs::read(&to).unwrap(), before);
    }

    #[test]
    fn forgetting_deletes_the_old_file_and_the_one_set_aside_too() {
        let dir = TempDir::new();
        let from = dir.0.join("roaming").join("signatures.json");
        let to = dir.0.join("local").join("signatures.json");
        let mut marks = SignatureMarks::load_moving(Some(to.clone()), Some(from.clone()));
        marks.add("Mine", draw(), 1).unwrap();
        // An old file turns up again (a profile that syncs back), and a bad file was set aside.
        fs::create_dir_all(from.parent().unwrap()).unwrap();
        fs::write(&from, b"{\"version\":1,\"marks\":[]}").unwrap();
        fs::write(aside_of(&to), b"what was there").unwrap();
        marks.clear().unwrap();
        assert!(!from.exists());
        assert!(!aside_of(&to).exists());
        assert!(SignatureMarks::load(Some(to)).is_empty());
    }

    #[test]
    fn a_file_that_cannot_be_read_is_kept_aside_and_not_written_over() {
        for content in [
            &b"not json at all"[..],
            &b"{\"version\":2,\"marks\":[]}"[..],
            &[0xff, 0xfe, 0x00][..],
            &b""[..],
        ] {
            let dir = TempDir::new();
            fs::write(dir.file(), content).unwrap();
            let mut marks = SignatureMarks::load(Some(dir.file()));
            assert!(marks.is_empty());
            // Nothing is written until a change comes; then the bad file is moved aside whole.
            assert_eq!(fs::read(dir.file()).unwrap(), content);
            marks.add("One", draw(), 1).unwrap();
            assert_eq!(fs::read(aside_of(&dir.file())).unwrap(), content);
            assert_eq!(SignatureMarks::load(Some(dir.file())).len(), 1);
        }
        // A file that is not there is not a bad file.
        let other = TempDir::new();
        SignatureMarks::load(Some(other.file()))
            .add("One", draw(), 1)
            .unwrap();
        assert!(!aside_of(&other.file()).exists());
    }

    #[test]
    fn a_file_that_goes_bad_while_the_app_runs_keeps_the_list_and_is_set_aside_at_the_next_change()
    {
        let dir = TempDir::new();
        let mut marks = SignatureMarks::load(Some(dir.file()));
        marks.add("One", draw(), 1).unwrap();
        // Another program leaves the file damaged.
        fs::write(dir.file(), b"half a list {\"vers").unwrap();
        assert_eq!(marks.list().len(), 1, "the list the app has is kept");
        assert_eq!(
            fs::read(dir.file()).unwrap(),
            b"half a list {\"vers",
            "a listing writes nothing"
        );
        marks.add("Two", typed(), 2).unwrap();
        assert_eq!(
            fs::read(aside_of(&dir.file())).unwrap(),
            b"half a list {\"vers"
        );
        assert_eq!(SignatureMarks::load(Some(dir.file())).len(), 2);
    }

    #[test]
    fn a_bad_file_never_replaces_one_that_was_set_aside_before() {
        let dir = TempDir::new();
        let mut marks = SignatureMarks::load(Some(dir.file()));
        for (n, content) in [
            b"first bad".to_vec(),
            b"second bad".to_vec(),
            b"third bad".to_vec(),
        ]
        .into_iter()
        .enumerate()
        {
            fs::write(dir.file(), content).unwrap();
            // The change is made from a list that was read before the file went bad.
            marks.list();
            marks
                .add(&format!("Picture {n}"), draw(), n as u64)
                .unwrap();
        }
        assert_eq!(fs::read(aside_name(&dir.file(), 0)).unwrap(), b"first bad");
        assert_eq!(fs::read(aside_name(&dir.file(), 1)).unwrap(), b"second bad");
        assert_eq!(fs::read(aside_name(&dir.file(), 2)).unwrap(), b"third bad");
        assert_eq!(SignatureMarks::load(Some(dir.file())).len(), 3);
        // Forgetting deletes every one of them.
        marks.clear().unwrap();
        for slot in 0..ASIDE_SLOTS {
            assert!(!aside_name(&dir.file(), slot).exists());
        }
    }

    #[test]
    fn nothing_is_written_when_the_bad_file_cannot_be_set_aside() {
        let dir = TempDir::new();
        fs::write(dir.file(), b"not a list").unwrap();
        // Every place for it is taken (one by a folder that has something in it).
        fs::create_dir(aside_name(&dir.file(), 0)).unwrap();
        fs::write(aside_name(&dir.file(), 0).join("inside"), b"x").unwrap();
        for slot in 1..ASIDE_SLOTS {
            fs::write(aside_name(&dir.file(), slot), b"earlier").unwrap();
        }
        let mut marks = SignatureMarks::load(Some(dir.file()));
        assert!(matches!(
            marks.add("One", draw(), 1),
            Err(StoreError::Write(_))
        ));
        assert!(marks.is_empty());
        assert_eq!(fs::read(dir.file()).unwrap(), b"not a list");
    }

    #[cfg(unix)]
    #[test]
    fn a_named_pipe_and_a_link_to_a_device_are_not_read() {
        let dir = TempDir::new();
        let pipe = dir.file().with_extension("pipe");
        let made = std::process::Command::new("mkfifo").arg(&pipe).status();
        if made.is_ok_and(|s| s.success()) {
            // Opening a pipe for reading would wait for ever: the answer must come at once.
            assert!(SignatureMarks::load(Some(pipe)).is_empty());
        }
        let link = dir.file().with_extension("link");
        if std::os::unix::fs::symlink("/dev/zero", &link).is_ok() {
            assert!(SignatureMarks::load(Some(link)).is_empty());
        }
    }

    #[test]
    fn a_picture_to_place_is_checked_before_the_engine_is_asked() {
        assert!(draw().checked().and_then(|m| m.to_mark()).is_some());
        assert!(typed().checked().and_then(|m| m.to_mark()).is_some());
        // An image is not placed through the JSON command.
        assert!(image().checked().and_then(|m| m.to_mark()).is_none());
        let args = |width, height| ImageArgs {
            doc: 1,
            page: 0,
            rect: [0.0, 0.0, 100.0, 50.0],
            color: [0, 0, 0, 255],
            width,
            height,
        };
        assert!(image_mark(&args(4, 2), &[0; 32]).is_ok());
        // Pixels that do not fill the size, or fill more of it.
        assert!(image_mark(&args(4, 2), &[0; 31]).is_err());
        assert!(image_mark(&args(4, 2), &[0; 33]).is_err());
        // A size over a cap.
        assert!(image_mark(&args(MAX_MARK_IMAGE_SIDE + 1, 1), &[]).is_err());
        assert!(image_mark(&args(0, 0), &[]).is_err());
    }

    #[test]
    fn the_pixels_come_as_the_raw_body_or_as_a_list_of_numbers_when_the_webview_fell_back() {
        use tauri::ipc::InvokeBody;
        let raw = InvokeBody::Raw(vec![7; 32]);
        assert_eq!(pixels_of(&raw, 32).unwrap().as_ref(), &[7u8; 32][..]);
        let list =
            |items: Vec<serde_json::Value>| InvokeBody::Json(serde_json::Value::Array(items));
        let numbers = list((0..32).map(|i| serde_json::json!(i * 8)).collect());
        let got = pixels_of(&numbers, 32).unwrap();
        assert_eq!(got.len(), 32);
        assert_eq!((got[0], got[1], got[31]), (0, 8, 248));
        // Too few or too many numbers, a number that is not a byte, something that is not a number.
        assert!(pixels_of(&list(vec![serde_json::json!(1); 31]), 32).is_err());
        assert!(pixels_of(&list(vec![serde_json::json!(1); 33]), 32).is_err());
        for bad_item in [
            serde_json::json!(256),
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!("1"),
            serde_json::json!(null),
        ] {
            let mut items = vec![serde_json::json!(1); 32];
            items[10] = bad_item;
            assert!(pixels_of(&list(items), 32).is_err());
        }
        // A body that is not a list of bytes.
        assert!(pixels_of(&InvokeBody::Json(serde_json::json!({})), 32).is_err());
        assert!(pixels_of(&InvokeBody::Json(serde_json::Value::Null), 32).is_err());
    }

    #[test]
    fn the_description_of_an_image_is_read_from_the_header_text() {
        let text =
            r#"{"doc":3,"page":1,"rect":[1,2,3,4],"color":[0,0,0,255],"width":4,"height":2}"#;
        let args: ImageArgs = serde_json::from_str(text).unwrap();
        assert_eq!((args.doc, args.page, args.width, args.height), (3, 1, 4, 2));
        assert_eq!(args.rect, [1.0, 2.0, 3.0, 4.0]);
        assert!(serde_json::from_str::<ImageArgs>(r#"{"doc":3}"#).is_err());
    }
}
