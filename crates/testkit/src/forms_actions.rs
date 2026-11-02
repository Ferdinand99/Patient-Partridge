//! Forms whose fields carry actions (reset, hide, links, submit, named actions, scripts), forms
//! whose fields are drawn on more than one page, and forms with a loop in their field tree. They
//! are for the tests of what filling in a form must not do and must do for every widget of a field.
//!
//! The fields come with a value so that a test can see that an action did not clear it:
//!
//! - `keep` holds "kept" and `victim` holds "victim value"; they are what an action would reset or
//!   hide
//! - `box_a` has `/A /ResetForm`, `box_u` has `/AA /U /ResetForm`, `box_d` has `/AA /D /Hide`
//!   aimed at `victim`
//! - `text_fo` and `text_bl` have `/ResetForm` as the action of that event (focus, blur)
//! - `text_script` has scripts on blur and keystroke and `box_link` a link: none of them reaches a
//!   reset, so they stay free next to the buttons that do
//! - `plain` and `plain_box` have no action
//! - `under_box` is a check box with no action, and a push button `over` with `/A /ResetForm` is
//!   drawn over its middle (it is later in the page, so it is the one found at that point)

use std::fmt::Write as _;

use super::{assemble, assemble_packed, build, build_objects, stream_object, PageSpec};

struct Objects {
    first: usize,
    bodies: Vec<Vec<u8>>,
}

impl Objects {
    fn new(first: usize) -> Objects {
        Objects {
            first,
            bodies: Vec::new(),
        }
    }

    fn reserve(&mut self) -> usize {
        self.bodies.push(Vec::new());
        self.first + self.bodies.len() - 1
    }

    fn set(&mut self, number: usize, body: impl Into<Vec<u8>>) {
        self.bodies[number - self.first] = body.into();
    }

    fn add(&mut self, body: impl Into<Vec<u8>>) -> usize {
        let number = self.reserve();
        self.set(number, body);
        number
    }
}

const RESET: &str = "<< /S /ResetForm >>";

/// A text field, merged with its widget, with a value and extra entries (actions) in its dictionary.
fn text(objs: &mut Objects, page: usize, name: &str, y: u32, value: &str, extra: &str) -> usize {
    let value_entry = if value.is_empty() {
        String::new()
    } else {
        format!("/V ({value}) /DV ()")
    };
    objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) {value_entry} \
         /Rect [100 {y} 300 {top}] /F 4 /P {page} 0 R /DA (/Helv 12 Tf 0 g) {extra} >>",
        top = y + 20
    ))
}

fn check_box(objs: &mut Objects, page: usize, name: &str, y: u32, extra: &str) -> usize {
    let on = objs.add(stream_object(
        "/Type /XObject /Subtype /Form /BBox [0 0 16 16]",
        "q 0 g 3 3 10 10 re f Q",
    ));
    let off = objs.add(stream_object(
        "/Type /XObject /Subtype /Form /BBox [0 0 16 16]",
        "",
    ));
    objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T ({name}) /V /Off /AS /Off \
         /Rect [320 {y} 336 {top}] /F 4 /P {page} 0 R \
         /AP << /N << /Yes {on} 0 R /Off {off} 0 R >> >> {extra} >>",
        top = y + 16
    ))
}

fn catalog(fields: &[usize]) -> String {
    let refs: String = fields.iter().fold(String::new(), |mut s, f| {
        let _ = write!(s, "{f} 0 R ");
        s
    });
    format!(
        "/AcroForm << /Fields [{refs}] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >>"
    )
}

