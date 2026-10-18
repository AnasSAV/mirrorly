//! First-run welcome: a live checklist of what it takes to connect. Shown
//! until the first successful connection, then never again (a marker file
//! in the data folder remembers that).

use device::DeviceEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Done,
    Active,
    Pending,
}

/// (title, how to do it).
pub const STEPS: [(&str, &str); 4] = [
    ("Install Apple Devices", "Free from the Microsoft Store. It adds the driver Windows needs for iPhone."),
    ("Plug in and tap Trust", "Connect with a USB cable, unlock your iPhone and tap Trust."),
    ("Turn on Developer Mode", "On iPhone: Settings › Privacy & Security › Developer Mode › On, then restart."),
    ("Start mirroring", "Mirrorly connects by itself. After this, Wi-Fi works too — no cable needed."),
];

#[derive(Debug, Clone)]
pub struct Welcome {
    /// Index of the step the user is on.
    pub active: usize,
}

impl Default for Welcome {
    fn default() -> Self {
        Self { active: 1 }
    }
}

impl Welcome {
    pub fn states(&self) -> [StepState; 4] {
        std::array::from_fn(|i| match i.cmp(&self.active) {
            std::cmp::Ordering::Less => StepState::Done,
            std::cmp::Ordering::Equal => StepState::Active,
            std::cmp::Ordering::Greater => StepState::Pending,
        })
    }

    /// Move to the step this event points at.
    pub fn on_event(&mut self, ev: &DeviceEvent) {
        if let Some(step) = step_for(ev) {
            self.active = step;
        }
    }
}

fn step_for(ev: &DeviceEvent) -> Option<usize> {
    match ev {
        DeviceEvent::Connected(_) => Some(3),
        DeviceEvent::Clipboard(_) => None,
        // The routine "Connecting…" says nothing new; later progress means
        // every prerequisite is met.
        DeviceEvent::Status(s) if s.starts_with("Connecting…") => None,
        DeviceEvent::Status(_) => Some(3),
        DeviceEvent::Disconnected(reason) => {
            let r = reason.to_ascii_lowercase();
            Some(if r.contains("apple mobile device service") || r.contains("27015") {
                0
            } else if r.contains("developer mode is off") {
                2
            } else if r.contains("no iphone on usb")
                || r.contains("trust")
                || r.contains("pairing record")
                || r.contains("lockdown session")
                || r.contains("phone locked")
            {
                1
            } else {
                3
            })
        }
    }
}

fn marker() -> std::path::PathBuf {
    device::paths::data_dir().join("onboarded")
}

/// True until the first successful connection on this PC.
pub fn first_run() -> bool {
    !marker().exists()
}

pub fn mark_done() {
    let _ = std::fs::create_dir_all(device::paths::data_dir());
    let _ = std::fs::write(marker(), b"1");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn after(evs: &[DeviceEvent]) -> usize {
        let mut w = Welcome::default();
        for e in evs {
            w.on_event(e);
        }
        w.active
    }

    #[test]
    fn follows_the_prerequisites() {
        let d = |s: &str| DeviceEvent::Disconnected(s.into());
        let s = |s: &str| DeviceEvent::Status(s.into());
        assert_eq!(after(&[d("cannot reach Apple Mobile Device Service on 127.0.0.1:27015")]), 0);
        assert_eq!(after(&[d("no iPhone on USB: plug it in, unlock it and tap Trust")]), 1);
        assert_eq!(after(&[d("Developer Mode is off: Settings > Privacy & Security > Developer Mode")]), 2);
        assert_eq!(after(&[s("Installing the Developer Disk Image…")]), 3);
        // "Connecting…" never moves the checklist backwards.
        assert_eq!(after(&[d("Developer Mode is off: x"), s("Connecting…")]), 2);
        let w = Welcome { active: 2 };
        assert_eq!(w.states(), [StepState::Done, StepState::Done, StepState::Active, StepState::Pending]);
    }
}
