//! The notification-area icon, while the iPad is a display and the Mirrorly
//! window is out of the way. Clicking it opens a small menu (stop, quit).
//!
//! Its messages arrive at the Mirrorly window, through a subclass of the
//! window procedure (the window may be hidden; it still gets them).

use anyhow::{Result, bail};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Shell::{
                DefSubclassProc, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
                NOTIFYICONDATAW, RemoveWindowSubclass, SetWindowSubclass, Shell_NotifyIconW,
            },
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, LoadIconW, MF_SEPARATOR, MF_STRING, PostMessageW,
                SetForegroundWindow, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP, WM_LBUTTONUP,
                WM_NULL, WM_RBUTTONUP,
            },
        },
    },
    core::{PCWSTR, w},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    StopDisplay,
    Quit,
}

const WM_TRAY: u32 = WM_APP + 0x31;
const ICON_ID: u32 = 1;
const SUBCLASS_ID: usize = 0x4D54_5259; // "MTRY"
const CMD_STOP: i32 = 1;
const CMD_QUIT: i32 = 2;

type Callback = Box<dyn Fn(TrayAction)>;

pub struct Tray {
    owner: HWND,
    callback: *mut Callback,
}

fn copy_into(dst: &mut [u16], s: &str) {
    let room = dst.len() - 1;
    for (d, c) in dst.iter_mut().take(room).zip(s.encode_utf16().chain(std::iter::once(0))) {
        *d = c;
    }
}

unsafe extern "system" fn tray_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, data: usize) -> LRESULT {
    unsafe {
        if msg == WM_TRAY && data != 0 {
            let ev = (lp.0 & 0xFFFF) as u32;
            if (ev == WM_LBUTTONUP || ev == WM_RBUTTONUP)
                && let Some(action) = menu(hwnd)
            {
                (*(data as *const Callback))(action);
            }
            return LRESULT(0);
        }
        DefSubclassProc(hwnd, msg, wp, lp)
    }
}

/// The icon's menu, at the pointer.
fn menu(hwnd: HWND) -> Option<TrayAction> {
    unsafe {
        let m = CreatePopupMenu().ok()?;
        let _ = AppendMenuW(m, MF_STRING, CMD_STOP as usize, w!("Stop using iPad as a display"));
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(m, MF_STRING, CMD_QUIT as usize, w!("Quit Mirrorly"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Without this the menu stays open when clicking elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(m);
        match cmd.0 {
            CMD_STOP => Some(TrayAction::StopDisplay),
            CMD_QUIT => Some(TrayAction::Quit),
            _ => None,
        }
    }
}

impl Tray {
    /// Add the icon; its menu choices go to `on_action`.
    pub fn new(owner: isize, tip: &str, on_action: impl Fn(TrayAction) + 'static) -> Result<Self> {
        let owner = HWND(owner as _);
        let callback: *mut Callback = Box::into_raw(Box::new(Box::new(on_action)));
        unsafe {
            let _ = SetWindowSubclass(owner, Some(tray_proc), SUBCLASS_ID, callback as usize);
            let mut nid = Self::data(owner);
            nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            nid.uCallbackMessage = WM_TRAY;
            // The app icon (resource 1, as for the window).
            nid.hIcon = LoadIconW(GetModuleHandleW(None).ok().map(Into::into), PCWSTR(1 as _)).unwrap_or_default();
            copy_into(&mut nid.szTip, tip);
            if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                let _ = RemoveWindowSubclass(owner, Some(tray_proc), SUBCLASS_ID);
                drop(Box::from_raw(callback));
                bail!("could not add the notification-area icon");
            }
        }
        Ok(Self { owner, callback })
    }

    fn data(owner: HWND) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: owner, uID: ICON_ID, ..Default::default() }
    }

    /// A one-off notification from the icon.
    pub fn notify(&self, title: &str, text: &str) {
        let mut nid = Self::data(self.owner);
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = NIIF_INFO;
        copy_into(&mut nid.szInfoTitle, title);
        copy_into(&mut nid.szInfo, text);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &Self::data(self.owner));
            let _ = RemoveWindowSubclass(self.owner, Some(tray_proc), SUBCLASS_ID);
            drop(Box::from_raw(self.callback));
        }
    }
}
