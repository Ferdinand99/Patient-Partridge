//! Signing a document with a certificate file, for the window.
//!
//! The window never names a file or holds a key. The person picks a certificate file in the dialog
//! of the system that this process opens, types the password (which is passed on to the signer and
//! overwritten), and the signer process (see `partridge-signer`) is the only one that ever sees the
//! file and the key. The engine prepares the document and never has the key either. The signed
//! file is a new file the person names in a dialog; the document that is open is not changed.
//!
//! Nothing here makes a promise about what a signature is worth: what the window says about the
//! result is "the signature holds, the signer is not verified", in the words of the signature
//! panel.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use partridge_engine_client::{
    Certificate, CertificateKey, ClientError, SaveOptions, SignOptions, SignPlace, SignatureFont,
    SignatureMark, SignerConfig, SignerSession,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use zeroize::Zeroize;

use super::sigmarks;
use super::updates::{self, UpdateState};
use super::{blocking, choose_save_target, CommandError, Engine};

/// The signer and what the person chose, between the steps of the dialog.
#[derive(Default)]
pub struct CertificateState {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// The certificate file that was picked. The window gets its name only.
    file: Option<PathBuf>,
    /// The signer, once a certificate file was opened.
    signer: Option<SignerSession>,
    /// The document the box was opened for. A key opened for one document signs no other.
    doc: Option<u64>,
    /// When the certificate file was opened; the key is let go after [`KEY_LIFETIME`].
    opened: Option<Instant>,
    /// Counts the openings, so that the watch of an earlier one knows it is not the last.
    generation: u64,
    /// A signature is being made: the signer is out of this state for the time it takes.
    signing: bool,
    /// An imported picture for the box, as pixels (the window decoded it).
    image: Option<SignatureMark>,
}

/// How long a key stays in the signer when nothing is signed with it. The signer ends after one
/// signature; a key that is opened and then forgotten about is let go by the app after this time.
const KEY_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// How often the watch looks at the clock.
const WATCH_EVERY: Duration = Duration::from_secs(10);

fn state(s: &CertificateState) -> std::sync::MutexGuard<'_, Inner> {
    s.inner.lock().unwrap_or_else(|e| e.into_inner())
}

impl CertificateState {
    /// Lets the key go: the signer ends and what was picked is forgotten.
    pub fn forget(&self) {
        let mut inner = state(self);
        inner.file = None;
        inner.image = None;
        inner.doc = None;
        inner.opened = None;
        if let Some(signer) = inner.signer.take() {
            signer.close();
        }
    }

    /// Lets the key go when it has been open longer than [`KEY_LIFETIME`]. The file that was
    /// picked stays, so that the person can type the password again.
    ///
    /// The watch goes on while a signature is being made (the signer is out of the state then,
    /// and comes back if the signing fails before it signs), and ends when the key is gone, was
    /// forgotten, or belongs to a later opening.
    fn let_go_if_old(&self, generation: u64) -> bool {
        let mut inner = state(self);
        if inner.generation != generation || inner.opened.is_none() {
            return true;
        }
        if inner.opened.is_some_and(|at| at.elapsed() >= KEY_LIFETIME) {
            if inner.signing {
                return false;
            }
            inner.opened = None;
            if let Some(signer) = inner.signer.take() {
                signer.close();
            }
            return true;
        }
        false
    }
}

/// Watches the key of one opening and lets it go when it is old. Ends when the key is gone.
fn watch(app: AppHandle, generation: u64) {
    std::thread::spawn(move || loop {
        std::thread::sleep(WATCH_EVERY);
        let certificate = app.state::<CertificateState>();
        if certificate.let_go_if_old(generation) {
            return;
        }
    });
}