/// Fields with actions that change a form (reset, hide), and fields without. See the top of this
/// file for the names.
pub fn forms_actions_pdf() -> Vec<u8> {
    let mut objs = Objects::new(6);
    let page = 4;
    let mut fields: Vec<usize> = Vec::new();
    // One row of 30 points for each field, from the top of the page down.
    let row = |n: u32| 740 - 30 * n;
    fields.push(text(&mut objs, page, "keep", row(0), "kept", ""));
    fields.push(text(&mut objs, page, "victim", row(1), "victim value", ""));
    fields.push(check_box(
        &mut objs,
        page,
        "box_a",
        row(2),
        &format!("/A {RESET}"),
    ));
    fields.push(check_box(
        &mut objs,
        page,
        "box_u",
        row(3),
        &format!("/AA << /U {RESET} >>"),
    ));
    fields.push(check_box(
        &mut objs,
        page,
        "box_d",
        row(4),
        "/AA << /D << /S /Hide /T (victim) /H true >> >>",
    ));
    for (i, (name, event)) in [("text_fo", "Fo"), ("text_bl", "Bl")]
        .into_iter()
        .enumerate()
    {
        fields.push(text(
            &mut objs,
            page,
            name,
            row(5 + i as u32),
            "",
            &format!("/AA << /{event} {RESET} >>"),
        ));
    }
    // Actions of widgets that cannot change the form stay free, in a document that holds a reset.
    fields.push(text(
        &mut objs,
        page,
        "text_script",
        row(8),
        "",
        "/AA << /Bl << /S /JavaScript /JS (1;) >> /K << /S /JavaScript /JS (event.rc = true;) >> >>",
    ));
    fields.push(check_box(
        &mut objs,
        page,
        "box_link",
        row(9),
        "/A << /S /URI /URI (https://example.com/) >>",
    ));
    fields.push(text(&mut objs, page, "plain", row(11), "", ""));
    fields.push(check_box(&mut objs, page, "plain_box", row(12), ""));
    // A check box with a push button over its middle, one before it and one after it in the page,
    // so that whichever of the two PDFium finds first at that point, it is a button.
    let under_y = row(13);
    let first_button = objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (over_first) \n         /Rect [316 {y0} 340 {y1}] /F 4 /P {page} 0 R /MK << /BG [0.8 0.8 0.8] /CA (Reset) >> \n         /A {RESET} >>",
        y0 = under_y - 4,
        y1 = under_y + 20
    ));
    fields.push(first_button);
    fields.push(check_box(&mut objs, page, "under_box", under_y, ""));
    let button = objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (over) \
         /Rect [316 {y0} 340 {y1}] /F 4 /P {page} 0 R /MK << /BG [0.8 0.8 0.8] /CA (Reset) >> \
         /A {RESET} >>",
        y0 = under_y - 4,
        y1 = under_y + 20
    ));
    fields.push(button);
    // The widgets of the page in the order of the file: the button is after the check box.
    let annots = fields.clone();
    let annot_list: String = annots.iter().fold(String::new(), |mut s, f| {
        let _ = write!(s, "{f} 0 R ");
        s
    });
    let spec = PageSpec {
        width: 612,
        height: 792,
        content: Some("0 g 10 10 1 1 re f\n".to_string()),
        extra: format!("/Annots [{annot_list}]"),
    };
    build(&[spec], &catalog(&fields), "", objs.bodies)
}

/// Fields with actions that cannot change the form: a link, a submit, a named action, a jump, a
/// launch and a script. Nothing in this file resets or hides a field, so the reader fills these
/// in. The names are `keep` ("kept"), `box_uri`, `text_named`, `text_submit`, `box_goto`,
/// `text_launch` and `text_js`.
pub fn forms_harmless_actions_pdf() -> Vec<u8> {
    let mut objs = Objects::new(6);
    let page = 4;
    let mut fields = vec![text(&mut objs, page, "keep", 700, "kept", "")];
    fields.push(check_box(
        &mut objs,
        page,
        "box_uri",
        660,
        "/A << /S /URI /URI (https://example.com/reset) >>",
    ));
    fields.push(text(
        &mut objs,
        page,
        "text_named",
        620,
        "",
        "/AA << /Fo << /S /Named /N /NextPage >> >>",
    ));
    fields.push(text(
        &mut objs,
        page,
        "text_submit",
        580,
        "",
        "/AA << /Bl << /S /SubmitForm /F << /FS /URL /F (https://example.com/submit) >> /Flags 4 >> >>",
    ));
    fields.push(check_box(
        &mut objs,
        page,
        "box_goto",
        540,
        "/AA << /U << /S /GoTo /D [4 0 R /Fit] >> >>",
    ));
    fields.push(text(
        &mut objs,
        page,
        "text_launch",
        500,
        "",
        "/AA << /Fo << /S /Launch /F (calc.exe) >> >>",
    ));
    fields.push(text(
        &mut objs,
        page,
        "text_js",
        460,
        "",
        "/AA << /K << /S /JavaScript /JS (event.rc = false;) >> /Bl << /S /JavaScript /JS (this.resetForm\\(\\);) >> >>",
    ));
    let annot_list: String = fields.iter().fold(String::new(), |mut s, f| {
        let _ = write!(s, "{f} 0 R ");
        s
    });
    let spec = PageSpec {
        width: 612,
        height: 792,
        content: Some("0 g 10 10 1 1 re f\n".to_string()),
        extra: format!("/Annots [{annot_list}]"),
    };
    build(&[spec], &catalog(&fields), "", objs.bodies)
}

