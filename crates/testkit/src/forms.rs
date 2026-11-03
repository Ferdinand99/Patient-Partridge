//! Documents with forms, for the tests of filling in and saving. Like the rest of the kit they are
//! generated in code, so no binary file lives in the repository.
//!
//! The field names the tests rely on (page 1 unless it says otherwise):
//!
//! - `fullname`: a required text field with the tooltip "Full name", no appearance stream
//! - `notes`: a multi-line text field, with an empty appearance stream
//! - `locked`: a read-only text field that holds "Fixed"
//! - `agree`: a check box (export value `Yes`), off
//! - `size`: a group of three radio buttons (`S`, `M`, `L`) of which none is on, and which cannot
//!   be turned off by pressing the one that is on
//! - `mode`: a group of two radio buttons (`A`, `B`) that can be turned off again
//! - `country`: a drop-down list (Norway, Sweden, Denmark)
//! - `city`: a drop-down list with a box to type in (Oslo, Bergen)
//! - `colours`: a list box (Red, Green, Blue, Yellow)
//! - `toppings`: a list box that takes several choices (Ham, Cheese, Olives, Onions)
//! - `reset`: a push button, which the reader does not list
//!
//! The fields come in the order above, which is the order of their widgets on the page.

use std::fmt::Write as _;

use super::{build, stream_object, PageSpec};

/// What a generated form is made of, beyond the fields every form has.
#[derive(Default, Clone)]
pub struct FormOptions {
    /// Turn the page this many degrees (0, 90, 180 or 270).
    pub rotate: u32,
    /// Scripts, which never run, and an action: a format and a calculation on fields, and an
    /// action on the push button.
    pub scripts: bool,
    /// The form carries XFA data. With `xfa_full` the page is the stub of a form that only XFA can
    /// show.
    pub xfa: bool,
    pub xfa_full: bool,
    /// A signature field holding a signature dictionary, as `signed_pdf` has.
    pub signed: bool,
    /// A signature field that holds nothing, as a form has before anybody signs it. With
    /// `signed` the form has both.
    pub empty_signature: bool,
    /// A second signature field that holds nothing, below the first.
    pub second_empty_signature: bool,
    /// Fields that sit under a parent field: `person.first` and `person.last`.
    pub nested: bool,
    /// An object that no page uses and that is this many bytes long, so that the file is big and
    /// the form is small.
    pub padding: usize,
    /// An XMP metadata stream on the catalog (`XMP_CATALOG_TITLE`), as most forms carry.
    pub xmp: bool,
}

/// A growing list of objects. The kit's `build` puts the catalog, the page tree and the font at 1 to
/// 3, then the page and its content stream, so the objects made here start at 6.
struct Objects {
    bodies: Vec<Vec<u8>>,
}

const FIRST_OBJECT: usize = 6;

impl Objects {
    fn new() -> Objects {
        Objects { bodies: Vec::new() }
    }

    /// Takes the next object number, to be filled in later with `set`.
    fn reserve(&mut self) -> usize {
        self.bodies.push(Vec::new());
        FIRST_OBJECT + self.bodies.len() - 1
    }

    fn set(&mut self, number: usize, body: impl Into<Vec<u8>>) {
        self.bodies[number - FIRST_OBJECT] = body.into();
    }

    fn add(&mut self, body: impl Into<Vec<u8>>) -> usize {
        let number = self.reserve();
        self.set(number, body);
        number
    }
}

/// An appearance stream for a button state: a box of `size` points with `content` drawn in it.
fn appearance(size: u32, content: &str) -> Vec<u8> {
    stream_object(
        &format!("/Type /XObject /Subtype /Form /BBox [0 0 {size} {size}]"),
        content,
    )
}

