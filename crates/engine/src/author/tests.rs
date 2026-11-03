//! The writer on its own, on documents made by the test kit (no PDFium): what it writes for each
//! kind of field, that what it writes reads back the same, the rules for names, and that damaged
//! bytes make it refuse and never panic. `tests/author.rs` has the same writer working through the
//! engine and the real PDFium.

use std::collections::HashMap;

use partridge_protocol::{
    AuthorOp, DesignField, DesignKind, DesignProps, DesignWidget, DesignedWidget, WidgetRef,
};
use partridge_testkit as kit;

use super::*;

fn geo(turns: u8) -> HashMap<usize, PageGeo> {
    (0..4)
        .map(|i| {
            (
                i,
                PageGeo {
                    turns,
                    visible: [0.0, 0.0, 612.0, 792.0],
                },
            )
        })
        .collect()
}

fn widget(page: u32, rect: [f32; 4]) -> DesignWidget {
    DesignWidget {
        page,
        rect,
        export: String::new(),
    }
}

fn field(name: &str, kind: DesignKind) -> DesignField {
    let mut props = DesignProps::default();
    if matches!(kind, DesignKind::Combo | DesignKind::List) {
        props.choices = vec!["Norge".into(), "Sverige".into(), "\u{c5}land".into()];
    }
    DesignField {
        name: name.into(),
        kind,
        props,
        widgets: vec![widget(0, [72.0, 100.0, 232.0, 122.0])],
    }
}

fn add(a: &mut Author, name: &str, kind: DesignKind) {
    a.apply(&[AuthorOp::Add(field(name, kind))]).unwrap();
}

fn listed(a: &mut Author, page: usize) -> Vec<DesignedWidget> {
    a.list_page(page).unwrap()
}

