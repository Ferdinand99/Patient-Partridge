//! Signing a document with a certificate, without ever risking a file that is on disk.
//!
//! The engine writes the document out with an empty signature at the end (it never sees a key), this
//! module hashes the file except the hole, has the signer make the signature, and writes the
//! signature into the hole. The new file is written and checked by the same rules a save follows
//! (see `save`): it is made next to its target, flushed, read back, and only then put in place with
//! one rename. The check in the middle is stricter than for a save. A fresh engine in its sandbox
//! opens the new file, and it must have the pages the document has, be one signature that holds
//! (the signer named by the certificate that was used, the byte range covering all of the file
//! but the signature itself), and have the pages the document has, apart from the new field.
//!
//! Signing is the last write. The document in the engine is not changed, and nothing writes the
//! new file after it is in place. The app opens the signed file as a document of its own, and that
//! document is not saved over (a save of a signed document is refused unless the person agrees
//! that the signature breaks, and a copy is the way out).
//!
//! A document with changes that were not saved is signed as it is in memory; the app asks the
//! person to save first, so that what is signed is a file they have.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use partridge_protocol::{
    ErrorCode, SignPlace, SignSpec, SignatureMark, SignatureState, MAX_SIGN_RESERVE,
    MIN_SIGN_RESERVE,
};
use partridge_signer_protocol::{Certificate, HashKind};
use ring::digest;

use crate::save::{write_checked, DocInfo, FileStamp, Fingerprint, SaveError, SaveOptions};
use crate::signer::SignerSession;
use crate::{ClientError, EngineClient, EngineConfig, SaveStreamError};

/// What the person chose for the signature.
#[derive(Debug, Clone)]
pub struct SignOptions {
    pub place: SignPlace,
    pub name: String,
    pub reason: String,
    pub location: String,
    /// A picture to show in the box. It proves nothing.
    pub mark: Option<SignatureMark>,
    /// The colour of a drawn or typed picture.
    pub color: [u8; 4],
    /// The time to write, from the clock of this computer, in seconds since 1970.
    pub signed_at: i64,
}

/// What a signature wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct SignOutcome {
    /// The signed file.
    pub path: PathBuf,
    pub bytes: u64,
    /// The name of the field the signature is in.
    pub field: String,
    /// The certificate that signed.
    pub certificate: Certificate,
}

/// Room left over in the signature after the longest one the signer says it makes.
const RESERVE_MARGIN: u32 = 64;

fn refused(code: ErrorCode, message: &str) -> ClientError {
    ClientError::Engine {
        code,
        message: message.into(),
    }
}

