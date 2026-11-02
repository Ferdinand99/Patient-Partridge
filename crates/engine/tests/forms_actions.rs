//! What filling in a form must not do, and what it must do for every widget of a field.
//!
//! - No action of the PDF changes the document while a field is filled in: not a reset, not a hide.
//!   A widget whose own actions reach one (through `/Next`, an indirect action or an array, with
//!   names written with escapes, in an object stream) is left alone, and so is every widget of a
//!   form in which the keystroke, validate, format or calculate event of any dictionary reaches one,
//!   or in which the search cannot settle (a loop, a missing target, too long a chain). Ordinary
//!   forms (a "Clear form" button, date and number fields with scripts, radio groups) stay fillable.
//! - The widget that is named is the one that changes, never the one that happens to lie on top of
//!   it at a point.
//! - A field with widgets on several pages gets a new appearance on every page.
//!
//! These tests run the engine in this process, without the sandbox (see `tests/forms.rs`).

use std::fs::File;
use std::sync::{Mutex, MutexGuard, OnceLock};

use partridge_engine::source::SourceFile;
use partridge_engine::Engine;
use partridge_protocol::{
    field_flag, ErrorCode, FormField, FormFieldKind, FormValue, Request, RequestBody, ResponseBody,
};
use partridge_testkit as kit;
use pdfium_render::prelude::*;

struct Shared(Engine);
unsafe impl Send for Shared {}

fn engine() -> MutexGuard<'static, Shared> {
    static ENGINE: OnceLock<Mutex<Shared>> = OnceLock::new();
    ENGINE
        .get_or_init(|| Mutex::new(Shared(Engine::new(&kit::pdfium_dir()))))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn ask(engine: &mut Engine, body: RequestBody) -> ResponseBody {
    engine.handle(Request { id: 1, body }).body
}

fn open(engine: &mut Engine, bytes: &[u8]) -> (u32, kit::TempFile) {
    let file = kit::TempFile::new("forms-actions.pdf", bytes);
    engine.set_document(File::open(file.path()).unwrap());
    match ask(engine, RequestBody::Open { password: None }) {
        ResponseBody::Opened { doc, .. } => (doc, file),
        other => panic!("{other:?}"),
    }
}

fn fields(engine: &mut Engine, doc: u32, page: u32) -> Vec<FormField> {
    match ask(engine, RequestBody::ListFormFields { doc, page }) {
        ResponseBody::FormFields { fields } => fields,
        other => panic!("{other:?}"),
    }
}

fn one(fields: &[FormField], name: &str) -> FormField {
    let found: Vec<&FormField> = fields.iter().filter(|f| f.name == name).collect();
    assert_eq!(found.len(), 1, "{name} in {:?}", names(fields));
    found[0].clone()
}

fn names(fields: &[FormField]) -> Vec<&str> {
    fields.iter().map(|f| f.name.as_str()).collect()
}

fn set(
    engine: &mut Engine,
    doc: u32,
    page: u32,
    field: &FormField,
    value: FormValue,
) -> ResponseBody {
    ask(
        engine,
        RequestBody::SetFormField {
            doc,
            page,
            index: field.index,
            name: field.name.clone(),
            value,
        },
    )
}

fn save_all(engine: &mut Engine, doc: u32) -> Vec<u8> {
    let ResponseBody::SaveReady { size } = ask(engine, RequestBody::SaveBegin { doc }) else {
        panic!("no SaveReady");
    };
    let mut out = Vec::new();
    while (out.len() as u32) < size {
        let len = (1 << 20).min(size - out.len() as u32);
        match ask(
            engine,
            RequestBody::SaveChunk {
                doc,
                offset: out.len() as u32,
                len,
            },
        ) {
            ResponseBody::SaveData { data, .. } => out.extend_from_slice(&data),
            other => panic!("{other:?}"),
        }
    }
    ask(engine, RequestBody::SaveEnd { doc });
    out
}

/// The value that fits a field, to try it.
fn value_for(field: &FormField) -> FormValue {
    match field.kind {
        FormFieldKind::Text => FormValue::Text("typed".into()),
        FormFieldKind::Checkbox | FormFieldKind::Radio => FormValue::Checked(true),
        FormFieldKind::Combo | FormFieldKind::List => FormValue::Choice(vec![0]),
    }
}

/// The normal appearance of the `index`th widget named `name` on `page`, as the document in the
/// engine has it.
fn appearance(engine: &mut Engine, doc: u32, page: u32, name: &str) -> Option<String> {
    engine
        .edit_for_tests(doc, |document| {
            let page = document.pages().get(page as i32).ok()?;
            for annotation in page.annotations().iter() {
                let Some(field) = annotation.as_form_field() else {
                    continue;
                };
                if field.name().as_deref() == Some(name) {
                    return field.appearance_mode_value(PdfAppearanceMode::Normal);
                }
            }
            None
        })
        .flatten()
}

// ---- actions ----------------------------------------------------------------------------------

#[test]
fn no_action_of_a_field_changes_the_form_while_it_is_filled_in() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_actions_pdf());
    let before = fields(engine, doc, 0);
    // Every field that the reader lists is tried, with a value that fits it. Whatever the answer,
    // the fields that an action would reset or hide are as they were afterwards.
    for field in &before {
        // The two fields that an action would change are not tried.
        if field.name == "keep" || field.name == "victim" {
            continue;
        }
        let _ = set(engine, doc, 0, field, value_for(field));
        let now = fields(engine, doc, 0);
        assert_eq!(
            one(&now, "keep").value,
            "kept",
            "an action of {:?} reset a field",
            field.name
        );
        let victim = one(&now, "victim");
        assert_eq!(
            victim.value, "victim value",
            "an action of {:?} changed a field",
            field.name
        );
    }
    // The whole page is as it was, apart from the fields that were filled in.
    let after = fields(engine, doc, 0);
    assert_eq!(names(&before), names(&after), "a field was hidden");
    // What is written to the file has no hidden field either.
    let saved = save_all(engine, doc);
    let flat: String = String::from_utf8_lossy(&saved)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(!flat.contains("/F6"), "a field got the hidden flag");
}

#[test]
fn a_push_button_over_a_check_box_is_not_pressed_and_the_named_box_is_the_one_that_changes() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_actions_pdf());
    let list = fields(engine, doc, 0);
    let under = one(&list, "under_box");
    let answer = set(engine, doc, 0, &under, FormValue::Checked(true));
    let ResponseBody::FormFields { fields: now } = answer else {
        panic!("the named check box was not turned on: {answer:?}");
    };
    assert!(one(&now, "under_box").has(field_flag::CHECKED));
    // The button over it would have reset the form.
    assert_eq!(one(&now, "keep").value, "kept");
    assert_eq!(one(&now, "victim").value, "victim value");
}

#[test]
fn a_request_that_fails_leaves_the_document_as_it_was() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_actions_pdf());
    let before = fields(engine, doc, 0);
    // A name that is not the one at that place: refused, nothing changes.
    let mut wrong = one(&before, "plain");
    wrong.name = "keep".into();
    match set(engine, doc, 0, &wrong, FormValue::Text("x".into())) {
        ResponseBody::Error {
            code: ErrorCode::FormFieldRefused,
            ..
        } => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(fields(engine, doc, 0), before);
}