fn refusal(r: Result<(), AuthorError>) -> String {
    match r {
        Err(AuthorError::Refused(m)) => m,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_field_is_an_update_after_the_bytes_that_were_given() {
    let bytes = kit::simple_pdf(2);
    let mut a = Author::new(bytes.clone(), geo(0)).unwrap();
    add(&mut a, "name", DesignKind::Text);
    let out = a.finish().unwrap();
    assert!(out.bytes.starts_with(&bytes), "the original is not touched");
    let text = String::from_utf8_lossy(&out.bytes[bytes.len()..]).into_owned();
    for want in [
        "/FT /Tx",
        "/T (name)",
        "/Subtype /Widget",
        "/Prev ",
        "/AcroForm",
        "/Annots",
        "/DR",
    ] {
        assert!(
            text.contains(want),
            "{want} is missing from the update:\n{text}"
        );
    }
    assert!(!text.contains("/NeedAppearances"));
    for forbidden in ["/AA", "/JS", "/JavaScript", "/A <<", "/SigFlags"] {
        assert!(!text.contains(forbidden), "{forbidden} in the update");
    }
    assert_eq!(out.maps[&0], vec![None]);
    assert!(out.redraw.contains(&0));
}

#[test]
fn every_kind_reads_back_as_it_was_written() {
    let mut a = Author::new(kit::simple_pdf(1), geo(0)).unwrap();
    let mut text = field("t", DesignKind::Text);
    text.props = DesignProps {
        tooltip: "Your name \u{e6}\u{f8}\u{e5}".into(),
        required: true,
        max_len: 20,
        font_size: 11.0,
        default_value: "Bl\u{e5}b\u{e6}r".into(),
        ..DesignProps::default()
    };
    text.widgets[0].rect = [10.0, 10.0, 200.0, 30.0];
    a.apply(&[AuthorOp::Add(text.clone())]).unwrap();
    let mut multi = field("multi", DesignKind::Text);
    multi.props.multiline = true;
    multi.widgets[0].rect = [10.0, 40.0, 200.0, 100.0];
    let mut pass = field("pass", DesignKind::Text);
    pass.props.password = true;
    pass.widgets[0].rect = [10.0, 110.0, 200.0, 130.0];
    let mut comb = field("comb", DesignKind::Text);
    comb.props.comb = true;
    comb.props.max_len = 8;
    comb.widgets[0].rect = [10.0, 140.0, 200.0, 160.0];
    let mut check = field("check", DesignKind::Checkbox);
    check.props.export_value = "Ja".into();
    check.props.default_value = "on".into();
    check.widgets[0].rect = [10.0, 170.0, 24.0, 184.0];
    let mut combo = field("combo", DesignKind::Combo);
    combo.props.editable = true;
    combo.props.default_value = "Sverige".into();
    combo.widgets[0].rect = [10.0, 200.0, 200.0, 220.0];
    let mut list = field("list", DesignKind::List);
    list.props.multiselect = true;
    list.props.default_value = "Norge\n\u{c5}land".into();
    list.widgets[0].rect = [10.0, 230.0, 200.0, 300.0];
    let mut sign = field("sign", DesignKind::Signature);
    sign.props.tooltip = "Sign here".into();
    sign.widgets[0].rect = [10.0, 310.0, 210.0, 360.0];
    let radio = DesignField {
        name: "size".into(),
        kind: DesignKind::Radio,
        props: DesignProps {
            default_value: "M".into(),
            ..DesignProps::default()
        },
        widgets: ["S", "M", "L"]
            .iter()
            .enumerate()
            .map(|(i, e)| DesignWidget {
                page: 0,
                rect: [10.0 + 30.0 * i as f32, 370.0, 24.0 + 30.0 * i as f32, 384.0],
                export: (*e).into(),
            })
            .collect(),
    };
    a.apply(&[multi, pass, comb, check, combo, list, sign, radio].map(AuthorOp::Add))
        .unwrap();
    let items = listed(&mut a, 0);
    let by =
        |name: &str| -> Vec<&DesignedWidget> { items.iter().filter(|w| w.name == name).collect() };
    let t = by("t")[0];
    assert_eq!(t.kind, DesignKind::Text);
    assert!(t.editable);
    assert_eq!(t.props.tooltip, text.props.tooltip);
    assert!(t.props.required);
    assert_eq!(t.props.max_len, 20);
    assert_eq!(t.props.font_size, 11.0);
    assert_eq!(t.props.default_value, "Bl\u{e5}b\u{e6}r");
    assert_eq!(t.rect, [10.0, 10.0, 200.0, 30.0]);
    assert!(by("multi")[0].props.multiline);
    assert!(by("pass")[0].props.password);
    let c = by("comb")[0];
    assert!(c.props.comb);
    assert_eq!(c.props.max_len, 8);
    let ck = by("check")[0];
    assert_eq!(ck.kind, DesignKind::Checkbox);
    assert_eq!(ck.export, "Ja");
    assert_eq!(ck.props.export_value, "Ja");
    assert_eq!(ck.props.default_value, "on");
    let co = by("combo")[0];
    assert_eq!(co.kind, DesignKind::Combo);
    assert!(co.props.editable);
    assert_eq!(co.props.choices, vec!["Norge", "Sverige", "\u{c5}land"]);
    assert_eq!(co.props.default_value, "Sverige");
    let li = by("list")[0];
    assert_eq!(li.kind, DesignKind::List);
    assert!(li.props.multiselect);
    assert_eq!(li.props.default_value, "Norge\n\u{c5}land");
    assert_eq!(by("sign")[0].kind, DesignKind::Signature);
    let size = by("size");
    assert_eq!(size.len(), 3);
    assert_eq!(
        size.iter().map(|w| w.export.as_str()).collect::<Vec<_>>(),
        ["S", "M", "L"]
    );
    assert_eq!(size[0].widgets, 3);
    assert_eq!(size[1].widget, 1);
    assert_eq!(size[1].props.default_value, "M");
}

#[test]
fn a_turned_page_gets_the_turn_written_into_the_field() {
    for turns in 1..4u8 {
        let mut a = Author::new(kit::simple_pdf(1), geo(turns)).unwrap();
        // The page is drawn 792 wide and 612 tall when it is turned a quarter.
        let mut f = field("t", DesignKind::Text);
        let rect = [72.0, 100.0, 232.0, 122.0];
        f.widgets[0].rect = rect;
        a.apply(&[AuthorOp::Add(f)]).unwrap();
        // The box as the reader sees it is the box that was drawn.
        let back = listed(&mut a, 0)[0].rect;
        for (x, y) in back.iter().zip(rect) {
            assert!((x - y).abs() < 1e-3, "turns {turns}: {back:?} != {rect:?}");
        }
        let out = a.finish().unwrap();
        let text = String::from_utf8_lossy(&out.bytes).into_owned();
        assert!(
            text.contains(&format!("/R {}", u32::from(turns) * 90)),
            "turns {turns}: no /MK /R"
        );
    }
    // A page that is not turned has none.
    let mut a = Author::new(kit::simple_pdf(1), geo(0)).unwrap();
    add(&mut a, "t", DesignKind::Text);
    let text = String::from_utf8_lossy(&a.finish().unwrap().bytes).into_owned();
    assert!(!text.contains("/R 90") && !text.contains("/MK << /BC [0 0 0] /R"));
}

#[test]
fn names_have_to_be_free_and_without_a_dot() {
    let mut a = Author::new(kit::simple_pdf(1), geo(0)).unwrap();
    add(&mut a, "a", DesignKind::Text);
    let m = refusal(a.apply(&[AuthorOp::Add(field("a", DesignKind::Checkbox))]));
    assert!(m.contains("already a field called"), "{m}");
    assert!(refusal(a.apply(&[AuthorOp::Add(field("a.b", DesignKind::Text))])).contains("dot"));
    assert!(refusal(a.apply(&[AuthorOp::Add(field("", DesignKind::Text))])).contains("name"));
    // The name of a field that was there before is taken too, and so is a part of a name.
    let mut b = Author::new(kit::forms_nested_pdf(), geo(0)).unwrap();
    let names = b.names().unwrap();
    assert!(names.contains(&"person.first".to_string()), "{names:?}");
    assert!(names.contains(&"person".to_string()), "{names:?}");
    let m = refusal(b.apply(&[AuthorOp::Add(field("person", DesignKind::Text))]));
    assert!(m.contains("already"), "{m}");
    let first = names.iter().find(|n| !n.contains('.')).unwrap().clone();
    assert!(
        refusal(b.apply(&[AuthorOp::Add(field(&first, DesignKind::Text))])).contains("already")
    );
    // A name nobody has is fine, case counts.
    b.apply(&[AuthorOp::Add(field("Person", DesignKind::Text))])
        .unwrap();
}

#[test]
fn a_field_that_cannot_be_drawn_is_refused_with_the_character_named() {
    let mut a = Author::new(kit::simple_pdf(1), geo(0)).unwrap();
    let mut f = field("t", DesignKind::Text);
    f.props.default_value = "price \u{20ac}5".into();
    let m = refusal(a.apply(&[AuthorOp::Add(f)]));
    assert!(m.contains("\u{20ac}") && m.contains("Latin"), "{m}");
    let mut c = field("c", DesignKind::Combo);
    c.props.choices = vec!["\u{65e5}\u{672c}".into()];
    assert!(refusal(a.apply(&[AuthorOp::Add(c)])).contains("cannot be drawn"));
    // The name and the tooltip are not drawn, so they may hold anything.
    let mut ok = field("\u{65e5}\u{672c}_\u{e6}\u{f8}\u{e5}", DesignKind::Text);
    ok.props.tooltip = "\u{20ac} \u{65e5}\u{672c}".into();
    a.apply(&[AuthorOp::Add(ok)]).unwrap();
    assert_eq!(
        listed(&mut a, 0)[0].name,
        "\u{65e5}\u{672c}_\u{e6}\u{f8}\u{e5}"
    );
    assert_eq!(
        listed(&mut a, 0)[0].props.tooltip,
        "\u{20ac} \u{65e5}\u{672c}"
    );
}

#[test]
fn a_box_outside_the_page_or_too_small_is_refused() {
    let mut a = Author::new(kit::simple_pdf(1), geo(0)).unwrap();
    let mut f = field("t", DesignKind::Text);
    f.widgets[0].rect = [600.0, 100.0, 700.0, 120.0];
    assert!(refusal(a.apply(&[AuthorOp::Add(f.clone())])).contains("inside the page"));
    f.widgets[0].rect = [10.0, 10.0, 12.0, 120.0];
    assert!(refusal(a.apply(&[AuthorOp::Add(f.clone())])).contains("too small"));
    f.widgets[0].page = 9;
    assert!(refusal(a.apply(&[AuthorOp::Add(f)])).contains("not there"));
}

#[test]
fn changing_a_field_changes_what_is_listed() {
    let mut a = Author::new(kit::simple_pdf(1), geo(0)).unwrap();
    add(&mut a, "t", DesignKind::Text);
    add(&mut a, "other", DesignKind::Text);
    let mut props = listed(&mut a, 0)[0].props.clone();
    props.tooltip = "hello".into();
    props.required = true;
    props.max_len = 5;
    props.default_value = "abc".into();
    a.apply(&[AuthorOp::Update {
        name: "t".into(),
        new_name: "renamed".into(),
        props: props.clone(),
        exports: vec![],
    }])
    .unwrap();
    let w = listed(&mut a, 0)
        .into_iter()
        .find(|w| w.name == "renamed")
        .unwrap();
    assert_eq!(w.props.tooltip, "hello");
    assert!(w.props.required);
    assert_eq!(w.props.max_len, 5);
    assert_eq!(w.props.default_value, "abc");
    assert!(!a.names().unwrap().contains(&"t".to_string()));
    // A new name that is taken, and a default that does not fit, are refused and change nothing.
    let m = refusal(a.apply(&[AuthorOp::Update {
        name: "renamed".into(),
        new_name: "other".into(),
        props: props.clone(),
        exports: vec![],
    }]));
    assert!(m.contains("already"), "{m}");
    props.default_value = "far too long".into();
    assert!(refusal(a.apply(&[AuthorOp::Update {
        name: "renamed".into(),
        new_name: String::new(),
        props,
        exports: vec![],
    }]))
    .contains("longer"));
    // A field that is not there.
    assert!(refusal(a.apply(&[AuthorOp::Remove {
        name: "nothing".into()
    }]))
    .contains("no field"));
}

#[test]
fn a_radio_group_can_grow_shrink_and_be_renamed() {
    let mut a = Author::new(kit::simple_pdf(1), geo(0)).unwrap();
    let radio = DesignField {
        name: "size".into(),
        kind: DesignKind::Radio,
        props: DesignProps {
            default_value: "M".into(),
            ..DesignProps::default()
        },
        widgets: ["S", "M"]
            .iter()
            .enumerate()
            .map(|(i, e)| DesignWidget {
                page: 0,
                rect: [10.0 + 30.0 * i as f32, 10.0, 24.0 + 30.0 * i as f32, 24.0],
                export: (*e).into(),
            })
            .collect(),
    };
    a.apply(&[AuthorOp::Add(radio)]).unwrap();
    a.apply(&[AuthorOp::AddWidget {
        name: "size".into(),
        widget: DesignWidget {
            page: 0,
            rect: [70.0, 10.0, 84.0, 24.0],
            export: "L".into(),
        },
    }])
    .unwrap();
    assert!(refusal(a.apply(&[AuthorOp::AddWidget {
        name: "size".into(),
        widget: DesignWidget {
            page: 0,
            rect: [100.0, 10.0, 114.0, 24.0],
            export: "L".into(),
        },
    }]))
    .contains("value"));
    assert_eq!(listed(&mut a, 0).len(), 3);
    // Rename the buttons; the one that is on stays on.
    let props = listed(&mut a, 0)[0].props.clone();
    a.apply(&[AuthorOp::Update {
        name: "size".into(),
        new_name: String::new(),
        props: DesignProps {
            default_value: "Medium".into(),
            ..props
        },
        exports: vec!["Small".into(), "Medium".into(), "Large".into()],
    }])
    .unwrap();
    let items = listed(&mut a, 0);
    assert_eq!(
        items.iter().map(|w| w.export.as_str()).collect::<Vec<_>>(),
        ["Small", "Medium", "Large"]
    );
    assert_eq!(items[0].props.default_value, "Medium");
    // Taking one away keeps the others; taking the last takes the field.
    a.apply(&[AuthorOp::RemoveWidget {
        name: "size".into(),
        widget: 0,
    }])
    .unwrap();
    assert_eq!(listed(&mut a, 0).len(), 2);
    a.apply(&[
        AuthorOp::RemoveWidget {
            name: "size".into(),
            widget: 0,
        },
        AuthorOp::RemoveWidget {
            name: "size".into(),
            widget: 0,
        },
    ])
    .unwrap();
    assert!(listed(&mut a, 0).is_empty());
    assert!(!a.names().unwrap().contains(&"size".to_string()));
}

#[test]
fn moving_a_widget_to_another_page_moves_it_in_the_lists() {
    let mut a = Author::new(kit::simple_pdf(2), geo(0)).unwrap();
    add(&mut a, "t", DesignKind::Text);
    a.apply(&[AuthorOp::Move {
        name: "t".into(),
        widget: 0,
        page: 1,
        rect: [20.0, 20.0, 120.0, 40.0],
    }])
    .unwrap();
    assert!(listed(&mut a, 0).is_empty());
    let on_two = listed(&mut a, 1);
    assert_eq!(on_two.len(), 1);
    assert_eq!(on_two[0].rect, [20.0, 20.0, 120.0, 40.0]);
    let out = a.finish().unwrap();
    assert_eq!(out.maps[&0], Vec::<Option<usize>>::new());
    assert_eq!(out.maps[&1], vec![None]);
    // Moving within the page keeps its place in the list.
    let mut a = Author::new(kit::simple_pdf(1), geo(0)).unwrap();
    add(&mut a, "t", DesignKind::Text);
    a.apply(&[AuthorOp::Move {
        name: "t".into(),
        widget: 0,
        page: 0,
        rect: [30.0, 30.0, 130.0, 50.0],
    }])
    .unwrap();
    assert_eq!(listed(&mut a, 0)[0].rect, [30.0, 30.0, 130.0, 50.0]);
    assert!(refusal(a.apply(&[AuthorOp::Move {
        name: "t".into(),
        widget: 4,
        page: 0,
        rect: [30.0, 30.0, 130.0, 50.0],
    }]))
    .contains("not there"));
}

#[test]
fn the_tab_order_is_the_order_of_the_annotations() {
    let mut a = Author::new(kit::simple_pdf(1), geo(0)).unwrap();
    for (i, n) in ["a", "b", "c"].iter().enumerate() {
        let mut f = field(n, DesignKind::Text);
        f.widgets[0].rect = [10.0, 10.0 + 30.0 * i as f32, 100.0, 30.0 + 30.0 * i as f32];
        a.apply(&[AuthorOp::Add(f)]).unwrap();
    }
    let names =
        |a: &mut Author| -> Vec<String> { listed(a, 0).into_iter().map(|w| w.name).collect() };
    assert_eq!(names(&mut a), ["a", "b", "c"]);
    let order = ["c", "a", "b"].map(|n| WidgetRef {
        name: n.into(),
        widget: 0,
    });
    a.apply(&[AuthorOp::Reorder {
        page: 0,
        order: order.to_vec(),
    }])
    .unwrap();
    assert_eq!(names(&mut a), ["c", "a", "b"]);
    // A name twice, and a name that is not there.
    let twice = vec![order[0].clone(), order[0].clone()];
    assert!(refusal(a.apply(&[AuthorOp::Reorder {
        page: 0,
        order: twice
    }]))
    .contains("twice"));
    let gone = vec![WidgetRef {
        name: "zzz".into(),
        widget: 0,
    }];
    assert!(refusal(a.apply(&[AuthorOp::Reorder {
        page: 0,
        order: gone
    }]))
    .contains("no field"));
    let out = a.finish().unwrap();
    // The ids of the annotations go with them: c was the third of the list and is the first now.
    assert_eq!(out.maps[&0], vec![None, None, None]);
}

#[test]
fn removing_a_field_takes_it_from_the_page_and_the_form() {
    let mut a = Author::new(kit::forms_pdf(), geo(0)).unwrap();
    let before = listed(&mut a, 0);
    assert!(before.len() >= 3, "{}", before.len());
    let editable = before.iter().find(|w| w.editable).unwrap().name.clone();
    a.apply(&[AuthorOp::Remove {
        name: editable.clone(),
    }])
    .unwrap();
    let after = listed(&mut a, 0);
    assert!(after.iter().all(|w| w.name != editable));
    assert!(after.len() < before.len());
    assert!(!a.names().unwrap().contains(&editable));
    let out = a.finish().unwrap();
    assert!(out.maps[&0].len() < before.len() + 20);
    assert!(out.maps[&0].iter().all(Option::is_some));
}

#[test]
fn what_is_not_ours_is_listed_but_left_alone() {
    let mut a = Author::new(kit::forms_nested_pdf(), geo(0)).unwrap();
    let items = listed(&mut a, 0);
    let nested = items.iter().find(|w| w.name == "person.first").unwrap();
    assert!(!nested.editable);
    let m = refusal(a.apply(&[AuthorOp::Remove {
        name: "person.first".into(),
    }]));
    assert!(
        m.contains("no field") || m.contains("cannot be changed"),
        "{m}"
    );
    let script = Author::new(kit::forms_script_pdf(), geo(0));
    if let Ok(mut s) = script {
        let acting: Vec<_> = listed(&mut s, 0)
            .into_iter()
            .filter(|w| !w.editable)
            .collect();
        assert!(!acting.is_empty(), "a field with an action is left alone");
    }
}

#[test]
fn encrypted_and_xfa_documents_are_refused() {
    // The trailer comes after the table, so a word added to it moves no object.
    let mut enc = kit::simple_pdf(1);
    let at = enc
        .windows(11)
        .position(|w| w == b"/Root 1 0 R")
        .expect("a root")
        + 11;
    enc.splice(at..at, b" /Encrypt 9 0 R".iter().copied());
    match Author::new(enc, geo(0)) {
        Err(AuthorError::Unsupported(m)) => assert!(m.contains("encrypted"), "{m}"),
        other => panic!("{:?}", other.err()),
    }
    // The same length, so that the table of the file still points at the objects.
    let mut xfa = kit::forms_pdf();
    let at = xfa.windows(7).position(|w| w == b"/Fields").unwrap();
    xfa[at..at + 7].copy_from_slice(b"/XFA   ");
    match Author::new(xfa, geo(0)) {
        Err(AuthorError::Unsupported(m)) => assert!(m.contains("XFA"), "{m}"),
        other => panic!("{:?}", other.err()),
    }
}

#[test]
fn too_many_fields_on_a_page_are_refused() {
    let mut a = Author::new(kit::forms_many_pdf(1000), geo(0)).unwrap();
    let mut f = field("one more", DesignKind::Text);
    f.widgets[0].rect = [10.0, 700.0, 100.0, 720.0];
    assert!(refusal(a.apply(&[AuthorOp::Add(f)])).contains("at most"));
}

#[test]
fn damaged_bytes_are_refused_and_never_panic() {
    let good = kit::forms_pdf();
    // Cut at many places.
    for cut in (0..good.len()).step_by(37) {
        let _ = Author::new(good[..cut].to_vec(), geo(0)).map(|mut a| {
            let _ = a.apply(&[AuthorOp::Add(field("x", DesignKind::Text))]);
            let _ = a.finish();
        });
    }
    // One byte changed at many places, to a few values.
    for at in (0..good.len()).step_by(11) {
        for value in [0u8, b'<', b']', b'9', 0xff] {
            let mut bytes = good.clone();
            bytes[at] = value;
            if let Ok(mut a) = Author::new(bytes, geo(0)) {
                let _ = a.names();
                let _ = a.list_page(0);
                let _ = a.apply(&[AuthorOp::Add(field("x", DesignKind::Radio))]);
                let _ = a.finish();
            }
        }
    }
}

fn number_at(bytes: &[u8], from: usize) -> (usize, usize) {
    let end = bytes[from..]
        .iter()
        .position(|b| !b.is_ascii_digit())
        .expect("a number")
        + from;
    (
        std::str::from_utf8(&bytes[from..end])
            .unwrap()
            .parse()
            .unwrap(),
        end,
    )
}

/// The update that was appended to `original`: the bytes before it are untouched, the last line
/// names a table, the table lists objects that are where it says they are, and the trailer names
/// the table that was there before.
fn assert_well_formed(original: &[u8], out: &[u8], previous_table: &str) {
    assert!(
        out.starts_with(original),
        "the bytes it was given were changed"
    );
    assert!(out.ends_with(b"%%EOF\n"));
    let at = out
        .windows(10)
        .rposition(|w| w == b"startxref\n")
        .expect("startxref");
    let (table, _) = number_at(out, at + 10);
    assert!(table >= original.len() && table < at);
    assert!(out[table..].starts_with(b"xref\n"));
    let mut i = table + 5;
    let mut seen = 0;
    while !out[i..].starts_with(b"trailer\n") {
        let (_, e) = number_at(out, i);
        let (count, e) = number_at(out, e + 1);
        i = e + 1;
        for _ in 0..count {
            let entry = &out[i..i + 20];
            assert!(entry.ends_with(b" n \n"));
            let offset: usize = std::str::from_utf8(&entry[..10]).unwrap().parse().unwrap();
            let generation: u32 = std::str::from_utf8(&entry[11..16])
                .unwrap()
                .parse()
                .unwrap();
            assert!(offset >= original.len() && offset < table);
            let (number, e) = number_at(out, offset);
            assert!(
                out[e..].starts_with(format!(" {generation} obj\n").as_bytes()),
                "object {number}"
            );
            assert!(out[offset..table].windows(7).any(|w| w == b"endobj\n"));
            i += 20;
            seen += 1;
        }
    }
    assert!(seen > 0, "an update with no object");
    let trailer = String::from_utf8_lossy(&out[i..at]).into_owned();
    assert!(
        trailer.contains(&format!("/Prev {previous_table}")),
        "{trailer}"
    );
}

#[test]
fn what_is_appended_is_a_well_formed_update_for_every_kind_of_change() {
    for bytes in [
        kit::simple_pdf(2),
        kit::forms_pdf(),
        kit::forms_many_pdf(25),
        kit::turned_pdf(),
    ] {
        let previous = {
            let at = bytes
                .windows(10)
                .rposition(|w| w == b"startxref\n")
                .unwrap();
            number_at(&bytes, at + 10).0
        };
        let mut a = Author::new(bytes.clone(), geo(1)).unwrap();
        for (name, kind) in [
            ("t", DesignKind::Text),
            ("c", DesignKind::Checkbox),
            ("co", DesignKind::Combo),
            ("l", DesignKind::List),
            ("s", DesignKind::Signature),
        ] {
            let mut f = field(&format!("new {name}"), kind);
            f.widgets[0].rect = [10.0 + 20.0 * name.len() as f32, 20.0, 90.0, 40.0];
            a.apply(&[AuthorOp::Add(f)]).unwrap();
        }
        let second = usize::from(a.page_count() > 1) as u32;
        a.apply(&[
            AuthorOp::Move {
                name: "new t".into(),
                widget: 0,
                page: second,
                rect: [20.0, 20.0, 120.0, 40.0],
            },
            AuthorOp::Remove {
                name: "new c".into(),
            },
            AuthorOp::Update {
                name: "new l".into(),
                new_name: "renamed".into(),
                props: DesignProps {
                    choices: vec!["x".into(), "y".into()],
                    ..DesignProps::default()
                },
                exports: vec![],
            },
        ])
        .unwrap();
        let out = a.finish().unwrap();
        assert_well_formed(&bytes, &out.bytes, &previous.to_string());
    }
}

// ---- bytes stay bytes ---------------------------------------------------------------------------

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn has(hay: &[u8], needle: &[u8]) -> bool {
    find(hay, needle).is_some()
}

/// The text of object `n` in the update appended after the first `from` bytes.
fn revision(out: &[u8], from: usize, n: u32) -> Option<Vec<u8>> {
    // The bytes before an update end in a line break, so the first object has one before it too.
    let hay = &out[from - 1..];
    let head = format!("\n{n} 0 obj\n");
    let start = find(hay, head.as_bytes())? + head.len();
    let end = find(&hay[start..], b"\nendobj\n")? + start;
    Some(hay[start..end].to_vec())
}

fn revised(out: &[u8], from: usize, n: u32) -> bool {
    revision(out, from, n).is_some()
}

#[test]
fn what_an_annotation_holds_is_not_read_as_text_when_its_page_is_written_again() {
    let bytes = kit::direct_annots_pdf();
    // None of what the page holds is text: the writer may not change a byte of it.
    for item in kit::DIRECT_ANNOTS {
        assert!(std::str::from_utf8(item).is_err());
    }
    let new = |name: &str, x: f32| {
        let mut f = field(name, DesignKind::Text);
        f.widgets[0].rect = [x, 20.0, x + 100.0, 40.0];
        f
    };
    let scenarios: Vec<(&str, Vec<AuthorOp>)> = vec![
        ("add", vec![AuthorOp::Add(new("a", 10.0))]),
        (
            "add and move",
            vec![
                AuthorOp::Add(new("a", 10.0)),
                AuthorOp::Move {
                    name: "a".into(),
                    widget: 0,
                    page: 0,
                    rect: [200.0, 200.0, 300.0, 220.0],
                },
            ],
        ),
        (
            "add and remove",
            vec![
                AuthorOp::Add(new("a", 10.0)),
                AuthorOp::Remove { name: "a".into() },
            ],
        ),
        (
            "add two and reorder",
            vec![
                AuthorOp::Add(new("a", 10.0)),
                AuthorOp::Add(new("b", 130.0)),
                AuthorOp::Reorder {
                    page: 0,
                    order: ["b", "a"]
                        .map(|n| WidgetRef {
                            name: n.into(),
                            widget: 0,
                        })
                        .to_vec(),
                },
            ],
        ),
    ];
    for (what, ops) in scenarios {
        let mut a = Author::new(bytes.clone(), geo(0)).unwrap();
        a.apply(&ops).unwrap();
        let out = a.finish().unwrap();
        let page = revision(&out.bytes, bytes.len(), 4)
            .unwrap_or_else(|| panic!("{what}: the page was not written again"));
        for item in kit::DIRECT_ANNOTS {
            assert!(
                has(&page, item),
                "{what}: an annotation of the page was changed in the new revision"
            );
        }
        // Nothing was replaced by the character that stands for a byte that is not text.
        assert!(!has(&out.bytes[bytes.len()..], b"\xEF\xBF\xBD"), "{what}");
        // The identifier of the file is the one it had, byte for byte.
        let tail = &out.bytes[bytes.len()..];
        let trailer = find(tail, b"trailer\n").expect("a trailer");
        assert!(
            has(&tail[trailer..], &kit::DIRECT_ID[1..]),
            "{what}: the identifier of the file was changed"
        );
    }
}

// ---- documents made by hand ---------------------------------------------------------------------

/// Objects 1.. (`objects[i]` is object `i + 1`) in a file with a plain table of their places.
fn assemble(objects: &[Vec<u8>], trailer_extra: &str) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let table = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R {trailer_extra} >>\nstartxref\n{table}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

fn pdf(objects: &[&str]) -> Vec<u8> {
    let bodies: Vec<Vec<u8>> = objects.iter().map(|s| s.as_bytes().to_vec()).collect();
    assemble(&bodies, "")
}

const PAGE: &str = "/Type /Page /Parent 2 0 R /MediaBox [0 0 612 792]";

/// A one-page document with a form of one text field, whose widget has the box `rect`.
fn one_widget_pdf(rect: &str) -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        &format!("<< {PAGE} /Annots [4 0 R] >>"),
        &format!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (wide) /Rect {rect} /P 3 0 R /F 4 >>"
        ),
        "<< /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) >>",
    ])
}