fn error(code: &str, message: &str) -> CommandError {
    CommandError {
        code: code.into(),
        message: message.into(),
    }
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The signer program installed next to the app.
fn signer_config() -> SignerConfig {
    SignerConfig::new(super::locate::signer_program())
}

/// What the window learns about the certificate. The texts come from the file: shown as text only.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CertificateView {
    name: String,
    issuer: String,
    /// The holder issued the certificate to themselves.
    self_signed: bool,
    serial: String,
    fingerprint: String,
    /// First and last second of the validity, seconds since 1970.
    not_before: i64,
    not_after: i64,
    /// The time on this computer when the file was opened, seconds since 1970.
    now: i64,
    /// `RSA 3072 bit`, `ECDSA P-256`.
    key: String,
    /// False when the certificate says it may not be used to sign documents.
    may_sign: bool,
    chain: u32,
}

impl CertificateView {
    fn of(c: &Certificate, now: i64) -> CertificateView {
        CertificateView {
            name: c.name.clone(),
            issuer: c.issuer.clone(),
            self_signed: c.self_signed,
            serial: c.serial.clone(),
            fingerprint: c.fingerprint.clone(),
            not_before: c.not_before,
            not_after: c.not_after,
            now,
            key: CertificateKey::text(c.key),
            may_sign: c.may_sign(),
            chain: c.chain,
        }
    }
}

/// Reads a certificate file: a plain file, not bigger than the signer takes.
fn read_certificate_file(path: &Path) -> Result<Vec<u8>, CommandError> {
    use std::io::Read;
    let meta = std::fs::metadata(path).map_err(|_| {
        error(
            "sign_file_unreadable",
            "The certificate file could not be read.",
        )
    })?;
    if !meta.is_file() {
        return Err(error("sign_file_unreadable", "That is not a file."));
    }
    if meta.len() as usize > partridge_engine_client::MAX_CERTIFICATE_FILE {
        return Err(error(
            "signer_too_large",
            "The certificate file is too big.",
        ));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    std::fs::File::open(path)
        .and_then(|f| {
            f.take(partridge_engine_client::MAX_CERTIFICATE_FILE as u64 + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|_| {
            error(
                "sign_file_unreadable",
                "The certificate file could not be read.",
            )
        })?;
    if bytes.len() > partridge_engine_client::MAX_CERTIFICATE_FILE {
        return Err(error(
            "signer_too_large",
            "The certificate file is too big.",
        ));
    }
    Ok(bytes)
}

/// Asks which certificate file to use, with the dialog of the system. Returns the name of the file
/// (not the folder), or nothing when the person cancels. A signer from an earlier choice is let go.
#[tauri::command]
pub async fn sign_pick_certificate(
    app: AppHandle,
    certificate: State<'_, CertificateState>,
) -> Result<Option<String>, CommandError> {
    let dialog_app = app.clone();
    let chosen = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = dialog_app
            .dialog()
            .file()
            .add_filter("Certificate files", &["p12", "pfx"]);
        if let Some(window) = tauri::Manager::get_webview_window(&dialog_app, "main") {
            dialog = dialog.set_parent(&window);
        }
        dialog.blocking_pick_file()
    })
    .await
    .map_err(|e| error("internal", &e.to_string()))?;
    let Some(chosen) = chosen else {
        return Ok(None);
    };
    let Ok(path) = chosen.into_path() else {
        return Err(error(
            "sign_file_unreadable",
            "That place is not a file on this computer.",
        ));
    };
    // A file that is not a plain file or is too big is told now, before a password is asked for.
    std::fs::metadata(&path)
        .map_err(|_| {
            error(
                "sign_file_unreadable",
                "The certificate file could not be read.",
            )
        })
        .and_then(|meta| {
            if !meta.is_file() {
                Err(error("sign_file_unreadable", "That is not a file."))
            } else if meta.len() as usize > partridge_engine_client::MAX_CERTIFICATE_FILE {
                Err(error(
                    "signer_too_large",
                    "The certificate file is too big.",
                ))
            } else {
                Ok(())
            }
        })?;
    certificate.forget();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    state(&certificate).file = Some(path);
    Ok(Some(name))
}

/// Opens the picked certificate file with the password. The password goes to the signer and is
/// overwritten here; it is not kept. A wrong password leaves the file picked for another try.
#[tauri::command]
pub async fn sign_load_certificate(
    app: AppHandle,
    certificate: State<'_, CertificateState>,
    doc: u64,
    mut password: String,
) -> Result<CertificateView, CommandError> {
    let path = state(&certificate).file.clone();
    let Some(path) = path else {
        password.zeroize();
        return Err(error("sign_no_file", "No certificate file is chosen."));
    };
    let taken = state(&certificate).signer.take();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let mut p12 = match read_certificate_file(&path) {
            Ok(bytes) => bytes,
            Err(e) => {
                password.zeroize();
                return (taken, Err(e));
            }
        };
        let result = (|| {
            let mut signer = match taken {
                Some(signer) => signer,
                None => SignerSession::start(&signer_config())?,
            };
            let loaded = signer.load(&p12, &password);
            Ok::<_, ClientError>((signer, loaded))
        })();
        p12.zeroize();
        password.zeroize();
        match result {
            Ok((signer, Ok(c))) => (Some(signer), Ok(c)),
            // A wrong password: the signer stays, ready for another try.
            Ok((
                signer,
                Err(
                    e @ ClientError::Signer {
                        code: "signer_wrong_password",
                        ..
                    },
                ),
            )) => (Some(signer), Err(CommandError::from(e))),
            Ok((signer, Err(e))) => {
                signer.close();
                (None, Err(CommandError::from(e)))
            }
            Err(e) => (None, Err(CommandError::from(e))),
        }
    })
    .await
    .map_err(|e| error("internal", &e.to_string()))?;
    let (signer, result) = outcome;
    let opened = result.is_ok();
    let generation = {
        let mut inner = state(&certificate);
        inner.signer = signer;
        if opened {
            inner.doc = Some(doc);
            inner.opened = Some(Instant::now());
            inner.generation += 1;
        }
        inner.generation
    };
    if opened {
        watch(app, generation);
    }
    result.map(|c| CertificateView::of(&c, now_seconds()))
}