/// A PDF text string in parentheses. Only plain characters are used by the callers, so only the
/// characters that need it are escaped.
fn pdf_string(text: &str) -> String {
    let mut out = String::from("(");
    for c in text.chars() {
        if matches!(c, '(' | ')' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push(')');
    out
}

fn options_array(labels: &[&str]) -> String {
    let mut out = String::from("[");
    for label in labels {
        let _ = write!(out, "{} ", pdf_string(label));
    }
    out.push(']');
    out
}

/// The label drawn on the page next to a field.
fn label(text: &str, x: u32, y: u32) -> String {
    format!("BT /F1 11 Tf {x} {y} Td {} Tj ET\n", pdf_string(text))
}

/// The fields every form has. Returns the objects and the numbers of the widgets, in page order.
fn standard_fields(objs: &mut Objects, opts: &FormOptions) -> (Vec<usize>, Vec<usize>, String) {
    let mut annots: Vec<usize> = Vec::new();
    let mut top_level: Vec<usize> = Vec::new();
    let mut content = String::new();
    let page = 4;
    // A text field `/Parent` is added when the fields are nested.
    // Each row is placed under the one before it: `row` returns the bottom edge of a field of the
    // given height.
    let mut cursor = 724u32;
    let mut row = |content: &mut String, text: &str, height: u32| -> u32 {
        let bottom = cursor - height;
        content.push_str(&label(text, 72, bottom + 6));
        cursor = bottom - 16;
        bottom
    };

    let calc = if opts.scripts {
        " /AA << /K << /S /JavaScript /JS (AFNumber_Keystroke\\(2, 0, 0, 0, \"\", true\\);) >> \
         /F << /S /JavaScript /JS (AFNumber_Format\\(2, 0, 0, 0, \"\", true\\);) >> \
         /C << /S /JavaScript /JS (event.value = 1 + 1;) >> >>"
    } else {
        ""
    };

    // fullname
    let at = row(&mut content, "Name:", 24);
    let (parent, widget_parent) = if opts.nested {
        let p = objs.reserve();
        (Some(p), format!(" /Parent {p} 0 R"))
    } else {
        (None, String::new())
    };
    let name_key = if opts.nested { "first" } else { "fullname" };
    let fullname = objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name_key}) /TU (Full name) /Ff 2 \
         /Rect [150 {at} 400 {top}] /F 4 /P {page} 0 R /DA (/Helv 12 Tf 0 g){widget_parent}{calc} \
         /MK << /BC [0] /BG [1 1 1] >> >>",
        top = at + 24
    ));
    annots.push(fullname);

    // notes
    let at = row(&mut content, "Notes:", 84);
    let empty_ap = objs.add(stream_object(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 84] /Resources << /Font << /Helv 3 0 R >> >>",
        "",
    ));
    let notes = objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (notes) /Ff 4096 \
         /Rect [150 {at} 450 {top}] /F 4 /P {page} 0 R /DA (/Helv 10 Tf 0 g) \
         /AP << /N {empty_ap} 0 R >> /MK << /BC [0] >> >>",
        top = at + 84
    ));
    annots.push(notes);

    // locked
    let at = row(&mut content, "Locked:", 24);
    let locked = objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (locked) /Ff 1 /V (Fixed) \
         /Rect [150 {at} 400 {top}] /F 4 /P {page} 0 R /DA (/Helv 12 Tf 0 g) /MK << /BC [0] >> >>",
        top = at + 24
    ));
    annots.push(locked);

    // agree
    let at = row(&mut content, "Agree:", 16);
    let on = objs.add(appearance(16, "q 0 0 0 rg 3 3 10 10 re f Q"));
    let off = objs.add(appearance(16, ""));
    let agree = objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /TU (I agree to the terms) \
         /V /Off /AS /Off /Rect [150 {at} 166 {top}] /F 4 /P {page} 0 R \
         /AP << /N << /Yes {on} 0 R /Off {off} 0 R >> >> /MK << /BC [0] /CA (4) >> >>",
        top = at + 16
    ));
    annots.push(agree);

    // size (radio, no toggle off) and mode (radio, toggles off)
    for (group, flags, states) in [
        ("size", 49152, ["S", "M", "L"].as_slice()),
        ("mode", 32768, ["A", "B"].as_slice()),
    ] {
        let at = row(&mut content, &format!("{group}:"), 16);
        let parent_field = objs.reserve();
        let mut kids = Vec::new();
        for (i, state) in states.iter().enumerate() {
            let on = objs.add(appearance(16, "q 0 0 0 rg 4 4 8 8 re f Q"));
            let off = objs.add(appearance(16, ""));
            let x = 150 + 40 * i as u32;
            let widget = objs.add(format!(
                "<< /Type /Annot /Subtype /Widget /Parent {parent_field} 0 R /Rect [{x} {at} {right} {top}] \
                 /F 4 /P {page} 0 R /AS /Off /AP << /N << /{state} {on} 0 R /Off {off} 0 R >> >> \
                 /MK << /BC [0] /CA (l) >> >>",
                right = x + 16,
                top = at + 16
            ));
            kids.push(widget);
            annots.push(widget);
        }
        let kid_list: String = kids.iter().map(|k| format!("{k} 0 R ")).collect();
        objs.set(
            parent_field,
            format!(
                "<< /FT /Btn /Ff {flags} /T ({group}) /TU ({group} choice) /V /Off /Kids [{kid_list}] >>"
            ),
        );
        top_level.push(parent_field);
    }

    // country
    let at = row(&mut content, "Country:", 22);
    let country = objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (country) /Ff 131072 \
         /Opt {} /Rect [150 {at} 350 {top}] /F 4 /P {page} 0 R /DA (/Helv 12 Tf 0 g) \
         /MK << /BC [0] /BG [1 1 1] >> >>",
        options_array(&["Norway", "Sweden", "Denmark"]),
        top = at + 22
    ));
    annots.push(country);

    // city (editable)
    let at = row(&mut content, "City:", 22);
    let city = objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (city) /Ff 393216 \
         /Opt {} /Rect [150 {at} 350 {top}] /F 4 /P {page} 0 R /DA (/Helv 12 Tf 0 g) \
         /MK << /BC [0] /BG [1 1 1] >> >>",
        options_array(&["Oslo", "Bergen"]),
        top = at + 22
    ));
    annots.push(city);

    // colours
    let at = row(&mut content, "Colours:", 64);
    let colours = objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (colours) /Ff 0 \
         /Opt {} /Rect [150 {at} 300 {top}] /F 4 /P {page} 0 R /DA (/Helv 11 Tf 0 g) \
         /MK << /BC [0] /BG [1 1 1] >> >>",
        options_array(&["Red", "Green", "Blue", "Yellow"]),
        top = at + 64
    ));
    annots.push(colours);

    // toppings (several choices)
    let at = row(&mut content, "Toppings:", 64);
    let toppings = objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (toppings) /Ff 2097152 \
         /Opt {} /Rect [150 {at} 300 {top}] /F 4 /P {page} 0 R /DA (/Helv 11 Tf 0 g) \
         /MK << /BC [0] /BG [1 1 1] >> >>",
        options_array(&["Ham", "Cheese", "Olives", "Onions"]),
        top = at + 64
    ));
    annots.push(toppings);

    // reset (a push button; never listed)
    let at = row(&mut content, "", 22);
    let action = if opts.scripts {
        " /A << /S /JavaScript /JS (this.resetForm\\(\\);) >>"
    } else {
        ""
    };
    let reset = objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (reset) /Ff 65536 \
         /Rect [150 {at} 220 {top}] /F 4 /P {page} 0 R /DA (/Helv 11 Tf 0 g) \
         /MK << /BC [0] /BG [0.8 0.8 0.8] /CA (Reset) >>{action} >>",
        top = at + 22
    ));
    annots.push(reset);

    // Fields that are not under a parent are top level themselves.
    let mut fields: Vec<usize> = vec![];
    if let Some(parent) = parent {
        // A second kid under the same parent: `person.last`, a plain text field at the right.
        let last = objs.add(format!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (last) /Parent {parent} 0 R \
             /Rect [420 700 560 724] /F 4 /P {page} 0 R /DA (/Helv 12 Tf 0 g) /MK << /BC [0] >> >>"
        ));
        annots.push(last);
        objs.set(
            parent,
            format!("<< /T (person) /Kids [{fullname} 0 R {last} 0 R] >>"),
        );
        fields.push(parent);
    } else {
        fields.push(fullname);
    }
    fields.extend([notes, locked, agree]);
    fields.extend(top_level);
    fields.extend([country, city, colours, toppings, reset]);
    (annots, fields, content)
}

