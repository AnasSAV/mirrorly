//! Plain-language status pages for connection events.

use device::DeviceEvent;

pub type Page = (String, String);

fn page(title: &str, detail: &str) -> Page {
    (title.to_string(), detail.to_string())
}

pub fn starting() -> Page {
    page("Starting Mirrorly…", "")
}

/// What to show for a connection event, if it changes the page.
pub fn describe(ev: &DeviceEvent) -> Option<Page> {
    match ev {
        DeviceEvent::Connected(_) | DeviceEvent::Clipboard(_) => None,
        DeviceEvent::Status(s) => Some(progress(s)),
        DeviceEvent::Disconnected(reason) if reason == device::USER_DISCONNECT => Some(page(
            "Disconnected",
            "Mirrorly has let go of your iPhone. It stays disconnected, even over USB or Wi-Fi, until you connect again.",
        )),
        DeviceEvent::Disconnected(reason) if reason.contains(device::conn::SEVERAL_ON_USB) => {
            Some(page("Choose an iPhone", "More than one iPhone is plugged in. Pick the one to mirror."))
        }
        DeviceEvent::Disconnected(reason) => Some(problem(reason)),
    }
}

/// The page for a progress message (used when connecting by hand).
pub fn progress_page(s: &str) -> Page {
    progress(s)
}

fn progress(s: &str) -> Page {
    if s.starts_with("Looking for the iPhone on Wi-Fi") {
        page("Looking for your iPhone…", "Keep your iPhone unlocked and on the same Wi-Fi network as this PC.")
    } else if let Some(rest) = s.strip_prefix("Connecting over Wi-Fi") {
        page("Connecting over Wi-Fi…", rest.trim_matches(|c: char| c == ' ' || c == '(' || c == ')' || c == '…'))
    } else if s.starts_with("Installing the Developer Disk Image") {
        page("Preparing your iPhone…", "Setting up developer services on your iPhone. The first time takes a little longer.")
    } else if s.starts_with("Setting up Wi-Fi") {
        page("Setting up Wi-Fi…", "One-time pairing, so Mirrorly can also connect without a cable.")
    } else {
        page("Connecting to iPhone…", "Plug in your iPhone with a USB cable and unlock it.")
    }
}

fn problem(reason: &str) -> Page {
    let r = reason.to_ascii_lowercase();
    if r.contains("apple mobile device service") || r.contains("27015") {
        page(
            "Apple Devices is required",
            "Install the Apple Devices app from the Microsoft Store (it provides the iPhone USB driver), then plug your iPhone in again.",
        )
    } else if r.contains("developer mode is off") {
        page(
            "Turn on Developer Mode",
            "On your iPhone: Settings › Privacy & Security › Developer Mode › On. The iPhone restarts; unlock it and tap Turn On.",
        )
    } else if r.contains("no iphone on usb") {
        // Before the Trust rule: this message also says "tap Trust".
        page(
            "Connect your iPhone",
            "Plug it in with a USB cable and unlock it. After the first USB connection, Mirrorly also finds it on the same Wi-Fi.",
        )
    } else if r.contains("pairing record") || r.contains("trust") || r.contains("lockdown session") {
        page("Trust this computer", "Unlock your iPhone and tap Trust when it asks, then enter your passcode.")
    } else if r.contains("phone locked") || r.contains("no video for") {
        page("Unlock your iPhone", "Mirroring continues as soon as your iPhone is unlocked.")
    } else if r.contains("camera or microphone") || r.contains("9022") {
        page("iPhone is busy", "Close Camera or any app using the camera or microphone, then try again.")
    } else if r.contains("switching to usb") {
        page("Switching to USB…", "")
    } else if r.contains("disk image installed") {
        page("Preparing your iPhone…", "Almost ready.")
    } else {
        page("Reconnecting…", reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn title(ev: DeviceEvent) -> String {
        describe(&ev).unwrap().0
    }

    #[test]
    fn user_disconnect_is_not_a_problem() {
        let ev = DeviceEvent::Disconnected(device::USER_DISCONNECT.into());
        assert_eq!(title(ev.clone()), "Disconnected");
        // The first-run checklist does not move backwards for it.
        let mut w = crate::onboarding::Welcome::default();
        let before = w.states();
        w.on_event(&ev);
        assert_eq!(w.states(), before);
    }

    #[test]
    fn maps_known_problems() {
        let d = |s: &str| DeviceEvent::Disconnected(s.into());
        assert_eq!(title(d("cannot reach Apple Mobile Device Service on 127.0.0.1:27015")), "Apple Devices is required");
        assert_eq!(title(d("Developer Mode is off: Settings > Privacy & Security > Developer Mode")), "Turn on Developer Mode");
        assert_eq!(title(d("no video for 5 s (is the phone locked?)")), "Unlock your iPhone");
        assert_eq!(title(d("no iPhone on USB: plug it in, unlock it and tap Trust")), "Connect your iPhone");
        assert_eq!(
            title(d("no iPhone on USB, and none found on Wi-Fi (same network? phone unlocked? Windows network set to Private?)")),
            "Connect your iPhone"
        );
        assert_eq!(title(d("something unexpected")), "Reconnecting…");
    }

    #[test]
    fn maps_progress() {
        let s = |s: &str| DeviceEvent::Status(s.into());
        assert_eq!(title(s("Connecting…")), "Connecting to iPhone…");
        assert_eq!(title(s("Looking for the iPhone on Wi-Fi…")), "Looking for your iPhone…");
        let p = describe(&s("Connecting over Wi-Fi (192.168.1.5)…")).unwrap();
        assert_eq!(p, ("Connecting over Wi-Fi…".to_string(), "192.168.1.5".to_string()));
    }
}
