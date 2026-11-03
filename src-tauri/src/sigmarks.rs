//! Pictures of signatures that the reader keeps to use again, and the commands that put a picture
//! on a page.
//!
//! The pictures live in one small JSON file in the app's data folder on this computer, kept the way
//! `positions.rs` keeps its file: the folder and the file are for the user alone on Unix, a
//! damaged or oversized file is ignored (the list starts empty), and "Forget saved signatures" in
//! the settings menu empties it. Nothing is sent anywhere. Anyone who can read the folder can use
//! the pictures: they are pictures, and prove nothing about who placed them.
//!
//! The file is user data on a disk that other programs can write, so everything in it is checked
//! again when it is read, with the same caps as a picture that is sent to the engine. An imported
//! image is kept as the PNG the window made of it, as text; this process never decodes it.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use partridge_engine_client::{ClientError, SignatureFont, SignatureMark};
use partridge_protocol::{mark_image_size_ok, mark_text_ok, ErrorCode};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::annotations::AnnotationInfo;
use crate::positions::{create_private_dir, read_plain_file, write_private_file};
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

/// The first characters of a PNG file in base64 (its eight byte signature).
const PNG_BASE64_START: &str = "iVBORw0KGg";

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
                    && png.starts_with(PNG_BASE64_START)
                    && is_base64(png)
            }
        };
        ok.then_some(self)
    }

    /// The picture as the engine takes it, for a drawn or a typed one. An image goes to the engine
    /// as pixels, through `add_signature_image`.
    fn to_mark(&self) -> Option<SignatureMark> {
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
}

impl SignatureMarks {
    /// Loads the list from `file`. With no file the list lives only as long as the app.
    pub fn load(file: Option<PathBuf>) -> SignatureMarks {
        let marks = file.as_deref().map(read_marks).unwrap_or_default();
        SignatureMarks { file, marks }
    }

    pub fn list(&self) -> Vec<StoredMark> {
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
        let before = self.marks.clone();
        let at = self
            .marks
            .iter()
            .position(|m| m.id == id)
            .ok_or(StoreError::Missing)?;
        self.marks.remove(at);
        self.keep(before)
    }

    /// Forgets every picture, in the app and in the file.
    pub fn clear(&mut self) -> Result<(), StoreError> {
        let before = std::mem::take(&mut self.marks);
        self.keep(before)
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

    fn save(&self) -> io::Result<()> {
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
        // Write beside the file and move it into place, so a crash cannot leave half a file.
        let temp = file.with_extension("json.tmp");
        write_private_file(&temp, &json)?;
        fs::rename(&temp, file)
    }
}

fn read_marks(file: &std::path::Path) -> Vec<StoredMark> {
    let Some(bytes) = read_plain_file(file, MAX_FILE_BYTES) else {
        return Vec::new();
    };
    let Ok(stored) = serde_json::from_slice::<Stored>(&bytes) else {
        return Vec::new();
    };
    if stored.version != 1 {
        return Vec::new();
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
    out
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

/// The name of the header that carries [`ImageArgs`].
const IMAGE_HEADER: &str = "x-signature-image";

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
    let tauri::ipc::InvokeBody::Raw(pixels) = request.body() else {
        return Err(bad("the pixels of the image are missing"));
    };
    let args: ImageArgs = request
        .headers()
        .get(IMAGE_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| serde_json::from_str(v).ok())
        .ok_or_else(|| bad("the description of the image is missing"))?;
    let mark = image_mark(&args, pixels)?;
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

    fn image() -> MarkBody {
        MarkBody::Image {
            width: 4,
            height: 2,
            png: format!("{PNG_BASE64_START}AAAAAA"),
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
        let again = SignatureMarks::load(Some(dir.file()));
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
        // Writing again keeps it so, also when the file is made wide open in the meantime.
        marks.add("Two", typed(), 2).unwrap();
        assert_eq!(mode(&file), 0o600);
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
        let again = SignatureMarks::load(Some(dir.file()));
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
                    png: format!("{PNG_BASE64_START}AAAAAA"),
                },
            ),
            (
                "an image with too many pixels",
                MarkBody::Image {
                    width: MAX_MARK_IMAGE_SIDE,
                    height: MAX_MARK_IMAGE_PIXELS / MAX_MARK_IMAGE_SIDE + 1,
                    png: format!("{PNG_BASE64_START}AAAAAA"),
                },
            ),
            (
                "an image that is not a PNG",
                MarkBody::Image {
                    width: 4,
                    height: 4,
                    png: "AAAAAAAAAAAAAAAA".into(),
                },
            ),
            (
                "an image that is not base64",
                MarkBody::Image {
                    width: 4,
                    height: 4,
                    png: format!("{PNG_BASE64_START}<script>x</script>"),
                },
            ),
            (
                "an image that is too big",
                MarkBody::Image {
                    width: 4,
                    height: 4,
                    png: format!("{PNG_BASE64_START}{}", "A".repeat(MAX_PNG_BASE64)),
                },
            ),
        ] {
            assert!(
                matches!(marks.add("n", body, 1), Err(StoreError::Picture)),
                "{what}"
            );
        }
        // At the caps it works: the biggest image text, the longest typed text.
        let big = format!(
            "{PNG_BASE64_START}{}",
            "A".repeat(MAX_PNG_BASE64 - PNG_BASE64_START.len())
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
        let loaded = SignatureMarks::load(Some(dir.file()));
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
    fn the_description_of_an_image_is_read_from_the_header_text() {
        let text =
            r#"{"doc":3,"page":1,"rect":[1,2,3,4],"color":[0,0,0,255],"width":4,"height":2}"#;
        let args: ImageArgs = serde_json::from_str(text).unwrap();
        assert_eq!((args.doc, args.page, args.width, args.height), (3, 1, 4, 2));
        assert_eq!(args.rect, [1.0, 2.0, 3.0, 4.0]);
        assert!(serde_json::from_str::<ImageArgs>(r#"{"doc":3}"#).is_err());
    }
}