/// A one page form with every kind of field. See the top of this file for the names.
pub fn forms_pdf() -> Vec<u8> {
    forms_with(&FormOptions::default())
}

/// A form with the fields of [`forms_pdf`] in a file that is at least `bytes` bytes long.
pub fn forms_padded_pdf(bytes: usize) -> Vec<u8> {
    forms_with(&FormOptions {
        padding: bytes,
        ..FormOptions::default()
    })
}

/// As [`forms_pdf`], with `options`.
pub fn forms_with(opts: &FormOptions) -> Vec<u8> {
    let mut objs = Objects::new();
    let (mut annots, fields, mut content) = standard_fields(&mut objs, opts);
    if opts.padding > 0 {
        objs.add(stream_object("", &"%".repeat(opts.padding)));
    }
    let mut field_refs: String = fields
        .iter()
        .map(|f| format!("{f} 0 R"))
        .collect::<Vec<_>>()
        .join(" ");

    let mut catalog = String::new();
    let mut acro_extra = String::new();
    if opts.signed {
        let signature = objs.add(format!(
            "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached \
             /ByteRange [0 100 200 100] /Contents <{}> /M (D:20260101000000Z) >>",
            "30".repeat(64)
        ));
        let widget = objs.add(format!(
            "<< /Type /Annot /Subtype /Widget /FT /Sig /T (signature) /Rect [72 40 250 80] \
             /F 132 /P 4 0 R /V {signature} 0 R >>"
        ));
        annots.push(widget);
        field_refs.push_str(&format!(" {widget} 0 R"));
        acro_extra.push_str(" /SigFlags 3");
    }
    if opts.empty_signature {
        let widget = objs.add(
            "<< /Type /Annot /Subtype /Widget /FT /Sig /T (empty_signature) \
             /Rect [72 100 250 140] /F 132 /P 4 0 R >>",
        );
        annots.push(widget);
        field_refs.push_str(&format!(" {widget} 0 R"));
    }
    if opts.second_empty_signature {
        let widget = objs.add(
            "<< /Type /Annot /Subtype /Widget /FT /Sig /T (second_signature) \
             /Rect [72 160 250 200] /F 132 /P 4 0 R >>",
        );
        annots.push(widget);
        field_refs.push_str(&format!(" {widget} 0 R"));
    }
    if opts.xmp {
        let stream = objs.add(stream_object(
            "/Type /Metadata /Subtype /XML",
            &crate::xmp_packet(crate::XMP_CATALOG_TITLE),
        ));
        catalog.push_str(&format!(" /Metadata {stream} 0 R"));
    }
    if opts.xfa {
        let template = objs.add(stream_object(
            "",
            "<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\"/>",
        ));
        acro_extra.push_str(&format!(" /XFA [(template) {template} 0 R]"));
        if opts.xfa_full {
            catalog.push_str(" /NeedsRendering true");
            content = "BT /F1 14 Tf 72 700 Td (Please wait... If this message is not eventually replaced \
                       the viewer cannot show this document.) Tj ET\n"
                .to_string();
        }
    }
    if opts.scripts {
        catalog.push_str(
            " /OpenAction << /S /JavaScript /JS (app.alert\\(\"hello\"\\);) >> \
             /Names << /JavaScript << /Names [(init) << /S /JavaScript /JS (this.calculateNow\\(\\);) >>] >> >>",
        );
        acro_extra.push_str(" /CO [");
        acro_extra.push_str(&field_refs.split(' ').take(3).collect::<Vec<_>>().join(" "));
        acro_extra.push(']');
    }

    let annot_list: String = annots.iter().map(|a| format!("{a} 0 R ")).collect();
    let rotate = if opts.rotate != 0 {
        format!(" /Rotate {}", opts.rotate)
    } else {
        String::new()
    };
    let spec = PageSpec {
        width: 612,
        height: 792,
        content: Some(format!("{}{}", content, demo_footer())),
        extra: format!("/Annots [{annot_list}]{rotate}"),
    };
    let catalog_extra = format!(
        "/AcroForm << /Fields [{field_refs}] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >>{acro_extra} >>{catalog}"
    );
    build(&[spec], &catalog_extra, "", objs.bodies)
}