/// Three pages. The text field `applicant` has two widgets, one on page 1 and one on page 3, and
/// the drop-down list `country` the same; both widgets have an appearance stream already (an empty
/// one), so nothing makes PDFium draw them again by itself. With `without_page` the widgets have no
/// `/P` entry, which is allowed.
pub fn forms_multipage_pdf(without_page: bool) -> Vec<u8> {
    // Objects: 1 catalog, 2 tree, 3 font, then page 1 (4) and its content (5), page 2 (6, 7) and
    // page 3 (8, 9), so the objects made here start at 10.
    let mut objs = Objects::new(10);
    let pages = [4usize, 8usize];
    let mut widgets: Vec<Vec<usize>> = vec![Vec::new(); 3];
    let p_entry = |page: usize| {
        if without_page {
            String::new()
        } else {
            format!("/P {page} 0 R")
        }
    };
    let applicant = objs.reserve();
    let country = objs.reserve();
    let mut kids_applicant = String::new();
    let mut kids_country = String::new();
    for (slot, page) in [(0usize, pages[0]), (2usize, pages[1])] {
        let ap = objs.add(stream_object(
            "/Type /XObject /Subtype /Form /BBox [0 0 200 20] /Resources << /Font << /Helv 3 0 R >> >>",
            "",
        ));
        let w = objs.add(format!(
            "<< /Type /Annot /Subtype /Widget /Parent {applicant} 0 R \
             /Rect [100 700 300 720] /F 4 {p} /DA (/Helv 12 Tf 0 g) /AP << /N {ap} 0 R >> >>",
            p = p_entry(page)
        ));
        widgets[slot].push(w);
        let _ = write!(kids_applicant, "{w} 0 R ");
        let ap = objs.add(stream_object(
            "/Type /XObject /Subtype /Form /BBox [0 0 200 20] /Resources << /Font << /Helv 3 0 R >> >>",
            "",
        ));
        let c = objs.add(format!(
            "<< /Type /Annot /Subtype /Widget /Parent {country} 0 R \
             /Rect [100 650 300 670] /F 4 {p} /DA (/Helv 12 Tf 0 g) /AP << /N {ap} 0 R >> >>",
            p = p_entry(page)
        ));
        widgets[slot].push(c);
        let _ = write!(kids_country, "{c} 0 R ");
    }
    objs.set(
        applicant,
        format!("<< /FT /Tx /T (applicant) /Kids [{kids_applicant}] >>"),
    );
    objs.set(
        country,
        format!(
            "<< /FT /Ch /Ff 131072 /T (country) /Opt [(Norway) (Sweden)] /Kids [{kids_country}] >>"
        ),
    );
    let spec = |slot: usize, n: usize| {
        let list: String = widgets[slot].iter().fold(String::new(), |mut s, w| {
            let _ = write!(s, "{w} 0 R ");
            s
        });
        PageSpec {
            width: 612,
            height: 792,
            content: Some(format!("BT /F1 14 Tf 72 740 Td (Page {n}) Tj ET\n")),
            extra: format!("/Annots [{list}]"),
        }
    };
    build(
        &[spec(0, 1), spec(1, 2), spec(2, 3)],
        &catalog(&[applicant, country]),
        "",
        objs.bodies,
    )
}

/// A field tree with a loop (a parent that is its own ancestor) and a very deep tree, so that the
/// names of the fields have to be worked out with a limit.
pub fn forms_loop_pdf() -> Vec<u8> {
    let mut objs = Objects::new(6);
    let a = objs.reserve();
    let b = objs.reserve();
    let widget = objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (leaf) /Parent {a} 0 R \
         /Rect [100 700 300 720] /F 4 /P 4 0 R /DA (/Helv 12 Tf 0 g) >>"
    ));
    objs.set(
        a,
        format!("<< /T (a) /Parent {b} 0 R /Kids [{widget} 0 R] >>"),
    );
    objs.set(b, format!("<< /T (b) /Parent {a} 0 R /Kids [{a} 0 R] >>"));
    // A chain of 300 parents above a second widget.
    let mut parent = None;
    let mut chain = Vec::new();
    for i in 0..300 {
        let n = objs.reserve();
        chain.push((n, i));
    }
    for (i, (n, _)) in chain.iter().enumerate() {
        let up = if i == 0 {
            String::new()
        } else {
            format!("/Parent {} 0 R", chain[i - 1].0)
        };
        let down = if i + 1 < chain.len() {
            format!("/Kids [{} 0 R]", chain[i + 1].0)
        } else {
            String::new()
        };
        objs.set(*n, format!("<< /T (n{i}) {up} {down} >>"));
        parent = Some(*n);
    }
    let deep = objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (deep) /Parent {} 0 R \
         /Rect [100 650 300 670] /F 4 /P 4 0 R /DA (/Helv 12 Tf 0 g) >>",
        parent.expect("the chain is not empty")
    ));
    let spec = PageSpec {
        width: 612,
        height: 792,
        content: Some("0 g 10 10 1 1 re f\n".to_string()),
        extra: format!("/Annots [{widget} 0 R {deep} 0 R]"),
    };
    build(&[spec], &catalog(&[a, chain[0].0]), "", objs.bodies)
}