#[test]
fn a_box_the_answer_cannot_carry_is_not_listed() {
    let mut a = Author::new(one_widget_pdf("[0 0 1000000000 20]"), geo(0)).unwrap();
    assert!(listed(&mut a, 0).is_empty(), "a box of a billion points");
    let mut a = Author::new(one_widget_pdf("[-30000000 0 10000000 20]"), geo(0)).unwrap();
    assert!(listed(&mut a, 0).is_empty());
    // A box on the page is listed, and so are the widgets of a document with several.
    let mut a = Author::new(one_widget_pdf("[0 0 612 20]"), geo(0)).unwrap();
    assert_eq!(listed(&mut a, 0).len(), 1);
    let mut a = Author::new(kit::forms_pdf(), geo(0)).unwrap();
    assert!(listed(&mut a, 0).len() >= 3);
}

// ---- the pages the writer finds are the pages PDFium finds ----------------------------------------

#[test]
fn a_page_tree_with_a_wrong_count_is_refused() {
    // A node that says it has 3 pages and lists one: PDFium goes by the counts, and its page 1 is
    // not the first page the lists name.
    let wrong = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [5 0 R 4 0 R] /Count 2 >>",
        "<< /Type /Page /Parent 5 0 R /MediaBox [0 0 612 792] >>",
        &format!("<< {PAGE} >>"),
        "<< /Type /Pages /Parent 2 0 R /Kids [3 0 R] /Count 3 >>",
    ]);
    match Author::new(wrong.clone(), geo(0)) {
        Err(AuthorError::Unsupported(m)) => {
            assert!(!m.contains("kids") && !m.contains("Count"), "{m}");
        }
        other => panic!("{:?}", other.err()),
    }
    assert!(Author::with_count(wrong, geo(0), 2).is_err());
    // The same tree with the right count is read, in the order the lists give.
    let right = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [5 0 R 4 0 R] /Count 2 >>",
        "<< /Type /Page /Parent 5 0 R /MediaBox [0 0 612 792] >>",
        &format!("<< {PAGE} >>"),
        "<< /Type /Pages /Parent 2 0 R /Kids [3 0 R] /Count 1 >>",
    ]);
    let a = Author::new(right.clone(), geo(0)).unwrap();
    assert_eq!(a.page_objs, vec![3, 4]);
    // PDFium says another number of pages: the tree is not the one it numbers.
    assert!(Author::with_count(right, geo(0), 3).is_err());
}

