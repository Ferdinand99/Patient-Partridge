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
