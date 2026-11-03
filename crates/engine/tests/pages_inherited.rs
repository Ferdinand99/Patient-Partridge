//! Page tools on documents whose page boxes and form widgets come from, or are shared through, the
//! page tree: a media box or a crop box that sits on a node above the pages, and one widget that
//! two pages list.
//!
//! PDFium reads some boxes of the page's own dictionary only, so a page that has none of its own
//! shows a size that is not the one it is drawn at. The tools have to work with the boxes a page
//! is shown with.
//!
//! These tests run the engine in this process, without the sandbox.

mod common;

use common::*;
use partridge_protocol::{
    FormField, PageSize, PagesRequest, PagesResponse, RequestBody, ResponseBody,
};
use partridge_testkit::{self as kit, contains, hex_form};

type Eng = partridge_engine::Engine;

fn pages_op(e: &mut Eng, op: PagesRequest) -> PagesResponse {
    match ask(e, RequestBody::Pages(op)) {
        ResponseBody::Pages(r) => r,
        other => panic!("{other:?}"),
    }
}

fn whole(sizes: &[PageSize]) -> Vec<(u32, u32)> {
    sizes
        .iter()
        .map(|p| (p.width.round() as u32, p.height.round() as u32))
        .collect()
}

fn changed_sizes(r: PagesResponse) -> Vec<(u32, u32)> {
    match r {
        PagesResponse::Changed { pages, .. } => whole(&pages),
        other => panic!("{other:?}"),
    }
}

fn fields(e: &mut Eng, doc: u32, page: u32) -> Vec<FormField> {
    match ask(e, RequestBody::ListFormFields { doc, page }) {
        ResponseBody::FormFields { fields } => fields,
        other => panic!("{other:?}"),
    }
}

fn sizes_of(e: &mut Eng, bytes: &[u8]) -> Vec<(u32, u32)> {
    open_sized(e, bytes, None).unwrap().1
}

// ---- crop and take a crop back on a page whose boxes come from the tree ---------------------------

#[test]
fn a_crop_and_the_undo_of_it_use_the_media_box_the_page_inherits() {
    let mut guard = engine();
    let e = &mut guard.0;
    // 400 by 1000 points, taller than the Letter page a reader falls back to.
    let (doc, start) = open_sized(e, &kit::inherited_media_box_pdf(), None).unwrap();
    assert_eq!(start, [(400, 1000)]);
    let PagesResponse::Cropped {
        pages,
        before,
        after,
    } = pages_op(
        e,
        PagesRequest::Crop {
            doc,
            pages: vec![0],
            margins: [10.0; 4],
        },
    )
    else {
        panic!("no crop answer");
    };
    // The box the page was shown with, and the box it is shown with now.
    assert_eq!(before, [[0.0, 0.0, 400.0, 1000.0]]);
    assert_eq!(after, [[10.0, 10.0, 390.0, 990.0]]);
    assert_eq!(whole(&pages), [(380, 980)]);

    // Taking the crop back gives the page its old size, and not a Letter page cut out of it.
    let back = changed_sizes(pages_op(
        e,
        PagesRequest::SetBoxes {
            doc,
            pages: vec![0],
            boxes: before.clone(),
        },
    ));
    assert_eq!(back, [(400, 1000)]);
    // The same in the file that is saved, and the top of the page is still on show.
    let saved = save_all(e, doc);
    let (again, reopened) = open_sized(e, &saved, None).unwrap();
    assert_eq!(reopened, [(400, 1000)]);
    let (text, rects) = page_text(e, again, 0);
    let top = rects[text.find("TOP-OF-PAGE").unwrap()];
    assert!(
        top[1] < 100.0,
        "the top of the page is still on show: {top:?}"
    );
}

