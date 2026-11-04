//! Between the page as it is drawn and the page's own coordinates.
//!
//! The window works on the page the way the reader sees it: points from the top left corner, with
//! the turn of the page applied. A field is written in the page's own coordinates (PDF user space),
//! and a page that is turned needs the turn written into the field as well, so the text of the
//! field reads upright on the screen.

/// How a page is shown: its turn in quarter turns clockwise, and the part of it that is visible as
/// `[left, bottom, right, top]` in the page's own coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageGeo {
    pub turns: u8,
    pub visible: [f32; 4],
}

impl PageGeo {
    /// Width and height of the page as it is drawn.
    pub fn drawn_size(&self) -> (f32, f32) {
        let w = self.visible[2] - self.visible[0];
        let h = self.visible[3] - self.visible[1];
        if self.turns.is_multiple_of(2) {
            (w, h)
        } else {
            (h, w)
        }
    }

    fn user_point(&self, x: f32, y: f32) -> (f32, f32) {
        let [l, b, r, t] = self.visible;
        match self.turns % 4 {
            0 => (l + x, t - y),
            1 => (l + y, b + x),
            2 => (r - x, b + y),
            _ => (r - y, t - x),
        }
    }

    fn drawn_point(&self, ux: f32, uy: f32) -> (f32, f32) {
        let [l, b, r, t] = self.visible;
        match self.turns % 4 {
            0 => (ux - l, t - uy),
            1 => (uy - b, ux - l),
            2 => (r - ux, uy - b),
            _ => (t - uy, r - ux),
        }
    }

    /// A box `[left, top, right, bottom]` of the drawn page as `[left, bottom, right, top]` of the
    /// page's own coordinates.
    pub fn to_user(&self, r: [f32; 4]) -> [f32; 4] {
        let (x1, y1) = self.user_point(r[0], r[1]);
        let (x2, y2) = self.user_point(r[2], r[3]);
        [x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2)]
    }

    /// The other way: a box of the page's own coordinates as `[left, top, right, bottom]` of the
    /// drawn page.
    pub fn to_drawn(&self, u: [f32; 4]) -> [f32; 4] {
        let (x1, y1) = self.drawn_point(u[0], u[1]);
        let (x2, y2) = self.drawn_point(u[2], u[3]);
        [x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geo(turns: u8) -> PageGeo {
        PageGeo {
            turns,
            visible: [50.0, 60.0, 500.0, 700.0],
        }
    }

    #[test]
    fn boxes_go_there_and_back() {
        for turns in 0..4 {
            let g = geo(turns);
            for r in [
                [10.0, 20.0, 110.0, 50.0],
                [0.0, 0.0, 30.0, 30.0],
                [200.0, 100.0, 220.0, 400.0],
            ] {
                let back = g.to_drawn(g.to_user(r));
                for (a, b) in back.iter().zip(r) {
                    assert!((a - b).abs() < 1e-3, "turns {turns}: {back:?} != {r:?}");
                }
            }
        }
    }

    #[test]
    fn an_upright_page_flips_the_vertical_axis() {
        let g = geo(0);
        // Top left 10 across and 20 down, 100 wide and 30 tall.
        assert_eq!(
            g.to_user([10.0, 20.0, 110.0, 50.0]),
            [60.0, 650.0, 160.0, 680.0]
        );
    }

    #[test]
    fn a_quarter_turn_swaps_the_sides() {
        let g = geo(1);
        assert_eq!(g.drawn_size(), (640.0, 450.0));
        // The top left corner of the drawn page is the bottom left corner of the page.
        let u = g.to_user([0.0, 0.0, 100.0, 20.0]);
        assert_eq!(u, [50.0, 60.0, 70.0, 160.0]);
        let g3 = geo(3);
        // The top left corner of a page turned three quarters is the top right corner of the page.
        assert_eq!(
            g3.to_user([0.0, 0.0, 100.0, 20.0]),
            [480.0, 600.0, 500.0, 700.0]
        );
        let g2 = geo(2);
        assert_eq!(
            g2.to_user([0.0, 0.0, 100.0, 20.0]),
            [400.0, 60.0, 500.0, 80.0]
        );
    }
}
