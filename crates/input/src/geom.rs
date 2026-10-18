//! Window <-> video picture <-> phone panel geometry.
//!
//! All window coordinates are physical pixels (winit's `PhysicalPosition`),
//! so DPI scaling never enters the math.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Aspect-fit a `cw` x `ch` picture into a `ww` x `wh` window, centered.
pub fn fit(ww: f64, wh: f64, cw: f64, ch: f64) -> Rect {
    if ww <= 0.0 || wh <= 0.0 || cw <= 0.0 || ch <= 0.0 {
        return Rect { x: 0.0, y: 0.0, w: ww.max(0.0), h: wh.max(0.0) };
    }
    let s = (ww / cw).min(wh / ch);
    let (w, h) = (cw * s, ch * s);
    Rect { x: (ww - w) / 2.0, y: (wh - h) / 2.0, w, h }
}

/// How the displayed picture is rotated relative to the phone panel's native
/// portrait orientation (clockwise).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rotation {
    #[default]
    R0,
    R90,
    R180,
    R270,
}

impl Rotation {
    pub fn next(self) -> Self {
        match self {
            Rotation::R0 => Rotation::R90,
            Rotation::R90 => Rotation::R180,
            Rotation::R180 => Rotation::R270,
            Rotation::R270 => Rotation::R0,
        }
    }

    pub fn prev(self) -> Self {
        self.next().next().next()
    }

    pub fn degrees(self) -> u32 {
        match self {
            Rotation::R0 => 0,
            Rotation::R90 => 90,
            Rotation::R180 => 180,
            Rotation::R270 => 270,
        }
    }

    /// Point in the displayed picture (u, v in 0..1) -> panel (x, y in 0..1).
    pub fn picture_to_panel(self, u: f64, v: f64) -> (f64, f64) {
        match self {
            Rotation::R0 => (u, v),
            Rotation::R90 => (v, 1.0 - u),
            Rotation::R180 => (1.0 - u, 1.0 - v),
            Rotation::R270 => (1.0 - v, u),
        }
    }

    /// Inverse of [`picture_to_panel`](Self::picture_to_panel).
    pub fn panel_to_picture(self, x: f64, y: f64) -> (f64, f64) {
        match self {
            Rotation::R0 => (x, y),
            Rotation::R90 => (1.0 - y, x),
            Rotation::R180 => (1.0 - x, 1.0 - y),
            Rotation::R270 => (y, 1.0 - x),
        }
    }
}

/// Maps window pixels to normalized panel coordinates for the picture drawn
/// in `view`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mapper {
    pub view: Rect,
    pub rotation: Rotation,
}

impl Mapper {
    /// Panel coordinates in 0..1, or `None` outside the picture. With
    /// `clamp`, points outside are pulled to the nearest edge (used while a
    /// drag is in progress so the finger can slide off the picture).
    pub fn to_panel(&self, px: f64, py: f64, clamp: bool) -> Option<(f64, f64)> {
        if self.view.w <= 0.0 || self.view.h <= 0.0 {
            return None;
        }
        let mut u = (px - self.view.x) / self.view.w;
        let mut v = (py - self.view.y) / self.view.h;
        let inside = (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v);
        if !inside {
            if !clamp {
                return None;
            }
            u = u.clamp(0.0, 1.0);
            v = v.clamp(0.0, 1.0);
        }
        Some(self.rotation.picture_to_panel(u, v))
    }

    /// Panel coordinates (0..1) -> window pixels, for drawing overlays.
    pub fn to_window(&self, x: f64, y: f64) -> (f64, f64) {
        let (u, v) = self.rotation.panel_to_picture(x, y);
        (self.view.x + u * self.view.w, self.view.y + v * self.view.h)
    }
}

/// 0..1 -> the 0..=65535 range HID touch reports use.
pub fn to_u16(v: f64) -> u16 {
    (v.clamp(0.0, 1.0) * 65535.0).round() as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
    }

    #[test]
    fn fit_letterboxes_and_pillarboxes() {
        // Tall phone picture in a wide window: pillarbox.
        let r = fit(1000.0, 1000.0, 828.0, 1792.0);
        assert!((r.h - 1000.0).abs() < 1e-9);
        assert!((r.w - 1000.0 * 828.0 / 1792.0).abs() < 1e-9);
        assert!((r.x - (1000.0 - r.w) / 2.0).abs() < 1e-9 && r.y == 0.0);
        // Wide picture in a tall window: letterbox.
        let r = fit(400.0, 1000.0, 1792.0, 828.0);
        assert!((r.w - 400.0).abs() < 1e-9 && r.x == 0.0);
    }

    #[test]
    fn rotations_round_trip() {
        for rot in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
            for p in [(0.1, 0.2), (0.0, 1.0), (0.75, 0.33)] {
                let q = rot.picture_to_panel(p.0, p.1);
                assert!(close(rot.panel_to_picture(q.0, q.1), p), "{rot:?} {p:?}");
            }
        }
    }

    #[test]
    fn r90_maps_picture_corners() {
        // Picture is the panel turned 90 deg clockwise: the panel's top-left
        // corner ends up at the picture's top-right.
        assert!(close(Rotation::R90.panel_to_picture(0.0, 0.0), (1.0, 0.0)));
        assert!(close(Rotation::R90.picture_to_panel(1.0, 0.0), (0.0, 0.0)));
    }

    #[test]
    fn mapper_inside_outside_and_clamp() {
        let m = Mapper { view: Rect { x: 100.0, y: 0.0, w: 200.0, h: 400.0 }, rotation: Rotation::R0 };
        assert!(close(m.to_panel(200.0, 200.0, false).unwrap(), (0.5, 0.5)));
        assert_eq!(m.to_panel(50.0, 200.0, false), None);
        assert!(close(m.to_panel(50.0, 500.0, true).unwrap(), (0.0, 1.0)));
        let (wx, wy) = m.to_window(0.5, 0.5);
        assert!(close((wx, wy), (200.0, 200.0)));
    }

    #[test]
    fn to_u16_bounds() {
        assert_eq!(to_u16(0.0), 0);
        assert_eq!(to_u16(1.0), 65535);
        assert_eq!(to_u16(-3.0), 0);
        assert_eq!(to_u16(0.5), 32768);
    }
}
