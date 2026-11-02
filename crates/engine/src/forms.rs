//! Filling in forms.
//!
//! The reader lists the fields of a page and sets one field at a time. Both go through PDFium's own
//! form code, the same code that Chrome uses, because a value is only half of what a field needs:
//! the appearance of the field (the picture that other readers draw) has to change with it, and a
//! check box or a radio button has a state in two places that have to agree. PDFium keeps all of
//! that right when the change is made the way a person makes it: focus the field, type, leave it.
//! So that is what `set` does, with the focus, the typing and the click done by function calls.
//!
//! # Actions
//!
//! PDF JavaScript is off: the form environment is made without a JavaScript platform, so no script
//! runs. That is all that switch does. PDFium runs the other actions of a field itself when the
//! field gets the focus, is pressed or is left, and two of them change the document: a reset of the
//! form (`ResetForm`) clears fields, and a hide (`Hide`) writes the hidden flag into the widgets of
//! other fields. The rest (a link, a jump, a launch, a submit, a named action) end in a callback
//! that this module does not give, so they do nothing.
//!
//! So the module makes sure that no action that changes the document can fire:
//!
//! - When the environment is made (while the window asks what kind of form the document has, which
//!   is a request that may take long), the document is written out once and searched for the names
//!   `/ResetForm` and `/Hide` (the written file has every object in the open, also those the
//!   original keeps in compressed streams). A document that has neither cannot reset or hide
//!   anything, and nothing more is needed. If a read of the file failed or the file changed
//!   meanwhile, the written document may lack objects, and the document is treated as holding one.
//! - In a document that has one of them, a widget that has an action of its own (`/A` or `/AA`) is
//!   not filled in: it is listed read only and marked, and a request for it is refused.
//! - PDFium reads some events of a widget from the field above it, not from the widget: the
//!   dictionary of additional actions is looked up through `/Parent`, up to 32 levels, and its
//!   keystroke and validate events can hold a reset or a hide as well as a script. The plain
//!   interface does not show those actions, so the written document is searched for them too: when
//!   an object that some other object names as its `/Parent` has additional actions (`/AA`), every
//!   widget that has a `/Parent` is treated like a widget with an action of its own.
//!   (That is wider than needed: it also catches the events PDFium reads from the widget alone.
//!   Narrowing it to the chain of one widget is left for later.)
//! - The widget that is named is the one that gets the focus and the key, never the widget found at
//!   a point, so a button that lies over a field is never pressed.
//! - No push button is listed, and nothing calls the functions that run the actions of a document
//!   or of a page.
//!
//! Everything a field says about itself (name, tooltip, value, the labels of its choices) is
//! untrusted. Strings are read with a limit and cut again by the protocol, and the number of fields
//! and of choices is capped.
//!
//! # The document handle
//!
//! `pdfium-render` keeps the handle of the document private, and its own form environment keeps the
//! form handle private as well. The calls below need both, so the document handle is read from the
//! pointer that the library prints for debugging (`{:?}` of a document is `PdfDocument {
//! FPDF_DOCUMENT: "0x..." }`), and a form environment of the engine's own is made on it. The crate
//! is pinned to an exact version and a test fails if the print changes. Before the pointer is used
//! it is checked: PDFium must report the page count the library reports for the same document.
//! The two environments do not interfere: pages are loaded again for every request, and everything
//! PDFium keeps for a loaded page goes away with it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::os::raw::{c_int, c_ulong};
use std::pin::Pin;
use std::ptr;

use partridge_protocol::{
    field_flag, ErrorCode, FormField, FormFieldKind, FormKind, FormOption, FormValue, ResponseBody,
    MAX_FORM_FIELDS_PER_PAGE, MAX_FORM_NAME, MAX_FORM_OPTIONS, MAX_FORM_OPTION_LABEL,
    MAX_FORM_TOOLTIP, MAX_FORM_VALUE, MAX_SAVE_BYTES,
};
use pdfium_render::prelude::*;

use crate::source::SourceGuard;
use crate::Failure;

/// `FPDF_ANNOT_WIDGET`.
const ANNOT_WIDGET: i32 = 20;

// `FPDF_FORMFIELD_*`.
const FIELD_CHECKBOX: i32 = 2;
const FIELD_RADIOBUTTON: i32 = 3;
const FIELD_COMBOBOX: i32 = 4;
const FIELD_LISTBOX: i32 = 5;
const FIELD_TEXTFIELD: i32 = 6;

// `FORMTYPE_*`.
const FORMTYPE_ACRO_FORM: i32 = 1;

// Bits of the annotation flags (`/F`).
const ANNOT_HIDDEN: i32 = 1 << 1;
const ANNOT_NO_VIEW: i32 = 1 << 5;
const ANNOT_READ_ONLY: i32 = 1 << 6;

// Bits of the field flags (`/Ff`).
const FF_READ_ONLY: i32 = 1;
const FF_REQUIRED: i32 = 1 << 1;
const FF_MULTILINE: i32 = 1 << 12;
const FF_PASSWORD: i32 = 1 << 13;
const FF_NO_TOGGLE_TO_OFF: i32 = 1 << 14;
const FF_COMBO_EDITABLE: i32 = 1 << 18;
const FF_MULTISELECT: i32 = 1 << 21;
const FF_COMB: i32 = 1 << 24;