/// Whether a certificate file is open in the signer. The window asks after a signing that did not
/// succeed: the signer ends after the one signature it makes (and lets the key go after a time), so
/// the password may be needed again.
#[tauri::command]
pub fn sign_certificate_open(certificate: State<'_, CertificateState>, doc: u64) -> bool {
    let inner = state(&certificate);
    inner.signer.is_some() && inner.doc == Some(doc)
}

/// Lets the key go and forgets the file that was picked.
#[tauri::command]
pub fn sign_forget_certificate(certificate: State<'_, CertificateState>) {
    certificate.forget();
}

/// An imported picture for the box of a certificate signature: the window decoded it and sends
/// raw pixels (see `add_signature_image`).
#[tauri::command]
pub fn sign_set_image_mark(
    certificate: State<'_, CertificateState>,
    request: tauri::ipc::Request<'_>,
) -> Result<(), CommandError> {
    let mark = sigmarks::image_mark_of_request(&request)?;
    state(&certificate).image = Some(mark);
    Ok(())
}

/// One signature field of a page.
#[derive(Serialize)]
pub struct FieldView {
    page: u32,
    rect: [f32; 4],
    name: String,
}

/// The signature fields the pages of the document have (from the file: names as text).
#[tauri::command]
pub async fn sign_list_fields(
    engine: State<'_, Engine>,
    doc: u64,
) -> Result<Vec<FieldView>, CommandError> {
    let pool = engine.0.clone();
    let fields = blocking(move || pool.sign_fields(doc)).await?;
    Ok(fields
        .into_iter()
        .map(|f| FieldView {
            page: f.page,
            rect: f.rect,
            name: f.name,
        })
        .collect())
}

/// Where the signature goes, as the window says it.
#[derive(Deserialize, Debug)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PlaceArgs {
    Invisible,
    Rect { page: u32, rect: [f32; 4] },
    Field { page: u32, name: String },
}

