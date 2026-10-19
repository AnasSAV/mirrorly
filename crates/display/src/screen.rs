//! Finding the virtual monitor that the Virtual Display Driver adds.
//!
//! The driver's adapter shows up in EnumDisplayDevices with the device
//! string "Virtual Display Driver"; its GDI name (\\.\DISPLAYn) identifies
//! the DXGI output to duplicate and the monitor rectangle touches map onto.

use anyhow::{Result, bail};
use windows::{
    Win32::{
        Foundation::{LPARAM, RECT},
        Graphics::Gdi::{
            DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
            MONITORINFOEXW,
        },
    },
    core::{BOOL, PCWSTR},
};

/// What the driver calls its display adapter.
const DRIVER_ADAPTER: &str = "Virtual Display Driver";

#[derive(Debug, Clone)]
pub struct VirtualScreen {
    /// GDI device name, e.g. `\\.\DISPLAY10`.
    pub device: String,
    /// Desktop rectangle in physical pixels (the process is DPI aware).
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

fn wide(s: &[u16]) -> String {
    String::from_utf16_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())])
}

/// Whether the Virtual Display Driver's adapter is present (installed and
/// enabled), with or without a monitor attached to the desktop.
pub fn driver_present() -> bool {
    adapters().iter().any(|(_, name, _)| name == DRIVER_ADAPTER)
}

/// (GDI device name, adapter name, attached to the desktop)
fn adapters() -> Vec<(String, String, bool)> {
    let mut out = Vec::new();
    for i in 0.. {
        let mut dd = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), i, &mut dd, 0) }.as_bool() {
            break;
        }
        const ATTACHED_TO_DESKTOP: u32 = 0x1;
        out.push((wide(&dd.DeviceName), wide(&dd.DeviceString), dd.StateFlags.0 & ATTACHED_TO_DESKTOP != 0));
    }
    out
}

/// The virtual monitor, if the driver is installed and its monitor is part
/// of the desktop.
pub fn find() -> Result<VirtualScreen> {
    let Some((device, _, attached)) = adapters().into_iter().find(|(_, name, _)| name == DRIVER_ADAPTER) else {
        bail!("the Virtual Display Driver is not installed");
    };
    if !attached {
        bail!("the virtual monitor ({device}) is not part of the desktop: extend the desktop to it in Settings › Display");
    }
    unsafe extern "system" fn each(hmon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        unsafe {
            let (want, out) = &mut *(data.0 as *mut (String, Option<RECT>));
            let mut mi = MONITORINFOEXW::default();
            mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            if GetMonitorInfoW(hmon, &mut mi as *mut _ as *mut MONITORINFO).as_bool() && wide(&mi.szDevice) == *want {
                *out = Some(mi.monitorInfo.rcMonitor);
            }
            BOOL(1)
        }
    }
    let mut data: (String, Option<RECT>) = (device.clone(), None);
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut data as *mut _ as isize));
    }
    let Some(r) = data.1 else { bail!("no monitor for {device}") };
    Ok(VirtualScreen { device, left: r.left, top: r.top, width: r.right - r.left, height: r.bottom - r.top })
}
