//! Geometry of the window chrome, shared by hit-testing (main thread) and
//! drawing (present thread) so the two can never disagree. All values are
//! physical pixels.
//!
//! Like macOS iPhone Mirroring, the bar is a strip *above* the phone screen
//! (the window grows to show it), so it never covers the screen's top edge,
//! where Notification Center and Control Center are pulled down.

use input::geom::Rect;

/// Height of the hover bar, logical px.
pub const BAR_HEIGHT: f64 = 36.0;

/// Things on the hover bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Close,
    Minimize,
    Zoom,
    Rotate,
    AppSwitcher,
    Home,
    /// What was copied on the iPhone: press and drag it out of the window.
    Shelf,
}

#[derive(Debug, Clone, Copy)]
pub struct Toolbar {
    /// The whole bar; pressing its background drags the window.
    pub bar: Rect,
    /// Traffic lights are circles: (centre x, centre y, radius).
    pub lights: [(Control, f64, f64, f64); 3],
    /// Icon buttons, right-aligned: square hit boxes.
    pub icons: [(Control, Rect); 3],
    /// The copied-on-iPhone chip, centred in the free middle of the bar.
    pub shelf: Option<Rect>,
}

/// iPhone display corners are ~10 % of the panel width (iPhone 11:
/// 41.5 pt on a 414 pt wide screen).
pub fn corner_radius(view: Rect) -> f64 {
    0.10 * view.w.min(view.h)
}

/// Lay out the controls inside `bar`, with the shelf chip when something
/// copied on the iPhone is waiting.
pub fn toolbar(bar: Rect, scale: f64, shelf: bool) -> Toolbar {
    let cy = bar.y + bar.h / 2.0;
    let r = 6.5 * scale;
    let step = 20.0 * scale;
    let x0 = bar.x + 14.0 * scale + r;
    let lights = [
        (Control::Close, x0, cy, r),
        (Control::Minimize, x0 + step, cy, r),
        (Control::Zoom, x0 + 2.0 * step, cy, r),
    ];
    let size = (28.0 * scale).min(bar.h);
    let gap = 4.0 * scale;
    let right = bar.x + bar.w - 10.0 * scale;
    let icon = |i: f64| Rect { x: right - (i + 1.0) * size - i * gap, y: cy - size / 2.0, w: size, h: size };
    let icons = [(Control::Home, icon(0.0)), (Control::AppSwitcher, icon(1.0)), (Control::Rotate, icon(2.0))];
    let shelf = shelf.then(|| {
        let left = x0 + 2.0 * step + r + 12.0 * scale;
        let right = icons[2].1.x - 10.0 * scale;
        let w = (150.0 * scale).min(right - left);
        let h = (24.0 * scale).min(bar.h - 4.0 * scale);
        Rect { x: left + (right - left - w) / 2.0, y: cy - h / 2.0, w, h }
    });
    let shelf = shelf.filter(|r| r.w >= 60.0 * scale);
    Toolbar { bar, lights, icons, shelf }
}

impl Toolbar {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        inside(self.bar, x, y)
    }

    /// Which control is under (x, y), if any. Light hit areas are a bit
    /// larger than the drawn circle.
    pub fn hit(&self, x: f64, y: f64) -> Option<Control> {
        for &(c, cx, cy, r) in &self.lights {
            if (x - cx).hypot(y - cy) <= r * 1.6 {
                return Some(c);
            }
        }
        if self.shelf.is_some_and(|r| inside(r, x, y)) {
            return Some(Control::Shelf);
        }
        self.icons.iter().find(|(_, rect)| inside(*rect, x, y)).map(|(c, _)| *c)
    }
}

fn inside(r: Rect, x: f64, y: f64) -> bool {
    x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar() -> Rect {
        Rect { x: 0.0, y: 0.0, w: 412.0, h: BAR_HEIGHT }
    }

    #[test]
    fn controls_are_inside_the_bar_and_distinct() {
        let t = toolbar(bar(), 1.0, false);
        for &(c, cx, cy, _) in &t.lights {
            assert!(t.contains(cx, cy));
            assert_eq!(t.hit(cx, cy), Some(c));
        }
        for &(c, r) in &t.icons {
            let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            assert!(t.contains(cx, cy));
            assert_eq!(t.hit(cx, cy), Some(c));
        }
        // The middle of the bar is plain background (drag area).
        assert_eq!(t.hit(206.0, 18.0), None);
        assert!(t.contains(206.0, 18.0));
        assert!(!t.contains(206.0, 60.0));
    }

    #[test]
    fn shelf_sits_between_lights_and_icons() {
        let t = toolbar(bar(), 1.0, true);
        let s = t.shelf.expect("room for the shelf");
        assert!(s.x > t.lights[2].1 + t.lights[2].3);
        assert!(s.x + s.w < t.icons[2].1.x);
        assert_eq!(t.hit(s.x + s.w / 2.0, s.y + s.h / 2.0), Some(Control::Shelf));
        // A narrow window has no room: no chip rather than an overlap.
        assert!(toolbar(Rect { w: 200.0, ..bar() }, 1.0, true).shelf.is_none());
    }

    #[test]
    fn scales_with_dpi() {
        let a = toolbar(bar(), 1.0, false);
        let b = toolbar(Rect { h: 2.0 * BAR_HEIGHT, w: 824.0, ..bar() }, 2.0, false);
        assert!((b.lights[0].3 - 2.0 * a.lights[0].3).abs() < 1e-9);
    }
}
