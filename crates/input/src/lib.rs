//! Turns window mouse/keyboard input into phone HID events.
//!
//! Pure data and geometry: no I/O. `device` executes the [`HidEvent`]s over
//! the tunnel; `app` uses [`geom`] both for the video viewport and for
//! mapping clicks, so the two can never disagree.

pub mod geom;
pub mod gesture;
pub mod keys;

/// One thing to deliver to the phone. Coordinates are the phone panel's
/// native (portrait) space normalized to 0..=65535.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HidEvent {
    /// Finger in contact at (x, y): first sample is touch-down, later ones are moves.
    Touch { x: u16, y: u16 },
    /// Finger lifted at (x, y).
    Release { x: u16, y: u16 },
    /// Bottom-edge system gesture (Home / App Switcher), Indigo digitizer
    /// path. `side` is the panel edge that is the bottom of what the user
    /// sees (a side edge when the picture is turned for landscape).
    Edge { phase: EdgePhase, x: f64, y: f64, side: Side },
    /// Press and release a hardware button after `hold_ms`.
    Button { button: Button },
    /// Keyboard usage (HID page 0x07) down or up.
    Key { usage: u16, down: bool },
    /// Scripted bottom-edge swipe-and-hold that opens the App Switcher:
    /// from `side`, `travel` (fraction of the panel) inwards, then a pause.
    AppSwitcher { side: Side, travel: f64 },
    /// Mouse-wheel scroll: a short finger drag from (x, y) moving by
    /// (dx, dy), in panel units (0..1), held still before lifting so the
    /// content scrolls without flinging.
    Scroll { x: u16, y: u16, dx: f64, dy: f64 },
}

/// An edge of the phone panel (its native portrait space).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

impl Side {
    /// The point on this edge at `along` (0..1) of its length.
    pub fn at(self, along: f64) -> (f64, f64) {
        match self {
            Side::Top => (along, 0.0),
            Side::Bottom => (along, 1.0),
            Side::Left => (0.0, along),
            Side::Right => (1.0, along),
        }
    }

    /// The point `depth` (0..1) inwards from this edge, at `along`.
    pub fn inward(self, along: f64, depth: f64) -> (f64, f64) {
        match self {
            Side::Top => (along, depth),
            Side::Bottom => (along, 1.0 - depth),
            Side::Left => (depth, along),
            Side::Right => (1.0 - depth, along),
        }
    }

    /// How far panel point `p` is from this edge (0 on the edge).
    pub fn distance(self, p: (f64, f64)) -> f64 {
        match self {
            Side::Top => p.1,
            Side::Bottom => 1.0 - p.1,
            Side::Left => p.0,
            Side::Right => 1.0 - p.0,
        }
    }

    /// Position of `p` along this edge (0..1).
    pub fn along(self, p: (f64, f64)) -> f64 {
        match self {
            Side::Top | Side::Bottom => p.0,
            Side::Left | Side::Right => p.1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgePhase {
    Start,
    Move,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Home,
    Lock,
    VolumeUp,
    VolumeDown,
}

impl Button {
    /// (usage page, usage, hold in ms). Consumer page values agree between
    /// idevice's hid tool and pymobiledevice3; Home and volume verified in
    /// Phase 0, Lock's 200 ms hold is idevice's value (unverified).
    pub fn usage(self) -> (u64, u64, u64) {
        match self {
            Button::Home => (0x0C, 0x40, 80),
            Button::Lock => (0x0C, 0x30, 200),
            Button::VolumeUp => (0x0C, 0xE9, 80),
            Button::VolumeDown => (0x0C, 0xEA, 80),
        }
    }
}
