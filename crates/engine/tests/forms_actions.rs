//! What filling in a form must not do, and what it must do for every widget of a field.
//!
//! - No action of the PDF changes the document while a field is filled in: not a reset, not a hide,
//!   whichever event of whichever field carries it (focus, blur, press, release, keystroke, format,
//!   validate, calculate), also when it sits on the field above a widget or above that, and also
//!   when the file keeps those fields in an object stream.
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
    let blocked = [
        "box_a", "box_u", "box_d", "text_fo", "text_bl", "text_k", "text_f", "text_v", "text_c",
    ];
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
    for name in ["keep", "victim", "plain", "plain_box", "under_box"] {
        let field = one(&list, name);
        assert!(
            !field.has(field_flag::ACTION_BLOCKED) && !field.has(field_flag::READ_ONLY),
            "{name}"
        );
    }
    // The buttons are not fields the reader fills in.
    assert!(list
        .iter()
        .all(|f| f.name != "over" && f.name != "over_first"));
    // Nothing the refused requests did changed the page.
    assert_eq!(fields(engine, doc, 0), list);
}

#[test]
fn an_event_on_the_field_above_a_widget_that_pdfium_reads_from_the_widget_alone_still_leaves_every_widget_alone(
) {
    // The blur and focus events of a widget are read from the widget alone, so the reset on the
    // field above `par` cannot fire. The rule does not tell them apart from the events PDFium reads
    // through the parent, and it cannot tell which widget sits under which field, so it leaves
    // every widget of the form alone, also the ones with no parent. That is wider than needed.
    let mut guard = engine();
    let engine = &mut guard.0;
    let (doc, _file) = open(engine, &kit::forms_parent_event_pdf());
    let _ = ask(engine, RequestBody::FormInfo { doc });
    let list = fields(engine, doc, 0);
    for name in ["keep", "box_a", "par"] {
        let field = one(&list, name);
        assert!(
            field.has(field_flag::ACTION_BLOCKED) && field.has(field_flag::READ_ONLY),
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
    assert_eq!(one(&fields(engine, doc, 0), "keep").value, "kept");
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
                    && field.has(field_flag::ACTION),
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
        // The field with no parent is left alone too: the plain interface does not say which widget
        // sits under which field, so one action above a widget stops them all.
        let keep = one(&list, "keep");
        assert!(keep.has(field_flag::ACTION_BLOCKED) && keep.has(field_flag::READ_ONLY));
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
                        && field.has(field_flag::ACTION),
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

#[test]
fn the_search_for_actions_above_a_widget_follows_parents_and_ignores_the_rest() {
    use partridge_engine::forms::holds_parent_action_for_tests as holds;
    // A field named as a parent by a widget, with additional actions.
    assert!(holds(
        b"%PDF-1.5\r\n4 0 obj\r\n<</Type/Annot/Parent 5 0 R>>\r\nendobj\r\n5 0 obj\r\n<</T(a)/AA<</K 6 0 R>>>>\r\nendobj\r\n"
    ));
    // Two levels: the actions sit on the parent of the parent.
    assert!(holds(
        b"4 0 obj\r\n<</Parent 5 0 R>>\r\nendobj\r\n5 0 obj\r\n<</Parent 6 0 R>>\r\nendobj\r\n6 0 obj\r\n<</AA<<>>>>\r\nendobj\r\n"
    ));
    // The order of the objects does not matter.
    assert!(holds(
        b"6 0 obj\r\n<</AA<<>>>>\r\nendobj\r\n4 0 obj\r\n<</Parent 6 0 R>>\r\nendobj\r\n"
    ));
    // A parent written in place.
    assert!(holds(
        b"4 0 obj\r\n<</Parent<</T(a)/AA<<>>>>>>\r\nendobj\r\n"
    ));
    // A text that looks like an end of object does not hide the actions behind it.
    assert!(holds(
        b"4 0 obj\r\n<</Parent 5 0 R>>\r\nendobj\r\n5 0 obj\r\n<</T(endobj)/AA<<>>>>\r\nendobj\r\n"
    ));
    // Actions on a widget that has no parent, and a parent without actions, are not it.
    assert!(!holds(
        b"4 0 obj\r\n<</Subtype/Widget/AA<<>>>>\r\nendobj\r\n"
    ));
    assert!(!holds(
        b"4 0 obj\r\n<</Parent 5 0 R>>\r\nendobj\r\n5 0 obj\r\n<</T(a)>>\r\nendobj\r\n"
    ));
    // A field with kids and additional actions, whatever its widgets say about their parents.
    assert!(holds(
        b"7 0 obj\r\n<</FT/Tx/T(t)/AA<</K 9 0 R>>/Kids[8 0 R 6 0 R]>>\r\nendobj\r\n"
    ));
    assert!(holds(
        b"4 0 obj\r\n<</Subtype/Widget/T(t)>>\r\nendobj\r\n7 0 obj\r\n<</Kids[4 0 R]/AA<<>>>>\r\nendobj\r\n"
    ));
    // Kids without actions (the page tree, a field), and other names that start the same way.
    assert!(!holds(
        b"2 0 obj\r\n<</Type/Pages/Kids[3 0 R]/Count 1>>\r\nendobj\r\n"
    ));
    assert!(!holds(
        b"7 0 obj\r\n<</KidsOfNobody[8 0 R]/AA<<>>>>\r\nendobj\r\n"
    ));
    // The page tree's parents and an object with actions that nothing names as a parent.
    assert!(!holds(
        b"3 0 obj\r\n<</Parent 2 0 R>>\r\nendobj\r\n7 0 obj\r\n<</AA<<>>>>\r\nendobj\r\n"
    ));
    // Other names that start the same way.
    assert!(!holds(
        b"4 0 obj\r\n<</ParentTree 5 0 R>>\r\nendobj\r\n5 0 obj\r\n<</AAA 1>>\r\nendobj\r\n"
    ));
    assert!(!holds(b""));
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
