//! The virtual monitor that the Virtual Display Driver adds: finding it,
//! and putting it on the desktop only while the iPad is a display.
//!
//! The driver's adapter shows up in EnumDisplayDevices with the device
//! string "Virtual Display Driver"; its GDI name (\\.\DISPLAYn) identifies
//! the DXGI output to duplicate.
//!
//! Its monitor is always "connected", so left alone it is a permanent
//! extra screen that windows can wander onto while nothing shows it.
//! `attach` and `detach` add it to and remove it from the desktop with
//! ChangeDisplaySettingsEx (no driver commands, no admin rights). Where it
//! sat is kept in a file, so it comes back in the same place, at the size
//! last used.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use windows::{
    Win32::{
        Foundation::{LPARAM, POINTL, RECT},
        Graphics::Gdi::{
            CDS_NORESET, CDS_TYPE, CDS_UPDATEREGISTRY, ChangeDisplaySettingsExW, DEVMODEW, DISP_CHANGE,
            DISP_CHANGE_SUCCESSFUL, DISPLAY_DEVICEW, DM_PELSHEIGHT, DM_PELSWIDTH, DM_POSITION, ENUM_CURRENT_SETTINGS,
            ENUM_DISPLAY_SETTINGS_MODE, EnumDisplayDevicesW, EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW,
            HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
        },
    },
    core::{BOOL, HSTRING, PCWSTR},
};

/// What the driver calls its display adapter.
const DRIVER_ADAPTER: &str = "Virtual Display Driver";

#[derive(Debug, Clone)]
pub struct VirtualScreen {
    /// GDI device name, e.g. `\\.\DISPLAY10`.
    pub device: String,
    /// Desktop rectangle in physical pixels (when the process is DPI aware).
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

fn wide(s: &[u16]) -> String {
    String::from_utf16_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())])
}

/// Whether the Virtual Display Driver's adapter is present (installed and
/// enabled), with or without its monitor on the desktop.
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

/// The driver's GDI device name, and whether it is on the desktop.
fn driver_device() -> Result<(String, bool)> {
    match adapters().into_iter().find(|(_, name, _)| name == DRIVER_ADAPTER) {
        Some((device, _, attached)) => Ok((device, attached)),
        None => bail!("the Virtual Display Driver is not installed"),
    }
}

/// Whether the virtual monitor is part of the desktop now.
pub fn is_attached() -> bool {
    driver_device().is_ok_and(|(_, attached)| attached)
}

/// Every monitor on the desktop: (GDI device name, rectangle).
fn monitors() -> Vec<(String, RECT)> {
    unsafe extern "system" fn each(hmon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        unsafe {
            let out = &mut *(data.0 as *mut Vec<(String, RECT)>);
            let mut mi = MONITORINFOEXW::default();
            mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            if GetMonitorInfoW(hmon, &mut mi as *mut _ as *mut MONITORINFO).as_bool() {
                out.push((wide(&mi.szDevice), mi.monitorInfo.rcMonitor));
            }
            BOOL(1)
        }
    }
    let mut out: Vec<(String, RECT)> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

/// The virtual monitor, if the driver is installed and its monitor is part
/// of the desktop.
pub fn find() -> Result<VirtualScreen> {
    let (device, attached) = driver_device()?;
    if !attached {
        bail!("the virtual monitor ({device}) is not part of the desktop");
    }
    let Some((_, r)) = monitors().into_iter().find(|(d, _)| *d == device) else { bail!("no monitor for {device}") };
    Ok(VirtualScreen { device, left: r.left, top: r.top, width: r.right - r.left, height: r.bottom - r.top })
}

/// Where the monitor was last: "x y width height".
fn place_file() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Mirrorly").join("virtual-display.txt")
}

fn parse_place(text: &str) -> Option<(i32, i32, u32, u32)> {
    let v: Vec<i64> = text.split_whitespace().filter_map(|t| t.parse().ok()).collect();
    match v[..] {
        [x, y, w, h] if w > 0 && h > 0 => Some((x as i32, y as i32, w as u32, h as u32)),
        _ => None,
    }
}

fn devmode() -> DEVMODEW {
    DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() }
}

/// The modes the driver offers for its monitor.
fn modes(device: &str) -> Vec<DEVMODEW> {
    let name = HSTRING::from(device);
    let mut out = Vec::new();
    for i in 0.. {
        let mut dm = devmode();
        if !unsafe { EnumDisplaySettingsW(&name, ENUM_DISPLAY_SETTINGS_MODE(i), &mut dm) }.as_bool() {
            break;
        }
        out.push(dm);
    }
    out
}