/// Text fields with the values that are awkward to hand over: a limit of length that does not fit a
/// signed 32 bit number (`huge_max`), and a value with a zero character in the middle (`nul`, which
/// reads "a", a zero, "b").
pub fn forms_awkward_values_pdf() -> Vec<u8> {
    let mut objs = Objects::new(6);
    let huge = text(&mut objs, 4, "huge_max", 700, "", "/MaxLen 3000000000");
    let nul = objs.add(
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (nul) /V <FEFF006100000062> \
         /Rect [100 650 300 670] /F 4 /P 4 0 R /DA (/Helv 12 Tf 0 g) >>",
    );
    let spec = PageSpec {
        width: 612,
        height: 792,
        content: Some("0 g 10 10 1 1 re f\n".to_string()),
        extra: format!("/Annots [{huge} 0 R {nul} 0 R]"),
    };
    build(&[spec], &catalog(&[huge, nul]), "", objs.bodies)
}

/// Thirty list boxes that share one list of 500 choices of 2,000 bytes each (1 MB), so that a
/// listing of the page that read every choice of every field would read 30 MB.
pub fn forms_shared_options_pdf() -> Vec<u8> {
    let mut objs = Objects::new(6);
    let label = "x".repeat(2_000);
    let mut list = String::from("[");
    for i in 0..500 {
        let _ = write!(list, "({i}{label}) ");
    }
    list.push(']');
    let options = objs.add(list);
    // An appearance that is there already, so that PDFium does not lay out the whole list of every
    // box when the page is loaded (that is PDFium's own cost and is not what the test is about).
    let ap = objs.add(stream_object(
        "/Type /XObject /Subtype /Form /BBox [0 0 100 15]",
        "",
    ));
    let mut fields = Vec::new();
    for i in 0..30u32 {
        fields.push(objs.add(format!(
            "<< /Type /Annot /Subtype /Widget /FT /Ch /T (list{i}) /Opt {options} 0 R \
             /Rect [100 {y} 200 {top}] /F 4 /P 4 0 R /DA (/Helv 9 Tf 0 g) /AP << /N {ap} 0 R >> >>",
            y = 700 - 20 * i,
            top = 715 - 20 * i
        )));
    }
    let annot_list: String = fields.iter().fold(String::new(), |mut s, f| {
        let _ = write!(s, "{f} 0 R ");
        s
    });
    let spec = PageSpec {
        width: 612,
        height: 792,
        content: Some("0 g 10 10 1 1 re f\n".to_string()),
        extra: format!("/Annots [{annot_list}]"),
    };
    build(&[spec], &catalog(&fields), "", objs.bodies)
}

/// What a field under other fields is, for [`forms_parent_actions_pdf`].
#[derive(Clone, Copy)]
enum Under {
    Text,
    Check,
    Combo,
    List,
}

