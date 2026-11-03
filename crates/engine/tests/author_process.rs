//! The form designer with the real engine process, the real PDFium and the sandbox, and the save
//! path of the app: the engine writes the fields after the bytes PDFium saved and loads the result,
//! the app saves the document the safe way (a second sandboxed engine checks the new file), and
//! another engine opens the file and reads the fields back. A step back goes through the copies the
//! page tools keep.
//!
//! The engine in the sandbox cannot read a font file or ask the system for anything, so this is also
//! the proof that the pictures of the fields can be made without them.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use partridge_engine_client::{
    field_flag, ClientError, EngineConfig, EnginePool, FormField, SandboxPolicy, SaveOptions,
};
use partridge_protocol::{AuthorOp, DesignField, DesignKind, DesignProps, DesignWidget, ErrorCode};
use partridge_testkit as kit;

fn config() -> EngineConfig {
    let mut config = EngineConfig::new(env!("CARGO_BIN_EXE_partridge-engine"));
    config.args = vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
    // The tests insist on the sandbox: an engine that cannot lock itself in fails them.
    config.sandbox = SandboxPolicy::Required;
    config
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "partridge-authorproc-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn field(name: &str, kind: DesignKind, rect: [f32; 4]) -> AuthorOp {
    let mut props = DesignProps::default();
    if matches!(kind, DesignKind::Combo | DesignKind::List) {
        props.choices = vec!["Norge".into(), "\u{c5}land".into()];
    }
    AuthorOp::Add(DesignField {
        name: name.into(),
        kind,
        props,
        widgets: vec![DesignWidget {
            page: 0,
            rect,
            export: String::new(),
        }],
    })
}

fn names(fields: &[FormField]) -> Vec<&str> {
    fields.iter().map(|f| f.name.as_str()).collect()
}

fn code(result: Result<impl std::fmt::Debug, ClientError>) -> ErrorCode {
    match result {
        Err(ClientError::Engine { code, .. }) => code,
        other => panic!("expected an engine error, got {other:?}"),
    }
}

#[test]
fn a_form_is_made_saved_the_safe_way_and_read_by_another_engine() {
    let dir = Dir::new("make");
    let path = dir.write("page.pdf", &kit::simple_pdf(2));
    let pool = EnginePool::new(config(), 4);
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    assert!(pool.author_check(handle).unwrap().is_empty());
    let step = pool
        .author_apply(
            handle,
            &[
                field("name", DesignKind::Text, [72.0, 72.0, 272.0, 94.0]),
                field("agree", DesignKind::Checkbox, [72.0, 110.0, 86.0, 124.0]),
                field("country", DesignKind::Combo, [72.0, 140.0, 272.0, 162.0]),
                field("sign", DesignKind::Signature, [72.0, 180.0, 272.0, 230.0]),
            ],
        )
        .unwrap();
    assert!(step.snapshot > 0);
    let list = pool.form_fields(handle, 0).unwrap();
    assert_eq!(names(&list), ["name", "agree", "country"]);
    let widgets = pool.author_list(handle, 0).unwrap();
    assert_eq!(widgets.len(), 4);
    assert!(widgets.iter().all(|w| w.editable));

    // The reader's own fill-in code takes a value, and the safe save writes everything.
    let name = &list[0];
    pool.set_form_field(
        handle,
        0,
        name.index,
        &name.name,
        &partridge_engine_client::FormValue::Text("Bj\u{f8}rn".into()),
    )
    .unwrap();
    let out = pool.save(handle, SaveOptions::default()).unwrap();
    assert_eq!(out.path, path);

    // Another engine reads the saved file: the fields, the value, and the place to sign.
    let pool2 = EnginePool::new(config(), 4);
    let (again, _) = pool2.open(&text(&path), None).unwrap();
    let read = pool2.form_fields(again, 0).unwrap();
    assert_eq!(names(&read), ["name", "agree", "country"]);
    assert_eq!(read[0].value, "Bj\u{f8}rn");
    assert!(!read[1].has(field_flag::CHECKED));
    assert_eq!(pool2.author_list(again, 0).unwrap().len(), 4);
    // An empty place to sign is not a signature.
    let client = partridge_engine_client::EngineClient::new(config());
    let opened = client.open(&text(&path), None).unwrap();
    assert_eq!(client.doc_info(opened.doc).unwrap().signatures, 0);

    // A step back goes through the copy, in the sandbox too, and forward again.
    let back = pool2
        .author_apply(
            again,
            &[AuthorOp::Remove {
                name: "agree".into(),
            }],
        )
        .unwrap();
    assert_eq!(
        names(&pool2.form_fields(again, 0).unwrap()),
        ["name", "country"]
    );
    let forward = pool2.pages_restore(again, back.snapshot).unwrap();
    assert_eq!(
        names(&pool2.form_fields(again, 0).unwrap()),
        ["name", "agree", "country"]
    );
    pool2.pages_restore(again, forward.snapshot).unwrap();
    assert_eq!(
        names(&pool2.form_fields(again, 0).unwrap()),
        ["name", "country"]
    );
}

#[test]
fn what_cannot_be_done_is_refused_through_the_whole_path_and_the_engine_goes_on() {
    let dir = Dir::new("refuse");
    let path = dir.write("page.pdf", &kit::simple_pdf(1));
    let pool = EnginePool::new(config(), 4);
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    pool.author_apply(
        handle,
        &[field("a", DesignKind::Text, [72.0, 72.0, 272.0, 94.0])],
    )
    .unwrap();
    // A name that is taken, a dot, a box outside the page.
    for ops in [
        vec![field("a", DesignKind::Text, [72.0, 100.0, 272.0, 122.0])],
        vec![field("a.b", DesignKind::Text, [72.0, 100.0, 272.0, 122.0])],
        vec![field("c", DesignKind::Text, [600.0, 100.0, 900.0, 122.0])],
    ] {
        assert_eq!(
            code(pool.author_apply(handle, &ops)),
            ErrorCode::AuthorRefused
        );
    }
    // Too long a name or too many changes never leave the app.
    let long = "n".repeat(600);
    assert_eq!(
        code(pool.author_apply(
            handle,
            &[field(&long, DesignKind::Text, [72.0, 100.0, 272.0, 122.0])]
        )),
        ErrorCode::InvalidRequest
    );
    // The engine goes on, with the document as it was.
    assert_eq!(names(&pool.form_fields(handle, 0).unwrap()), ["a"]);
    assert!(pool.render(handle, 0, 100, 130).is_ok());

    // An encrypted document and an XFA form are told with a reason.
    let enc = dir.write("enc.pdf", &kit::encrypted_pdf("pw", 1));
    let (enc_handle, _) = pool.open(&text(&enc), Some("pw")).unwrap();
    assert_eq!(
        code(pool.author_check(enc_handle)),
        ErrorCode::AuthorUnsupported
    );
    let xfa = dir.write("xfa.pdf", &kit::xfa_full_pdf());
    let (xfa_handle, _) = pool.open(&text(&xfa), None).unwrap();
    assert_eq!(
        code(pool.author_check(xfa_handle)),
        ErrorCode::AuthorUnsupported
    );
}