#[test]
fn a_page_that_the_tree_lists_twice_is_refused() {
    let twice = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 3 0 R] /Count 2 >>",
        &format!("<< {PAGE} >>"),
    ]);
    assert!(Author::new(twice, geo(0)).is_err());
}

// ---- objects that are one and the same ----------------------------------------------------------

fn two_pages(annots_one: &str, annots_two: &str, catalog_extra: &str, more: &[&str]) -> Vec<u8> {
    let mut objects = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>"),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_string(),
        format!("<< {PAGE} {annots_one} >>"),
        format!("<< {PAGE} {annots_two} >>"),
    ];
    objects.extend(more.iter().map(|s| (*s).to_string()));
    let refs: Vec<&str> = objects.iter().map(String::as_str).collect();
    pdf(&refs)
}

fn one_field(a: &mut Author, name: &str, page: u32) -> Result<(), AuthorError> {
    let mut f = field(name, DesignKind::Text);
    f.widgets[0].page = page;
    a.apply(&[AuthorOp::Add(f)])
}

#[test]
fn two_pages_that_share_a_list_of_annotations_are_not_written() {
    let bytes = two_pages("/Annots 5 0 R", "/Annots 5 0 R", "", &["[ ]"]);
    let mut a = Author::new(bytes.clone(), geo(0)).unwrap();
    // Reading is fine.
    assert!(listed(&mut a, 0).is_empty());
    // A field on either page would be on both.
    assert!(matches!(
        one_field(&mut a, "x", 0),
        Err(AuthorError::Unsupported(_))
    ));
    let mut b = Author::new(bytes, geo(0)).unwrap();
    assert!(matches!(
        one_field(&mut b, "x", 1),
        Err(AuthorError::Unsupported(_))
    ));
    // A list that only one page names is not shared.
    let mut c = Author::new(two_pages("/Annots 5 0 R", "", "", &["[ ]"]), geo(0)).unwrap();
    one_field(&mut c, "x", 0).unwrap();
    one_field(&mut c, "y", 1).unwrap();
    assert!(c.finish().is_ok());
}