fn demo_footer() -> String {
    // A bar at the bottom, so that a page with a form still has something that is not a field.
    "0.1 0.3 0.8 rg 36 12 540 8 re f\n".to_string()
}

/// A form with scripts, which never run, and an action on a push button.
pub fn forms_script_pdf() -> Vec<u8> {
    forms_with(&FormOptions {
        scripts: true,
        ..FormOptions::default()
    })
}

/// A form on a page that is turned a quarter turn.
pub fn forms_rotated_pdf(degrees: u32) -> Vec<u8> {
    forms_with(&FormOptions {
        rotate: degrees,
        ..FormOptions::default()
    })
}

/// A form that carries XFA data next to its ordinary fields (a hybrid form).
pub fn xfa_pdf() -> Vec<u8> {
    forms_with(&FormOptions {
        xfa: true,
        ..FormOptions::default()
    })
}

/// A form that only XFA can show: the page is the "please wait" stub.
pub fn xfa_full_pdf() -> Vec<u8> {
    forms_with(&FormOptions {
        xfa: true,
        xfa_full: true,
        ..FormOptions::default()
    })
}

/// A form with a signature field that holds a signature (a save breaks it).
pub fn forms_signed_pdf() -> Vec<u8> {
    forms_with(&FormOptions {
        signed: true,
        ..FormOptions::default()
    })
}