#[test]
fn actions_that_cannot_change_a_form_do_not_stop_it_from_being_filled_in() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_harmless_actions_pdf());
    let before = fields(engine, doc, 0);
    assert_eq!(before.len(), 7);
    for field in &before {
        if field.name == "keep" {
            continue;
        }
        assert!(
            !field.has(field_flag::READ_ONLY),
            "{} was left read only",
            field.name
        );
        match set(engine, doc, 0, field, value_for(field)) {
            ResponseBody::FormFields { fields: now } => {
                assert_eq!(one(&now, "keep").value, "kept", "{}", field.name);
            }
            other => panic!("{}: {other:?}", field.name),
        }
    }
    // They still carry the mark that says they have an action.
    let after = fields(engine, doc, 0);
    assert!(one(&after, "box_uri").has(field_flag::ACTION));
    assert!(one(&after, "text_named").has(field_flag::ACTION));
}

// ---- widgets on several pages ------------------------------------------------------------------

#[test]
fn a_field_with_widgets_on_several_pages_gets_a_new_appearance_on_every_page() {
    for without_page in [false, true] {
        let mut guard = engine();
        let engine = &mut guard.0;
        let (doc, _file) = open(engine, &kit::forms_multipage_pdf(without_page));
        let first = fields(engine, doc, 0);
        let applicant = one(&first, "applicant");
        let country = one(&first, "country");
        let after = set(
            engine,
            doc,
            0,
            &applicant,
            FormValue::Text("Ada Lovelace".into()),
        );
        assert!(
            matches!(after, ResponseBody::FormFields { .. }),
            "{after:?}"
        );
        let first = fields(engine, doc, 0);
        let after = set(
            engine,
            doc,
            0,
            &one(&first, "country"),
            FormValue::Choice(vec![1]),
        );
        assert!(
            matches!(after, ResponseBody::FormFields { .. }),
            "{after:?}"
        );
        let _ = country;
        // The value is the field's, so the widget on page 3 reports it too.
        let third = fields(engine, doc, 2);
        assert_eq!(one(&third, "applicant").value, "Ada Lovelace");

        // The file that is saved has the new appearance in both widgets.
        let saved = save_all(engine, doc);
        let (second, _file2) = open(engine, &saved);
        for page in [0u32, 2] {
            let drawn = appearance(engine, second, page, "applicant").unwrap_or_default();
            assert!(
                drawn.contains("Ada Lovelace"),
                "page {} (without_page {without_page}): {drawn:?}",
                page + 1
            );
            let drawn = appearance(engine, second, page, "country").unwrap_or_default();
            assert!(
                drawn.contains("Sweden"),
                "page {} (without_page {without_page}): {drawn:?}",
                page + 1
            );
        }
    }
}

#[test]
fn files_for_other_tools_with_a_field_on_two_pages_are_written() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let original = kit::forms_multipage_pdf(false);
    let (doc, _file) = open(engine, &original);
    let list = fields(engine, doc, 0);
    let _ = set(
        engine,
        doc,
        0,
        &one(&list, "applicant"),
        FormValue::Text("Ada Lovelace".into()),
    );
    let list = fields(engine, doc, 0);
    let _ = set(
        engine,
        doc,
        0,
        &one(&list, "country"),
        FormValue::Choice(vec![1]),
    );
    let filled = save_all(engine, doc);
    let Some(dir) = std::env::var_os("PARTRIDGE_FORMS_OUT") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("multipage-empty.pdf"), &original).unwrap();
    std::fs::write(dir.join("multipage-filled.pdf"), &filled).unwrap();
    std::fs::write(
        dir.join("multipage-expected.txt"),
        "applicant\tAda Lovelace\ncountry\tSweden\n",
    )
    .unwrap();
}

// ---- the field tree ----------------------------------------------------------------------------

#[test]
fn a_loop_in_the_field_tree_and_a_very_deep_tree_do_not_hang_the_engine() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_loop_pdf());
    let started = std::time::Instant::now();
    let list = fields(engine, doc, 0);
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    // The widget under the loop is listed with a name that is bounded; the one under 300 parents may
    // be left out by PDFium, which is fine as long as the engine does not stop.
    assert!(!list.is_empty(), "{:?}", names(&list));
    assert!(list.iter().all(|f| f.name.len() <= 512));
    // And they can be filled in.
    for field in &list {
        match set(engine, doc, 0, field, FormValue::Text("x".into())) {
            ResponseBody::FormFields { .. } => {}
            other => panic!("{}: {other:?}", field.name),
        }
    }
}

// ---- what is left alone -------------------------------------------------------------------------

#[test]
fn a_field_with_an_action_that_could_change_the_form_is_left_alone_and_says_so() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_actions_pdf());
    let list = fields(engine, doc, 0);
    let blocked = ["box_a", "box_u", "box_d", "text_fo", "text_bl"];
    for name in blocked {
        let field = one(&list, name);
        assert!(
            field.has(field_flag::ACTION_BLOCKED)
                && field.has(field_flag::READ_ONLY)
                && field.has(field_flag::ACTION),
            "{name}: {:#x}",
            field.flags
        );
        match set(engine, doc, 0, &field, value_for(&field)) {
            ResponseBody::Error {
                code: ErrorCode::FormFieldRefused,
                ..
            } => {}
            other => panic!("{name}: {other:?}"),
        }
    }
    // The others are filled in as usual.
    for name in [
        "keep",
        "victim",
        "plain",
        "plain_box",
        "under_box",
        "text_script",
        "box_link",
    ] {
        let field = one(&list, name);
        assert!(
            !field.has(field_flag::ACTION_BLOCKED)
                && !field.has(field_flag::FORM_BLOCKED)
                && !field.has(field_flag::READ_ONLY),
            "{name}"
        );
    }
    // The scripts and the link do not stop a field from being filled in, next to the buttons that
    // reset the form.
    for name in ["text_script", "box_link"] {
        let field = one(&list, name);
        match set(engine, doc, 0, &field, value_for(&field)) {
            ResponseBody::FormFields { fields: now } => {
                assert_eq!(one(&now, "keep").value, "kept", "{name}");
                assert_eq!(one(&now, "victim").value, "victim value", "{name}");
            }
            other => panic!("{name}: {other:?}"),
        }
    }
    let list = fields(engine, doc, 0);
    // The buttons are not fields the reader fills in.
    assert!(list
        .iter()
        .all(|f| f.name != "over" && f.name != "over_first"));
    // Nothing the refused requests did changed the page.
    assert_eq!(fields(engine, doc, 0), list);
}