#[test]
fn a_crop_uses_the_crop_box_the_page_inherits() {
    let mut guard = engine();
    let e = &mut guard.0;
    // The node shows 200 by 100 points of a 300 by 200 page.
    let (doc, start) = open_sized(e, &kit::inherited_crop_box_pdf(), None).unwrap();
    assert_eq!(start, [(200, 100)]);
    let PagesResponse::Cropped {
        pages,
        before,
        after,
    } = pages_op(
        e,
        PagesRequest::Crop {
            doc,
            pages: vec![0],
            margins: [10.0; 4],
        },
    )
    else {
        panic!("no crop answer");
    };
    assert_eq!(before, [[50.0, 50.0, 250.0, 150.0]]);
    assert_eq!(after, [[60.0, 60.0, 240.0, 140.0]]);
    assert_eq!(whole(&pages), [(180, 80)]);
    let back = changed_sizes(pages_op(
        e,
        PagesRequest::SetBoxes {
            doc,
            pages: vec![0],
            boxes: before,
        },
    ));
    assert_eq!(back, [(200, 100)]);
}

// ---- a new page is the size that was asked for -----------------------------------------------------

#[test]
fn a_blank_page_is_the_size_asked_for_under_a_node_with_a_crop_box() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, start) = open_sized(e, &kit::inherited_crop_box_pdf(), None).unwrap();
    assert_eq!(start, [(200, 100)]);
    // The size of the other pages, and a size that the node's crop box would cut down.
    for (width, height) in [(200.0, 100.0), (260.0, 180.0), (120.0, 90.0)] {
        let sizes = changed_sizes(pages_op(
            e,
            PagesRequest::InsertBlank {
                doc,
                at: 0,
                width,
                height,
            },
        ));
        assert_eq!(
            sizes[0],
            (width as u32, height as u32),
            "a blank page of {width} by {height}"
        );
        // The page that was there is as it was.
        assert_eq!(*sizes.last().unwrap(), (200, 100));
    }
    // Saved and read again, every new page has its size.
    let saved = save_all(e, doc);
    assert_eq!(
        sizes_of(e, &saved),
        [(120, 90), (260, 180), (200, 100), (200, 100)]
    );
}

// ---- deleting a page leaves what a page that stays shows ------------------------------------------

/// Where the field of the shared widget is drawn on a page of 200 by 100 points drawn at one pixel
/// to the point, counted from the top.
const FIELD_BOX: [f32; 4] = [10.0, 60.0, 190.0, 90.0];

#[test]
fn deleting_a_page_leaves_the_value_and_look_of_a_widget_a_page_that_stays_also_lists() {
    let mut guard = engine();
    let e = &mut guard.0;
    let (doc, _) = open_sized(e, &kit::shared_widget_pdf(), None).unwrap();
    let shown = |e: &mut Eng| Shot::take(e, doc, 0, 200, 100).inked(FIELD_BOX);
    let ink_before = shown(e);
    assert!(ink_before > 0, "the field is drawn on the first page");
    assert_eq!(fields(e, doc, 0)[0].value, kit::SHARED_WIDGET_VALUE);

    changed_sizes(pages_op(
        e,
        PagesRequest::Delete {
            doc,
            pages: vec![1],
        },
    ));
    assert_eq!(fields(e, doc, 0)[0].value, kit::SHARED_WIDGET_VALUE);
    assert_eq!(shown(e), ink_before, "the field is drawn as it was");
    let saved = save_all(e, doc);
    assert!(contains(&saved, kit::SHARED_WIDGET_VALUE), "the value");
    assert!(
        contains(&saved, &hex_form(kit::SHARED_WIDGET_LOOK)),
        "the look of the value"
    );
    let again = open(e, &saved, None).unwrap();
    assert_eq!(fields(e, again, 0)[0].value, kit::SHARED_WIDGET_VALUE);
}

#[test]
fn deleting_a_page_leaves_the_value_of_a_field_whose_other_widget_stays() {
    let mut guard = engine();
    let e = &mut guard.0;
    let doc = open(e, &kit::shared_field_pdf(), None).unwrap();
    assert_eq!(fields(e, doc, 0)[0].value, "Alice");
    changed_sizes(pages_op(
        e,
        PagesRequest::Delete {
            doc,
            pages: vec![1],
        },
    ));
    assert_eq!(fields(e, doc, 0)[0].value, "Alice");
    assert!(contains(&save_all(e, doc), "Alice"));
}