fn digest_of(bytes: &[u8], hole: (usize, usize), hash: HashKind) -> Vec<u8> {
    let algorithm = match hash {
        HashKind::Sha256 => &digest::SHA256,
        HashKind::Sha384 => &digest::SHA384,
    };
    let mut context = digest::Context::new(algorithm);
    context.update(&bytes[..hole.0]);
    context.update(&bytes[hole.1..]);
    context.finish().as_ref().to_vec()
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Opens `path` in a fresh, sandboxed engine and checks that it is the document that was signed:
/// it opens and has the pages the document has, it is not protected, it holds one signature that
/// is intact and is the one the certificate made (the sub-filter, the fingerprint of the
/// certificate, a byte range that covers the whole file but the hole), and its pages are those of
/// the document (the same description, not counting the signature field).
pub fn verify_signed(
    config: &EngineConfig,
    path: &Path,
    certificate: &Certificate,
    expected: &Fingerprint,
    pages: u32,
) -> Result<(), SaveError> {
    let verify = |why: String| SaveError::Verify(why);
    let text = path.to_string_lossy();
    let file_len = std::fs::metadata(path)
        .map_err(|e| verify(format!("the file could not be looked at ({e})")))?
        .len();
    let client = EngineClient::new(config.clone());
    let result = (|| {
        let opened = client
            .open(&text, None)
            .map_err(|e| verify(format!("it does not open ({e})")))?;
        if opened.pages.len() as u32 != pages {
            return Err(verify(format!(
                "it has {} pages, the document has {pages}",
                opened.pages.len()
            )));
        }
        let got = client
            .doc_info(opened.doc)
            .map_err(|e| verify(format!("its description could not be read ({e})")))?;
        if got.security != 0 {
            return Err(verify("it is protected, and the document is not".into()));
        }
        let report = client
            .signatures(opened.doc, now())
            .map_err(|e| verify(format!("its signature could not be checked ({e})")))?;
        if report.total != 1 || report.unlisted != 0 || report.items.len() != 1 {
            return Err(verify(format!(
                "it holds {} signatures and {} marks of others, not one signature",
                report.total, report.unlisted
            )));
        }
        let item = &report.items[0];
        if !matches!(item.state, SignatureState::UnknownSigner | SignatureState::Trusted) {
            return Err(verify(format!(
                "its signature does not hold ({:?}, {:?})",
                item.state, item.reason
            )));
        }
        if item.sub_filter != "ETSI.CAdES.detached" {
            return Err(verify("its signature is of another form than the one made".into()));
        }
        if item.fingerprint.as_deref() != Some(certificate.fingerprint.as_str()) {
            return Err(verify("its signature is not made with the certificate that was used".into()));
        }
        // The signature covers the whole file but the place of the signature itself.
        let range = &item.byte_range;
        if range.len() != 4
            || range[0] != 0
            || range[1] > range[2]
            || range[2].checked_add(range[3]) != Some(file_len)
            || item.file_len != file_len
        {
            return Err(verify("its signature does not cover the whole file".into()));
        }
        let seen = client
            .sign_fingerprint(opened.doc)
            .map_err(|e| verify(format!("its pages could not be looked at ({e})")))?;
        if seen.failed > 0 {
            return Err(verify(format!("{} of its pages could not be read", seen.failed)));
        }
        if seen.pages != expected.pages || seen.digest != expected.digest {
            return Err(verify(
                "its pages (their size, text or contents) are not those of the document".into(),
            ));
        }
        Ok(())
    })();
    client.kill();
    result
}

/// Signs the document `doc` of `client` and writes the signed file to `target`. `expected` is the
/// state `target` was in when the person chose it (`None` when it did not exist).
#[allow(clippy::too_many_arguments)]
pub(crate) fn sign_document(
    config: &EngineConfig,
    client: &EngineClient,
    doc: u64,
    signer: &mut SignerSession,
    options: &SignOptions,
    target: &Path,
    expected: Option<&FileStamp>,
    save_options: SaveOptions,
    still_open: &dyn Fn() -> bool,
) -> Result<SignOutcome, ClientError> {
    let Some(certificate) = signer.certificate().cloned() else {
        return Err(ClientError::Signer {
            code: "signer_not_loaded",
            message: "no certificate file is loaded".into(),
        });
    };
    let info: DocInfo = client.doc_info(doc)?;
    if info.pages == 0 {
        return Err(SaveError::Verify("the document has no pages".into()).into());
    }
    if info.security != 0 {
        return Err(refused(
            ErrorCode::SignEncrypted,
            "the document is encrypted, so it cannot be signed",
        ));
    }
    // What the pages are like now, as the engine that holds the document sees them. The file that
    // is written must give the same, apart from the new signature field.
    let print = client.sign_fingerprint(doc)?;
    if print.failed > 0 {
        return Err(SaveError::SourceUnreadable.into());
    }
    if print.pages == 0 || print.pages != info.pages {
        return Err(SaveError::Verify(format!(
            "the document has {} pages in one description and {} in the other",
            info.pages, print.pages
        ))
        .into());
    }
    let hash = certificate.hash();
    let reserve = (certificate.signature_bytes + RESERVE_MARGIN).clamp(MIN_SIGN_RESERVE, MAX_SIGN_RESERVE);
    if certificate.signature_bytes + RESERVE_MARGIN > MAX_SIGN_RESERVE {
        return Err(ClientError::Signer {
            code: "signer_too_large",
            message: "the certificates make a signature that is too big".into(),
        });
    }

    let mut field = String::new();
    let outcome = {
        let mut stream = |sink: &mut dyn FnMut(&[u8]) -> io::Result<()>| -> Result<u64, SaveStreamError> {
            let spec = SignSpec {
                doc: 0,
                place: options.place.clone(),
                name: options.name.clone(),
                reason: options.reason.clone(),
                location: options.location.clone(),
                signed_at: options.signed_at,
                mark: options.mark.clone(),
                color: options.color,
                reserve,
            };
            let mut prepared = client.sign_prepare(doc, spec)?;
            let hole = (prepared.hole_start, prepared.hole_end);
            let digest = digest_of(&prepared.bytes, hole, hash);
            let cms = signer.sign_digest(&digest, hash)?;
            let room = (hole.1 - hole.0 - 2) / 2;
            if cms.len() > room {
                return Err(SaveStreamError::Client(ClientError::Signer {
                    code: "signer_too_large",
                    message: "the signature does not fit the room that was kept for it".into(),
                }));
            }
            // The signature goes in as hexadecimal; what is left of the hole stays zeros.
            for (i, byte) in cms.iter().enumerate() {
                let at = hole.0 + 1 + 2 * i;
                prepared.bytes[at] = b"0123456789abcdef"[usize::from(byte >> 4)];
                prepared.bytes[at + 1] = b"0123456789abcdef"[usize::from(byte & 15)];
            }
            field = prepared.field.clone();
            for piece in prepared.bytes.chunks(1 << 20) {
                sink(piece).map_err(SaveStreamError::Sink)?;
            }
            Ok(prepared.bytes.len() as u64)
        };
        write_checked(
            target,
            expected,
            save_options,
            info,
            print,
            &mut stream,
            &|path, info, print| verify_signed(config, path, &certificate, print, info.pages),
            still_open,
        )?
    };
    Ok(SignOutcome {
        path: outcome.path,
        bytes: outcome.bytes,
        field,
        certificate,
    })
}