#[test]
fn an_event_on_the_field_above_a_widget_that_pdfium_reads_from_the_widget_alone_blocks_nothing() {
    // The blur and focus events of a widget are read from the widget alone, so the reset on the
    // field above `par` cannot fire when `par` is filled in. Nothing is left alone for it: only the
    // check box with the reset of its own is. The reset is shown not to fire by filling in `par`
    // and looking at `keep`.
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_parent_event_pdf());
    let _ = ask(engine, RequestBody::FormInfo { doc });
    let list = fields(engine, doc, 0);
    for name in ["keep", "par"] {
        let field = one(&list, name);
        assert!(
            !field.has(field_flag::ACTION_BLOCKED)
                && !field.has(field_flag::FORM_BLOCKED)
                && !field.has(field_flag::READ_ONLY),
            "{name}: {:#x}",
            field.flags
        );
    }
    assert!(one(&list, "box_a").has(field_flag::ACTION_BLOCKED));
    assert!(!one(&list, "box_a").has(field_flag::FORM_BLOCKED));
    let par = one(&list, "par");
    match set(engine, doc, 0, &par, FormValue::Text("typed".into())) {
        ResponseBody::FormFields { fields: now } => {
            assert_eq!(one(&now, "par").value, "typed");
            assert_eq!(one(&now, "keep").value, "kept");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_document_with_no_reset_or_hide_blocks_nothing_even_with_other_actions() {
    let mut guard = engine();
    let engine = &mut guard.0;
    for bytes in [kit::forms_harmless_actions_pdf(), kit::forms_script_pdf()] {
        let (doc, _file) = open(engine, &bytes);
        let list = fields(engine, doc, 0);
        assert!(list.iter().any(|f| f.has(field_flag::ACTION)));
        assert!(list.iter().all(|f| !f.has(field_flag::ACTION_BLOCKED)));
    }
}

#[test]
fn the_names_of_a_reset_and_a_hide_are_found_whole_and_only_whole() {
    use partridge_engine::forms::holds_state_action_for_tests as holds;
    assert!(holds(b"<</S/ResetForm>>"));
    assert!(holds(b"<< /S /ResetForm /Fields [] >>"));
    assert!(holds(b"<</S/Hide/T(a)>>"));
    assert!(holds(b"x /Hide"));
    // Names that only start with the same letters are others: view preferences, for one.
    assert!(!holds(
        b"<</ViewerPreferences<</HideToolbar true/HideMenubar true>>>>"
    ));
    assert!(!holds(b"/ResetFormFlags /HideWindowUI"));
    assert!(!holds(b"nothing"));
}

#[test]
fn values_that_do_not_fit_the_wire_are_listed_in_a_way_that_cannot_hurt() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_awkward_values_pdf());
    let list = fields(engine, doc, 0);
    // A limit that does not fit a signed 32 bit number is left out, so that the window can set
    // `maxLength` with it.
    let huge = one(&list, "huge_max");
    assert!(
        huge.max_len == 0 || huge.max_len <= i32::MAX as u32,
        "{}",
        huge.max_len
    );
    // A value with a zero character in it is refused, not cut: PDFium's editor stops at the zero.
    match set(engine, doc, 0, &huge, FormValue::Text("a\0b".into())) {
        ResponseBody::Error {
            code: ErrorCode::FormFieldRefused,
            ..
        } => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(fields(engine, doc, 0), list);
    // A value with a zero character in it cannot be shown whole, so the field is left alone.
    let nul = one(&list, "nul");
    assert!(nul.has(field_flag::VALUE_CUT), "{:?}", nul.value);
    match set(engine, doc, 0, &nul, FormValue::Text("x".into())) {
        ResponseBody::Error {
            code: ErrorCode::FormFieldRefused,
            ..
        } => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_page_whose_fields_share_one_huge_list_of_choices_is_listed_with_a_bounded_amount_of_reading() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_shared_options_pdf());
    let started = std::time::Instant::now();
    let list = fields(engine, doc, 0);
    // Every field would read 10 MB of choices; the listing stops reading at a few MB.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert!(list.len() < 30, "{} fields", list.len());
    assert!(!list.is_empty());
    // The field whose list was cut is not one that can be changed.
    assert!(list.iter().any(|f| f.has(field_flag::READ_ONLY)));
}

// ---- actions on the field above a widget ---------------------------------------------------------

const ABOVE_BLOCKED: [&str; 9] = [
    "pk_text", "pv_text", "pv_hide", "pk_next", "pk_combo", "pv_list", "pk_check", "gp_text",
    "gp_check",
];

#[test]
fn a_reset_or_a_hide_on_the_field_above_a_widget_does_not_fire_because_the_widget_is_left_alone() {
    // PDFium reads the keystroke and validate events of a widget from the field above it, up the
    // chain, and they run a reset or a hide like any other event. Each case is tried with the fields
    // in plain objects and in an object stream.
    for packed in [false, true] {
        let mut guard = engine();
        let engine = &mut guard.0;
        let (doc, _file) = open(engine, &kit::forms_parent_actions_pdf(packed, true));
        let list = fields(engine, doc, 0);
        for name in ABOVE_BLOCKED {
            let field = one(&list, name);
            assert!(
                field.has(field_flag::ACTION_BLOCKED)
                    && field.has(field_flag::READ_ONLY)
                    && field.has(field_flag::FORM_BLOCKED),
                "{name} (packed {packed}): {:#x}",
                field.flags
            );
            match set(engine, doc, 0, &field, value_for(&field)) {
                ResponseBody::Error {
                    code: ErrorCode::FormFieldRefused,
                    ..
                } => {}
                other => panic!("{name} (packed {packed}): {other:?}"),
            }
            let now = fields(engine, doc, 0);
            assert_eq!(
                one(&now, "keep").value,
                "kept",
                "{name} (packed {packed}) reset a field"
            );
            assert_eq!(names(&now), names(&list), "{name} hid a field");
        }
        // The field with no parent is left alone too: an event that PDFium reads from the field above
        // a widget reaches a reset, and no link between objects says which widget it belongs to.
        let keep = one(&list, "keep");
        assert!(keep.has(field_flag::FORM_BLOCKED) && keep.has(field_flag::READ_ONLY));
        assert_eq!(names(&fields(engine, doc, 0)), names(&list));
    }
}

/// The ways the field `t` of `forms_kids_actions_pdf` can hold a reset or a hide.
const KIDS_EVENTS: [(&str, &str); 3] = [
    ("a reset on the keystroke", "/K << /S /ResetForm >>"),
    ("a reset on the validate event", "/V << /S /ResetForm >>"),
    (
        "a hide of keep on the validate event",
        "/V << /S /Hide /T (keep) /H true >>",
    ),
];

#[test]
fn a_reset_or_a_hide_on_a_field_does_not_fire_for_a_widget_in_its_kids_whose_parent_does_not_lead_there(
) {
    // PDFium builds a field from the field down, through `/Kids`: a widget listed there belongs to
    // the field whatever its own `/Parent` says. The widget `t` has a name of its own and no parent
    // that leads to its field, so a search that only follows `/Parent` up from the widgets misses
    // the field and lets the action fire.
    for (what, events) in KIDS_EVENTS {
        for parent in [kit::KidsParent::Absent, kit::KidsParent::Plain] {
            let mut guard = engine();
            let engine = &mut guard.0;
            let (doc, _file) = open(engine, &kit::forms_kids_actions_pdf(Some(events), parent));
            // The window asks what kind of form it is before it lists the fields.
            assert!(
                matches!(
                    ask(engine, RequestBody::FormInfo { doc }),
                    ResponseBody::FormInfo {
                        kind: partridge_protocol::FormKind::Acro,
                        ..
                    }
                ),
                "{what} ({parent:?})"
            );
            let list = fields(engine, doc, 0);
            for name in ["t", "keep"] {
                let field = one(&list, name);
                assert!(
                    field.has(field_flag::ACTION_BLOCKED)
                        && field.has(field_flag::READ_ONLY)
                        && field.has(field_flag::FORM_BLOCKED),
                    "{name}, {what} ({parent:?}): {:#x}",
                    field.flags
                );
                match set(engine, doc, 0, &field, value_for(&field)) {
                    ResponseBody::Error {
                        code: ErrorCode::FormFieldRefused,
                        ..
                    } => {}
                    other => panic!("{name}, {what} ({parent:?}): {other:?}"),
                }
            }
            let now = fields(engine, doc, 0);
            assert_eq!(
                one(&now, "keep").value,
                "kept",
                "{what} ({parent:?}) reset a field"
            );
            assert_eq!(
                one(&now, "t").value,
                "",
                "{what} ({parent:?}) changed the field"
            );
            assert_eq!(names(&now), names(&list), "{what} ({parent:?}) hid a field");
        }
    }
}

#[test]
fn a_field_built_from_its_kids_with_no_actions_is_filled_in() {
    for parent in [kit::KidsParent::Absent, kit::KidsParent::Plain] {
        let mut guard = engine();
        let engine = &mut guard.0;
        let (doc, _file) = open(engine, &kit::forms_kids_actions_pdf(None, parent));
        let list = fields(engine, doc, 0);
        let t = one(&list, "t");
        assert!(!t.has(field_flag::ACTION_BLOCKED) && !t.has(field_flag::READ_ONLY));
        match set(engine, doc, 0, &t, FormValue::Text("typed".into())) {
            ResponseBody::FormFields { fields: now } => {
                assert_eq!(one(&now, "t").value, "typed", "{parent:?}");
                assert_eq!(one(&now, "keep").value, "kept", "{parent:?}");
            }
            other => panic!("{parent:?}: {other:?}"),
        }
    }
}

#[test]
fn widgets_under_fields_with_no_actions_are_filled_in_in_a_form_that_holds_a_reset() {
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_plain_parents_pdf());
    let list = fields(engine, doc, 0);
    for name in ["keep", "pc_plain", "pc_deep"] {
        let field = one(&list, name);
        assert!(
            !field.has(field_flag::ACTION_BLOCKED) && !field.has(field_flag::READ_ONLY),
            "{name}: {:#x}",
            field.flags
        );
        if name == "keep" {
            continue;
        }
        match set(engine, doc, 0, &field, value_for(&field)) {
            ResponseBody::FormFields { fields: now } => {
                assert_eq!(one(&now, "keep").value, "kept", "{name}");
                assert_eq!(one(&now, name).value, "typed", "{name}");
            }
            other => panic!("{name}: {other:?}"),
        }
    }
    // The check box with the reset is left alone, as before.
    assert!(one(&list, "box_a").has(field_flag::ACTION_BLOCKED));
}