/// A form with a signature field that nobody has signed: no signature dictionary, no byte range.
pub fn forms_empty_signature_pdf() -> Vec<u8> {
    forms_with(&FormOptions {
        empty_signature: true,
        ..FormOptions::default()
    })
}

/// A form with two signature fields that hold nothing.
pub fn forms_two_empty_signatures_pdf() -> Vec<u8> {
    forms_with(&FormOptions {
        empty_signature: true,
        second_empty_signature: true,
        ..FormOptions::default()
    })
}

/// A form with one signature field that holds a signature and one that holds nothing.
pub fn forms_signed_and_empty_signature_pdf() -> Vec<u8> {
    forms_with(&FormOptions {
        signed: true,
        empty_signature: true,
        ..FormOptions::default()
    })
}

/// A form whose first text field is `person.first`, with a sibling `person.last`.
pub fn forms_nested_pdf() -> Vec<u8> {
    forms_with(&FormOptions {
        nested: true,
        ..FormOptions::default()
    })
}

/// A page with `count` text fields named `f0`, `f1`, ... in a grid, to try the cap on the number
/// of fields of a page.
pub fn forms_many_pdf(count: usize) -> Vec<u8> {
    let mut objs = Objects::new();
    let mut annots = String::new();
    for i in 0..count {
        let x = 10 + (i % 20) * 29;
        let y = 20 + (i / 20) * 14;
        let n = objs.add(format!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (f{i}) /Rect [{x} {y} {r} {t}] /F 4 /P 4 0 R \
             /DA (/Helv 6 Tf 0 g) >>",
            r = x + 27,
            t = y + 12
        ));
        let _ = write!(annots, "{n} 0 R ");
    }
    let spec = PageSpec {
        width: 612,
        height: 792,
        content: Some("0 g 10 10 1 1 re f\n".to_string()),
        extra: format!("/Annots [{annots}]"),
    };
    build(
        &[spec],
        &format!("/AcroForm << /Fields [{annots}] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >>"),
        "",
        objs.bodies,
    )
}

/// A form made to push the limits of the protocol: a field with a name, a tooltip and a value far
/// longer than the caps, a list box with a thousand choices that have long labels, and a text field
/// whose value holds characters of every width.
pub fn forms_hostile_pdf() -> Vec<u8> {
    let mut objs = Objects::new();
    let long_name = "n".repeat(100_000);
    let long_tip = "t\\(\\)".repeat(60_000);
    let long_value = "v".repeat(200_000);
    let text = objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T ({long_name}) /TU ({long_tip}) /V ({long_value}) \
         /Rect [72 700 300 724] /F 4 /P 4 0 R /DA (/Helv 12 Tf 0 g) >>"
    ));
    let labels: Vec<String> = (0..1_000)
        .map(|i| format!("choice {i} {}", "x".repeat(2_000)))
        .collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let list = objs.add(format!(
        "<< /Type /Annot /Subtype /Widget /FT /Ch /T (many) /Ff 2097152 /Opt {} \
         /Rect [72 500 300 650] /F 4 /P 4 0 R /DA (/Helv 11 Tf 0 g) >>",
        options_array(&refs)
    ));
    let unicode = objs.add(
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T <FEFF00E500F800E6> /TU <FEFF263A> \
         /V <FEFF05D005D105D2D83DDE00> /Rect [72 450 300 474] /F 4 /P 4 0 R /DA (/Helv 12 Tf 0 g) >>",
    );
    let spec = PageSpec {
        width: 612,
        height: 792,
        content: Some("0 g 10 10 1 1 re f\n".to_string()),
        extra: format!("/Annots [{text} 0 R {list} 0 R {unicode} 0 R]"),
    };
    build(
        &[spec],
        &format!(
            "/AcroForm << /Fields [{text} 0 R {list} 0 R {unicode} 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >>"
        ),
        "",
        objs.bodies,
    )
}
