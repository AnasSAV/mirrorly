//! Executes [`HidEvent`]s on the phone's HID services.
//!
//! Runs as its own task so input never waits behind video work. Sends are
//! fire-and-forget XPC messages (Phase 0: ~0.1 ms each). Anything with a
//! duration (button holds, the App Switcher swipe) is put on a deadline
//! scheduler instead of sleeping, so live input keeps flowing meanwhile.

use std::{
    cmp::Reverse,
    collections::BinaryHeap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::Duration,
};

use anyhow::Result;
use idevice::{
    ReadWrite,
    core_device::{
        ButtonState, DigitizerEdge, DigitizerEventType, DigitizerTarget, IndigoHidClient,
        TOUCHSCREEN_STATE_CONTACT, TOUCHSCREEN_STATE_RELEASE, UniversalHidServiceClient,
    },
};
use input::{EdgePhase, HidEvent};
use tokio::{sync::mpsc, time::Instant};

pub type Uhs = UniversalHidServiceClient<Box<dyn ReadWrite>>;
pub type Indigo = IndigoHidClient<Box<dyn ReadWrite>>;

/// An input event and when the window received it (µs on the app epoch).
pub type Stamped = (u64, HidEvent);

#[derive(Debug, Default)]
pub struct HidStats {
    pub sent: AtomicU64,
    pub coalesced: AtomicU64,
    /// Last window-event -> send-start delay, µs.
    pub last_queue_us: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Touch { contact: bool, x: u16, y: u16 },
    /// Indigo digitizer event: bottom-edge gestures, and touches on the
    /// Indigo path (`edge = None`).
    Edge { phase: DigitizerEventType, x: f64, y: f64, edge: DigitizerEdge },
    Button { page: u64, usage: u64, down: bool },
    Key { usage: u16, down: bool },
}

/// Scheduler entry; ordered by due time, then insertion order.
#[derive(Debug, PartialEq)]
struct Due(Instant, u64, Op);
impl Eq for Due {}
impl PartialOrd for Due {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Due {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        (self.0, self.1).cmp(&(o.0, o.1))
    }
}

/// Which service carries ordinary touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPath {
    /// 58-byte touchscreen reports on `universalhidservice` (verified over USB).
    Uhs,
    /// `IndigoDigitizerEvent` with `edge = none` on `hid.indigo`.
    Indigo,
}

pub(crate) async fn run(
    mut uhs: Uhs,
    mut indigo: Indigo,
    rx: &mut mpsc::UnboundedReceiver<Stamped>,
    now_us: fn() -> u64,
    stats: Arc<HidStats>,
    touch_path: TouchPath,
) -> Result<()> {
    let mut indigo_touch_down = false;
    let mut sched: BinaryHeap<Reverse<Due>> = BinaryHeap::new();
    let mut seq = 0u64;
    let mut batch = Vec::new();
    let mut touching = false;
    loop {
        let next = sched.peek().map(|Reverse(d)| d.0);
        tokio::select! {
            biased;
            first = rx.recv() => {
                let Some(first) = first else { return Ok(()) };
                batch.clear();
                batch.push(first);
                while let Ok(e) = rx.try_recv() {
                    batch.push(e);
                }
                for i in 0..batch.len() {
                    let (t, ev) = batch[i];
                    if superseded(touching, &ev, batch.get(i + 1).map(|b| &b.1)) {
                        stats.coalesced.fetch_add(1, Relaxed);
                        continue;
                    }
                    match ev {
                        HidEvent::Touch { .. } => touching = true,
                        HidEvent::Release { .. } => touching = false,
                        _ => {}
                    }
                    stats.last_queue_us.store(now_us().saturating_sub(t), Relaxed);
                    let now = Instant::now();
                    for (delay, op) in expand(ev) {
                        let op = route_touch(op, touch_path, &mut indigo_touch_down);
                        if delay.is_zero() {
                            exec(&mut uhs, &mut indigo, op).await?;
                            stats.sent.fetch_add(1, Relaxed);
                        } else {
                            seq += 1;
                            sched.push(Reverse(Due(now + delay, seq, op)));
                        }
                    }
                }
            }
            _ = tokio::time::sleep_until(next.unwrap_or_else(Instant::now)), if next.is_some() => {
                let now = Instant::now();
                while sched.peek().is_some_and(|Reverse(d)| d.0 <= now) {
                    let Reverse(Due(_, _, op)) = sched.pop().unwrap();
                    exec(&mut uhs, &mut indigo, op).await?;
                    stats.sent.fetch_add(1, Relaxed);
                }
            }
        }
    }
}

/// On the Indigo path, turn touchscreen samples into digitizer events:
/// first contact = Start, later contacts = Position, release = End.
fn route_touch(op: Op, path: TouchPath, down: &mut bool) -> Op {
    let Op::Touch { contact, x, y } = op else { return op };
    if path == TouchPath::Uhs {
        return op;
    }
    let phase = match (contact, *down) {
        (true, false) => DigitizerEventType::Start,
        (true, true) => DigitizerEventType::Position,
        (false, _) => DigitizerEventType::End,
    };
    *down = contact;
    Op::Edge { phase, x: x as f64 / 65535.0, y: y as f64 / 65535.0, edge: DigitizerEdge::None }
}