#[test]
fn a_part_of_the_form_that_is_also_something_else_is_not_written() {
    // The list of fields is a page list of annotations.
    let bytes = two_pages(
        "/Annots 6 0 R",
        "",
        "/AcroForm 5 0 R",
        &["<< /Fields 6 0 R >>", "[ ]"],
    );
    let mut a = Author::new(bytes, geo(0)).unwrap();
    assert!(matches!(
        one_field(&mut a, "x", 0),
        Err(AuthorError::Unsupported(_))
    ));
    // The form is a page list of annotations.
    let bytes = two_pages("/Annots 5 0 R", "", "/AcroForm 5 0 R", &["[ ]"]);
    if let Ok(mut a) = Author::new(bytes, geo(0)) {
        assert!(one_field(&mut a, "x", 0).is_err());
    }
    // The catalog is a page list of annotations.
    let bytes = two_pages("/Annots 1 0 R", "", "", &[]);
    if let Ok(mut a) = Author::new(bytes, geo(0)) {
        assert!(one_field(&mut a, "x", 0).is_err());
    }
    // The form is the catalog.
    let bytes = two_pages("", "", "/AcroForm 1 0 R", &[]);
    assert!(Author::new(bytes, geo(0)).is_err());
    // The list of fields is a page.
    let bytes = two_pages("", "", "/AcroForm 5 0 R", &["<< /Fields 3 0 R >>"]);
    if let Ok(mut a) = Author::new(bytes, geo(0)) {
        assert!(one_field(&mut a, "x", 0).is_err());
    }
    // The resources of the form are the list of fields.
    let bytes = two_pages(
        "",
        "",
        "/AcroForm 5 0 R",
        &["<< /Fields 6 0 R /DR 6 0 R >>", "[ ]"],
    );
    if let Ok(mut a) = Author::new(bytes, geo(0)) {
        assert!(one_field(&mut a, "x", 0).is_err());
    }
    // A form that is its own is written.
    let bytes = two_pages("", "", "/AcroForm 5 0 R", &["<< /Fields [ ] >>"]);
    let mut a = Author::new(bytes, geo(0)).unwrap();
    one_field(&mut a, "x", 0).unwrap();
    a.finish().unwrap();
}