#[test]
fn actions_on_the_field_above_a_widget_that_cannot_change_a_form_block_nothing() {
    for packed in [false, true] {
        let mut guard = engine();
        let engine = &mut guard.0;
        let (doc, _file) = open(engine, &kit::forms_parent_actions_pdf(packed, false));
        let list = fields(engine, doc, 0);
        assert_eq!(list.len(), 10, "{:?}", names(&list));
        for field in &list {
            assert!(
                !field.has(field_flag::ACTION_BLOCKED) && !field.has(field_flag::READ_ONLY),
                "{} (packed {packed}): {:#x}",
                field.name,
                field.flags
            );
        }
        for name in ABOVE_BLOCKED {
            let field = one(&list, name);
            match set(engine, doc, 0, &field, value_for(&field)) {
                ResponseBody::FormFields { fields: now } => {
                    assert_eq!(one(&now, "keep").value, "kept", "{name}");
                }
                other => panic!("{name} (packed {packed}): {other:?}"),
            }
        }
    }
}

// ---- what the engine does before the first listing and when the file cannot be read ----------

#[test]
fn the_form_environment_is_made_by_the_question_about_the_form_and_only_for_a_form_that_is_filled_in(
) {
    // The environment writes the whole document out to look for actions, which is why it is made
    // while the window asks what kind of form there is (a request that may take long and reports
    // its progress) and not at the first listing.
    let mut guard = engine();
    let engine = &mut guard.0;
    for (bytes, made, what) in [
        (kit::forms_pdf(), true, "a form"),
        (kit::simple_pdf(2), false, "a document without a form"),
        (kit::xfa_pdf(), false, "an XFA form"),
    ] {
        let (doc, _file) = open(engine, &bytes);
        assert_eq!(
            engine.form_scans_for_tests(doc),
            None,
            "{what}: before the question"
        );
        let _ = ask(engine, RequestBody::FormInfo { doc });
        assert_eq!(
            engine.form_scans_for_tests(doc).is_some(),
            made,
            "{what}: after the question"
        );
        ask(engine, RequestBody::Close { doc });
    }
}

#[test]
fn the_pages_of_a_field_are_searched_for_once_and_then_remembered() {
    for without_page in [false, true] {
        let mut guard = engine();
        let engine = &mut guard.0;
        let (doc, _file) = open(engine, &kit::forms_multipage_pdf(without_page));
        let _ = ask(engine, RequestBody::FormInfo { doc });
        assert_eq!(engine.form_scans_for_tests(doc), Some(0));
        let list = fields(engine, doc, 0);
        assert_eq!(
            engine.form_scans_for_tests(doc),
            Some(0),
            "a listing searches nothing"
        );
        for round in 1..=3 {
            let _ = set(
                engine,
                doc,
                0,
                &one(&list, "applicant"),
                FormValue::Text(format!("Ada {round}")),
            );
            assert_eq!(
                engine.form_scans_for_tests(doc),
                Some(1),
                "change {round} of the same field (without_page {without_page})"
            );
        }
        // Another field has to be searched for on its own.
        let _ = set(
            engine,
            doc,
            0,
            &one(&list, "country"),
            FormValue::Choice(vec![1]),
        );
        assert_eq!(engine.form_scans_for_tests(doc), Some(2));
        // What the search found is used: the widget on page 3 has the value of the last change.
        assert_eq!(one(&fields(engine, doc, 2), "applicant").value, "Ada 3");
    }
}

/// A file that can be made to fail.
struct Flaky {
    file: File,
    broken: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl SourceFile for Flaky {
    fn read_at(&mut self, pos: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.broken.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(std::io::Error::other("the share is gone"));
        }
        self.file.read_at(pos, buf)
    }

    fn state(&mut self) -> std::io::Result<(u64, Option<std::time::SystemTime>)> {
        self.file.state()
    }

    fn is_plain_file(&mut self) -> bool {
        true
    }
}

#[test]
fn a_form_whose_file_failed_to_read_while_it_was_searched_is_treated_as_holding_an_action() {
    use std::sync::{atomic::AtomicBool, atomic::Ordering, Arc};
    // PDFium leaves out what it cannot read and goes on, so the document written out for the
    // search may lack the very action it looks for. A read that failed on the way means the
    // answer "no action" cannot be trusted. The document here has no action at all, so only the
    // failed read can make its fields be left alone.
    for fail in [false, true] {
        let mut guard = engine();
        let engine = &mut guard.0;
        let file = kit::TempFile::new("forms-flaky.pdf", &kit::forms_padded_pdf(200_000));
        let broken = Arc::new(AtomicBool::new(false));
        engine.set_document_source(Box::new(Flaky {
            file: File::open(file.path()).unwrap(),
            broken: broken.clone(),
        }));
        let ResponseBody::Opened { doc, .. } = ask(engine, RequestBody::Open { password: None })
        else {
            panic!("the document did not open");
        };
        // The file is several blocks long, and a look at all of it (the search for signatures)
        // leaves the guard holding its last block, so that the next read of the first one goes to
        // the file.
        assert!(matches!(
            ask(engine, RequestBody::DocInfo { doc }),
            ResponseBody::DocInfo { .. }
        ));
        broken.store(fail, Ordering::SeqCst);
        let _ = ask(engine, RequestBody::FormInfo { doc });
        broken.store(false, Ordering::SeqCst);
        let list = fields(engine, doc, 0);
        assert!(!list.is_empty());
        for field in &list {
            assert_eq!(
                field.has(field_flag::ACTION_BLOCKED) && field.has(field_flag::READ_ONLY),
                fail,
                "{} (failed read: {fail}): {:#x}",
                field.name,
                field.flags
            );
        }
    }
}

// ---- what an action reaches ------------------------------------------------------------------------

