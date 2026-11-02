//! Filling in a form with the real engine process, the real PDFium and the sandbox, and saving it
//! with the save path of the app: the engine serialises, the app writes a new file, a second
//! sandboxed engine checks it (the same pages, the same text, the same fields on every page), and
//! the new file replaces the old one. Then another engine opens the file and reads the values back.
//!
//! The engine in the sandbox cannot read a font file or ask the system for anything, so this is also
//! the proof that the appearance of a field can be made without them.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use partridge_engine_client::{
    field_flag, ClientError, EngineConfig, EnginePool, FormField, FormKind, FormValue,
    SandboxPolicy, SaveError, SaveOptions,
};
use partridge_protocol::ErrorCode;
use partridge_testkit as kit;

fn config() -> EngineConfig {
    let mut config = EngineConfig::new(env!("CARGO_BIN_EXE_partridge-engine"));
    config.args = vec!["--pdfium-dir".into(), kit::pdfium_dir().into_os_string()];
    // The tests insist on the sandbox: an engine that cannot lock itself in fails them.
    config.sandbox = SandboxPolicy::Required;
    config
}

/// A folder of its own for one test, removed again at the end.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "partridge-formproc-{name}-{}-{}",
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
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn one(fields: &[FormField], name: &str) -> FormField {
    let found: Vec<&FormField> = fields.iter().filter(|f| f.name == name).collect();
    assert_eq!(found.len(), 1, "{name}");
    found[0].clone()
}

fn selected(field: &FormField) -> Vec<&str> {
    field
        .options
        .iter()
        .filter(|o| o.selected)
        .map(|o| o.label.as_str())
        .collect()
}

fn code(result: Result<impl std::fmt::Debug, ClientError>) -> ErrorCode {
    match result {
        Err(ClientError::Engine { code, .. }) => code,
        other => panic!("expected an engine error, got {other:?}"),
    }
}

#[test]
fn a_form_is_filled_in_saved_and_read_back_by_another_engine() {
    let dir = Dir::new("fill");
    let path = dir.write("form.pdf", &kit::forms_pdf());
    let pool = EnginePool::new(config(), 4);
    let (handle, _) = pool.open(&text(&path), None).unwrap();

    assert_eq!(pool.form_info(handle).unwrap(), (FormKind::Acro, false));
    let mut fields = pool.form_fields(handle, 0).unwrap();
    assert_eq!(fields.len(), 13);
    let mut set = |fields: &[FormField], name: &str, index: usize, value: FormValue| {
        let target: Vec<&FormField> = fields.iter().filter(|f| f.name == name).collect();
        let target = target[index];
        pool.set_form_field(handle, 0, target.index, &target.name, &value)
            .unwrap()
    };
    fields = set(
        &fields,
        "fullname",
        0,
        FormValue::Text("Ada Lovelace".into()),
    );
    fields = set(
        &fields,
        "notes",
        0,
        FormValue::Text("First line\nSecond line".into()),
    );
    fields = set(&fields, "agree", 0, FormValue::Checked(true));
    fields = set(&fields, "size", 1, FormValue::Checked(true));
    fields = set(&fields, "country", 0, FormValue::Choice(vec![1]));
    fields = set(&fields, "city", 0, FormValue::Text("Troms\u{f8}".into()));
    fields = set(&fields, "colours", 0, FormValue::Choice(vec![2]));
    fields = set(&fields, "toppings", 0, FormValue::Choice(vec![0, 2]));
    assert_eq!(one(&fields, "fullname").value, "Ada Lovelace");

    // The save path writes a new file, has a second engine check it, and replaces the old one.
    let out = pool.save(handle, SaveOptions::default()).unwrap();
    assert_eq!(out.path, path);
    let first_saved = fs::read(&path).unwrap();
    assert_eq!(dir.names(), vec!["form.pdf".to_string()]);

    // Another engine reads the saved file.
    let pool2 = EnginePool::new(config(), 4);
    let (again, _) = pool2.open(&text(&path), None).unwrap();
    let read = pool2.form_fields(again, 0).unwrap();
    assert_eq!(one(&read, "fullname").value, "Ada Lovelace");
    assert_eq!(one(&read, "notes").value, "First line\nSecond line");
    assert!(one(&read, "agree").has(field_flag::CHECKED));
    let on: Vec<bool> = read
        .iter()
        .filter(|f| f.name == "size")
        .map(|f| f.has(field_flag::CHECKED))
        .collect();
    assert_eq!(on, [false, true, false]);
    assert_eq!(selected(&one(&read, "country")), ["Sweden"]);
    assert_eq!(one(&read, "city").value, "Troms\u{f8}");
    assert_eq!(selected(&one(&read, "colours")), ["Blue"]);
    assert_eq!(selected(&one(&read, "toppings")), ["Ham", "Olives"]);

    // Filled in again and saved again, the document is still the same document.
    let fields = pool2.form_fields(again, 0).unwrap();
    let full = one(&fields, "fullname");
    pool2
        .set_form_field(
            again,
            0,
            full.index,
            &full.name,
            &FormValue::Text("Grace".into()),
        )
        .unwrap();
    pool2.save(again, SaveOptions::default()).unwrap();
    let pool3 = EnginePool::new(config(), 4);
    let (third, _) = pool3.open(&text(&path), None).unwrap();
    assert_eq!(
        one(&pool3.form_fields(third, 0).unwrap(), "fullname").value,
        "Grace"
    );

    // For the tools of `scripts/check-forms.sh`: the file as the sandboxed engine saved it.
    if let Some(folder) = std::env::var_os("PARTRIDGE_FORMS_OUT") {
        fs::create_dir_all(&folder).unwrap();
        fs::write(Path::new(&folder).join("sandbox-first.pdf"), first_saved).unwrap();
    }
}

