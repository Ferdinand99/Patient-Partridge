//! The messages of the form designer for the fuzz targets: valid messages built from fuzzer bytes,
//! and the caps as assertions. `arb_ops` is also what the target of the writer feeds to it.

use arbitrary::{Result, Unstructured};
use partridge_protocol::{
    AuthorOp, AuthorRequest, AuthorResponse, DesignField, DesignKind, DesignProps, DesignWidget,
    DesignedWidget, WidgetRef, MAX_AUTHOR_NAMES, MAX_AUTHOR_OPS, MAX_AUTHOR_WIDGETS, MAX_BOX_VALUE,
    MAX_EXPORT_VALUE, MAX_FORM_FIELDS_PER_PAGE, MAX_FORM_NAME, MAX_FORM_OPTIONS,
    MAX_FORM_OPTION_LABEL, MAX_FORM_TOOLTIP, MAX_FORM_VALUE, MAX_LISTED_WIDGETS,
};

use crate::{chance, count_up_to, string_up_to};

const KINDS: [DesignKind; 6] = [
    DesignKind::Text,
    DesignKind::Checkbox,
    DesignKind::Radio,
    DesignKind::Combo,
    DesignKind::List,
    DesignKind::Signature,
];

/// A name: often one of a few, so that the changes of a run meet each other (a name that is taken,
/// a field that is changed after it was made); now and then something the rules refuse.
pub fn arb_name(u: &mut Unstructured) -> Result<String> {
    Ok(match u.int_in_range(0..=11u8)? {
        0 => "a".into(),
        1 => "b".into(),
        2 => "c".into(),
        3 => "a.b".into(),
        4 => String::new(),
        5 => " ".into(),
        6 => "person".into(),
        7 => "\u{e6}\u{f8}\u{e5}".into(),
        _ => string_up_to(u, 40)?,
    })
}

/// A number the box of a widget may hold: finite and inside the bound the decoder keeps.
fn coordinate(u: &mut Unstructured, page_side: f32) -> Result<f32> {
    if chance(u, 12)? {
        return Ok(if u.arbitrary()? {
            MAX_BOX_VALUE
        } else {
            -MAX_BOX_VALUE
        });
    }
    Ok(u.int_in_range(-40..=(page_side as i32 + 40))? as f32)
}

/// A box that is often a usable one inside a Letter page, and now and then not.
pub fn arb_rect(u: &mut Unstructured) -> Result<[f32; 4]> {
    if chance(u, 4)? {
        return Ok([
            coordinate(u, 612.0)?,
            coordinate(u, 792.0)?,
            coordinate(u, 612.0)?,
            coordinate(u, 792.0)?,
        ]);
    }
    let left = u.int_in_range(0..=500)? as f32;
    let top = u.int_in_range(0..=700)? as f32;
    let w = u.int_in_range(4..=100)? as f32;
    let h = u.int_in_range(4..=80)? as f32;
    Ok([left, top, left + w, top + h])
}

pub fn arb_props(u: &mut Unstructured) -> Result<DesignProps> {
    let choices_count = count_up_to(u, MAX_FORM_OPTIONS.min(6))?;
    let mut choices = Vec::new();
    for _ in 0..choices_count {
        choices.push(string_up_to(u, MAX_FORM_OPTION_LABEL)?);
    }
    Ok(DesignProps {
        tooltip: string_up_to(u, MAX_FORM_TOOLTIP)?,
        required: u.arbitrary()?,
        read_only: u.arbitrary()?,
        multiline: u.arbitrary()?,
        password: u.arbitrary()?,
        comb: u.arbitrary()?,
        max_len: if chance(u, 3)? {
            u.int_in_range(0..=40)?
        } else {
            0
        },
        editable: u.arbitrary()?,
        multiselect: u.arbitrary()?,
        font_size: if chance(u, 2)? {
            u.int_in_range(0..=120)? as f32
        } else {
            0.0
        },
        default_value: if chance(u, 2)? {
            // Often one of the choices.
            match choices.first() {
                Some(c) if u.arbitrary()? => c.clone(),
                _ => string_up_to(u, 40)?,
            }
        } else {
            String::new()
        },
        choices,
        export_value: string_up_to(u, MAX_EXPORT_VALUE.min(12))?,
    })
}