/// Every widget of the form is left alone, says why, and refuses a request; nothing was reset.
fn assert_whole_form_left_alone(engine: &mut Engine, doc: u32, what: &str) {
    // The window asks what kind of form it is before it lists the fields.
    let _ = ask(engine, RequestBody::FormInfo { doc });
    let list = fields(engine, doc, 0);
    assert!(list.len() >= 2, "{what}: {:?}", names(&list));
    for field in &list {
        assert!(
            field.has(field_flag::ACTION_BLOCKED)
                && field.has(field_flag::READ_ONLY)
                && field.has(field_flag::FORM_BLOCKED),
            "{what}: {}: {:#x}",
            field.name,
            field.flags
        );
        match set(engine, doc, 0, field, value_for(field)) {
            ResponseBody::Error {
                code: ErrorCode::FormFieldRefused,
                ..
            } => {}
            other => panic!("{what}: {}: {other:?}", field.name),
        }
    }
    let now = fields(engine, doc, 0);
    assert_eq!(one(&now, "keep").value, "kept", "{what} reset a field");
    assert_eq!(names(&now), names(&list), "{what} hid a field");
}

/// The field `target` is left alone for an action of its own, and nothing else is; the other fields
/// are filled in, and none of them resets `keep`.
fn assert_only_the_target_left_alone(engine: &mut Engine, doc: u32, what: &str) {
    let _ = ask(engine, RequestBody::FormInfo { doc });
    let list = fields(engine, doc, 0);
    let mut sorted = names(&list);
    sorted.sort_unstable();
    assert_eq!(sorted, ["keep", "plain", "target"], "{what}");
    let target = one(&list, "target");
    assert!(
        target.has(field_flag::ACTION_BLOCKED)
            && target.has(field_flag::READ_ONLY)
            && !target.has(field_flag::FORM_BLOCKED),
        "{what}: {:#x}",
        target.flags
    );
    match set(engine, doc, 0, &target, value_for(&target)) {
        ResponseBody::Error {
            code: ErrorCode::FormFieldRefused,
            ..
        } => {}
        other => panic!("{what}: {other:?}"),
    }
    for name in ["keep", "plain"] {
        let field = one(&list, name);
        assert!(
            !field.has(field_flag::ACTION_BLOCKED)
                && !field.has(field_flag::FORM_BLOCKED)
                && !field.has(field_flag::READ_ONLY),
            "{what}: {name}: {:#x}",
            field.flags
        );
    }
    match set(
        engine,
        doc,
        0,
        &one(&list, "plain"),
        FormValue::Text("typed".into()),
    ) {
        ResponseBody::FormFields { fields: now } => {
            assert_eq!(one(&now, "keep").value, "kept", "{what}");
            assert_eq!(one(&now, "plain").value, "typed", "{what}");
        }
        other => panic!("{what}: {other:?}"),
    }
}

const SCRIPT: &str = "<< /S /JavaScript /JS (1;) >>";

/// Ways a widget's own action can reach a reset or a hide: the objects to put in the file (they are
/// numbered from 6) and the entries of the widget.
fn own_reaches() -> Vec<(&'static str, Vec<String>, String)> {
    let reset = "<< /S /ResetForm >>".to_string();
    let uri = "<< /S /URI /URI (https://example.com/) >>";
    vec![
        (
            "a reset on focus",
            vec![],
            "/AA << /Fo << /S /ResetForm >> >>".into(),
        ),
        (
            "a hide of keep on blur",
            vec![],
            "/AA << /Bl << /S /Hide /T (keep) /H true >> >>".into(),
        ),
        ("a reset as /A", vec![], "/A << /S /ResetForm >>".into()),
        (
            "a reset through /Next of a script",
            vec![],
            format!("/AA << /Fo << /S /JavaScript /JS (1;) /Next {reset} >> >>"),
        ),
        (
            "an indirect action",
            vec![reset.clone()],
            "/AA << /Fo 6 0 R >>".into(),
        ),
        (
            "an indirect /AA",
            vec!["<< /Fo << /S /ResetForm >> >>".into()],
            "/AA 6 0 R".into(),
        ),
        (
            "/Next as a reference",
            vec![
                reset.clone(),
                "<< /S /JavaScript /JS (1;) /Next 6 0 R >>".into(),
            ],
            "/AA << /Fo 7 0 R >>".into(),
        ),
        (
            "an array of /Next",
            vec![],
            format!("/AA << /Fo << /S /JavaScript /JS (1;) /Next [ {uri} {reset} ] >> >>"),
        ),
        (
            "an array of references, itself a reference",
            vec![reset.clone(), "[ 6 0 R ]".into()],
            "/AA << /Fo << /S /Named /N /NextPage /Next 7 0 R >> >>".into(),
        ),
        (
            "a chain of four",
            vec![
                reset.clone(),
                "<< /S /URI /URI (https://example.com/) /Next 6 0 R >>".into(),
                "<< /S /JavaScript /JS (2;) /Next 7 0 R >>".into(),
                "<< /S /Named /N /NextPage /Next 8 0 R >>".into(),
            ],
            "/A 9 0 R".into(),
        ),
        (
            "a type given as a reference",
            vec!["/ResetForm".into()],
            "/AA << /Fo << /S 6 0 R >> >>".into(),
        ),
        (
            "names written with escapes",
            vec![],
            "/A#41 << /F#6F << /S /Reset#46orm >> >>".into(),
        ),
        (
            "an escaped /Next",
            vec![],
            "/AA << /Bl << /S /JavaScript /JS (1;) /Ne#78t << /S /Hi#64e /T (keep) /H true >> >> >>"
                .into(),
        ),
    ]
}

#[test]
fn a_widget_whose_own_action_reaches_a_reset_or_a_hide_is_left_alone_however_the_action_is_written()
{
    for packed in [false, true] {
        for (what, actions, extra) in own_reaches() {
            let actions: Vec<&str> = actions.iter().map(String::as_str).collect();
            let mut guard = engine();
            let engine = &mut guard.0;
            let (doc, _file) = open(engine, &kit::forms_action_pdf(&actions, &extra, packed));
            assert_only_the_target_left_alone(engine, doc, &format!("{what} (packed {packed})"));
            ask(engine, RequestBody::Close { doc });
        }
    }
}

#[test]
fn scripts_and_links_that_do_not_reach_a_reset_or_a_hide_block_nothing_next_to_a_reset_button() {
    // The document holds a reset (the button `clear`), and the widget has actions that are scripts,
    // links and a long chain of them that goes nowhere.
    let chain: Vec<String> = (0..5)
        .map(|i| {
            let next = if i < 4 {
                format!("/Next {} 0 R", 7 + i)
            } else {
                String::new()
            };
            format!("<< /S /JavaScript /JS ({i};) {next} >>")
        })
        .collect();
    let chain: Vec<&str> = chain.iter().map(String::as_str).collect();
    let cases: [(&str, &[&str], &str); 5] = [
        (
            "a script on focus",
            &[],
            "/AA << /Fo << /S /JavaScript /JS (1;) >> >>",
        ),
        (
            "a link",
            &[],
            "/A << /S /URI /URI (https://example.com/reset) >>",
        ),
        (
            "a script with /Next to a link",
            &[],
            "/AA << /Fo << /S /JavaScript /JS (1;) /Next << /S /URI /URI (https://example.com/) >> >> >>",
        ),
        ("a chain of scripts", &chain, "/AA << /Fo 6 0 R >>"),
        ("an empty /AA", &[], "/AA << >>"),
    ];
    for (what, actions, extra) in cases {
        let mut guard = engine();
        let engine = &mut guard.0;
        let (doc, _file) = open(engine, &kit::forms_action_pdf(actions, extra, false));
        let _ = ask(engine, RequestBody::FormInfo { doc });
        let list = fields(engine, doc, 0);
        assert_eq!(names(&list), ["keep", "plain", "target"], "{what}");
        for field in &list {
            assert!(
                !field.has(field_flag::ACTION_BLOCKED)
                    && !field.has(field_flag::FORM_BLOCKED)
                    && !field.has(field_flag::READ_ONLY),
                "{what}: {}: {:#x}",
                field.name,
                field.flags
            );
            // `keep` is what a reset would clear, so it is left as it is.
            if field.name == "keep" {
                continue;
            }
            match set(engine, doc, 0, field, value_for(field)) {
                ResponseBody::FormFields { fields: now } => {
                    assert_eq!(one(&now, "keep").value, "kept", "{what}: {}", field.name);
                }
                other => panic!("{what}: {}: {other:?}", field.name),
            }
        }
        ask(engine, RequestBody::Close { doc });
    }
}

