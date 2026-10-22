//! Placement math for the floating widget, in physical screen pixels.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    fn center2(&self) -> (i64, i64) {
        (2 * i64::from(self.x) + i64::from(self.w), 2 * i64::from(self.y) + i64::from(self.h))
    }

    pub fn overlap(&self, other: &Rect) -> i64 {
        let w = self.right().min(other.right()) - self.x.max(other.x);
        let h = self.bottom().min(other.bottom()) - self.y.max(other.y);
        if w <= 0 || h <= 0 {
            0
        } else {
            i64::from(w) * i64::from(h)
        }
    }

    /// Moves (never resizes) `self` so it lies inside `area` as far as it fits.
    pub fn clamped_to(self, area: &Rect) -> Rect {
        let x = self.x.min(area.right() - self.w).max(area.x);
        let y = self.y.min(area.bottom() - self.h).max(area.y);
        Rect { x, y, ..self }
    }
}

/// The monitor showing most of `r`, or `None` when `r` is off-screen (for
/// example a monitor that was unplugged since the position was saved).
pub fn monitor_of(r: &Rect, monitors: &[Rect]) -> Option<Rect> {
    monitors.iter().copied().filter(|m| m.overlap(r) > 0).max_by_key(|m| m.overlap(r))
}

/// The widget's rectangle after switching to a `w`×`h` size: the corner that
/// is nearest to the monitor's edges stays put, so a bubble parked in the
/// bottom-right corner opens its panel up and to the left instead of off the
/// screen.
pub fn resize_anchored(old: Rect, w: i32, h: i32, monitor: &Rect) -> Rect {
    let (cx, cy) = old.center2();
    let (mx, my) = monitor.center2();
    let x = if cx > mx { old.right() - w } else { old.x };
    let y = if cy > my { old.bottom() - h } else { old.y };
    Rect::new(x, y, w, h).clamped_to(monitor)
}

/// First-run spot: the top-right corner, clear of a top bar.
pub fn default_rect(w: i32, h: i32, monitor: &Rect, scale: f32) -> Rect {
    let side = (24.0 * scale).round() as i32;
    let top = (56.0 * scale).round() as i32;
    Rect::new(monitor.right() - w - side, monitor.y + top, w, h).clamped_to(monitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: Rect = Rect { x: 0, y: 0, w: 1920, h: 1080 };

    #[test]
    fn bubble_in_bottom_right_opens_up_and_left() {
        let bubble = Rect::new(1840, 1000, 60, 60);
        let panel = resize_anchored(bubble, 340, 500, &MON);
        assert_eq!(panel, Rect::new(1560, 560, 340, 500));
        // And collapses back to the same spot.
        assert_eq!(resize_anchored(panel, 60, 60, &MON), bubble);
    }

    #[test]
    fn bubble_in_top_left_opens_down_and_right() {
        let bubble = Rect::new(20, 30, 60, 60);
        assert_eq!(resize_anchored(bubble, 340, 500, &MON), Rect::new(20, 30, 340, 500));
    }

    #[test]
    fn growing_never_leaves_the_monitor() {
        let bubble = Rect::new(-10, 1050, 60, 60);
        let panel = resize_anchored(bubble, 340, 500, &MON);
        assert_eq!(panel.x, 0);
        assert_eq!(panel.bottom(), 1080);
    }

    #[test]
    fn second_monitor() {
        let left = Rect::new(-1280, 0, 1280, 1024);
        let monitors = [MON, left];
        let w = Rect::new(-300, 100, 60, 60);
        assert_eq!(monitor_of(&w, &monitors), Some(left));
        assert_eq!(monitor_of(&Rect::new(5000, 5000, 60, 60), &monitors), None);
        // Straddling: the monitor with the larger share wins.
        assert_eq!(monitor_of(&Rect::new(-20, 0, 340, 500), &monitors), Some(MON));
    }

    #[test]
    fn default_spot_is_top_right() {
        let r = default_rect(340, 500, &MON, 1.0);
        assert_eq!(r, Rect::new(1920 - 340 - 24, 56, 340, 500));
        let r2 = default_rect(120, 120, &Rect::new(1920, 0, 2560, 1440), 2.0);
        assert_eq!((r2.right(), r2.y), (1920 + 2560 - 48, 112));
    }
}