/// The names of the two actions that change a document when they fire.
const STATE_ACTIONS: [&[u8]; 2] = [b"/ResetForm", b"/Hide"];

/// Most bytes (UTF-16) of text that one listing of a page reads from PDFium. A hostile document can
/// make a thousand fields share one list of choices; this keeps the work of a listing bounded.
const MAX_LIST_READ_BYTES: usize = 4 * 1024 * 1024;

/// Most pages looked at for the other widgets of a field, and most that are kept loaded.
const MAX_SIBLING_SCAN: usize = 5_000;
const MAX_SIBLING_PAGES: usize = 64;

/// `/P` bits of the permissions: fill in form fields (9) and change annotations, which includes
/// filling in forms in older files (6).
const PERMIT_FILL: u32 = (1 << 8) | (1 << 5);

/// The words of a script, wherever the file keeps them.
const ACTION_WORDS: [&[u8]; 2] = [b"/JavaScript", b"/JS"];

/// Most annotations of a page that are looked at. A page cannot make the engine walk a list of
/// millions of them.
const MAX_ANNOTS_SCANNED: usize = 20_000;

/// Most bytes (UTF-16) of one string that are read from PDFium. A longer string is not read; it is
/// listed as empty (and, for a value, marked cut).
const MAX_READ_BYTES: usize = 1024 * 1024;

/// Precision of the page to device conversion: PDFium answers in whole pixels, so the conversion
/// asks for a bitmap that is this many times larger than the page and divides the answer.
const PRECISION: f32 = 16.0;

/// The form environment of one document.
pub struct FormEnv {
    bindings: &'static dyn PdfiumLibraryBindings,
    document: FPDF_DOCUMENT,
    handle: FPDF_FORMHANDLE,
    /// PDFium keeps the pointer to this until the environment is closed, so it must not move.
    _info: Pin<Box<FPDF_FORMFILLINFO>>,
    /// The document holds a reset or a hide action somewhere (or could not be searched).
    state_actions: bool,
    /// The document holds a reset or a hide, and a field above a widget has additional actions (or
    /// the document could not be searched): a widget that has a `/Parent` is left alone.
    parent_actions: bool,
    /// The permissions of the document allow filling in forms.
    may_fill: bool,
    /// The pages that hold widgets of a field that has them on more than one page, by full name,
    /// found by the first change of that field. Looking for them loads pages, so it is done once.
    spread: RefCell<HashMap<String, Vec<u32>>>,
}

impl Drop for FormEnv {
    fn drop(&mut self) {
        // SAFETY: the handle was made by `FPDFDOC_InitFormFillEnvironment` and is closed once. The
        // engine drops this before the document it belongs to.
        unsafe { self.bindings.FPDFDOC_ExitFormFillEnvironment(self.handle) };
    }
}

/// The handle PDFium has for a document, or `None` when it cannot be found out safely.
pub fn document_handle(
    bindings: &dyn PdfiumLibraryBindings,
    document: &PdfDocument<'_>,
) -> Option<FPDF_DOCUMENT> {
    let text = format!("{document:?}");
    let digits: String = text
        .split("0x")
        .nth(1)?
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    let address = usize::from_str_radix(&digits, 16).ok()?;
    if address == 0 || address % std::mem::align_of::<usize>() != 0 {
        return None;
    }
    let handle = address as FPDF_DOCUMENT;
    // SAFETY: the pointer is the one the library holds for this document; the count is how it is
    // checked, and it is only a read.
    let count = unsafe { bindings.FPDF_GetPageCount(handle) };
    (count >= 0 && count as usize == document.pages().len() as usize).then_some(handle)
}

/// True when `bytes` holds the PDF name `name` (such as `/Hide`) as a whole name: the byte after it
/// is white space or a delimiter, so `/HideToolbar` is not `/Hide`.
fn has_name(bytes: &[u8], name: &[u8]) -> bool {
    let regular = is_regular;
    let mut from = 0;
    while let Some(at) = bytes[from..]
        .windows(name.len())
        .position(|window| window == name)
    {
        let end = from + at + name.len();
        if bytes.get(end).is_none_or(|b| !regular(*b)) {
            return true;
        }
        from = from + at + 1;
    }
    false
}

/// Whether a document written out holds an action that changes a document when it fires.
fn holds_state_action(bytes: &[u8]) -> bool {
    STATE_ACTIONS.iter().any(|name| has_name(bytes, name))
}

/// Whether `b` can be part of a name or a number (it is not white space or a delimiter).
fn is_regular(b: u8) -> bool {
    !matches!(
        b,
        0 | 9
            | 10
            | 12
            | 13
            | 32
            | b'('
            | b')'
            | b'<'
            | b'>'
            | b'['
            | b']'
            | b'{'
            | b'}'
            | b'/'
            | b'%'
    )
}

fn skip_space(bytes: &[u8], mut at: usize) -> usize {
    while bytes
        .get(at)
        .is_some_and(|b| matches!(b, 0 | 9 | 10 | 12 | 13 | 32))
    {
        at += 1;
    }
    at
}