#[test]
fn an_event_that_pdfium_reads_from_another_dictionary_that_reaches_a_reset_leaves_the_whole_form_alone(
) {
    // Keystroke, validate, format and calculate, with the action written in several ways, on the
    // widget's own /AA (PDFium looks for them on the field first, and the file does not say which
    // dictionary that is).
    let shapes: [(&str, &[&str], &str); 3] = [
        ("directly", &[], "<< /S /ResetForm >>"),
        (
            "through /Next",
            &[],
            "<< /S /JavaScript /JS (1;) /Next << /S /Hide /T (keep) /H true >> >>",
        ),
        ("as an indirect action", &["<< /S /ResetForm >>"], "6 0 R"),
    ];
    for event in ["K", "V", "F", "C"] {
        for (how, actions, action) in shapes {
            for packed in [false, true] {
                let mut guard = engine();
                let engine = &mut guard.0;
                let extra = format!("/AA << /{event} {action} >>");
                let (doc, _file) = open(engine, &kit::forms_action_pdf(actions, &extra, packed));
                assert_whole_form_left_alone(
                    engine,
                    doc,
                    &format!("/{event} {how} (packed {packed})"),
                );
                ask(engine, RequestBody::Close { doc });
            }
        }
    }
}

#[test]
fn two_dictionaries_with_one_full_name_do_not_let_a_reset_or_a_hide_through() {
    // PDFium joins them by name alone: no `/Kids` and no `/Parent` link the two. The `/AA` sits on
    // a plain object that no page shows, or on a widget of its own.
    let events = [
        ("a reset on the keystroke", "/K << /S /ResetForm >>"),
        ("a reset on the validate event", "/V << /S /ResetForm >>"),
        (
            "a hide of keep on the validate event",
            "/V << /S /Hide /T (keep) /H true >>",
        ),
    ];
    for (what, aa) in events {
        for holder_widget in [false, true] {
            let mut guard = engine();
            let engine = &mut guard.0;
            let (doc, _file) = open(engine, &kit::forms_same_name_pdf(aa, holder_widget));
            assert_whole_form_left_alone(
                engine,
                doc,
                &format!("{what} (holder is a widget: {holder_widget})"),
            );
            ask(engine, RequestBody::Close { doc });
        }
    }
}

#[test]
fn a_search_that_cannot_settle_leaves_the_whole_form_alone() {
    // A loop in /Next, a reference to nothing, and a chain that is too long: each counts as reaching
    // a reset, for the whole form, though no reset is in the chain.
    let long: Vec<String> = (0..40)
        .map(|i| {
            if i < 39 {
                format!("<< /S /JavaScript /JS (1;) /Next {} 0 R >>", 7 + i)
            } else {
                SCRIPT.to_string()
            }
        })
        .collect();
    let long: Vec<&str> = long.iter().map(String::as_str).collect();
    let loop_two = [
        "<< /S /JavaScript /JS (1;) /Next 7 0 R >>",
        "<< /S /JavaScript /JS (2;) /Next 6 0 R >>",
    ];
    let loop_one = ["<< /S /JavaScript /JS (1;) /Next 6 0 R >>"];
    let cases: [(&str, &[&str], &str); 7] = [
        ("a loop of two actions", &loop_two, "/AA << /Fo 6 0 R >>"),
        (
            "an action that is its own next",
            &loop_one,
            "/AA << /Bl 6 0 R >>",
        ),
        (
            "a /Next that is nowhere",
            &[],
            "/AA << /Fo << /S /JavaScript /JS (1;) /Next 900 0 R >> >>",
        ),
        ("an action that is nowhere", &[], "/AA << /Fo 900 0 R >>"),
        ("an /AA that is nowhere", &[], "/AA 900 0 R"),
        ("a chain of forty scripts", &long, "/AA << /Fo 6 0 R >>"),
        (
            "a /Next array that holds a loop",
            &["<< /S /JavaScript /JS (1;) /Next [ 6 0 R ] >>"],
            "/A 6 0 R",
        ),
    ];
    for (what, actions, extra) in cases {
        let mut guard = engine();
        let engine = &mut guard.0;
        let (doc, _file) = open(engine, &kit::forms_action_pdf(actions, extra, false));
        assert_whole_form_left_alone(engine, doc, what);
        ask(engine, RequestBody::Close { doc });
    }
    // A chain of five scripts goes nowhere and blocks nothing: it is the length that counts.
    let mut short_chain: Vec<String> = long.iter().take(5).map(|s| s.to_string()).collect();
    short_chain[4] = SCRIPT.to_string();
    let short_chain: Vec<&str> = short_chain.iter().map(String::as_str).collect();
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(
        engine,
        &kit::forms_action_pdf(&short_chain, "/AA << /Fo 6 0 R >>", false),
    );
    let _ = ask(engine, RequestBody::FormInfo { doc });
    assert!(fields(engine, doc, 0)
        .iter()
        .all(|f| !f.has(field_flag::ACTION_BLOCKED) && !f.has(field_flag::READ_ONLY)));
}

#[test]
fn ordinary_forms_with_a_clear_button_stay_fillable() {
    // A "Clear form" button (a reset) next to date and number fields with format, keystroke,
    // validate and calculate scripts, on one widget and on two, and radio groups. Scripts never run,
    // so none of them can clear anything.
    for two_widgets in [false, true] {
        let mut guard = engine();
        let engine = &mut guard.0;
        let (doc, _file) = open(engine, &kit::forms_clear_pdf(two_widgets));
        assert!(matches!(
            ask(engine, RequestBody::FormInfo { doc }),
            ResponseBody::FormInfo { .. }
        ));
        let list = fields(engine, doc, 0);
        let expected = if two_widgets { 9 } else { 7 };
        assert_eq!(list.len(), expected, "{:?}", names(&list));
        if !two_widgets {
            // The scripts are on the widget itself, so the field says it has actions.
            for name in ["born", "amount"] {
                assert!(one(&list, name).has(field_flag::ACTION));
            }
        }
        for field in &list {
            assert!(
                !field.has(field_flag::ACTION_BLOCKED)
                    && !field.has(field_flag::FORM_BLOCKED)
                    && !field.has(field_flag::READ_ONLY),
                "two_widgets {two_widgets}: {}: {:#x}",
                field.name,
                field.flags
            );
        }
        // Every one of them can be filled in, and no change clears `keep`.
        for round in 0..list.len() {
            let now = fields(engine, doc, 0);
            let field = &now[round];
            // `keep` is what a reset would clear, so it is left as it is.
            if field.name == "keep" {
                continue;
            }
            match set(engine, doc, 0, field, value_for(field)) {
                ResponseBody::FormFields { fields: after } => {
                    assert_eq!(
                        one(&after, "keep").value,
                        "kept",
                        "two_widgets {two_widgets}: {}",
                        field.name
                    );
                }
                other => panic!("two_widgets {two_widgets}: {}: {other:?}", field.name),
            }
        }
        let after = fields(engine, doc, 0);
        assert!(after
            .iter()
            .filter(|f| f.name == "choice")
            .any(|f| f.has(field_flag::CHECKED)));
        assert!(one(&after, "agree").has(field_flag::CHECKED));
        assert!(after
            .iter()
            .filter(|f| f.name == "born")
            .all(|f| f.value == "typed"));
        ask(engine, RequestBody::Close { doc });
    }
}

