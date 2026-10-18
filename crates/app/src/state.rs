//! State shared between the window (main) thread and the present thread.

use std::sync::{Arc, Mutex, atomic::AtomicBool};

use device::hid::HidStats;
use input::geom::{Mapper, Rotation, fit};

use crate::metrics::Counters;

/// iPhone 11 panel, used only until the phone reports its own size.
const DEFAULT_SCREEN: (u32, u32) = (828, 1792);

#[derive(Debug, Clone, Copy)]
pub struct Indicator {
    /// Panel coordinates, 0..1.
    pub panel: (f64, f64),
    pub down_us: u64,
    pub released_us: Option<u64>,
}

#[derive(Debug)]
pub struct View {
    /// Back-buffer / client area size in physical pixels.
    pub window: (u32, u32),
    /// Size of the decoder texture holding the latest picture.
    pub tex: Option<(u32, u32)>,
    /// Panel size in pixels, portrait, as reported by the phone.
    pub screen: (u32, u32),
    /// How the picture is turned for display. The phone always streams the
    /// panel in portrait (verified at GATE 1: landscape apps arrive sideways),
    /// and no API to read its orientation is known, so the user picks this.
    pub rotation: Rotation,
    pub indicator: Option<Indicator>,
}

impl Default for View {
    fn default() -> Self {
        Self { window: (1, 1), tex: None, screen: DEFAULT_SCREEN, rotation: Rotation::R0, indicator: None }
    }
}

impl View {
    pub fn sideways(&self) -> bool {
        matches!(self.rotation, Rotation::R90 | Rotation::R270)
    }

    /// Size of the picture as displayed (after rotation).
    pub fn content(&self) -> (f64, f64) {
        let (w, h) = (self.screen.0 as f64, self.screen.1 as f64);
        if self.sideways() { (h, w) } else { (w, h) }
    }

    pub fn mapper(&self) -> Mapper {
        let (cw, ch) = self.content();
        Mapper { view: fit(self.window.0 as f64, self.window.1 as f64, cw, ch), rotation: self.rotation }
    }

    /// Fraction of the texture that is real picture. The content sits in the
    /// top-left (Phase 1 measurement on the iPhone 11: 828x1792 inside a
    /// 832x1808 coded frame, inside a larger, aligned decoder texture).
    pub fn uv_max(&self) -> (f32, f32) {
        match self.tex {
            Some((w, h)) if w > 0 && h > 0 => (
                (self.screen.0 as f64 / w as f64).min(1.0) as f32,
                (self.screen.1 as f64 / h as f64).min(1.0) as f32,
            ),
            _ => (1.0, 1.0),
        }
    }
}

#[derive(Default)]
pub struct Shared {
    pub view: Mutex<View>,
    /// Debug overlay on/off (F10).
    pub overlay: AtomicBool,
    pub counters: Counters,
    /// The current session's input statistics, once a device is running.
    pub hid_stats: Mutex<Option<Arc<HidStats>>>,
}