/// The picture for the box: none, a drawn or a typed one (the same shapes as a saved signature), or
/// the imported one, which was handed over before as pixels.
#[derive(Deserialize, Debug)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum MarkArgs {
    None,
    Draw {
        width: f32,
        height: f32,
        line: f32,
        strokes: Vec<Vec<[f32; 2]>>,
    },
    Type {
        font: String,
        text: String,
    },
    Image,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SignArgs {
    place: PlaceArgs,
    reason: String,
    location: String,
    mark: MarkArgs,
    color: [u8; 4],
}

/// The file the signature was written to.
#[derive(Serialize)]
pub struct SignedInfo {
    path: String,
    name: String,
    field: String,
    /// The name in the certificate that signed.
    signer: String,
    /// SHA-256 of the certificate that signed.
    fingerprint: String,
}

/// The file name to start the dialog from: the document's name with `(signed)` before the ending.
fn signed_name(current: &Path) -> PathBuf {
    let stem = current
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".to_string());
    current.with_file_name(format!("{stem} (signed).pdf"))
}

fn mark_of(
    args: MarkArgs,
    certificate: &CertificateState,
) -> Result<Option<SignatureMark>, CommandError> {
    let mark = match args {
        MarkArgs::None => return Ok(None),
        MarkArgs::Draw {
            width,
            height,
            line,
            strokes,
        } => SignatureMark::Strokes {
            width,
            height,
            line,
            strokes,
        },
        MarkArgs::Type { font, text } => SignatureMark::Typed {
            font: SignatureFont::from_name(&font).ok_or_else(|| {
                error(
                    "invalid_request",
                    "That font is not one of the signature fonts.",
                )
            })?,
            text,
        },
        MarkArgs::Image => state(certificate)
            .image
            .clone()
            .ok_or_else(|| error("invalid_request", "No picture was given."))?,
    };
    if !mark.within_caps() {
        return Err(error(
            "invalid_request",
            "That picture is outside the limits for a signature picture.",
        ));
    }
    Ok(Some(mark))
}