/// A field whose widget has no action, and whose additional actions `aa` (an `/AA` entry, or
/// nothing) sit on the field above the widget, or on the field above that when `levels` is 2. Returns
/// the field that is listed in the form and the widget.
fn under(
    objs: &mut Objects,
    page: usize,
    name: &str,
    y: u32,
    kind: Under,
    aa: &str,
    levels: usize,
) -> (usize, usize) {
    let top = objs.reserve();
    let mid = if levels > 1 {
        Some(objs.reserve())
    } else {
        None
    };
    let (field_entries, rect, widget_extra) = match kind {
        Under::Text => (
            "/FT /Tx".to_string(),
            format!("100 {y} 300 {}", y + 20),
            String::new(),
        ),
        Under::Combo => (
            "/FT /Ch /Ff 131072 /Opt [(a) (b)]".to_string(),
            format!("100 {y} 300 {}", y + 20),
            String::new(),
        ),
        Under::List => (
            "/FT /Ch /Opt [(a) (b)]".to_string(),
            format!("100 {y} 300 {}", y + 20),
            String::new(),
        ),
        Under::Check => {
            let on = objs.add(stream_object(
                "/Type /XObject /Subtype /Form /BBox [0 0 16 16]",
                "q 0 g 3 3 10 10 re f Q",
            ));
            let off = objs.add(stream_object(
                "/Type /XObject /Subtype /Form /BBox [0 0 16 16]",
                "",
            ));
            (
                "/FT /Btn /V /Off".to_string(),
                format!("320 {y} 336 {}", y + 16),
                format!("/AS /Off /AP << /N << /Yes {on} 0 R /Off {off} 0 R >> >>"),
            )
        }
    };
    let nearest = mid.unwrap_or(top);
    let widget = objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /Parent {nearest} 0 R /Rect [{rect}] /F 4 /P {page} 0 R \
         /DA (/Helv 12 Tf 0 g) {widget_extra} >>"
    ));
    if let Some(mid) = mid {
        objs.set(mid, format!("<< /Parent {top} 0 R /Kids [{widget} 0 R] >>"));
    }
    let child = mid.unwrap_or(widget);
    objs.set(
        top,
        format!("<< {field_entries} /T ({name}) /Kids [{child} 0 R] {aa} >>"),
    );
    (top, widget)
}

/// Fields whose widgets have no action, with additional actions on the field above them (or above
/// that). The field `keep` holds "kept". With `dangerous` the actions are resets and a hide of
/// `keep`; without it they are a link and a script, so the file holds nothing that changes a form.
/// With `packed` the fields are kept in an object stream. The names:
///
/// - `pk_text`, `pk_combo`, `pk_check`: the keystroke event of the field above holds a reset
/// - `pv_text`, `pv_list`: the validate event holds a reset
/// - `pv_hide`: the validate event hides `keep`
/// - `pk_next`: the keystroke event is a script that goes on to a reset (`/Next`)
/// - `gp_text`, `gp_check`: the keystroke event is on the field above the field above the widget
pub fn forms_parent_actions_pdf(packed: bool, dangerous: bool) -> Vec<u8> {
    const LINK: &str = "<< /S /URI /URI (https://example.com/) >>";
    let mut objs = Objects::new(6);
    let page = 4;
    let reset = if dangerous { RESET } else { LINK };
    let hide = if dangerous {
        "<< /S /Hide /T (keep) /H true >>".to_string()
    } else {
        LINK.to_string()
    };
    let next = if dangerous {
        format!("<< /S /JavaScript /JS (1;) /Next {RESET} >>")
    } else {
        "<< /S /JavaScript /JS (1;) >>".to_string()
    };
    let row = |n: u32| 740 - 30 * n;
    let mut fields = Vec::new();
    let mut widgets = Vec::new();
    let keep = text(&mut objs, page, "keep", row(0), "kept", "");
    fields.push(keep);
    widgets.push(keep);
    let cases: [(&str, Under, String, usize); 9] = [
        ("pk_text", Under::Text, format!("/AA << /K {reset} >>"), 1),
        ("pv_text", Under::Text, format!("/AA << /V {reset} >>"), 1),
        ("pv_hide", Under::Text, format!("/AA << /V {hide} >>"), 1),
        ("pk_next", Under::Text, format!("/AA << /K {next} >>"), 1),
        ("pk_combo", Under::Combo, format!("/AA << /K {reset} >>"), 1),
        ("pv_list", Under::List, format!("/AA << /V {reset} >>"), 1),
        ("pk_check", Under::Check, format!("/AA << /K {reset} >>"), 1),
        ("gp_text", Under::Text, format!("/AA << /K {reset} >>"), 2),
        ("gp_check", Under::Check, format!("/AA << /K {reset} >>"), 2),
    ];
    for (i, (name, kind, aa, levels)) in cases.iter().enumerate() {
        let (field, widget) = under(&mut objs, page, name, row(1 + i as u32), *kind, aa, *levels);
        fields.push(field);
        widgets.push(widget);
    }
    let annot_list: String = widgets.iter().fold(String::new(), |mut s, f| {
        let _ = write!(s, "{f} 0 R ");
        s
    });
    let spec = PageSpec {
        width: 612,
        height: 792,
        content: Some("0 g 10 10 1 1 re f\n".to_string()),
        extra: format!("/Annots [{annot_list}]"),
    };
    let objects = build_objects(&[spec], &catalog(&fields), objs.bodies);
    if packed {
        assemble_packed(&objects, 6)
    } else {
        assemble(&objects, "")
    }
}

