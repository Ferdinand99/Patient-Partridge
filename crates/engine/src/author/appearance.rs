//! The pictures the designer writes itself: a check box and a radio button in each of their
//! states, and an empty place to sign.
//!
//! PDFium makes the picture of a text field and of a list when it draws the page, but it does not
//! know the words a designer chooses for the states of a button, and it does not draw a place to
//! sign. Each picture is a form object with its own box. The box is the size of the field as it is
//! drawn, and the matrix turns it to the page's own coordinates, so a page that is turned shows
//! the picture upright.

use super::syntax::num;

const BLACK: &str = "0 0 0";

/// The matrix that carries a picture drawn upright onto a page turned by `turns` quarter turns.
pub fn matrix(turns: u8) -> Option<&'static str> {
    match turns % 4 {
        0 => None,
        1 => Some("[0 1 -1 0 0 0]"),
        2 => Some("[-1 0 0 -1 0 0]"),
        _ => Some("[0 -1 1 0 0 0]"),
    }
}

/// The text of a form object.
pub fn form_object(width: f32, height: f32, turns: u8, resources: &str, content: &str) -> String {
    let m = matrix(turns)
        .map(|m| format!(" /Matrix {m}"))
        .unwrap_or_default();
    format!(
        "<< /Type /XObject /Subtype /Form /BBox [0 0 {} {}]{m} /Resources << {resources} >> /Length {} >>\nstream\n{content}\nendstream",
        num(width),
        num(height),
        content.len()
    )
}

/// A circle as four curves.
fn circle(cx: f32, cy: f32, r: f32) -> String {
    let k = 0.552_284_8 * r;
    format!(
        "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c",
        num(cx + r),
        num(cy),
        num(cx + r),
        num(cy + k),
        num(cx + k),
        num(cy + r),
        num(cx),
        num(cy + r),
        num(cx - k),
        num(cy + r),
        num(cx - r),
        num(cy + k),
        num(cx - r),
        num(cy),
        num(cx - r),
        num(cy - k),
        num(cx - k),
        num(cy - r),
        num(cx),
        num(cy - r),
        num(cx + k),
        num(cy - r),
        num(cx + r),
        num(cy - k),
        num(cx + r),
        num(cy),
    )
}

/// The frame of a check box.
fn frame(w: f32, h: f32) -> String {
    format!(
        "q {BLACK} RG 1 w 0.5 0.5 {} {} re S Q\n",
        num((w - 1.0).max(0.0)),
        num((h - 1.0).max(0.0))
    )
}

pub fn check_off(w: f32, h: f32) -> String {
    frame(w, h)
}

pub fn check_on(w: f32, h: f32) -> String {
    let side = w.min(h);
    let weight = (side * 0.12).max(1.0);
    format!(
        "{}q {BLACK} RG {} w 1 J 1 j {} {} m {} {} l {} {} l S Q",
        frame(w, h),
        num(weight),
        num(w * 0.22),
        num(h * 0.52),
        num(w * 0.42),
        num(h * 0.26),
        num(w * 0.80),
        num(h * 0.76),
    )
}

pub fn radio_off(w: f32, h: f32) -> String {
    let r = (w.min(h) / 2.0 - 0.5).max(0.5);
    format!("q {BLACK} RG 1 w {} S Q", circle(w / 2.0, h / 2.0, r))
}

pub fn radio_on(w: f32, h: f32) -> String {
    let r = (w.min(h) / 2.0 - 0.5).max(0.5);
    format!(
        "{}\nq {BLACK} rg {} f Q",
        radio_off(w, h),
        circle(w / 2.0, h / 2.0, r * 0.5)
    )
}

/// An empty place to sign: a dashed box with a hint.
pub fn signature(w: f32, h: f32) -> String {
    format!(
        "q 0.95 0.95 0.95 rg 0 0 {} {} re f Q\nq 0.4 0.4 0.4 RG 0.75 w [3 2] 0 d 0.5 0.5 {} {} re S Q\nBT /Helv 9 Tf 0.35 g 5 {} Td (Sign here) Tj ET",
        num(w),
        num(h),
        num((w - 1.0).max(0.0)),
        num((h - 1.0).max(0.0)),
        num((h / 2.0 - 3.0).max(1.0)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_a_form_object_of_the_right_length() {
        let content = check_on(14.0, 14.0);
        let object = form_object(14.0, 14.0, 0, "", &content);
        assert!(object.contains(&format!("/Length {}", content.len())));
        assert!(object.contains("/BBox [0 0 14 14]"));
        assert!(!object.contains("/Matrix"));
        assert!(form_object(14.0, 20.0, 1, "", &content).contains("/Matrix [0 1 -1 0 0 0]"));
        assert!(form_object(14.0, 20.0, 3, "", &content).contains("/Matrix [0 -1 1 0 0 0]"));
    }

    #[test]
    fn states_differ() {
        assert_ne!(check_on(14.0, 14.0), check_off(14.0, 14.0));
        assert_ne!(radio_on(14.0, 14.0), radio_off(14.0, 14.0));
        assert!(radio_on(14.0, 14.0).contains(" f Q"));
        assert!(signature(200.0, 50.0).contains("Sign here"));
    }

    #[test]
    fn tiny_boxes_do_not_make_negative_numbers() {
        for s in [check_on(4.0, 4.0), radio_on(4.0, 4.0), signature(4.0, 4.0)] {
            assert!(!s.contains("NaN") && !s.contains("inf"));
        }
        assert!(!check_off(0.5, 0.5).contains(" -"));
    }
}