/// The unsigned number that starts at `at`, and where it ends. At most ten digits.
fn number_at(bytes: &[u8], at: usize) -> Option<(u64, usize)> {
    let digits = bytes[at.min(bytes.len())..]
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    if digits == 0 || digits > 10 {
        return None;
    }
    let text = std::str::from_utf8(&bytes[at..at + digits]).ok()?;
    Some((text.parse().ok()?, at + digits))
}

/// The number of the object whose header (`12 0 obj`) starts at `at`.
fn header_at(bytes: &[u8], at: usize) -> Option<u64> {
    let (number, end) = number_at(bytes, at)?;
    let next = skip_space(bytes, end);
    if next == end {
        return None;
    }
    let (_, end) = number_at(bytes, next)?;
    let next = skip_space(bytes, end);
    if next == end || !bytes[next..].starts_with(b"obj") {
        return None;
    }
    bytes
        .get(next + 3)
        .is_none_or(|b| !is_regular(*b))
        .then_some(number)
}

/// The objects of a document as PDFium writes it, each with its number and its bytes (up to the
/// header of the next one). A writer puts every object on lines of its own: a string in a written
/// dictionary has its line breaks and brackets escaped, so a header at the start of a line is the
/// start of an object, whatever the strings in the objects before it say. A stream can hold any
/// bytes, and a header-like line in one only cuts the stream into more parts, which can make the
/// search find more, never less.
fn written_objects(bytes: &[u8]) -> Vec<(u64, &[u8])> {
    let mut starts: Vec<(usize, u64)> = Vec::new();
    for at in 0..bytes.len() {
        let line_start = at == 0 || matches!(bytes[at - 1], b'\r' | b'\n');
        if line_start && bytes[at].is_ascii_digit() {
            if let Some(number) = header_at(bytes, at) {
                starts.push((at, number));
            }
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(i, (start, number))| {
            let end = starts.get(i + 1).map_or(bytes.len(), |(next, _)| *next);
            (*number, &bytes[*start..end])
        })
        .collect()
}

/// Whether a written document holds a field above a widget that has additional actions.
///
/// PDFium reads the keystroke and validate events (and format and calculate, which only hold
/// scripts here) of a widget from its field, looking up `/Parent` as far as 32 levels. Those events
/// can hold a reset or a hide. So: the objects that are named as a `/Parent` by some object are
/// the fields above widgets (a grandparent is the `/Parent` of a parent), and any of them with an
/// `/AA` counts. A `/Parent` that is a dictionary written in place counts when the object holds an
/// `/AA` at all. More than needed is caught on purpose: an `/AA` in the same object as a `/Parent`
/// is not told apart from one that sits in a different dictionary of it.
fn holds_parent_action(bytes: &[u8]) -> bool {
    let mut parents: Vec<u64> = Vec::new();
    let mut with_actions: Vec<u64> = Vec::new();
    for (number, body) in written_objects(bytes) {
        let actions = has_name(body, b"/AA");
        if actions {
            with_actions.push(number);
        }
        let mut from = 0;
        while let Some(at) = body[from..]
            .windows(b"/Parent".len())
            .position(|w| w == b"/Parent")
        {
            let end = from + at + b"/Parent".len();
            from = end;
            if body.get(end).is_some_and(|b| is_regular(*b)) {
                continue;
            }
            let value = skip_space(body, end);
            if body[value.min(body.len())..].starts_with(b"<<") {
                if actions {
                    return true;
                }
                continue;
            }
            if let Some((parent, after)) = number_at(body, value) {
                let next = skip_space(body, after);
                if let Some((_, after)) = number_at(body, next) {
                    let r = skip_space(body, after);
                    if body.get(r) == Some(&b'R') {
                        parents.push(parent);
                    }
                }
            }
        }
    }
    parents.sort_unstable();
    with_actions
        .iter()
        .any(|number| parents.binary_search(number).is_ok())
}

fn refused(message: &str) -> Failure {
    (ErrorCode::FormFieldRefused, message.to_string())
}

fn unsupported(message: &str) -> Failure {
    (ErrorCode::FormUnsupported, message.to_string())
}

/// What kind of form a document has, and whether it has scripts the reader does not run.
pub fn info(
    pdfium: &'static Pdfium,
    document: &PdfDocument<'_>,
    source: Option<&std::sync::Mutex<SourceGuard>>,
    progress: &mut dyn FnMut(u32),
) -> Result<ResponseBody, Failure> {
    let bindings = pdfium.bindings();
    // A document whose handle cannot be found out is not a document without a form: say so.
    let Some(handle) = document_handle(bindings, document) else {
        return Err(unsupported("the form of this document cannot be reached"));
    };
    let kind = form_kind(bindings, handle);
    // The words of a script, wherever the file keeps them. PDFium's own count is of the document
    // level scripts; the words catch the others that are not packed away in compressed objects.
    // When the file cannot be scanned, the form is treated as having actions.
    let has_actions = kind != FormKind::None && {
        // SAFETY: a read of the count of scripts; nothing is run.
        let scripts = unsafe { bindings.FPDFDoc_GetJavaScriptActionCount(handle) } > 0;
        scripts
            || match source {
                Some(guard) => crate::source::lock(guard)
                    .contains_any(&ACTION_WORDS, progress)
                    .unwrap_or(true),
                None => false,
            }
    };
    Ok(ResponseBody::FormInfo { kind, has_actions })
}

fn form_kind(bindings: &dyn PdfiumLibraryBindings, handle: FPDF_DOCUMENT) -> FormKind {
    // SAFETY: a read.
    match unsafe { bindings.FPDF_GetFormType(handle) } {
        0 => FormKind::None,
        FORMTYPE_ACRO_FORM => FormKind::Acro,
        _ => FormKind::Xfa,
    }
}

impl FormEnv {
    /// Makes the environment for a document, or says why the reader cannot fill it in.
    ///
    /// The document is written out once to look for actions, which reads the whole file. `progress`
    /// is told now and then, so a request that may take long can show that it works. `source` is the
    /// file the document is read from: if a read of it failed or it changed, the written document
    /// may lack objects (PDFium leaves out what it cannot read and goes on), and the document is
    /// treated as holding an action.
    pub fn create(
        pdfium: &'static Pdfium,
        document: &PdfDocument<'_>,
        source: Option<&std::sync::Mutex<SourceGuard>>,
        progress: &mut dyn FnMut(u32),
    ) -> Result<FormEnv, Failure> {
        let bindings = pdfium.bindings();
        let Some(handle) = document_handle(bindings, document) else {
            return Err(unsupported("the form of this document cannot be reached"));
        };
        match form_kind(bindings, handle) {
            FormKind::Acro => {}
            FormKind::Xfa => {
                return Err(unsupported(
                    "this form is an XFA form, which the reader does not support",
                ))
            }
            FormKind::None => return Err(unsupported("this document has no form")),
        }
        // Written out once, the document shows every action it holds, also those kept in compressed
        // parts of the original. If it cannot be written, it is treated as holding one.
        let written = crate::save::serialise_up_to(document, MAX_SAVE_BYTES, progress);
        let damaged = source.is_some_and(|guard| crate::source::lock(guard).damaged());
        let (state_actions, parent_actions) = match written {
            Ok(bytes) if !damaged => {
                let state = holds_state_action(&bytes);
                (state, state && holds_parent_action(&bytes))
            }
            _ => (true, true),
        };
        // SAFETY: a read of the permissions.
        let may_fill = unsafe { bindings.FPDF_GetDocPermissions(handle) } as u32 & PERMIT_FILL != 0;
        // SAFETY: every field of the structure is a number, a pointer or an optional function, and
        // zero is a valid value for all of them: no function is given, and the platform for
        // JavaScript (`m_pJsPlatform`) is null, which is what keeps PDF scripts from running.
        let mut info: Pin<Box<FPDF_FORMFILLINFO>> =
            Box::pin(unsafe { std::mem::zeroed::<FPDF_FORMFILLINFO>() });
        info.version = 2;
        let info_ptr: *mut FPDF_FORMFILLINFO = &mut *info.as_mut();
        // SAFETY: the document handle is valid (see `document_handle`), the structure is pinned.
        let form = unsafe { bindings.FPDFDOC_InitFormFillEnvironment(handle, info_ptr) };
        if form.is_null() {
            return Err(unsupported("the form of this document could not be opened"));
        }
        Ok(FormEnv {
            bindings,
            document: handle,
            handle: form,
            _info: info,
            state_actions,
            parent_actions,
            may_fill,
            spread: RefCell::new(HashMap::new()),
        })
    }

    fn load(&self, index: u32) -> Result<LoadedPage<'_>, Failure> {
        // SAFETY: the handles belong to this environment; the page is closed by `LoadedPage`.
        let page = unsafe { self.bindings.FPDF_LoadPage(self.document, index as c_int) };
        if page.is_null() {
            return Err((
                ErrorCode::PageOutOfRange,
                "the page does not exist".to_string(),
            ));
        }
        // SAFETY: the page was just loaded.
        unsafe { self.bindings.FORM_OnAfterLoadPage(page, self.handle) };
        Ok(LoadedPage {
            env: self,
            page,
            index,
            spent: Cell::new(0),
        })
    }

    /// The fillable fields of a page.
    pub fn list(&self, index: u32) -> Result<Vec<FormField>, Failure> {
        let page = self.load(index)?;
        let mut fields = Vec::new();
        let mut ordinal = 0u32;
        for annot in page.annots() {
            if !annot.is_widget() {
                continue;
            }
            let at = ordinal;
            ordinal += 1;
            if fields.len() >= MAX_FORM_FIELDS_PER_PAGE || page.spent.get() > MAX_LIST_READ_BYTES {
                break;
            }
            if let Some(field) = page.describe(at, &annot) {
                fields.push(field);
            }
        }
        Ok(fields)
    }

    /// Sets one field. `name` is the name the caller saw at `ordinal`; if the widget there is not
    /// that field any more the request is refused.
    pub fn set(
        &self,
        index: u32,
        ordinal: u32,
        name: &str,
        value: &FormValue,
    ) -> Result<(), Failure> {
        let page = self.load(index)?;
        let mut found = None;
        let mut at = 0u32;
        for annot in page.annots() {
            if !annot.is_widget() {
                continue;
            }
            if at == ordinal {
                found = Some(annot);
                break;
            }
            at += 1;
        }
        let Some(annot) = found else {
            return Err(refused("there is no such field on the page"));
        };
        let Some(field) = page.describe(ordinal, &annot) else {
            return Err(refused("that is not a field the reader fills in"));
        };
        if field.name != cut(name, MAX_FORM_NAME) {
            return Err(refused("the field on the page is not the one named"));
        }
        if field.has(field_flag::ACTION_BLOCKED) {
            return Err(refused(
                "the field has an action that is not run, so it is left alone",
            ));
        }
        if field.has(field_flag::READ_ONLY) || field.has(field_flag::VALUE_CUT) {
            return Err(refused("the field is read only"));
        }
        // PDFium's editor stops at a zero character, so the rest of such a value would be lost with
        // no word said.
        if matches!(value, FormValue::Text(text) if text.contains('\0')) {
            return Err(refused("a value cannot hold a zero character"));
        }
        // The other widgets of the field, on other pages, are kept loaded until the field has been
        // left: PDFium draws a new appearance for the widgets it has a page for, and a widget on a
        // page that is not loaded would keep its old one.
        let _siblings = self.siblings(&page, index, &annot, &field.name);
        let result = match (field.kind, value) {
            (FormFieldKind::Text, FormValue::Text(text)) => page.set_text(&annot, text),
            (FormFieldKind::Combo, FormValue::Text(text)) if field.has(field_flag::EDITABLE) => {
                if text.is_empty() && !field.options.is_empty() && !field.value.is_empty() {
                    // PDFium puts the first choice back in a box that is emptied.
                    Err(refused("a drop-down list cannot be emptied"))
                } else {
                    page.set_text(&annot, text)
                }
            }
            (FormFieldKind::Checkbox | FormFieldKind::Radio, FormValue::Checked(on)) => {
                page.set_checked(&annot, &field, *on)
            }
            (FormFieldKind::Combo | FormFieldKind::List, FormValue::Choice(picked)) => {
                page.set_choice(&annot, &field, picked)
            }
            _ => Err(refused("that value does not fit the kind of field")),
        };
        // Whatever happened, no field keeps the focus: a field that is still being edited has not
        // written its value yet.
        // SAFETY: the handle belongs to this environment.
        unsafe { self.bindings.FORM_ForceToKillFocus(self.handle) };
        result
    }
}