/// A form that holds a reset (on the check box `box_a`) and fields under other fields that have no
/// additional actions: `pc_plain` has one field above its widget and `pc_deep` two. They, and
/// `keep` ("kept"), are filled in as usual.
pub fn forms_plain_parents_pdf() -> Vec<u8> {
    let mut objs = Objects::new(6);
    let page = 4;
    let keep = text(&mut objs, page, "keep", 740, "kept", "");
    let boxed = check_box(&mut objs, page, "box_a", 710, &format!("/A {RESET}"));
    let (plain, plain_widget) = under(&mut objs, page, "pc_plain", 680, Under::Text, "", 1);
    let (deep, deep_widget) = under(&mut objs, page, "pc_deep", 650, Under::Text, "", 2);
    let fields = [keep, boxed, plain, deep];
    let annot_list = format!("{keep} 0 R {boxed} 0 R {plain_widget} 0 R {deep_widget} 0 R");
    let spec = PageSpec {
        width: 612,
        height: 792,
        content: Some("0 g 10 10 1 1 re f\n".to_string()),
        extra: format!("/Annots [{annot_list}]"),
    };
    build(&[spec], &catalog(&fields), "", objs.bodies)
}

/// What the widget `t` of `forms_kids_actions_pdf` says about its `/Parent`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KidsParent {
    /// No `/Parent` at all.
    Absent,
    /// A `/Parent` that is an object which is no field and holds nothing.
    Plain,
}

/// A form in which a field is built from its `/Kids`, as PDFium does it: the field `t` has two
/// kids, a widget with no name (so that the first kid is no field of its own and PDFium makes `t`
/// the terminal field) and the widget `t`, which has a name of its own, the same as its field's,
/// and no `/Parent` that leads to the field (none, or a plain object). The additional actions
/// (`events`, such as `/K << /S /ResetForm >>`) sit on the field `t`; PDFium reads the keystroke
/// and validate events of the widget from there. With `None` the field has no `/AA`. `keep` holds
/// "kept" and is what a reset clears or a hide removes. Both widgets are on the page.
pub fn forms_kids_actions_pdf(events: Option<&str>, parent: KidsParent) -> Vec<u8> {
    let mut objs = Objects::new(6);
    let page = 4;
    let keep = text(&mut objs, page, "keep", 740, "kept", "");
    let dummy = objs.add("<< /Type /Annot /Subtype /Widget /Rect [0 0 1 1] >>");
    let field = objs.reserve();
    let plain = objs.add("<< /Note (nothing here) >>");
    let parent_entry = match parent {
        KidsParent::Absent => String::new(),
        KidsParent::Plain => format!("/Parent {plain} 0 R"),
    };
    let widget = objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /T (t) /Rect [100 700 300 720] /F 4 /P {page} 0 R \
         /DA (/Helv 12 Tf 0 g) {parent_entry} >>"
    ));
    let aa = events.map_or(String::new(), |events| format!("/AA << {events} >>"));
    objs.set(
        field,
        format!("<< /FT /Tx /T (t) {aa} /Kids [{dummy} 0 R {widget} 0 R] >>"),
    );
    let spec = PageSpec {
        width: 612,
        height: 792,
        content: Some("0 g 10 10 1 1 re f\n".to_string()),
        extra: format!("/Annots [{keep} 0 R {widget} 0 R]"),
    };
    build(&[spec], &catalog(&[keep, field]), "", objs.bodies)
}