#[test]
fn what_is_not_allowed_is_refused_through_the_whole_path() {
    let dir = Dir::new("refuse");
    let path = dir.write("form.pdf", &kit::forms_pdf());
    let pool = EnginePool::new(config(), 4);
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    let fields = pool.form_fields(handle, 0).unwrap();
    let locked = one(&fields, "locked");
    assert_eq!(
        code(pool.set_form_field(
            handle,
            0,
            locked.index,
            &locked.name,
            &FormValue::Text("changed".into())
        )),
        ErrorCode::FormFieldRefused
    );
    let full = one(&fields, "fullname");
    assert_eq!(
        code(pool.set_form_field(handle, 0, full.index, "notes", &FormValue::Text("x".into()))),
        ErrorCode::FormFieldRefused
    );
    // A refused request changes nothing and the engine goes on.
    assert_eq!(pool.form_fields(handle, 0).unwrap(), fields);
    assert!(pool.render(handle, 0, 100, 130).is_ok());

    // An XFA form is shown, and not filled in.
    let xfa = dir.write("xfa.pdf", &kit::xfa_full_pdf());
    let (xfa_handle, _) = pool.open(&text(&xfa), None).unwrap();
    assert_eq!(pool.form_info(xfa_handle).unwrap().0, FormKind::Xfa);
    assert_eq!(
        code(pool.form_fields(xfa_handle, 0)),
        ErrorCode::FormUnsupported
    );
    assert!(pool.render(xfa_handle, 0, 100, 130).is_ok());
}

#[test]
fn a_signed_form_asks_before_the_save_breaks_the_signature() {
    let dir = Dir::new("signed");
    let path = dir.write("form.pdf", &kit::forms_signed_pdf());
    let pool = EnginePool::new(config(), 4);
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    let fields = pool.form_fields(handle, 0).unwrap();
    let full = one(&fields, "fullname");
    pool.set_form_field(
        handle,
        0,
        full.index,
        &full.name,
        &FormValue::Text("x".into()),
    )
    .unwrap();
    let before = fs::read(&path).unwrap();
    match pool.save(handle, SaveOptions::default()) {
        Err(ClientError::Save(SaveError::Signed { signatures })) => assert!(signatures >= 1),
        other => panic!("{other:?}"),
    }
    // The file is as it was, and nothing is left beside it.
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(dir.names(), vec!["form.pdf".to_string()]);
    // Once the reader agrees, the values are saved.
    pool.save(
        handle,
        SaveOptions {
            allow_signed: true,
            ..SaveOptions::default()
        },
    )
    .unwrap();
    let (again, _) = pool.open(&text(&path), None).unwrap();
    assert_eq!(
        one(&pool.form_fields(again, 0).unwrap(), "fullname").value,
        "x"
    );
}

#[test]
fn a_form_with_scripts_is_filled_in_and_the_scripts_do_nothing() {
    let dir = Dir::new("scripts");
    let path = dir.write("form.pdf", &kit::forms_script_pdf());
    let pool = EnginePool::new(config(), 4);
    let (handle, _) = pool.open(&text(&path), None).unwrap();
    assert_eq!(pool.form_info(handle).unwrap(), (FormKind::Acro, true));
    let fields = pool.form_fields(handle, 0).unwrap();
    assert!(one(&fields, "fullname").has(field_flag::ACTION));
    let notes = one(&fields, "notes");
    let fields = pool
        .set_form_field(
            handle,
            0,
            notes.index,
            &notes.name,
            &FormValue::Text("hi".into()),
        )
        .unwrap();
    // The calculation would have put a value in the first field.
    assert_eq!(one(&fields, "fullname").value, "");
    pool.save(handle, SaveOptions::default()).unwrap();
}