// ---- a check box that is in two places ----------------------------------------------------------

/// A check box with two widgets, one on each page, that have a word of their own (`A` and `B`). The
/// value of the field is on the field.
fn checkbox_in_two_places() -> Vec<u8> {
    let widget = |page: u32, word: &str, x: u32| {
        format!(
            "<< /Type /Annot /Subtype /Widget /Rect [{x} 10 {} 24] /P {page} 0 R /Parent 5 0 R /AS /Off /F 4 /AP << /N << /{word} 11 0 R /Off 12 0 R >> >> >>",
            x + 14
        )
    };
    two_pages(
        "/Annots [6 0 R]",
        "/Annots [7 0 R]",
        "/AcroForm 8 0 R",
        &[
            "<< /FT /Btn /T (agree) /V /Off /Kids [6 0 R 7 0 R] >>",
            &widget(3, "A", 10),
            &widget(4, "B", 40),
            "<< /Fields [5 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 9 0 R /ZaDb 10 0 R >> >> >>",
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
            "<< /Type /Font /Subtype /Type1 /BaseFont /ZapfDingbats >>",
            "<< /Type /XObject /Subtype /Form /BBox [0 0 14 14] >>",
            "<< /Type /XObject /Subtype /Form /BBox [0 0 14 14] >>",
        ],
    )
}

fn update(a: &mut Author, name: &str, props: DesignProps) {
    a.apply(&[AuthorOp::Update {
        name: name.into(),
        new_name: String::new(),
        props,
        exports: vec![],
    }])
    .unwrap();
}

#[test]
fn the_value_of_a_check_box_in_two_places_is_on_the_field() {
    let bytes = checkbox_in_two_places();
    let mut a = Author::new(bytes.clone(), geo(0)).unwrap();
    let shown = listed(&mut a, 0);
    assert_eq!(shown.len(), 1);
    assert!(shown[0].editable);
    assert_eq!(shown[0].props.export_value, "A");
    // Starting checked: the field is on, and so is each widget, with its own word.
    let mut props = shown[0].props.clone();
    props.default_value = "on".into();
    update(&mut a, "agree", props.clone());
    let out = a.finish().unwrap();
    let field = revision(&out.bytes, bytes.len(), 5).expect("the field was written");
    assert_eq!(get(&field, "V"), Some(&b"/A"[..]));
    assert_eq!(get(&field, "DV"), Some(&b"/A"[..]));
    let first = revision(&out.bytes, bytes.len(), 6).expect("a widget was written");
    let second = revision(&out.bytes, bytes.len(), 7).expect("a widget was written");
    assert_eq!(get(&first, "AS"), Some(&b"/A"[..]));
    // The value of the field names one word, so only the widget that has it is on.
    assert_eq!(get(&second, "AS"), Some(&b"/Off"[..]));
    for widget in [&first, &second] {
        assert_eq!(get(widget, "V"), None);
        assert_eq!(get(widget, "DV"), None);
    }
    // The words and the pictures of the widgets are the ones they had.
    assert!(has(&second, b"/B 11 0 R"));
    assert!(!has(&out.bytes[bytes.len()..], b"/Subtype /Form"));

    // A change that is not about the word or the start leaves the widgets alone.
    let mut a = Author::new(bytes.clone(), geo(0)).unwrap();
    let mut props = listed(&mut a, 0)[0].props.clone();
    props.tooltip = "Tick it".into();
    update(&mut a, "agree", props);
    let out = a.finish().unwrap();
    let field = revision(&out.bytes, bytes.len(), 5).unwrap();
    assert!(has(&field, b"/TU (Tick it)"));
    assert!(!revised(&out.bytes, bytes.len(), 6));
    assert!(!revised(&out.bytes, bytes.len(), 7));

    // A new word is on every widget, and on the field when it is on.
    let mut a = Author::new(bytes.clone(), geo(0)).unwrap();
    let mut props = listed(&mut a, 0)[0].props.clone();
    props.default_value = "on".into();
    update(&mut a, "agree", props.clone());
    props.export_value = "Z".into();
    update(&mut a, "agree", props);
    let out = a.finish().unwrap();
    let field = revision(&out.bytes, bytes.len(), 5).unwrap();
    assert_eq!(get(&field, "V"), Some(&b"/Z"[..]));
    for n in [6, 7] {
        let widget = revision(&out.bytes, bytes.len(), n).unwrap();
        assert_eq!(get(&widget, "AS"), Some(&b"/Z"[..]), "widget {n}");
        assert!(has(&widget, b"/Z "), "widget {n}");
    }

    // Turning it off again: the field is off, and the widgets are.
    let mut a = Author::new(bytes.clone(), geo(0)).unwrap();
    let mut props = listed(&mut a, 0)[0].props.clone();
    props.default_value = "on".into();
    update(&mut a, "agree", props.clone());
    props.default_value = String::new();
    update(&mut a, "agree", props);
    let out = a.finish().unwrap();
    let field = revision(&out.bytes, bytes.len(), 5).unwrap();
    assert_eq!(get(&field, "V"), Some(&b"/Off"[..]));
    assert_eq!(get(&field, "DV"), None);
}

