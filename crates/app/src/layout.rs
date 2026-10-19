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
    /// Disconnect from the iPhone, or connect again (the bar button).
    Power,
    /// The Connect button on the disconnected page.
    Connect,
    /// A phone in the picker (index into the list): connect to it.
    Phone(usize),
    /// Forget that phone.
    Forget(usize),
    /// What was copied on the iPhone: press and drag it out of the window.
    Shelf,
    /// Use the iPad as a second monitor (bar button, iPads only).
    Display,
    /// Stop that (the button on the display page).
    StopDisplay,
}

#[derive(Debug, Clone)]
pub struct Toolbar {
    /// The whole bar; pressing its background drags the window.
    pub bar: Rect,
    /// Traffic lights are circles: (centre x, centre y, radius).
    pub lights: [(Control, f64, f64, f64); 3],
    /// Icon buttons, right-aligned (right to left): square hit boxes.
    pub icons: Vec<(Control, Rect)>,
    /// The copied-on-iPhone chip, centred in the free middle of the bar.
    pub shelf: Option<Rect>,
}

/// Screen corner radius. iPhone corners are ~10 % of the panel width
/// (iPhone 11: 41.5 pt on a 414 pt wide screen); iPad corners are far
/// tighter (iPad 10th generation: 18 pt on 820 pt, ~2.2 %). iPads are much
/// squarer (short/long side above 0.6, iPhones below 0.5), which tells them
/// apart without knowing the model.
pub fn corner_radius(view: Rect) -> f64 {
    let (short, long) = (view.w.min(view.h), view.w.max(view.h));
    let tablet = long > 0.0 && short / long > 0.6;
    short * if tablet { 0.022 } else { 0.10 }
}

/// Lay out the controls inside `bar`, with the shelf chip when something
/// copied on the iPhone is waiting and the display button for an iPad.
pub fn toolbar(bar: Rect, scale: f64, shelf: bool, display: bool) -> Toolbar {
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
    let mut icons = vec![
        (Control::Home, icon(0.0)),
        (Control::AppSwitcher, icon(1.0)),
        (Control::Rotate, icon(2.0)),
        (Control::Power, icon(3.0)),
    ];
    if display {
        icons.push((Control::Display, icon(4.0)));
    }
    let leftmost = icons.last().map(|i| i.1.x).unwrap_or(right);
    let shelf = shelf.then(|| {
        let left = x0 + 2.0 * step + r + 12.0 * scale;
        let right = leftmost - 10.0 * scale;
        let w = (150.0 * scale).min(right - left);
        let h = (24.0 * scale).min(bar.h - 4.0 * scale);
        Rect { x: left + (right - left - w) / 2.0, y: cy - h / 2.0, w, h }
    });
    let shelf = shelf.filter(|r| r.w >= 60.0 * scale);
    Toolbar { bar, lights, icons, shelf }
}

impl Toolbar {
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

/// The Connect button on the disconnected page, under the page's text.
pub fn connect_button(view: Rect, scale: f64) -> Rect {
    let (w, h) = (150.0 * scale, 38.0 * scale);
    Rect { x: view.x + (view.w - w) / 2.0, y: view.y + view.h * 0.42 + 112.0 * scale, w, h }
}

/// Height of the picker's header (title and hint) above the rows.
pub const PICKER_HEADER: f64 = 96.0;

/// Rows of the iPhone picker on the disconnected page: (row, Forget button)
/// for as many of `n` phones as fit.
pub fn picker_rows(view: Rect, scale: f64, n: usize) -> Vec<(Rect, Rect)> {
    let pad = 18.0 * scale;
    let (h, gap) = (60.0 * scale, 10.0 * scale);
    let top = picker_top(view, scale) + PICKER_HEADER * scale;
    let bottom = view.y + view.h - 24.0 * scale;
    (0..n)
        .map(|i| Rect { x: view.x + pad, y: top + i as f64 * (h + gap), w: view.w - 2.0 * pad, h })
        .take_while(|r| r.y + r.h <= bottom)
        .map(|r| {
            let fw = 72.0 * scale;
            (r, Rect { x: r.x + r.w - fw - 8.0 * scale, y: r.y + (r.h - 30.0 * scale) / 2.0, w: fw, h: 30.0 * scale })
        })
        .collect()
}

/// Where the picker's title sits.
pub fn picker_top(view: Rect, _scale: f64) -> f64 {
    view.y + view.h * 0.16
}

pub fn inside(r: Rect, x: f64, y: f64) -> bool {
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
        let t = toolbar(bar(), 1.0, false, true);
        assert_eq!(t.icons.last().map(|i| i.0), Some(Control::Display));
        assert!(!toolbar(bar(), 1.0, false, false).icons.iter().any(|i| i.0 == Control::Display), "iPads only");
        for &(c, cx, cy, _) in &t.lights {
            assert!(inside(t.bar, cx, cy));
            assert_eq!(t.hit(cx, cy), Some(c));
        }
        for &(c, r) in &t.icons {
            let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            assert!(inside(t.bar, cx, cy));
            assert_eq!(t.hit(cx, cy), Some(c));
        }
        // The middle of the bar is plain background (drag area).
        assert_eq!(t.hit(206.0, 18.0), None);
        assert!(inside(t.bar, 206.0, 18.0));
        assert!(!inside(t.bar, 206.0, 60.0));
    }

    #[test]
    fn shelf_sits_between_lights_and_icons() {
        let t = toolbar(bar(), 1.0, true, true);
        let s = t.shelf.expect("room for the shelf");
        assert!(s.x > t.lights[2].1 + t.lights[2].3);
        assert!(s.x + s.w < t.icons.last().unwrap().1.x);
        assert_eq!(t.hit(s.x + s.w / 2.0, s.y + s.h / 2.0), Some(Control::Shelf));
        // A narrow window has no room: no chip rather than an overlap.
        assert!(toolbar(Rect { w: 230.0, ..bar() }, 1.0, true, false).shelf.is_none());
    }

    #[test]
    fn picker_rows_fit_and_do_not_overlap() {
        let view = Rect { x: 6.0, y: 6.0, w: 400.0, h: 860.0 };
        let rows = picker_rows(view, 1.0, 20);
        assert!(!rows.is_empty() && rows.len() < 20, "only as many rows as fit");
        for w in rows.windows(2) {
            assert!(w[0].0.y + w[0].0.h < w[1].0.y);
        }
        for (row, forget) in &rows {
            assert!(row.y + row.h <= view.y + view.h);
            assert!(inside(*row, forget.x + 1.0, forget.y + 1.0), "Forget sits inside its row");
        }
    }

    #[test]
    fn ipad_corners_are_tighter_than_iphone_corners() {
        // iPhone 11 panel and iPad 10th generation panel, either way up.
        let iphone = corner_radius(Rect { x: 0.0, y: 0.0, w: 414.0, h: 896.0 });
        let ipad = corner_radius(Rect { x: 0.0, y: 0.0, w: 820.0, h: 1180.0 });
        let ipad_landscape = corner_radius(Rect { x: 0.0, y: 0.0, w: 1180.0, h: 820.0 });
        assert!((iphone - 41.4).abs() < 0.1);
        assert!((ipad - 18.0).abs() < 0.2);
        assert_eq!(ipad, ipad_landscape);
    }

    #[test]
    fn scales_with_dpi() {
        let a = toolbar(bar(), 1.0, false, false);
        let b = toolbar(Rect { h: 2.0 * BAR_HEIGHT, w: 824.0, ..bar() }, 2.0, false, false);
        assert!((b.lights[0].3 - 2.0 * a.lights[0].3).abs() < 1e-9);
    }
}