/// A form that holds a reset (on the check box `box_a`), a field `keep` ("kept") with no parent and
/// the text field `par`, whose actions (blur and focus, both a reset) are on the field above its
/// one widget, not on the widget. PDFium reads those two events from the widget alone, so they are
/// harmless here, and the rule of the engine (which looks at the events PDFium reads from another
/// dictionary, and at the actions of the widget itself) leaves `keep` and `par` free.
pub fn forms_parent_event_pdf() -> Vec<u8> {
    let mut objs = Objects::new(6);
    let page = 4;
    let keep = text(&mut objs, page, "keep", 740, "kept", "");
    let boxed = check_box(&mut objs, page, "box_a", 710, &format!("/A {RESET}"));
    let parent = objs.reserve();
    let kid = objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /Parent {parent} 0 R \
         /Rect [100 680 300 700] /F 4 /P {page} 0 R /DA (/Helv 12 Tf 0 g) >>"
    ));
    objs.set(
        parent,
        format!(
            "<< /FT /Tx /T (par) /V (parent value) /Kids [{kid} 0 R] \
             /AA << /Bl {RESET} /Fo {RESET} >> >>"
        ),
    );
    let spec = PageSpec {
        width: 612,
        height: 792,
        content: Some("0 g 10 10 1 1 re f\n".to_string()),
        extra: format!("/Annots [{keep} 0 R {boxed} 0 R {kid} 0 R]"),
    };
    build(&[spec], &catalog(&[keep, boxed, parent]), "", objs.bodies)
}

/// A push button `clear` with `/A /ResetForm`, as an ordinary form has to clear it. It is no field
/// that the reader fills in.
fn clear_button(objs: &mut Objects, page: usize, y: u32) -> usize {
    objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (clear) \
         /Rect [400 {y} 480 {top}] /F 4 /P {page} 0 R /MK << /BG [0.8 0.8 0.8] /CA (Clear) >> \
         /A {RESET} >>",
        top = y + 20
    ))
}

/// A form in which the way an action is written is the thing tried. The objects `actions` are
/// numbered from 6 in the order given (so `"6 0 R"` is the first), and the fields come after them:
///
/// - `keep` holds "kept", and `plain` has no action
/// - `target` has `target_extra` among the entries of its dictionary (for example `/AA << /Fo 6 0 R
///   >>`), so that what the action does can be judged by what happens to the other fields
/// - `clear` is a push button with `/A /ResetForm`, so that the document holds a reset whatever the
///   actions say
///
/// With `packed` the objects are kept in an object stream.
pub fn forms_action_pdf(actions: &[&str], target_extra: &str, packed: bool) -> Vec<u8> {
    let mut objs = Objects::new(6);
    let page = 4;
    for action in actions {
        objs.add(action.as_bytes());
    }
    let keep = text(&mut objs, page, "keep", 740, "kept", "");
    let plain = text(&mut objs, page, "plain", 700, "", "");
    let target = text(&mut objs, page, "target", 660, "", target_extra);
    let clear = clear_button(&mut objs, page, 620);
    let fields = [keep, plain, target, clear];
    let annot_list: String = fields.iter().fold(String::new(), |mut s, f| {
        let _ = write!(s, "{f} 0 R ");
        s
    });
    let spec = PageSpec {
        width: 612,
        height: 792,
        content: Some("0 g 10 10 1 1 re f\n".to_string()),
        extra: format!("/Annots [{annot_list}]"),
    };
    let objects = build_objects(&[spec], &catalog(&fields), objs.bodies);
    if packed {
        assemble_packed(&objects, 6)
    } else {
        assemble(&objects, "")
    }
}

/// Two dictionaries with one full name, `t`, that PDFium joins by name alone (no `/Kids` and no
/// `/Parent` links them), and `keep` ("kept"). `aa` is the content of an `/AA` dictionary (such as
/// `/K << /S /ResetForm >>`) that sits on the first of the two. That one is a plain object that no
/// page shows, or with `holder_widget` a widget of its own on the page; the second is a widget on the
/// page with no action. All three are listed in `/Fields`.
pub fn forms_same_name_pdf(aa: &str, holder_widget: bool) -> Vec<u8> {
    let mut objs = Objects::new(6);
    let page = 4;
    let keep = text(&mut objs, page, "keep", 740, "kept", "");
    let first = if holder_widget {
        text(&mut objs, page, "t", 700, "", &format!("/AA << {aa} >>"))
    } else {
        objs.add(format!("<< /FT /Tx /T (t) /AA << {aa} >> >>"))
    };
    let second = text(&mut objs, page, "t", 660, "", "");
    let annots = if holder_widget {
        format!("{keep} 0 R {first} 0 R {second} 0 R")
    } else {
        format!("{keep} 0 R {second} 0 R")
    };
    let spec = PageSpec {
        width: 612,
        height: 792,
        content: Some("0 g 10 10 1 1 re f\n".to_string()),
        extra: format!("/Annots [{annots}]"),
    };
    build(&[spec], &catalog(&[keep, first, second]), "", objs.bodies)
}