// ---- widgets that the file and PDFium count differently ----------------------------------------------

#[test]
fn an_aa_given_as_a_reference_to_a_reference_is_read() {
    // PDFium follows a reference to a reference when it looks up `/AA`. Objects are numbered from 6.
    let reset = "<< /Fo << /S /ResetForm >> >>";
    let target_cases: [(&str, &[&str], &str); 3] = [
        ("a reset on focus", &["7 0 R", reset], "/AA 6 0 R"),
        (
            "a hide on blur",
            &["7 0 R", "<< /Bl << /S /Hide /T (keep) /H true >> >>"],
            "/AA 6 0 R",
        ),
        (
            "three references deep",
            &["7 0 R", "8 0 R", reset],
            "/AA 6 0 R",
        ),
    ];
    let whole_cases: [(&str, &[&str], &str); 3] = [
        (
            "a reset on the keystroke",
            &["7 0 R", "<< /K << /S /ResetForm >> >>"],
            "/AA 6 0 R",
        ),
        (
            "a reset on the validate event",
            &["7 0 R", "<< /V << /S /ResetForm >> >>"],
            "/AA 6 0 R",
        ),
        (
            "a hide on the validate event",
            &["7 0 R", "<< /V << /S /Hide /T (keep) /H true >> >>"],
            "/AA 6 0 R",
        ),
    ];
    for packed in [false, true] {
        for (what, actions, extra) in target_cases {
            let mut guard = engine();
            let engine = &mut guard.0;
            let (doc, _file) = open(engine, &kit::forms_action_pdf(actions, extra, packed));
            assert_only_the_target_left_alone(engine, doc, &format!("{what} (packed {packed})"));
            ask(engine, RequestBody::Close { doc });
        }
        for (what, actions, extra) in whole_cases {
            let mut guard = engine();
            let engine = &mut guard.0;
            let (doc, _file) = open(engine, &kit::forms_action_pdf(actions, extra, packed));
            assert_whole_form_left_alone(engine, doc, &format!("{what} (packed {packed})"));
            ask(engine, RequestBody::Close { doc });
        }
    }
}

#[test]
fn a_widget_that_is_none_in_pdfium_cannot_stand_in_for_the_real_one() {
    // The decoy comes first in the page's list and has the box and the keys of `target`, but it is
    // an annotation of no kind for PDFium. The widgets after it must not be taken for the wrong ones.
    let link = "/AA << /Fo << /S /URI /URI (https://example.com/) >> >>";
    let reset = "/AA << /Fo << /S /ResetForm >> >>";
    let hide = "/AA << /Bl << /S /Hide /T (keep) /H true >> >>";
    for as_stream in [false, true] {
        for (what, decoy, target) in [
            ("a reset on focus", link, reset),
            (
                "a hide on blur",
                "/AA << /Bl << /S /URI /URI (https://example.com/) >> >>",
                hide,
            ),
        ] {
            let mut guard = engine();
            let engine = &mut guard.0;
            let (doc, _file) = open(engine, &kit::forms_decoy_pdf(as_stream, decoy, target));
            assert_only_the_target_left_alone(
                engine,
                doc,
                &format!("{what} (decoy is a stream: {as_stream})"),
            );
            ask(engine, RequestBody::Close { doc });
        }
    }
}

/// `date`, an ordinary text field with an action of its own, can be filled in and nothing is
/// reset or hidden by it; no field of the form is left alone.
fn assert_date_is_filled_in(engine: &mut Engine, doc: u32, what: &str) {
    let _ = ask(engine, RequestBody::FormInfo { doc });
    let list = fields(engine, doc, 0);
    let mut sorted = names(&list);
    sorted.sort_unstable();
    assert!(
        sorted.starts_with(&["date", "keep", "plain"]),
        "{what}: {sorted:?}"
    );
    for field in &list {
        assert!(
            !field.has(field_flag::ACTION_BLOCKED)
                && !field.has(field_flag::FORM_BLOCKED)
                && !field.has(field_flag::READ_ONLY),
            "{what}: {}: {:#x}",
            field.name,
            field.flags
        );
    }
    let date = one(&list, "date");
    assert!(date.has(field_flag::ACTION), "{what}: {:#x}", date.flags);
    match set(engine, doc, 0, &date, FormValue::Text("typed".into())) {
        ResponseBody::FormFields { fields: now } => {
            assert_eq!(one(&now, "keep").value, "kept", "{what}");
            assert_eq!(one(&now, "date").value, "typed", "{what}");
            assert_eq!(names(&now).len(), list.len(), "{what} hid a field");
        }
        other => panic!("{what}: {other:?}"),
    }
}

const DATE_SCRIPT: &str =
    r#"/AA << /F << /S /JavaScript /JS (AFDate_FormatEx\("dd.mm.yyyy"\);) >> >>"#;
const LINK: &str = "/A << /S /URI /URI (https://example.com/) >>";

#[test]
fn a_reset_or_a_hide_on_a_dictionary_with_no_box_does_not_block_a_field_with_the_same_keys() {
    // PDFium never runs these from this engine: the events of a page and a document are not
    // called, a bookmark is not followed, and `Fo` and `Bl` are not read from a parent. A field with
    // a format script (or a link) is an ordinary field and is filled in.
    let cases: [(&str, kit::Unplaced); 6] = [
        (
            "page open, hide",
            kit::Unplaced {
                page: "/AA << /O << /S /Hide /T (keep) /H true >> >>",
                date: DATE_SCRIPT,
                clear: true,
                ..Default::default()
            },
        ),
        (
            "page open, reset, no Clear button",
            kit::Unplaced {
                page: "/AA << /O << /S /ResetForm >> >>",
                date: DATE_SCRIPT,
                ..Default::default()
            },
        ),
        (
            "catalog, reset on close",
            kit::Unplaced {
                catalog: "/AA << /WC << /S /ResetForm >> >>",
                date: DATE_SCRIPT,
                clear: true,
                ..Default::default()
            },
        ),
        (
            "field above the widget, reset on focus",
            kit::Unplaced {
                parent: Some("/AA << /Fo << /S /ResetForm >> >>"),
                date: DATE_SCRIPT,
                clear: true,
                ..Default::default()
            },
        ),
        (
            "bookmark, reset",
            kit::Unplaced {
                objects: &[
                    "<< /Type /Outlines /First 7 0 R /Last 7 0 R /Count 1 >>",
                    "<< /Title (b) /Parent 6 0 R /A << /S /ResetForm >> >>",
                ],
                catalog: "/Outlines 6 0 R",
                date: LINK,
                clear: true,
                ..Default::default()
            },
        ),
        (
            "field above the widget, reset on release",
            kit::Unplaced {
                parent: Some("/A << /S /ResetForm >>"),
                date: LINK,
                clear: true,
                ..Default::default()
            },
        ),
    ];
    for (what, spec) in &cases {
        let mut guard = engine();
        let engine = &mut guard.0;
        let (doc, _file) = open(engine, &kit::forms_unplaced_pdf(spec));
        assert_date_is_filled_in(engine, doc, what);
        ask(engine, RequestBody::Close { doc });
    }
}