impl FormEnv {
    /// The pages, other than `index`, that hold other widgets of the field of `annot`, loaded. Empty
    /// for a field that has all its widgets on its own page (the usual case, also for radio
    /// groups), so that nothing but the page itself is read then.
    fn siblings(
        &self,
        page: &LoadedPage<'_>,
        index: u32,
        annot: &Annot<'_>,
        name: &str,
    ) -> Vec<LoadedPage<'_>> {
        // SAFETY: a read.
        let total = unsafe {
            self.bindings
                .FPDFAnnot_GetFormControlCount(self.handle, annot.handle)
        }
        .max(0) as usize;
        let here = page.count_named(name);
        if total <= here {
            return Vec::new();
        }
        // A field that was changed before: its pages are known.
        let known = self.spread.borrow().get(name).cloned();
        if let Some(pages) = known.filter(|pages| pages.contains(&index)) {
            return pages
                .iter()
                .filter(|other| **other != index)
                .filter_map(|other| self.load(*other).ok())
                .collect();
        }
        // SAFETY: a read.
        let pages = unsafe { self.bindings.FPDF_GetPageCount(self.document) }.max(0) as usize;
        let mut kept = Vec::new();
        let mut found = here;
        for other in 0..pages.min(MAX_SIBLING_SCAN) {
            if other as u32 == index {
                continue;
            }
            let Ok(loaded) = self.load(other as u32) else {
                continue;
            };
            let count = loaded.count_named(name);
            if count == 0 {
                continue;
            }
            found += count;
            kept.push(loaded);
            if found >= total || kept.len() >= MAX_SIBLING_PAGES {
                break;
            }
        }
        // The next change of this field does not search again. (Not kept for more fields than a
        // page can hold, so that a hostile document cannot make this grow.)
        let mut spread = self.spread.borrow_mut();
        if spread.len() < MAX_FORM_FIELDS_PER_PAGE {
            let mut pages: Vec<u32> = kept.iter().map(|p| p.index).collect();
            pages.push(index);
            spread.insert(name.to_string(), pages);
        }
        kept
    }
}