/// Signs the document with the certificate that was opened and writes the signed file where the
/// person says (a dialog the app opens: the window cannot name a file). The document that is open
/// is not changed. Refused when the document has changes that were not saved: what is signed is a
/// file the person has. `None` when the person cancels the dialog.
#[tauri::command]
pub async fn sign_document(
    app: AppHandle,
    engine: State<'_, Engine>,
    update_state: State<'_, UpdateState>,
    certificate: State<'_, CertificateState>,
    doc: u64,
    args: SignArgs,
) -> Result<Option<SignedInfo>, CommandError> {
    if update_state.unsaved().is_dirty(doc) {
        return Err(error(
            "sign_unsaved",
            "This document has changes that are not saved.",
        ));
    }
    if !sign_certificate_open(certificate.clone(), doc) {
        return Err(error("signer_not_loaded", "No certificate file is open."));
    }
    let pool = engine.0.clone();
    let Some(current) = pool.source_path(doc) else {
        return Err(error("unknown_document", "The document is not open."));
    };
    let mark = mark_of(args.mark, &certificate)?;
    let place = match args.place {
        PlaceArgs::Invisible => SignPlace::Invisible,
        PlaceArgs::Rect { page, rect } => SignPlace::Rect { page, rect },
        PlaceArgs::Field { page, name } => SignPlace::Field { page, name },
    };
    // The dialog is the person's to answer for as long as they like; the gate (which an update
    // install closes) is entered only afterwards.
    let Some(target) = choose_save_target(&app, signed_name(&current)).await? else {
        return Ok(None);
    };
    let _permit = update_state
        .gate()
        .enter()
        .map_err(|_| updates::updating_error())?;
    let Some(mut signer) = ({
        let mut inner = state(&certificate);
        let taken = inner.signer.take();
        inner.signing = taken.is_some();
        taken
    }) else {
        return Err(error("signer_not_loaded", "No certificate file is open."));
    };
    let options = SignOptions {
        place,
        reason: args.reason,
        location: args.location,
        mark,
        color: args.color,
        // The time is this computer's clock, and nobody else has confirmed it.
        signed_at: now_seconds().max(0),
    };
    let task = tauri::async_runtime::spawn_blocking(move || {
        let result = pool.sign_as(doc, &target, &mut signer, &options, SaveOptions::default());
        (signer, result)
    })
    .await
    .map_err(|e| error("internal", &e.to_string()))?;
    let (signer, result) = task;
    state(&certificate).signing = false;
    match result {
        Ok(outcome) => {
            // Signed: the key is let go at once.
            signer.close();
            certificate.forget();
            Ok(Some(SignedInfo {
                name: outcome
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: outcome.path.to_string_lossy().into_owned(),
                field: outcome.field,
                signer: outcome.certificate.name.clone(),
                fingerprint: outcome.certificate.fingerprint.clone(),
            }))
        }
        Err(e) => {
            // Not signed: the certificate stays open for another try, unless the signer is gone
            // (it ends after the one signature it makes, also when the file was not written).
            if signer.certificate().is_some() {
                state(&certificate).signer = Some(signer);
            } else {
                signer.close();
            }
            Err(CommandError::from(e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signed_copy_is_named_after_the_document() {
        assert_eq!(
            signed_name(Path::new("/home/anna/contract.pdf")),
            PathBuf::from("/home/anna/contract (signed).pdf")
        );
        assert_eq!(
            signed_name(Path::new("report.v2.PDF")),
            PathBuf::from("report.v2 (signed).pdf")
        );
    }

    #[test]
    fn a_file_that_is_not_a_plain_file_or_is_too_big_is_refused_before_the_signer_sees_it() {
        let dir = std::env::temp_dir().join(format!("partridge-certsign-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            read_certificate_file(&dir).unwrap_err().code,
            "sign_file_unreadable"
        );
        assert_eq!(
            read_certificate_file(&dir.join("missing.p12"))
                .unwrap_err()
                .code,
            "sign_file_unreadable"
        );
        let big = dir.join("big.p12");
        std::fs::write(
            &big,
            vec![0u8; partridge_engine_client::MAX_CERTIFICATE_FILE + 1],
        )
        .unwrap();
        assert_eq!(
            read_certificate_file(&big).unwrap_err().code,
            "signer_too_large"
        );
        let ok = dir.join("ok.p12");
        std::fs::write(&ok, b"abc").unwrap();
        assert_eq!(read_certificate_file(&ok).unwrap(), b"abc".to_vec());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_certificate_reaches_the_window_with_the_clock_of_this_computer() {
        let c = Certificate {
            name: "Anna".into(),
            issuer: "Anna".into(),
            self_signed: true,
            serial: "01".into(),
            fingerprint: "ab".repeat(32),
            not_before: 10,
            not_after: 20,
            key: CertificateKey::EcdsaP256,
            usage: 0,
            chain: 1,
            signature_bytes: 900,
        };
        let view = CertificateView::of(&c, 15);
        assert_eq!(view.key, "ECDSA P-256");
        assert!(view.self_signed && view.may_sign);
        assert_eq!((view.not_before, view.not_after, view.now), (10, 20, 15));
        let json = serde_json::to_value(&view).unwrap();
        assert!(json.get("selfSigned").is_some());
        assert!(json.get("password").is_none());
    }

    #[test]
    fn forgetting_the_certificate_forgets_the_choices() {
        let s = CertificateState::default();
        state(&s).file = Some(PathBuf::from("a.p12"));
        state(&s).image = Some(SignatureMark::Typed {
            font: partridge_engine_client::SignatureFont::Allura,
            text: "x".into(),
        });
        s.forget();
        assert!(state(&s).file.is_none() && state(&s).image.is_none());
    }
}