/// A move is dropped only when a newer sample of the same contact is already
/// queued right behind it (coalesce under backlog, never otherwise). A Touch
/// is a move only while a contact is already down (`touching`); the
/// touch-down sample itself is never dropped.
fn superseded(touching: bool, ev: &HidEvent, next: Option<&HidEvent>) -> bool {
    match (ev, next) {
        (HidEvent::Touch { .. }, Some(HidEvent::Touch { .. })) => touching,
        (
            HidEvent::Edge { phase: EdgePhase::Move, .. },
            Some(HidEvent::Edge { phase: EdgePhase::Move | EdgePhase::End, .. }),
        ) => true,
        _ => false,
    }
}

/// Turn one event into wire ops, each with a delay from now.
fn expand(ev: HidEvent) -> Vec<(Duration, Op)> {
    let now = Duration::ZERO;
    match ev {
        HidEvent::Touch { x, y } => vec![(now, Op::Touch { contact: true, x, y })],
        HidEvent::Release { x, y } => vec![(now, Op::Touch { contact: false, x, y })],
        HidEvent::Edge { phase, x, y } => {
            let phase = match phase {
                EdgePhase::Start => DigitizerEventType::Start,
                EdgePhase::Move => DigitizerEventType::Position,
                EdgePhase::End => DigitizerEventType::End,
            };
            vec![(now, Op::Edge { phase, x, y, edge: DigitizerEdge::Bottom })]
        }
        HidEvent::Key { usage, down } => vec![(now, Op::Key { usage, down })],
        HidEvent::Button { button } => {
            let (page, usage, hold) = button.usage();
            vec![
                (now, Op::Button { page, usage, down: true }),
                (Duration::from_millis(hold), Op::Button { page, usage, down: false }),
            ]
        }
        HidEvent::AppSwitcher => app_switcher_script(),
    }
}

/// Bottom-edge swipe to 30 % of the height over 400 ms, hold 800 ms,
/// release: the timing that opened the App Switcher in Phase 0 (Test C).
fn app_switcher_script() -> Vec<(Duration, Op)> {
    const STEP_MS: u64 = 8;
    const SWIPE_MS: u64 = 400;
    const HOLD_MS: u64 = 800;
    const END_Y: f64 = 0.70;
    let mut ops = vec![(Duration::ZERO, Op::Edge { phase: DigitizerEventType::Start, x: 0.5, y: 1.0, edge: DigitizerEdge::Bottom })];
    let mut t = STEP_MS;
    while t <= SWIPE_MS + HOLD_MS {
        let f = (t as f64 / SWIPE_MS as f64).min(1.0);
        let y = 1.0 + (END_Y - 1.0) * f;
        ops.push((Duration::from_millis(t), Op::Edge { phase: DigitizerEventType::Position, x: 0.5, y, edge: DigitizerEdge::Bottom }));
        t += STEP_MS;
    }
    ops.push((Duration::from_millis(t), Op::Edge { phase: DigitizerEventType::End, x: 0.5, y: END_Y, edge: DigitizerEdge::Bottom }));
    ops
}

async fn exec(uhs: &mut Uhs, indigo: &mut Indigo, op: Op) -> Result<()> {
    match op {
        Op::Touch { contact, x, y } => {
            let state = if contact { TOUCHSCREEN_STATE_CONTACT } else { TOUCHSCREEN_STATE_RELEASE };
            uhs.send_touchscreen(state, x, y, None).await?;
        }
        Op::Edge { phase, x, y, edge } => {
            indigo
                .send_digitizer((x, y), None, phase, edge, DigitizerTarget::MainScreen)
                .await?;
        }
        Op::Button { page, usage, down } => {
            let st = if down { ButtonState::Down } else { ButtonState::Up };
            indigo.send_button(page, usage, st).await?;
        }
        Op::Key { usage, down } => {
            let st = if down { ButtonState::Down } else { ButtonState::Up };
            indigo.send_keyboard(usage as u64, st).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use input::Button;

    #[test]
    fn moves_coalesce_only_behind_newer_samples() {
        let t = HidEvent::Touch { x: 1, y: 1 };
        let t2 = HidEvent::Touch { x: 2, y: 2 };
        let r = HidEvent::Release { x: 1, y: 1 };
        assert!(superseded(true, &t, Some(&t2)), "move behind a newer move");
        assert!(!superseded(false, &t, Some(&t2)), "touch-down is never dropped");
        assert!(!superseded(true, &t, Some(&r)), "last sample before release is kept");
        assert!(!superseded(true, &t, None));
        assert!(!superseded(true, &r, Some(&t)));
        let m = HidEvent::Edge { phase: EdgePhase::Move, x: 0.5, y: 0.9 };
        let end = HidEvent::Edge { phase: EdgePhase::End, x: 0.5, y: 0.8 };
        assert!(superseded(false, &m, Some(&end)));
        assert!(!superseded(false, &end, None));
    }

    #[test]
    fn button_is_down_then_up_after_hold() {
        let ops = expand(HidEvent::Button { button: Button::Home });
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].0, Duration::ZERO);
        assert_eq!(ops[1].0, Duration::from_millis(80));
        assert!(matches!(ops[1].1, Op::Button { down: false, usage: 0x40, .. }));
    }

    #[test]
    fn app_switcher_script_shape() {
        let ops = app_switcher_script();
        assert!(matches!(ops.first().unwrap().1, Op::Edge { phase: DigitizerEventType::Start, y, .. } if y == 1.0));
        assert!(matches!(ops.last().unwrap().1, Op::Edge { phase: DigitizerEventType::End, .. }));
        assert!(ops.last().unwrap().0 >= Duration::from_millis(1200));
        assert!(ops.windows(2).all(|w| w[0].0 <= w[1].0));
    }
}