fn arb_widget(u: &mut Unstructured) -> Result<DesignWidget> {
    Ok(DesignWidget {
        // Mostly the first pages, now and then one that is not there.
        page: if chance(u, 8)? {
            u.arbitrary()?
        } else {
            u.int_in_range(0..=3)?
        },
        rect: arb_rect(u)?,
        export: string_up_to(u, MAX_EXPORT_VALUE.min(8))?,
    })
}

pub fn arb_field(u: &mut Unstructured) -> Result<DesignField> {
    let n = if chance(u, 4)? {
        count_up_to(u, MAX_AUTHOR_WIDGETS.min(5))?
    } else {
        1
    };
    let mut widgets = Vec::new();
    for _ in 0..n {
        widgets.push(arb_widget(u)?);
    }
    Ok(DesignField {
        name: arb_name(u)?,
        kind: KINDS[u.int_in_range(0..=KINDS.len() - 1)?],
        props: arb_props(u)?,
        widgets,
    })
}

fn arb_ref(u: &mut Unstructured) -> Result<WidgetRef> {
    Ok(WidgetRef {
        name: arb_name(u)?,
        widget: u.int_in_range(0..=3)?,
    })
}

pub fn arb_op(u: &mut Unstructured) -> Result<AuthorOp> {
    Ok(match u.int_in_range(0..=6u8)? {
        0 | 1 => AuthorOp::Add(arb_field(u)?),
        2 => AuthorOp::Remove { name: arb_name(u)? },
        3 => AuthorOp::RemoveWidget {
            name: arb_name(u)?,
            widget: u.int_in_range(0..=3)?,
        },
        4 => AuthorOp::AddWidget {
            name: arb_name(u)?,
            widget: arb_widget(u)?,
        },
        5 => AuthorOp::Move {
            name: arb_name(u)?,
            widget: u.int_in_range(0..=3)?,
            page: u.int_in_range(0..=3)?,
            rect: arb_rect(u)?,
        },
        _ => match u.int_in_range(0..=1u8)? {
            0 => {
                let n = count_up_to(u, MAX_AUTHOR_WIDGETS.min(5))?;
                let mut exports = Vec::new();
                for _ in 0..n {
                    exports.push(string_up_to(u, 8)?);
                }
                AuthorOp::Update {
                    name: arb_name(u)?,
                    new_name: arb_name(u)?,
                    props: arb_props(u)?,
                    exports,
                }
            }
            _ => {
                let n = count_up_to(u, MAX_FORM_FIELDS_PER_PAGE.min(5))?;
                let mut order = Vec::new();
                for _ in 0..n {
                    order.push(arb_ref(u)?);
                }
                AuthorOp::Reorder {
                    page: u.int_in_range(0..=3)?,
                    order,
                }
            }
        },
    })
}

/// A batch of changes, at most as many as one request holds.
pub fn arb_ops(u: &mut Unstructured, most: usize) -> Result<Vec<AuthorOp>> {
    let n = count_up_to(u, most.min(MAX_AUTHOR_OPS))?;
    (0..n).map(|_| arb_op(u)).collect()
}

/// A request of the designer that the encoder writes without cutting anything.
pub fn arb_author_request(u: &mut Unstructured) -> Result<AuthorRequest> {
    let doc: u32 = u.arbitrary()?;
    Ok(match u.int_in_range(0..=2u8)? {
        0 => AuthorRequest::Check { doc },
        1 => AuthorRequest::List {
            doc,
            page: u.arbitrary()?,
        },
        _ => AuthorRequest::Apply {
            doc,
            ops: arb_ops(u, MAX_AUTHOR_OPS)?,
        },
    })
}

fn arb_designed(u: &mut Unstructured, heavy: bool) -> Result<DesignedWidget> {
    let mut props = arb_props(u)?;
    if !heavy {
        props.choices.truncate(1);
        props.tooltip.truncate(0);
        props.default_value.truncate(0);
    }
    Ok(DesignedWidget {
        name: string_up_to(u, if heavy { MAX_FORM_NAME } else { 12 })?,
        kind: KINDS[u.int_in_range(0..=KINDS.len() - 1)?],
        widget: u.arbitrary()?,
        widgets: u.arbitrary()?,
        rect: arb_rect(u)?,
        export: string_up_to(u, MAX_EXPORT_VALUE)?,
        editable: u.arbitrary()?,
        props,
    })
}