struct LoadedPage<'e> {
    env: &'e FormEnv,
    page: FPDF_PAGE,
    /// The index of the page in the document.
    index: u32,
    /// Bytes of text read from PDFium for this page so far (see `MAX_LIST_READ_BYTES`).
    spent: Cell<usize>,
}

impl Drop for LoadedPage<'_> {
    fn drop(&mut self) {
        // SAFETY: the page was loaded by `FormEnv::load` and is closed once.
        unsafe {
            self.env
                .bindings
                .FORM_OnBeforeClosePage(self.page, self.env.handle);
            self.env.bindings.FPDF_ClosePage(self.page);
        }
    }
}

/// An annotation handle, closed again when dropped.
struct Annot<'e> {
    env: &'e FormEnv,
    handle: FPDF_ANNOTATION,
}

impl Drop for Annot<'_> {
    fn drop(&mut self) {
        // SAFETY: the handle came from `FPDFPage_GetAnnot` and is closed once.
        unsafe { self.env.bindings.FPDFPage_CloseAnnot(self.handle) };
    }
}

impl Annot<'_> {
    fn is_widget(&self) -> bool {
        // SAFETY: a read.
        unsafe { self.env.bindings.FPDFAnnot_GetSubtype(self.handle) == ANNOT_WIDGET }
    }
}

