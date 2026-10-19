//! Left-button drag state machine: ordinary touches, or (when the press
//! starts in the bottom edge zone) the Indigo bottom-edge system gesture.
//!
//! Phase 0 established that touchscreen reports never trigger the bottom-edge
//! Home / App Switcher gestures, while Indigo digitizer events with
//! `edge = bottom` and 0..1 coordinates do. Top-edge gestures work with plain
//! touches, so only the bottom edge is special. "Bottom" is the bottom of
//! what the user sees: with the picture turned for landscape it is a side of
//! the panel, and the swipe is sent from that side, flagged as that edge.

use crate::{EdgePhase, HidEvent, Side, geom::to_u16};

/// Fraction of the panel height, measured from the bottom, that starts an
/// edge gesture instead of a touch.
pub const DEFAULT_EDGE_ZONE: f64 = 0.03;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Idle,
    Touch,
    Edge,
}

#[derive(Debug)]
pub struct Gesture {
    mode: Mode,
    last: (f64, f64),
    /// Panel edge of the current edge gesture.
    side: Side,
    pub edge_zone: f64,
}

impl Default for Gesture {
    fn default() -> Self {
        Self { mode: Mode::Idle, last: (0.0, 0.0), side: Side::Bottom, edge_zone: DEFAULT_EDGE_ZONE }
    }
}

impl Gesture {
    pub fn active(&self) -> bool {
        self.mode != Mode::Idle
    }

    /// Button pressed at panel point `p` (0..1). `bottom` is the panel edge
    /// at the bottom of what the user sees (see `Rotation::bottom_side`).
    pub fn down(&mut self, p: (f64, f64), bottom: Side) -> HidEvent {
        self.last = p;
        if bottom.distance(p) <= self.edge_zone {
            self.mode = Mode::Edge;
            self.side = bottom;
            // The gesture must begin on the edge itself.
            let (x, y) = bottom.at(bottom.along(p));
            HidEvent::Edge { phase: EdgePhase::Start, x, y, side: bottom }
        } else {
            self.mode = Mode::Touch;
            HidEvent::Touch { x: to_u16(p.0), y: to_u16(p.1) }
        }
    }

    pub fn moved(&mut self, p: (f64, f64)) -> Option<HidEvent> {
        if p == self.last {
            return None;
        }
        self.last = p;
        match self.mode {
            Mode::Idle => None,
            Mode::Touch => Some(HidEvent::Touch { x: to_u16(p.0), y: to_u16(p.1) }),
            Mode::Edge => Some(HidEvent::Edge { phase: EdgePhase::Move, x: p.0, y: p.1, side: self.side }),
        }
    }

    /// Button released (or the pointer left the window mid-drag).
    pub fn up(&mut self, p: Option<(f64, f64)>) -> Option<HidEvent> {
        let p = p.unwrap_or(self.last);
        let mode = std::mem::replace(&mut self.mode, Mode::Idle);
        match mode {
            Mode::Idle => None,
            Mode::Touch => Some(HidEvent::Release { x: to_u16(p.0), y: to_u16(p.1) }),
            Mode::Edge => Some(HidEvent::Edge { phase: EdgePhase::End, x: p.0, y: p.1, side: self.side }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_drag_sequence() {
        let mut g = Gesture::default();
        assert_eq!(g.down((0.5, 0.5), Side::Bottom), HidEvent::Touch { x: 32768, y: 32768 });
        assert_eq!(g.moved((0.5, 0.5)), None, "no duplicate samples");
        assert!(matches!(g.moved((0.6, 0.5)), Some(HidEvent::Touch { .. })));
        assert!(matches!(g.up(None), Some(HidEvent::Release { x: 39321, .. })));
        assert_eq!(g.up(None), None);
        assert!(!g.active());
    }

    #[test]
    fn bottom_zone_starts_edge_gesture_on_the_edge() {
        let mut g = Gesture::default();
        let e = g.down((0.4, 0.985), Side::Bottom);
        assert_eq!(e, HidEvent::Edge { phase: EdgePhase::Start, x: 0.4, y: 1.0, side: Side::Bottom });
        assert!(matches!(g.moved((0.4, 0.7)), Some(HidEvent::Edge { phase: EdgePhase::Move, side: Side::Bottom, .. })));
        assert!(matches!(g.up(Some((0.4, 0.6))), Some(HidEvent::Edge { phase: EdgePhase::End, .. })));
    }

    #[test]
    fn landscape_bottom_is_a_side_of_the_panel() {
        // Turned for landscape, the bottom of the view is the panel's left.
        let mut g = Gesture::default();
        let e = g.down((0.01, 0.5), Side::Left);
        assert_eq!(e, HidEvent::Edge { phase: EdgePhase::Start, x: 0.0, y: 0.5, side: Side::Left });
        assert!(matches!(g.moved((0.5, 0.5)), Some(HidEvent::Edge { side: Side::Left, .. })));
        // The panel's bottom is then just a place to touch.
        let mut g = Gesture::default();
        assert!(matches!(g.down((0.4, 0.99), Side::Left), HidEvent::Touch { .. }));
    }
}