/// An ordinary form with a "Clear form" button (a reset) and fields that have the scripts such a
/// form has: `born` has a date format and keystroke script, `amount` a number format, keystroke,
/// validate and calculate script. With `two_widgets` each of them is a field with two widgets and
/// the scripts are on the field; otherwise field and widget are one object. The other fields are
/// `keep` ("kept") and `name` (text), `agree` (a check box) and `choice`, a group of two radio
/// buttons. Nothing here reaches a reset except the button, and every field can be filled in.
pub fn forms_clear_pdf(two_widgets: bool) -> Vec<u8> {
    let script = |js: &str| format!("<< /S /JavaScript /JS ({js}) >>");
    let date = format!(
        "/F {} /K {}",
        script(r#"AFDate_FormatEx\("dd.mm.yyyy"\);"#),
        script(r#"AFDate_KeystrokeEx\("dd.mm.yyyy"\);"#)
    );
    let number = format!(
        "/F {} /K {} /V {} /C {}",
        script(r#"AFNumber_Format\(2, 0, 0, 0, "", true\);"#),
        script(r#"AFNumber_Keystroke\(2, 0, 0, 0, "", true\);"#),
        script(r"AFRange_Validate\(true, 0, true, 1000\);"),
        script(r#"AFSimple_Calculate\("SUM", new Array\("born"\)\);"#)
    );
    let mut objs = Objects::new(6);
    let page = 4;
    let mut fields = Vec::new();
    let mut annots = Vec::new();
    let keep = text(&mut objs, page, "keep", 740, "kept", "");
    fields.push(keep);
    annots.push(keep);
    let name = text(&mut objs, page, "name", 710, "", "");
    fields.push(name);
    annots.push(name);
    for (field_name, y, aa) in [("born", 680u32, &date), ("amount", 650u32, &number)] {
        if two_widgets {
            let field = objs.reserve();
            let mut kids = String::new();
            for x in [100u32, 320] {
                let widget = objs.add(format!(
                    "<< /Type /Annot /Subtype /Widget /Parent {field} 0 R \
                     /Rect [{x} {y} {right} {top}] /F 4 /P {page} 0 R /DA (/Helv 12 Tf 0 g) >>",
                    right = x + 200,
                    top = y + 20
                ));
                let _ = write!(kids, "{widget} 0 R ");
                annots.push(widget);
            }
            objs.set(
                field,
                format!("<< /FT /Tx /T ({field_name}) /AA << {aa} >> /Kids [{kids}] >>"),
            );
            fields.push(field);
        } else {
            let field = text(
                &mut objs,
                page,
                field_name,
                y,
                "",
                &format!("/AA << {aa} >>"),
            );
            fields.push(field);
            annots.push(field);
        }
    }
    let agree = check_box(&mut objs, page, "agree", 610, "");
    fields.push(agree);
    annots.push(agree);
    // A group of two radio buttons.
    let group = objs.reserve();
    let mut kids = String::new();
    for (i, state) in ["A", "B"].into_iter().enumerate() {
        let on = objs.add(stream_object(
            "/Type /XObject /Subtype /Form /BBox [0 0 16 16]",
            "q 0 g 4 4 8 8 re f Q",
        ));
        let off = objs.add(stream_object(
            "/Type /XObject /Subtype /Form /BBox [0 0 16 16]",
            "",
        ));
        let x = 100 + 40 * i as u32;
        let widget = objs.add(format!(
            "<< /Type /Annot /Subtype /Widget /Parent {group} 0 R /Rect [{x} 580 {right} 596] \
             /F 4 /P {page} 0 R /AS /Off /AP << /N << /{state} {on} 0 R /Off {off} 0 R >> >> >>",
            right = x + 16
        ));
        let _ = write!(kids, "{widget} 0 R ");
        annots.push(widget);
    }
    objs.set(
        group,
        format!("<< /FT /Btn /Ff 32768 /T (choice) /V /Off /Kids [{kids}] >>"),
    );
    fields.push(group);
    let clear = clear_button(&mut objs, page, 540);
    fields.push(clear);
    annots.push(clear);
    let annot_list: String = annots.iter().fold(String::new(), |mut s, f| {
        let _ = write!(s, "{f} 0 R ");
        s
    });
    let spec = PageSpec {
        width: 612,
        height: 792,
        content: Some("0 g 10 10 1 1 re f\n".to_string()),
        extra: format!("/Annots [{annot_list}]"),
    };
    build(&[spec], &catalog(&fields), "", objs.bodies)
}