fn check(r: DISP_CHANGE, what: &str) -> Result<()> {
    if r != DISP_CHANGE_SUCCESSFUL {
        bail!("{what}: ChangeDisplaySettingsEx returned {}", r.0);
    }
    Ok(())
}

/// Stage a change for one monitor, then apply what is staged.
fn apply(device: &str, dm: &DEVMODEW, what: &str) -> Result<()> {
    unsafe {
        let name = HSTRING::from(device);
        check(ChangeDisplaySettingsExW(&name, Some(dm), None, CDS_UPDATEREGISTRY | CDS_NORESET, None), what)?;
        check(ChangeDisplaySettingsExW(PCWSTR::null(), None, None, CDS_TYPE(0), None), what)
    }
}

/// Put the virtual monitor on the desktop: where it was last, at `prefer`
/// (the iPad's own resolution) when the driver offers that size, else the
/// size last used, else its largest. The first time it goes to the right
/// of the other monitors.
pub fn attach(prefer: Option<(u32, u32)>) -> Result<VirtualScreen> {
    let (device, attached) = driver_device()?;
    if !attached {
        let saved = std::fs::read_to_string(place_file()).ok().and_then(|t| parse_place(&t));
        let offered = modes(&device);
        let pick = |w: u32, h: u32| {
            offered
                .iter()
                .filter(|m| m.dmPelsWidth == w && m.dmPelsHeight == h)
                .max_by_key(|m| (m.dmDisplayFrequency.min(60), m.dmBitsPerPel))
        };
        let mode = prefer
            .and_then(|(w, h)| pick(w, h))
            .or_else(|| saved.and_then(|(_, _, w, h)| pick(w, h)))
            .or_else(|| offered.iter().max_by_key(|m| (m.dmPelsWidth as u64 * m.dmPelsHeight as u64, m.dmDisplayFrequency.min(60))))
            .copied()
            .context("the virtual monitor offers no display modes")?;
        let at = match saved {
            Some((x, y, _, _)) => POINTL { x, y },
            None => POINTL { x: monitors().iter().map(|(_, r)| r.right).max().unwrap_or(0), y: 0 },
        };
        let mut dm = mode;
        dm.Anonymous1.Anonymous2.dmPosition = at;
        dm.dmFields |= DM_POSITION | DM_PELSWIDTH | DM_PELSHEIGHT;
        apply(&device, &dm, "attach the virtual monitor")?;
        tracing::info!("virtual monitor on: {}x{} at ({}, {})", dm.dmPelsWidth, dm.dmPelsHeight, at.x, at.y);
    }
    // The desktop takes a moment to include it.
    let start = Instant::now();
    loop {
        match find() {
            Ok(s) => return Ok(s),
            Err(e) if start.elapsed() > Duration::from_secs(3) => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Take the virtual monitor off the desktop (Windows moves its windows to
/// the other monitors), remembering where it was.
pub fn detach() -> Result<()> {
    let (device, attached) = driver_device()?;
    if !attached {
        return Ok(());
    }
    let mut cur = devmode();
    if unsafe { EnumDisplaySettingsW(&HSTRING::from(device.as_str()), ENUM_CURRENT_SETTINGS, &mut cur) }.as_bool() {
        let p = unsafe { cur.Anonymous1.Anonymous2.dmPosition };
        let file = place_file();
        let place = format!("{} {} {} {}\n", p.x, p.y, cur.dmPelsWidth, cur.dmPelsHeight);
        if let Err(e) = std::fs::create_dir_all(file.parent().unwrap_or(&file)).and_then(|_| std::fs::write(&file, place)) {
            tracing::warn!("cannot save where the virtual monitor was: {e}");
        }
    }
    // Width and height 0 take it off the desktop.
    let mut dm = devmode();
    dm.dmFields = DM_POSITION | DM_PELSWIDTH | DM_PELSHEIGHT;
    apply(&device, &dm, "detach the virtual monitor")?;
    tracing::info!("virtual monitor off");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_place() {
        assert_eq!(parse_place("1920 0 2360 1640\n"), Some((1920, 0, 2360, 1640)));
        assert_eq!(parse_place("-2360 -200 2360 1640"), Some((-2360, -200, 2360, 1640)));
        assert_eq!(parse_place("1920 0 0 0"), None, "a detached size is no place");
        assert_eq!(parse_place("junk"), None);
    }
}