#[test]
fn a_move_keeps_the_spelling_of_a_state_that_is_not_text() {
    // The state of the button is the bytes `Ja` and 0xF8, as a name.
    let bytes = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        &format!("<< {PAGE} /Annots [4 0 R] >>"),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (c) /Rect [10 10 24 24] /P 3 0 R /AS /Off /F 4 /V /Off /AP << /N << /Ja#F8 6 0 R /Off 7 0 R >> >> >>",
        "<< /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 8 0 R /ZaDb 8 0 R >> >> >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 14 14] >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 14 14] >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]);
    let mut a = Author::new(bytes.clone(), geo(0)).unwrap();
    a.apply(&[AuthorOp::Move {
        name: "c".into(),
        widget: 0,
        page: 0,
        rect: [100.0, 100.0, 120.0, 120.0],
    }])
    .unwrap();
    let out = a.finish().unwrap();
    let widget = revision(&out.bytes, bytes.len(), 4).unwrap();
    assert!(
        has(&widget, b"/Ja#F8 "),
        "{}",
        String::from_utf8_lossy(&widget)
    );
    assert!(!has(&widget, b"#EF#BF#BD"));
}

// ---- what is not ours stays as it is ------------------------------------------------------------

/// A form that another program made: its resources and its list of fonts are objects of their own,
/// its `Helv` is not the standard one, and it asks to have its pictures made again.
fn foreign_form() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        &format!("<< {PAGE} >>"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Symbol >>",
        "<< /Fields [ ] /DA (/Helv 10 Tf 0 g) /DR 6 0 R /NeedAppearances true >>",
        "<< /Font 7 0 R /Encoding << >> >>",
        "<< /Helv 4 0 R >>",
    ])
}

#[test]
fn the_resources_of_a_form_that_is_not_ours_are_kept_as_they_are() {
    let bytes = foreign_form();
    let mut a = Author::new(bytes.clone(), geo(0)).unwrap();
    one_field(&mut a, "x", 0).unwrap();
    let mut sign = field("sign", DesignKind::Signature);
    sign.widgets[0].rect = [10.0, 100.0, 210.0, 150.0];
    a.apply(&[AuthorOp::Add(sign)]).unwrap();
    let out = a.finish().unwrap();
    let update = &out.bytes[bytes.len()..];
    // The font the form has is not touched, and none is made in its place.
    assert!(!revised(&out.bytes, bytes.len(), 4));
    assert!(!has(update, b"/BaseFont /Helvetica"));
    // The list of fonts gets the one it lacks and keeps the one it has.
    let fonts = revision(&out.bytes, bytes.len(), 7).expect("the list of fonts");
    assert!(has(&fonts, b"/Helv 4 0 R"));
    assert!(has(&fonts, b"/ZaDb "));
    // The resources are not written again, and the form keeps its own words.
    assert!(!revised(&out.bytes, bytes.len(), 6));
    let form = revision(&out.bytes, bytes.len(), 5).expect("the form");
    assert_eq!(get(&form, "NeedAppearances"), Some(&b"true"[..]));
    assert_eq!(get(&form, "DA"), Some(&b"(/Helv 10 Tf 0 g)"[..]));
    // The picture of a place to sign uses the font of the form.
    assert!(has(update, b"/Font << /Helv 4 0 R >>"));
}

#[test]
fn a_form_with_both_fonts_is_not_given_new_ones() {
    let bytes = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        &format!("<< {PAGE} >>"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        "<< /Fields [ ] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 4 0 R /ZaDb 4 0 R >> >> >>",
    ]);
    let mut a = Author::new(bytes.clone(), geo(0)).unwrap();
    one_field(&mut a, "x", 0).unwrap();
    let out = a.finish().unwrap();
    let update = &out.bytes[bytes.len()..];
    assert!(!has(update, b"/BaseFont"), "a font was made again");
}

#[test]
fn a_size_and_a_limit_that_were_not_changed_are_kept() {
    let bytes = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        &format!("<< {PAGE} /Annots [4 0 R] >>"),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (big) /Rect [10 10 200 30] /P 3 0 R /F 4 /DA (/Helv 200 Tf 0 g) /MaxLen 40000 >>",
        "<< /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 6 0 R /ZaDb 6 0 R >> >> >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]);
    let mut a = Author::new(bytes.clone(), geo(0)).unwrap();
    let shown = listed(&mut a, 0);
    assert!(shown[0].editable);
    assert_eq!(shown[0].props.font_size, 200.0);
    assert_eq!(shown[0].props.max_len, 40000);
    let mut props = shown[0].props.clone();
    props.tooltip = "A tooltip".into();
    update(&mut a, "big", props.clone());
    let out = a.finish().unwrap();
    let field = revision(&out.bytes, bytes.len(), 4).unwrap();
    assert!(has(&field, b"/Helv 200 Tf"));
    assert_eq!(get(&field, "MaxLen"), Some(&b"40000"[..]));
    // A size the designer does not write is refused when it is asked for.
    let mut a = Author::new(bytes.clone(), geo(0)).unwrap();
    props.font_size = 300.0;
    assert!(refusal(a.apply(&[AuthorOp::Update {
        name: "big".into(),
        new_name: String::new(),
        props: props.clone(),
        exports: vec![],
    }]))
    .contains("size"));
    props.font_size = 12.0;
    props.max_len = 100;
    update(&mut a, "big", props);
    let out = a.finish().unwrap();
    let field = revision(&out.bytes, bytes.len(), 4).unwrap();
    assert!(has(&field, b"/Helv 12 Tf"));
    assert_eq!(get(&field, "MaxLen"), Some(&b"100"[..]));
}