/// An answer of the designer that the encoder writes without cutting anything.
pub fn arb_author_response(u: &mut Unstructured) -> Result<AuthorResponse> {
    Ok(if u.arbitrary()? {
        let n = count_up_to(u, MAX_AUTHOR_NAMES)?;
        let mut names = Vec::new();
        for _ in 0..n {
            names.push(string_up_to(u, if n > 8 { 12 } else { MAX_FORM_NAME })?);
        }
        AuthorResponse::Ready { names }
    } else {
        let n = count_up_to(u, MAX_LISTED_WIDGETS)?;
        let mut items = Vec::new();
        for _ in 0..n {
            items.push(arb_designed(u, n <= 4)?);
        }
        AuthorResponse::Widgets { items }
    })
}

// ---- the caps as assertions --------------------------------------------------------------------

fn check_props(p: &DesignProps) {
    assert!(p.tooltip.len() <= MAX_FORM_TOOLTIP);
    assert!(p.default_value.len() <= MAX_FORM_VALUE);
    assert!(p.export_value.len() <= MAX_EXPORT_VALUE);
    assert!(p.choices.len() <= MAX_FORM_OPTIONS);
    assert!(p.choices.iter().all(|c| c.len() <= MAX_FORM_OPTION_LABEL));
    assert!(p.font_size.is_finite() && p.font_size >= 0.0);
}

fn check_rect(r: &[f32; 4]) {
    assert!(
        r.iter().all(|v| v.is_finite() && v.abs() <= MAX_BOX_VALUE),
        "box {r:?}"
    );
}

/// Panics when a request of the designer is outside the caps the protocol promises.
pub fn check_author_request_caps(request: &AuthorRequest) {
    let AuthorRequest::Apply { ops, .. } = request else {
        return;
    };
    assert!(ops.len() <= MAX_AUTHOR_OPS, "{} changes", ops.len());
    for op in ops {
        match op {
            AuthorOp::Add(f) => {
                assert!(f.name.len() <= MAX_FORM_NAME);
                assert!(f.widgets.len() <= MAX_AUTHOR_WIDGETS);
                check_props(&f.props);
                f.widgets.iter().for_each(|w| check_rect(&w.rect));
            }
            AuthorOp::Remove { name } | AuthorOp::RemoveWidget { name, .. } => {
                assert!(name.len() <= MAX_FORM_NAME)
            }
            AuthorOp::AddWidget { name, widget } => {
                assert!(name.len() <= MAX_FORM_NAME);
                check_rect(&widget.rect);
            }
            AuthorOp::Move { name, rect, .. } => {
                assert!(name.len() <= MAX_FORM_NAME);
                check_rect(rect);
            }
            AuthorOp::Update {
                name,
                new_name,
                props,
                exports,
            } => {
                assert!(name.len() <= MAX_FORM_NAME && new_name.len() <= MAX_FORM_NAME);
                check_props(props);
                assert!(exports.len() <= MAX_AUTHOR_WIDGETS);
            }
            AuthorOp::Reorder { order, .. } => {
                assert!(order.len() <= MAX_FORM_FIELDS_PER_PAGE)
            }
        }
    }
}

/// Panics when an answer of the designer is outside the caps the protocol promises.
pub fn check_author_response_caps(response: &AuthorResponse) {
    match response {
        AuthorResponse::Ready { names } => {
            assert!(names.len() <= MAX_AUTHOR_NAMES);
            assert!(names.iter().all(|n| n.len() <= MAX_FORM_NAME));
        }
        AuthorResponse::Widgets { items } => {
            assert!(items.len() <= MAX_LISTED_WIDGETS);
            for w in items {
                assert!(w.name.len() <= MAX_FORM_NAME);
                check_props(&w.props);
                check_rect(&w.rect);
            }
        }
    }
}