/// Cuts `s` to at most `max` bytes on a character boundary.
fn cut(s: &str, max: usize) -> String {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Line breaks of a value as the window writes them: `\n`.
fn unix_breaks(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

impl<'e> LoadedPage<'e> {
    fn bindings(&self) -> &'static dyn PdfiumLibraryBindings {
        self.env.bindings
    }

    /// The annotations of the page, as many as `MAX_ANNOTS_SCANNED`.
    fn annots(&self) -> impl Iterator<Item = Annot<'e>> + '_ {
        // SAFETY: a read.
        let count = unsafe { self.bindings().FPDFPage_GetAnnotCount(self.page) }.max(0) as usize;
        (0..count.min(MAX_ANNOTS_SCANNED)).filter_map(move |i| {
            // SAFETY: the index is below the count; the handle is closed by `Annot`.
            let handle = unsafe { self.bindings().FPDFPage_GetAnnot(self.page, i as c_int) };
            (!handle.is_null()).then_some(Annot {
                env: self.env,
                handle,
            })
        })
    }

    /// A string from PDFium. `None` when it is longer than `MAX_READ_BYTES`, when this page has
    /// used up its share of reading, or when it holds a zero character with more after it (what
    /// follows would be lost).
    fn string(&self, read: impl Fn(*mut FPDF_WCHAR, c_ulong) -> c_ulong) -> Option<String> {
        let len = read(ptr::null_mut(), 0) as usize;
        if len <= 2 {
            return Some(String::new());
        }
        if len > MAX_READ_BYTES || self.spent.get() > MAX_LIST_READ_BYTES {
            return None;
        }
        self.spent.set(self.spent.get() + len);
        let mut buffer = vec![0u16; len.div_ceil(2)];
        let got = read(buffer.as_mut_ptr(), len as c_ulong) as usize;
        if got == 0 {
            return Some(String::new());
        }
        // The text ends at the zero that PDFium puts at the end; a zero before that is a character
        // of the text.
        let text_len = (got / 2).min(buffer.len());
        let end = buffer[..text_len]
            .iter()
            .rposition(|c| *c != 0)
            .map_or(0, |i| i + 1);
        if buffer[..end].contains(&0) {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..end]))
    }

    /// A box in page space as `[left, top, right, bottom]` on the page as it is drawn.
    fn drawn_rect(&self, rect: &FS_RECTF) -> Option<[f32; 4]> {
        // SAFETY: reads of the size of the page.
        let (width, height) = unsafe {
            (
                self.bindings().FPDF_GetPageWidthF(self.page),
                self.bindings().FPDF_GetPageHeightF(self.page),
            )
        };
        if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
            return None;
        }
        let w = (width * PRECISION).round().clamp(PRECISION, 4.0e6) as c_int;
        let h = (height * PRECISION).round().clamp(PRECISION, 4.0e6) as c_int;
        let point = |x: f32, y: f32| -> Option<(f32, f32)> {
            let (mut dx, mut dy) = (0 as c_int, 0 as c_int);
            // SAFETY: the page is loaded; the answers go to two integers.
            let ok = unsafe {
                self.bindings().FPDF_PageToDevice(
                    self.page, 0, 0, w, h, 0, x as f64, y as f64, &mut dx, &mut dy,
                )
            };
            (ok != 0).then_some((dx as f32 / PRECISION, dy as f32 / PRECISION))
        };
        let (x1, y1) = point(rect.left, rect.top)?;
        let (x2, y2) = point(rect.right, rect.bottom)?;
        let out = [x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2)];
        out.iter().all(|v| v.is_finite()).then_some(out)
    }

    /// Describes the widget at `ordinal`, or `None` when it is not a field the reader fills in
    /// (a push button, a signature, a field that is hidden, or one without a place on the page).
    fn describe(&self, ordinal: u32, annot: &Annot<'_>) -> Option<FormField> {
        let b = self.bindings();
        let form = self.env.handle;
        let a = annot.handle;
        // SAFETY (all the reads below): the handles are valid for the life of the page.
        let kind = match unsafe { b.FPDFAnnot_GetFormFieldType(form, a) } {
            FIELD_CHECKBOX => FormFieldKind::Checkbox,
            FIELD_RADIOBUTTON => FormFieldKind::Radio,
            FIELD_COMBOBOX => FormFieldKind::Combo,
            FIELD_LISTBOX => FormFieldKind::List,
            FIELD_TEXTFIELD => FormFieldKind::Text,
            _ => return None,
        };
        let annot_flags = unsafe { b.FPDFAnnot_GetFlags(a) };
        if annot_flags & (ANNOT_HIDDEN | ANNOT_NO_VIEW) != 0 {
            return None;
        }
        let mut raw = FS_RECTF {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        };
        if unsafe { b.FPDFAnnot_GetRect(a, &mut raw) } == 0 {
            return None;
        }
        let rect = self.drawn_rect(&raw)?;
        if rect[2] - rect[0] < 1.0 || rect[3] - rect[1] < 1.0 {
            return None;
        }
        let ff = unsafe { b.FPDFAnnot_GetFormFieldFlags(form, a) };
        let mut flags = 0u16;
        if ff & FF_READ_ONLY != 0 || annot_flags & ANNOT_READ_ONLY != 0 {
            flags |= field_flag::READ_ONLY;
        }
        if ff & FF_REQUIRED != 0 {
            flags |= field_flag::REQUIRED;
        }
        if unsafe { b.FPDFAnnot_HasKey(a, "AA") } != 0 || unsafe { b.FPDFAnnot_HasKey(a, "A") } != 0
        {
            flags |= field_flag::ACTION;
            // In a document that holds a reset or a hide, a widget with an action of its own could
            // fire it when it gets the focus, is pressed or is left: it is left alone.
            if self.env.state_actions {
                flags |= field_flag::ACTION_BLOCKED | field_flag::READ_ONLY;
            }
        }
        // The same for a widget under a field that has additional actions: PDFium reads some of
        // them through `/Parent` (see the top of this file).
        if self.env.parent_actions && unsafe { b.FPDFAnnot_HasKey(a, "Parent") } != 0 {
            flags |= field_flag::ACTION | field_flag::ACTION_BLOCKED | field_flag::READ_ONLY;
        }
        if !self.env.may_fill {
            flags |= field_flag::READ_ONLY;
        }
        let name = self
            .string(|buffer, len| unsafe { b.FPDFAnnot_GetFormFieldName(form, a, buffer, len) })
            .unwrap_or_default();
        let tooltip = self
            .string(|buffer, len| unsafe {
                b.FPDFAnnot_GetFormFieldAlternateName(form, a, buffer, len)
            })
            .unwrap_or_default();
        let mut field = FormField {
            index: ordinal,
            kind,
            flags,
            // Quarter turns of the page: the field is drawn turned with it.
            turns: (unsafe { b.FPDFPage_GetRotation(self.page) }.rem_euclid(4)) as u8,
            rect,
            font_size: 0.0,
            name: cut(&name, MAX_FORM_NAME),
            tooltip: cut(&tooltip, MAX_FORM_TOOLTIP),
            value: String::new(),
            max_len: 0,
            options: Vec::new(),
        };
        let mut size = 0.0f32;
        if unsafe { b.FPDFAnnot_GetFontSize(form, a, &mut size) } != 0
            && size.is_finite()
            && size > 0.0
        {
            field.font_size = size.min(200.0);
        }
        let mut max = 0.0f32;
        if unsafe { b.FPDFAnnot_GetNumberValue(a, "MaxLen", &mut max) } != 0
            && max.is_finite()
            && max >= 1.0
            && max < i32::MAX as f32
        {
            field.max_len = max as u32;
        }
        match kind {
            FormFieldKind::Text => {
                if ff & FF_MULTILINE != 0 {
                    field.flags |= field_flag::MULTILINE;
                }
                if ff & FF_PASSWORD != 0 {
                    field.flags |= field_flag::PASSWORD;
                }
                if ff & FF_COMB != 0 {
                    field.flags |= field_flag::COMB;
                }
                self.read_value(&mut field, annot);
            }
            FormFieldKind::Checkbox | FormFieldKind::Radio => {
                if unsafe { b.FPDFAnnot_IsChecked(form, a) } != 0 {
                    field.flags |= field_flag::CHECKED;
                }
                if kind == FormFieldKind::Radio && ff & FF_NO_TOGGLE_TO_OFF != 0 {
                    field.flags |= field_flag::NO_TOGGLE_OFF;
                }
            }
            FormFieldKind::Combo | FormFieldKind::List => {
                if kind == FormFieldKind::Combo && ff & FF_COMBO_EDITABLE != 0 {
                    field.flags |= field_flag::EDITABLE;
                    self.read_value(&mut field, annot);
                }
                if ff & FF_MULTISELECT != 0 {
                    field.flags |= field_flag::MULTISELECT;
                }
                let count = unsafe { b.FPDFAnnot_GetOptionCount(form, a) }.max(0) as usize;
                if count > MAX_FORM_OPTIONS {
                    // Choices the window cannot show would be choices the reader cannot see being
                    // replaced: the field is left alone.
                    field.flags |= field_flag::READ_ONLY;
                }
                for i in 0..count.min(MAX_FORM_OPTIONS) {
                    let Some(label) = self.string(|buffer, len| unsafe {
                        b.FPDFAnnot_GetOptionLabel(form, a, i as c_int, buffer, len)
                    }) else {
                        // Choices that cannot be read in full cannot be changed safely.
                        field.flags |= field_flag::READ_ONLY;
                        break;
                    };
                    field.options.push(FormOption {
                        selected: unsafe { b.FPDFAnnot_IsOptionSelected(form, a, i as c_int) } != 0,
                        label: cut(&label, MAX_FORM_OPTION_LABEL),
                    });
                }
            }
        }
        Some(field)
    }

    fn read_value(&self, field: &mut FormField, annot: &Annot<'_>) {
        let b = self.bindings();
        let (form, a) = (self.env.handle, annot.handle);
        match self
            .string(|buffer, len| unsafe { b.FPDFAnnot_GetFormFieldValue(form, a, buffer, len) })
        {
            Some(value) => {
                let value = unix_breaks(&value);
                if value.len() > MAX_FORM_VALUE {
                    field.flags |= field_flag::VALUE_CUT;
                }
                field.value = cut(&value, MAX_FORM_VALUE);
            }
            None => field.flags |= field_flag::VALUE_CUT,
        }
    }

    /// How many widgets of the page belong to the field with this full name.
    fn count_named(&self, name: &str) -> usize {
        let b = self.bindings();
        let form = self.env.handle;
        let mut count = 0;
        for annot in self.annots() {
            if !annot.is_widget() {
                continue;
            }
            let full = self
                .string(|buffer, len| unsafe {
                    b.FPDFAnnot_GetFormFieldName(form, annot.handle, buffer, len)
                })
                .unwrap_or_default();
            if cut(&full, MAX_FORM_NAME) == name {
                count += 1;
            }
        }
        count
    }

    fn focus(&self, annot: &Annot<'_>) -> Result<(), Failure> {
        // SAFETY: the handles are valid.
        let ok = unsafe {
            self.bindings()
                .FORM_SetFocusedAnnot(self.env.handle, annot.handle)
        };
        if ok == 0 {
            return Err(refused("the field could not be reached"));
        }
        Ok(())
    }

    fn set_text(&self, annot: &Annot<'_>, text: &str) -> Result<(), Failure> {
        let b = self.bindings();
        self.focus(annot)?;
        // A line break is a carriage return inside PDFium's editor.
        let wide: Vec<u16> = unix_breaks(text)
            .replace('\n', "\r")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: the page and the form are loaded; `wide` ends with a zero.
        unsafe {
            b.FORM_SelectAllText(self.env.handle, self.page);
            b.FORM_ReplaceSelection(self.env.handle, self.page, wide.as_ptr());
        }
        Ok(())
    }

    fn set_checked(&self, annot: &Annot<'_>, field: &FormField, on: bool) -> Result<(), Failure> {
        let b = self.bindings();
        let is_on =
            |a: &Annot<'_>| unsafe { b.FPDFAnnot_IsChecked(self.env.handle, a.handle) } != 0;
        if is_on(annot) == on {
            return Ok(());
        }
        if !on && field.has(field_flag::NO_TOGGLE_OFF) {
            return Err(refused("this button cannot be turned off"));
        }
        // The space bar of a person, in the widget that has the focus. (A click would go to the
        // widget found at a point, and that can be another one, such as a button over this one.)
        self.focus(annot)?;
        // SAFETY: the page and the form are loaded.
        unsafe {
            b.FORM_OnChar(self.env.handle, self.page, 32, 0);
        }
        if is_on(annot) != on {
            return Err(refused("the field did not change"));
        }
        Ok(())
    }

    fn set_choice(
        &self,
        annot: &Annot<'_>,
        field: &FormField,
        picked: &[u32],
    ) -> Result<(), Failure> {
        let b = self.bindings();
        let count = field.options.len();
        if picked.iter().any(|p| *p as usize >= count) {
            return Err(refused("a choice is not in the list"));
        }
        if picked.len() > 1 && !field.has(field_flag::MULTISELECT) {
            return Err(refused("the field takes one choice"));
        }
        if picked.is_empty() && field.kind == FormFieldKind::Combo {
            // PDFium cannot switch the choice of a drop-down list off.
            let any = field.options.iter().any(|o| o.selected);
            return if any {
                Err(refused("a drop-down list cannot be cleared"))
            } else {
                Ok(())
            };
        }
        self.focus(annot)?;
        let selected = |i: usize| unsafe {
            b.FORM_IsIndexSelected(self.env.handle, self.page, i as c_int) != 0
        };
        // First what is wanted, then what is not: a list that takes one choice drops the old one
        // when the new one is picked.
        for i in 0..count {
            if picked.contains(&(i as u32)) && !selected(i) {
                unsafe { b.FORM_SetIndexSelected(self.env.handle, self.page, i as c_int, 1) };
            }
        }
        for i in 0..count {
            if !picked.contains(&(i as u32)) && selected(i) {
                unsafe { b.FORM_SetIndexSelected(self.env.handle, self.page, i as c_int, 0) };
            }
        }
        Ok(())
    }
}

/// For the tests of the engine: whether a written document holds a reset or a hide.
#[doc(hidden)]
pub fn holds_state_action_for_tests(bytes: &[u8]) -> bool {
    holds_state_action(bytes)
}

/// For the tests of the engine: whether a written document holds a field above a widget that has
/// additional actions.
#[doc(hidden)]
pub fn holds_parent_action_for_tests(bytes: &[u8]) -> bool {
    holds_parent_action(bytes)
}