#[test]
fn a_radio_group_can_be_renamed_without_saying_which_button_is_on_again() {
    let mut a = Author::new(kit::simple_pdf(1), geo(0)).unwrap();
    let radio = DesignField {
        name: "size".into(),
        kind: DesignKind::Radio,
        props: DesignProps {
            default_value: "M".into(),
            ..DesignProps::default()
        },
        widgets: ["S", "M"]
            .iter()
            .enumerate()
            .map(|(i, e)| DesignWidget {
                page: 0,
                rect: [10.0 + 30.0 * i as f32, 10.0, 24.0 + 30.0 * i as f32, 24.0],
                export: (*e).into(),
            })
            .collect(),
    };
    a.apply(&[AuthorOp::Add(radio)]).unwrap();
    let props = listed(&mut a, 0)[0].props.clone();
    assert_eq!(props.default_value, "M");
    // The default is the word it had; the words are new. The button that was on stays on.
    a.apply(&[AuthorOp::Update {
        name: "size".into(),
        new_name: String::new(),
        props,
        exports: vec!["Small".into(), "Medium".into()],
    }])
    .unwrap();
    let items = listed(&mut a, 0);
    assert_eq!(items[0].export, "Small");
    assert_eq!(items[1].export, "Medium");
    assert_eq!(items[0].props.default_value, "Medium");
}

// ---- the bytes that come out ----------------------------------------------------------------------

#[test]
fn the_buffer_that_comes_out_has_no_room_the_document_will_not_use() {
    // A document of a few megabytes, with exactly the room for its own bytes.
    let big = kit::padded_pdf(3 << 20).to_vec();
    assert_eq!(big.capacity(), big.len());
    let mut a = Author::new(big.clone(), geo(0)).unwrap();
    add(&mut a, "t", DesignKind::Text);
    let out = a.finish().unwrap();
    assert!(out.bytes.len() > big.len());
    let spare = out.bytes.capacity() - out.bytes.len();
    assert!(spare < 16 * 1024, "{spare} bytes of room");
}

// ---- a word another program chose is not asked about again ----------------------------------------

/// A check box from another program whose word has an accent: the on-state is `S` and the bytes of
/// the letter i with an acute accent, as a name.
fn accented_checkbox() -> Vec<u8> {
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        &format!("<< {PAGE} /Annots [4 0 R] >>"),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (si) /Rect [10 10 24 24] /P 3 0 R /AS /Off /F 4 /V /Off /AP << /N << /S#C3#AD 6 0 R /Off 7 0 R >> >> >>",
        "<< /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 8 0 R /ZaDb 8 0 R >> >> >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 14 14] >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 14 14] >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ])
}

/// A radio group from another program, one of whose words has the letter o with a stroke.
fn accented_radio() -> Vec<u8> {
    let kid = |word: &str, x: u32| {
        format!(
            "<< /Type /Annot /Subtype /Widget /Rect [{x} 10 {} 24] /P 3 0 R /Parent 4 0 R /AS /Off /F 4 /AP << /N << /{word} 6 0 R /Off 7 0 R >> >> >>",
            x + 14
        )
    };
    pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        &format!("<< {PAGE} /Annots [8 0 R 9 0 R] >>"),
        "<< /FT /Btn /Ff 49152 /T (grp) /V /Off /Kids [8 0 R 9 0 R] >>",
        "<< /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 10 0 R /ZaDb 10 0 R >> >> >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 14 14] >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 14 14] >>",
        &kid("F#C3#B8rste", 10),
        &kid("Andre", 40),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ])
}

#[test]
fn a_check_box_whose_word_has_an_accent_can_be_changed_without_changing_the_word() {
    let bytes = accented_checkbox();
    let mut a = Author::new(bytes.clone(), geo(0)).unwrap();
    let shown = listed(&mut a, 0);
    assert!(shown[0].editable);
    assert_eq!(shown[0].props.export_value, "S\u{ed}");
    // Only the tooltip.
    let mut props = shown[0].props.clone();
    props.tooltip = "Yes or no".into();
    update(&mut a, "si", props.clone());
    // A rename and Required.
    props.required = true;
    a.apply(&[AuthorOp::Update {
        name: "si".into(),
        new_name: "renamed".into(),
        props: props.clone(),
        exports: vec![],
    }])
    .unwrap();
    // Starting checked keeps the word.
    props.default_value = "on".into();
    a.apply(&[AuthorOp::Update {
        name: "renamed".into(),
        new_name: String::new(),
        props: props.clone(),
        exports: vec![],
    }])
    .unwrap();
    let out = a.finish().unwrap();
    let field = revision(&out.bytes, bytes.len(), 4).unwrap();
    assert_eq!(get(&field, "V"), Some(&b"/S#C3#AD"[..]));
    // A new word with an accent is still refused.
    let mut a = Author::new(bytes, geo(0)).unwrap();
    let mut props = listed(&mut a, 0)[0].props.clone();
    props.export_value = "J\u{e5}".into();
    assert!(refusal(a.apply(&[AuthorOp::Update {
        name: "si".into(),
        new_name: String::new(),
        props,
        exports: vec![],
    }]))
    .contains("cannot be used"));
}

#[test]
fn a_radio_group_whose_word_has_an_accent_can_be_changed_without_changing_the_words() {
    let bytes = accented_radio();
    let mut a = Author::new(bytes.clone(), geo(0)).unwrap();
    let shown = listed(&mut a, 0);
    assert_eq!(shown.len(), 2);
    assert!(shown[0].editable);
    assert_eq!(shown[0].export, "F\u{f8}rste");
    let props = shown[0].props.clone();
    // The tooltip, with the words not sent, and with the words as they are.
    let mut with_tip = props.clone();
    with_tip.tooltip = "Pick one".into();
    update(&mut a, "grp", with_tip.clone());
    a.apply(&[AuthorOp::Update {
        name: "grp".into(),
        new_name: "renamed".into(),
        props: with_tip.clone(),
        exports: vec!["F\u{f8}rste".into(), "Andre".into()],
    }])
    .unwrap();
    // Another button gets a new word: that one is asked about, and may not have an accent.
    let refused_word = refusal(a.apply(&[AuthorOp::Update {
        name: "renamed".into(),
        new_name: String::new(),
        props: with_tip.clone(),
        exports: vec!["F\u{f8}rste".into(), "Andr\u{e9}".into()],
    }]));
    assert!(refused_word.contains("cannot be used"), "{refused_word}");
    // The words still have to differ.
    let twice = refusal(a.apply(&[AuthorOp::Update {
        name: "renamed".into(),
        new_name: String::new(),
        props: with_tip,
        exports: vec!["Andre".into(), "Andre".into()],
    }]));
    assert!(twice.contains("its own"), "{twice}");
    let out = a.finish().unwrap();
    assert!(has(
        &revision(&out.bytes, bytes.len(), 4).unwrap(),
        b"/T (renamed)"
    ));
}

#[test]
fn a_widget_that_is_also_the_catalog_is_not_written() {
    // The one widget of the field is object 1, the catalog: a change to it would be written over
    // when the catalog is written with the form.
    let bytes = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        &format!("<< {PAGE} >>"),
        "<< /FT /Tx /T (f) /Kids [1 0 R] >>",
        "<< /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 6 0 R /ZaDb 6 0 R >> >> >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]);
    let mut a = Author::new(bytes, geo(0)).unwrap();
    let moved = a.apply(&[AuthorOp::Move {
        name: "f".into(),
        widget: 0,
        page: 0,
        rect: [10.0, 10.0, 100.0, 30.0],
    }]);
    assert!(
        matches!(moved, Err(AuthorError::Unsupported(_))),
        "{moved:?}"
    );
}