#[test]
fn a_box_written_with_references_is_still_the_box_of_the_widget() {
    // PDFium reads the box through references. With a Clear button in the file, the field is
    // matched to its own record by its box, so the box has to be read the same way here.
    let cases: [(&str, &[&str], &str); 3] = [
        ("the box is a reference", &["[100 660 300 680]"], "6 0 R"),
        (
            "every number is a reference",
            &["100", "660", "300", "680"],
            "[6 0 R 7 0 R 8 0 R 9 0 R]",
        ),
        ("one number is a reference", &["680"], "[100 660 300 6 0 R]"),
    ];
    for (what, objects, rect) in cases {
        let mut guard = engine();
        let engine = &mut guard.0;
        let spec = kit::Unplaced {
            objects,
            date: DATE_SCRIPT,
            rect: Some(rect),
            clear: true,
            ..Default::default()
        };
        let (doc, _file) = open(engine, &kit::forms_unplaced_pdf(&spec));
        assert_date_is_filled_in(engine, doc, what);
        ask(engine, RequestBody::Close { doc });
    }
}

#[test]
fn a_second_trailer_in_a_stream_and_a_trailer_that_cannot_be_read_steer_nothing() {
    // A metadata stream holds the text of another trailer and of a document whose widget at the
    // place of `target` is harmless. The real trailer can be read, or it holds a value nested too
    // deep for the search.
    let deep = format!("/Z {}1{}", "[".repeat(55), "]".repeat(55));
    for (what, trailer) in [
        ("a readable trailer", ""),
        ("a trailer nested too deep", deep.as_str()),
    ] {
        let mut guard = engine();
        let engine = &mut guard.0;
        let (doc, _file) = open(engine, &kit::forms_trailer_pdf(trailer, ""));
        assert_only_the_target_left_alone(engine, doc, what);
        ask(engine, RequestBody::Close { doc });
    }
}

#[test]
fn a_page_tree_with_holes_does_not_move_a_widget_away_from_its_action() {
    // Kids that name objects that are not there make PDFium number the pages as it meets them, so the
    // same request can be answered from another page later. `target` is left alone every time it is
    // listed, whatever pages were asked for before.
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_holes_pdf());
    let _ = ask(engine, RequestBody::FormInfo { doc });
    let mut listed = 0;
    for page in [0u32, 1, 2, 3, 0, 2, 1, 3, 1, 3] {
        let ResponseBody::FormFields { fields: list } =
            ask(engine, RequestBody::ListFormFields { doc, page })
        else {
            continue;
        };
        for field in &list {
            if field.name == "target" {
                listed += 1;
                assert!(
                    field.has(field_flag::ACTION_BLOCKED) && field.has(field_flag::READ_ONLY),
                    "page {page}: {:#x}",
                    field.flags
                );
                assert!(
                    matches!(
                        set(engine, doc, page, field, value_for(field)),
                        ResponseBody::Error {
                            code: ErrorCode::FormFieldRefused,
                            ..
                        }
                    ),
                    "page {page}"
                );
            }
            if field.name == "keep" || field.name == "other" {
                assert!(
                    !field.has(field_flag::ACTION_BLOCKED),
                    "page {page}: {}",
                    field.name
                );
            }
        }
        if let Some(keep) = list.iter().find(|f| f.name == "keep") {
            assert_eq!(keep.value, "kept", "page {page}");
        }
    }
    assert!(listed >= 2, "target was listed {listed} times");
}

#[test]
fn a_stream_of_trailer_words_does_not_hold_the_search_up() {
    // The same form with 1 MiB of repeats of the word that used to start a search of its own, in
    // the stream. The form is still read, and still quickly.
    let bytes = kit::forms_trailer_pdf("", &"trailer(".repeat(1 << 17));
    let started = std::time::Instant::now();
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &bytes);
    let _ = ask(engine, RequestBody::FormInfo { doc });
    let list = fields(engine, doc, 0);
    assert!(one(&list, "target").has(field_flag::ACTION_BLOCKED));
    assert!(!one(&list, "plain").has(field_flag::ACTION_BLOCKED));
    assert!(started.elapsed().as_secs() < 30, "{:?}", started.elapsed());
    ask(engine, RequestBody::Close { doc });
}

// ---- the same forms, rewritten by another program ---------------------------------------------------

fn required() -> bool {
    std::env::var_os("PARTRIDGE_REQUIRE_READERS").is_some()
}

fn have_qpdf() -> bool {
    let found = std::process::Command::new("qpdf")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !found {
        assert!(!required(), "qpdf is required and is not installed");
        eprintln!("qpdf is not installed: its checks are skipped");
    }
    found
}

/// The file written again by qpdf in four ways: object streams, linearized, RC4 128 and AES 256 (the
/// last two with an empty user password).
fn qpdf_variants(bytes: &[u8]) -> Vec<(&'static str, Vec<u8>)> {
    let dir = std::env::temp_dir().join(format!("partridge-forms-qpdf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("in.pdf");
    std::fs::write(&input, bytes).unwrap();
    let ways: [(&str, &[&str]); 4] = [
        ("object streams", &["--object-streams=generate"]),
        ("linearized", &["--linearize"]),
        (
            "RC4 128",
            &["--allow-weak-crypto", "--encrypt", "", "owner", "128", "--"],
        ),
        ("AES 256", &["--encrypt", "", "owner", "256", "--"]),
    ];
    let mut out = Vec::new();
    for (what, args) in ways {
        let output = dir.join(format!("{}.pdf", what.replace(' ', "-")));
        let status = std::process::Command::new("qpdf")
            .args(args)
            .arg(&input)
            .arg(&output)
            .output()
            .unwrap();
        // 3 is a warning that still wrote the file.
        assert!(
            status.status.success() || status.status.code() == Some(3),
            "qpdf {what}: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        out.push((what, std::fs::read(&output).unwrap()));
    }
    let _ = std::fs::remove_dir_all(&dir);
    out
}

#[test]
fn forms_rewritten_by_qpdf_are_judged_as_the_plain_files_are() {
    if !have_qpdf() {
        return;
    }
    // Ordinary forms stay fillable.
    for two_widgets in [false, true] {
        for (what, bytes) in qpdf_variants(&kit::forms_clear_pdf(two_widgets)) {
            let what = format!("{what}, two widgets {two_widgets}");
            let mut guard = engine();
            let engine = &mut guard.0;
            let (doc, _file) = open(engine, &bytes);
            let _ = ask(engine, RequestBody::FormInfo { doc });
            let list = fields(engine, doc, 0);
            assert_eq!(list.len(), if two_widgets { 9 } else { 7 }, "{what}");
            for field in &list {
                assert!(
                    !field.has(field_flag::ACTION_BLOCKED)
                        && !field.has(field_flag::FORM_BLOCKED)
                        && !field.has(field_flag::READ_ONLY),
                    "{what}: {}: {:#x}",
                    field.name,
                    field.flags
                );
            }
            let field = list.iter().find(|f| f.name == "born").unwrap().clone();
            match set(engine, doc, 0, &field, value_for(&field)) {
                ResponseBody::FormFields { fields: after } => {
                    assert_eq!(one(&after, "keep").value, "kept", "{what}");
                }
                other => panic!("{what}: {other:?}"),
            }
            ask(engine, RequestBody::Close { doc });
        }
    }
    // A field with a reset of its own is still left alone, and only that one.
    let plain = kit::forms_action_pdf(&[], "/AA << /Fo << /S /ResetForm >> >>", false);
    for (what, bytes) in qpdf_variants(&plain) {
        let mut guard = engine();
        let engine = &mut guard.0;
        let (doc, _file) = open(engine, &bytes);
        assert_only_the_target_left_alone(engine, doc, what);
        ask(engine, RequestBody::Close { doc });
    }
}
