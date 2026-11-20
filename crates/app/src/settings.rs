//! The settings menu: the gear button on the bar, or a right-click on the
//! bar. For now it holds one choice: which app on the phone keeps the
//! documents dropped on it (iOS only lets other programs save into apps
//! that share files with a computer, see `device::docs`).

use device::docs::Host;
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, POINT, WPARAM},
        UI::WindowsAndMessaging::{
            AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, MENU_ITEM_FLAGS, MF_CHECKED, MF_GRAYED, MF_SEPARATOR,
            MF_STRING, PostMessageW, SetForegroundWindow, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu,
            WM_NULL,
        },
    },
    core::{HSTRING, PCWSTR},
};

/// What the user picked for documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// Let Mirrorly pick (VLC or Chrome first).
    Automatic,
    App(Host),
}

const CMD_AUTO: i32 = 1;
const CMD_APP: i32 = 100;

/// Which item is ticked: `None` for Automatic, or the app's index. A chosen
/// app this phone doesn't have counts as Automatic here.
fn ticked(apps: &[Host], chosen: Option<&str>) -> Option<usize> {
    chosen.and_then(|id| apps.iter().position(|h| h.bundle_id == id))
}

/// Show the menu at the pointer and return the pick. `apps` is `None`
/// while no phone is connected.
pub fn show(hwnd: isize, apps: Option<&[Host]>, chosen: Option<&str>) -> Option<Choice> {
    let hwnd = HWND(hwnd as _);
    unsafe {
        let m = CreatePopupMenu().ok()?;
        let add = |flags: MENU_ITEM_FLAGS, id: i32, text: &str| {
            let _ = AppendMenuW(m, flags, id as usize, &HSTRING::from(text));
        };
        let note = |text: &str| add(MF_STRING | MF_GRAYED, 0, text);
        note("Save documents dropped on the phone in:");
        match apps {
            None => note("    (connect your iPhone or iPad to choose)"),
            Some([]) => note("    (no app on it keeps files: install VLC or Chrome)"),
            Some(apps) => {
                let tick = ticked(apps, chosen);
                let check = |on: bool| if on { MF_CHECKED } else { MENU_ITEM_FLAGS(0) };
                add(MF_STRING | check(tick.is_none()), CMD_AUTO, &format!("Automatic ({})", apps[0].name));
                for (i, app) in apps.iter().enumerate() {
                    add(MF_STRING | check(tick == Some(i)), CMD_APP + i as i32, &app.name);
                }
                let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
                note("They go in a Mirrorly folder: Files › On My iPhone (or iPad) › the app.");
                note("Only apps that share files with a computer can be listed.");
            }
        }
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Without this the menu stays open when clicking elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(m);
        match cmd.0 {
            CMD_AUTO => Some(Choice::Automatic),
            n if n >= CMD_APP => apps?.get((n - CMD_APP) as usize).cloned().map(Choice::App),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_the_chosen_app_or_automatic() {
        let host = |id: &str| Host { bundle_id: id.into(), name: id.into() };
        let apps = [host("org.videolan.vlc-ios"), host("com.example.notes")];
        assert_eq!(ticked(&apps, None), None);
        assert_eq!(ticked(&apps, Some("com.example.notes")), Some(1));
        assert_eq!(ticked(&apps, Some("com.example.gone")), None, "not on this phone: Automatic");
    }
}
