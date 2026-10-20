//! Mirrorly: mirror and control an iPhone from Windows.
//!
//! Threads: main (winit window + input mapping), network (device crate:
//! tunnel, RTP receive, HID send), decode (FFmpeg d3d11va), present
//! (swapchain). Input goes window -> network directly and never waits on
//! decode or present.
//!
//! The window follows macOS's iPhone Mirroring: just the phone screen with
//! rounded corners, a bar that appears when the pointer nears the top
//! (window buttons, Home, App Switcher, Rotate), and a status page when
//! there is nothing to show.
//!
//! Files move through the iPhone's clipboard: copy photos or files on the
//! iPhone and a chip on the bar can be dragged out to Windows; drop PC files
//! on the window and they are pasted into the app open on the iPhone. Text
//! and images copied on Windows go onto the iPhone's clipboard as they are
//! copied; files copied in Explorer go when Ctrl+V is pressed in Mirrorly.
//! The other way, text, photos and files copied on the iPhone land on the
//! Windows clipboard.
//!
//! Screenshots and recordings of the phone's screen go to Pictures and
//! Videos (record.rs); a recording is the phone's own stream, not
//! re-encoded.
//!
//! An iPad can also be a second monitor (the display crate): mirroring it
//! then stops, and once its Safari shows the display this window steps
//! aside to a notification-area icon.

// Release builds are a GUI app: no console window (logs go to a file).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bar;
mod drag;
mod gpu;
mod layout;
mod metrics;
mod onboarding;
mod present;
mod record;
mod render;
mod sink;
mod state;
mod status;
mod tray;
mod ui;
mod winclip;

use std::{
    collections::HashSet,
    sync::{Arc, atomic::Ordering::Relaxed, mpsc},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use device::{
    DeviceEvent, DeviceHandle, RctlMode,
    clipboard::{ClipEvent, PushContent},
};
use input::{Button, HidEvent, gesture::Gesture, keys::hid_usage};
use video::{D3dLock, FrameSlot};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    keyboard::{KeyCode, PhysicalKey},
    platform::windows::{IconExtWindows, MonitorHandleExtWindows, WindowAttributesExtWindows},
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Icon, Window, WindowId},
};

use crate::{
    layout::Control,
    state::{Indicator, Shared, Shelf},
};

#[derive(Parser, Debug)]
#[command(name = "Mirrorly", version, about = "Mirror and control an iPhone from Windows")]
struct Args {
    /// Present with vsync (sync interval 1) instead of immediately.
    #[arg(long)]
    vsync: bool,
    /// RTCP RCTL report layout sent to the phone's encoder.
    #[arg(long, value_enum, default_value_t = Rctl::Pmd3)]
    rctl: Rctl,
    /// Do not install the Developer Disk Image automatically.
    #[arg(long)]
    no_auto_ddi: bool,
    /// USB only: no Wi-Fi fallback and no Wi-Fi pairing.
    #[arg(long)]
    no_wifi: bool,
    /// Start with the latency overlay shown (F10 toggles it).
    #[arg(long)]
    overlay: bool,
    /// Write one line of stage timestamps per presented frame to this CSV.
    #[arg(long)]
    timings_csv: Option<std::path::PathBuf>,
    /// Open a console window and log there instead of the log file.
    #[arg(long)]
    console: bool,
    /// Internal: the one-time iPad display setup, run elevated by Mirrorly
    /// itself (the iPad's resolution, e.g. 2360x1640).
    #[arg(long = "setup-display", hide = true, value_name = "WxH")]
    setup_display: Option<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Rctl {
    Pmd3,
    Idevice,
    Off,
}

/// The app epoch as both an `Instant` and a raw QueryPerformanceCounter
/// value (read back to back), so DXGI's QPC timestamps can be put on the
/// same µs clock as everything else.
struct Epoch {
    instant: Instant,
    qpc: i64,
    qpc_freq: i64,
}

fn epoch() -> &'static Epoch {
    static E: std::sync::OnceLock<Epoch> = std::sync::OnceLock::new();
    E.get_or_init(|| {
        use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
        let (mut qpc, mut freq) = (0i64, 0i64);
        let instant = Instant::now();
        unsafe {
            let _ = QueryPerformanceCounter(&mut qpc);
            let _ = QueryPerformanceFrequency(&mut freq);
        }
        Epoch { instant, qpc, qpc_freq: freq.max(1) }
    })
}

/// µs on one QueryPerformanceCounter-backed clock shared by every thread.
fn now_us() -> u64 {
    epoch().instant.elapsed().as_micros() as u64
}

/// A raw QPC value (e.g. DXGI's SyncQPCTime) on the `now_us` clock.
fn qpc_to_us(qpc: i64) -> u64 {
    let e = epoch();
    (((qpc - e.qpc).max(0) as i128 * 1_000_000) / e.qpc_freq as i128) as u64
}

#[link(name = "winmm")]
unsafe extern "system" {
    fn timeBeginPeriod(period_ms: u32) -> u32;
}

#[derive(Debug)]
enum UserEvent {
    Device(DeviceEvent),
    /// The iPhones that can be chosen changed.
    Phones(Vec<device::phones::Phone>),
    /// Mouse input on the hover bar.
    Bar(bar::BarInput),
    /// The Windows clipboard changed.
    WinClipboard,
    /// iPhone files are on the Windows clipboard now (its sequence number).
    ClipWritten(Option<u32>),
    /// A choice from the notification-area icon's menu.
    Tray(tray::TrayAction),
    /// The iPad display setup finished: Ok(false) if permission was refused.
    DisplaySetup(Result<bool, String>),
    /// A screenshot or recording was saved (what to say), or not.
    Saved(Result<String, String>),
}

/// Probe taps: count, pause before each (screen must be still), hold.
const PROBE_TAPS: u32 = 10;
const PROBE_GAP: Duration = Duration::from_millis(1200);
const PROBE_HOLD: Duration = Duration::from_millis(80);

/// Window sizes, as a fraction of the monitor (the green light and
/// Ctrl+wheel step through them).
const SIZES: [f64; 4] = [0.55, 0.7, 0.85, 0.95];
const DEFAULT_SIZE: usize = 2;
/// Dark frame around the screen, like the edge of the phone (logical px).
const BEZEL: f64 = 6.0;
/// The bar stays this long after the pointer leaves the window.
const BAR_LINGER: Duration = Duration::from_millis(600);
/// While the display page shows, how often to check for the iPad.
const DISPLAY_POLL: Duration = Duration::from_millis(300);
/// Opening the display on the iPad by itself: give up after this, and
/// leave it to the user.
const OPEN_TIMEOUT: Duration = Duration::from_secs(25);
/// The tap that gives the page full screen: held this long, then mirroring
/// stops this long after it.
const TAP_HOLD: Duration = Duration::from_millis(80);
const TAP_SETTLE: Duration = Duration::from_millis(700);

/// Mirrorly opening the display page on the iPad itself, while it still
/// mirrors (and so can type and tap there).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Opening {
    /// Safari is being brought up and the address typed, since then.
    Typing(Instant),
    /// The page connected; tapped at that moment (released yet?).
    Tapped(Instant, bool),
}
/// One wheel notch moves the finger this far (fraction of the screen height).
const SCROLL_PER_NOTCH: f64 = 0.12;
/// A wheel scroll gesture lasts about this long; notches arriving meanwhile
/// are added to the next one.
const SCROLL_BUSY: Duration = Duration::from_millis(130);

struct App {
    args: Args,
    proxy: EventLoopProxy<UserEvent>,
    shared: Arc<Shared>,
    slot: Arc<FrameSlot>,
    window: Option<Arc<Window>>,
    hwnd: isize,
    device: Option<DeviceHandle>,
    gesture: Gesture,
    cursor: (f64, f64),
    cursor_inside: bool,
    keys_down: HashSet<u16>,
    modifiers: winit::keyboard::ModifiersState,
    phone: Option<String>,
    link: Option<&'static str>,
    size_level: usize,
    /// Hover bar: a window of its own above the screen, shown while the
    /// pointer is over the mirror or the bar.
    bar: Option<bar::Bar>,
    /// Pointer position on the bar, while it is there.
    bar_cursor: Option<(f64, f64)>,
    hide_bar_at: Option<Instant>,
    scroll_busy_until: Instant,
    scroll_pending: f64,
    /// Fetched from the iPhone's clipboard, offered on the bar's chip.
    shelf_files: Vec<std::path::PathBuf>,
    /// Files dropped on the window, sent together once the drop is over.
    dropped: Vec<std::path::PathBuf>,
    toast_until: Option<Instant>,
    /// Windows clipboard sequence number last put on the iPhone, and a hash
    /// of that content (apps often announce one copy twice).
    clip_sent: u32,
    clip_sent_hash: Option<u64>,
    /// When the Windows / iPhone clipboard last changed: the newer one is
    /// what a paste on the iPhone should get.
    win_copy_at: Option<Instant>,
    phone_copy_at: Option<Instant>,
    /// Mirrorly is writing iPhone files to the Windows clipboard (on a
    /// helper thread): that change is not a Windows copy.
    clip_writing: bool,
    /// The iPad as a second monitor, while it is one.
    display: Option<display::DisplaySession>,
    /// Its notification-area icon (the window is hidden meanwhile).
    tray: Option<tray::Tray>,
    /// The window stepped aside for the display.
    display_hidden: bool,
    /// The one-time display setup is running (administrator prompt).
    display_setup: bool,
    /// Opening the page on the iPad, before mirroring stops.
    opening: Option<Opening>,
    /// The screen being recorded.
    recording: Option<record::Recording>,
    /// The GPU the picture is decoded on, and the lock its context needs
    /// (screenshots read the picture back).
    gpu: Option<(windows::Win32::Graphics::Direct3D11::ID3D11Device, windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext, Arc<D3dLock>)>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl App {
    fn send(&self, ev: HidEvent) {
        if let Some(d) = self.device.as_ref().filter(|d| d.wants_connection()) {
            d.send(now_us(), ev);
        }
    }

    fn set_title(&self) {
        if let Some(w) = &self.window {
            let mut t = match (&self.phone, self.link) {
                (Some(p), Some(l)) => format!("Mirrorly — {p} · {l}"),
                (Some(p), None) => format!("Mirrorly — {p}"),
                _ => "Mirrorly".to_string(),
            };
            if self.recording.is_some() {
                t.push_str(" · Recording");
            }
            w.set_title(&t);
        }
    }

    fn scale(&self) -> f64 {
        self.window.as_ref().map(|w| w.scale_factor()).unwrap_or(1.0)
    }

    fn start(&mut self, el: &ActiveEventLoop) -> Result<()> {
        // The iPad's virtual monitor is only on the desktop while it is a
        // display; one left on (crash, power cut) is taken off.
        if display::monitor_on()
            && let Err(e) = display::monitor_off()
        {
            tracing::warn!("{e:#}");
        }
        let attrs = Window::default_attributes()
            .with_title("Mirrorly")
            .with_decorations(false)
            .with_resizable(false)
            .with_window_icon(Icon::from_resource(1, None).ok())
            .with_class_name(WINDOW_CLASS)
            .with_inner_size(PhysicalSize::new(400u32, 866u32));
        let window = Arc::new(el.create_window(attrs)?);
        self.hwnd = match window.window_handle()?.as_raw() {
            RawWindowHandle::Win32(h) => h.hwnd.get(),
            _ => anyhow::bail!("not a Win32 window"),
        };
        self.window = Some(window.clone());
        {
            let mut c = self.shared.chrome.lock().unwrap();
            c.status = status::starting();
            if onboarding::first_run() {
                c.welcome = Some(onboarding::Welcome::default());
            }
        }
        self.shared.view.lock().unwrap().bezel = BEZEL * window.scale_factor();
        self.apply_shape();
        self.apply_size();
        let bar_proxy = self.proxy.clone();
        self.bar = Some(bar::Bar::new(self.hwnd, window.scale_factor(), move |input| {
            let _ = bar_proxy.send_event(UserEvent::Bar(input));
        })?);
        self.place_bar();

        // GPU on the adapter that drives this window's monitor.
        let hmon = window.current_monitor().map(|m| windows::Win32::Graphics::Gdi::HMONITOR(m.hmonitor() as _));
        let gpu = gpu::Gpu::new(hmon)?;
        tracing::info!("GPU: {} (tearing {})", gpu.adapter_name, gpu.tearing);
        let swapchain = gpu.swapchain(windows::Win32::Foundation::HWND(self.hwnd as _))?;
        let renderer = render::Renderer::new(gpu.device.clone(), gpu.context.clone(), swapchain)?;
        {
            let size = window.inner_size();
            self.shared.view.lock().unwrap().window = (size.width, size.height);
            self.shared.chrome.lock().unwrap().scale = window.scale_factor();
        }

        let lock = Arc::new(D3dLock::default());
        self.gpu = Some((gpu.device.clone(), gpu.context.clone(), lock.clone()));
        let dpi = window.scale_factor();
        let csv = match &self.args.timings_csv {
            Some(path) => Some(spawn_csv_logger(path)?),
            None => None,
        };
        self.shared.overlay.store(self.args.overlay, Relaxed);
        self.threads.push(present::spawn(
            present::SendRenderer(renderer),
            self.slot.clone(),
            self.shared.clone(),
            lock.clone(),
            present::PresentOpts { vsync: self.args.vsync, dot_radius: 14.0 * dpi, dpi_scale: dpi as f32, csv, qpc_to_us },
            now_us,
        )?);

        let (au_tx, au_rx) = mpsc::channel();
        // The decoder's keyframe requests go through the device handle once
        // it exists (it is created below); until then there is nothing to ask.
        let decoder_kf: Arc<std::sync::Mutex<Option<device::KeyframeRequester>>> = Arc::default();
        let decoder_kf2 = decoder_kf.clone();
        let shared = self.shared.clone();
        self.threads.push(video::spawn_decoder(
            gpu.device.clone(),
            lock,
            au_rx,
            self.slot.clone(),
            move || {
                shared.counters.decode_errors.fetch_add(1, Relaxed);
                shared.counters.keyframe_requests.fetch_add(1, Relaxed);
                if let Some(k) = decoder_kf2.lock().unwrap().as_ref() {
                    k.request();
                }
            },
            now_us,
        )?);

        let clip_proxy = self.proxy.clone();
        if let Err(e) = winclip::listen(self.hwnd, move || {
            let _ = clip_proxy.send_event(UserEvent::WinClipboard);
        }) {
            tracing::warn!("cannot watch the Windows clipboard: {e}");
        }

        let phones_proxy = self.proxy.clone();
        device::phones::spawn_watch(move |list| {
            let _ = phones_proxy.send_event(UserEvent::Phones(list));
        })?;

        let proxy = self.proxy.clone();
        let rctl = match self.args.rctl {
            Rctl::Pmd3 => RctlMode::Pmd3,
            Rctl::Idevice => RctlMode::Idevice,
            Rctl::Off => RctlMode::Off,
        };
        let handle = device::start(
            sink::VideoSink::new(au_tx, self.shared.clone()),
            move |ev| {
                let _ = proxy.send_event(UserEvent::Device(ev));
            },
            device::Options {
                auto_install_ddi: !self.args.no_auto_ddi,
                wifi: !self.args.no_wifi,
                rctl,
                hid_settle: Duration::from_millis(500),
                now_us,
            },
        )?;
        *decoder_kf.lock().unwrap() = Some(handle.keyframes.clone());
        *self.shared.hid_stats.lock().unwrap() = Some(handle.hid_stats.clone());
        self.device = Some(handle);
        self.set_title();
        Ok(())
    }

    /// Size the window to the picture's aspect at the current size level.
    fn apply_size(&mut self) {
        let Some(w) = self.window.clone() else { return };
        if w.fullscreen().is_some() {
            return;
        }
        let (mw, mh) = w
            .current_monitor()
            .map(|m| (m.size().width as f64, m.size().height as f64))
            .unwrap_or((1920.0, 1080.0));
        let ((cw, ch), b) = {
            let v = self.shared.view.lock().unwrap();
            (v.content(), v.bezel)
        };
        let f = SIZES[self.size_level];
        let r = input::geom::fit(mw * f - 2.0 * b, mh * f - 2.0 * b, cw, ch);
        let _ = w.request_inner_size(PhysicalSize::new(
            (r.w + 2.0 * b).round() as u32,
            (r.h + 2.0 * b).round() as u32,
        ));
    }

    /// Ask Windows 11 to round the window's corners. (A window clip region
    /// would allow the phone's large radius, but measured: it forces DWM
    /// composition, +9.7 ms Present -> scan-out and 0 % direct flips. The
    /// phone's own screen corners are drawn in the shader instead.)
    fn apply_shape(&self) {
        use windows::Win32::{
            Foundation::HWND,
            Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute},
        };
        if std::env::var_os("MIRROR_SQUARE").is_some() {
            return;
        }
        let pref = DWMWCP_ROUND;
        unsafe {
            let _ = DwmSetWindowAttribute(
                HWND(self.hwnd as _),
                DWMWA_WINDOW_CORNER_PREFERENCE,
                (&pref as *const windows::Win32::Graphics::Dwm::DWM_WINDOW_CORNER_PREFERENCE).cast(),
                std::mem::size_of_val(&pref) as u32,
            );
        }
    }

    fn control(&mut self, el: &ActiveEventLoop, c: Control) {
        match c {
            Control::Close => self.quit(el),
            Control::Minimize => {
                if let Some(w) = &self.window {
                    w.set_minimized(true);
                }
            }
            Control::Zoom => {
                self.size_level = (self.size_level + 1) % SIZES.len();
                self.apply_size();
            }
            Control::Rotate => self.rotate(true),
            Control::AppSwitcher => self.send(self.app_switcher()),
            Control::Home => self.send(HidEvent::Button { button: Button::Home }),
            Control::Shelf => self.drag_shelf(),
            Control::Display => self.start_display(),
            Control::Screenshot => self.screenshot(),
            Control::Record => self.toggle_recording(),
            Control::StopDisplay => self.stop_display(),
            Control::Power => {
                let on = self.shared.chrome.lock().unwrap().disconnected;
                self.set_connected(on);
            }
            Control::Connect => self.set_connected(true),
            Control::Phone(i) => {
                let phone = self.shared.chrome.lock().unwrap().phones.get(i).cloned();
                if let (Some(p), Some(d)) = (phone, &self.device) {
                    tracing::info!("user picks {}", p.name);
                    d.connect_to(Some(p.udid));
                    self.set_connected(true);
                }
            }
            Control::Forget(i) => {
                let phone = self.shared.chrome.lock().unwrap().phones.get(i).cloned();
                if let Some(p) = phone {
                    match device::wifi::forget(&p.udid) {
                        Ok(()) => {
                            tracing::info!("forgot {}", p.name);
                            {
                                let mut c = self.shared.chrome.lock().unwrap();
                                c.phones.retain(|x| x.udid != p.udid || x.usb);
                                if let Some(x) = c.phones.iter_mut().find(|x| x.udid == p.udid) {
                                    x.remembered = false;
                                }
                                c.hover = None;
                            }
                            self.toast(format!("Forgot {}. Plug it in to use it again.", p.name), 4.0);
                        }
                        Err(e) => {
                            tracing::warn!("forget {}: {e:#}", p.name);
                            self.toast("Couldn't forget that iPhone", 3.0);
                        }
                    }
                }
            }
        }
    }

    /// The picker row or Forget button under (x, y), while it is shown.
    fn picker_at(&self, x: f64, y: f64) -> Option<Control> {
        let c = self.shared.chrome.lock().unwrap();
        if !c.disconnected || c.phones.is_empty() || c.display.is_some() {
            return None;
        }
        let (m, _) = self.mapper();
        layout::picker_rows(m.view, self.scale(), c.phones.len()).into_iter().enumerate().find_map(|(i, (row, forget))| {
            if c.phones[i].remembered && layout::inside(forget, x, y) {
                Some(Control::Forget(i))
            } else {
                layout::inside(row, x, y).then_some(Control::Phone(i))
            }
        })
    }

    /// Disconnect from the iPhone (Mirrorly then leaves it alone, even when
    /// it is plugged in or on Wi-Fi), or connect again.
    fn set_connected(&mut self, on: bool) {
        let Some(d) = &self.device else { return };
        tracing::info!("user {}", if on { "connects" } else { "disconnects" });
        d.set_connected(on);
        self.release_all();
        {
            let mut c = self.shared.chrome.lock().unwrap();
            c.disconnected = !on;
            c.hover = None;
            if on {
                c.status = status::progress_page("Connecting…");
            }
        }
        self.slot.request_redraw();
    }

    /// The Connect button, when the disconnected page shows it (no phones
    /// to list).
    fn connect_button_at(&self, x: f64, y: f64) -> bool {
        {
            let c = self.shared.chrome.lock().unwrap();
            if !c.disconnected || !c.phones.is_empty() || c.display.is_some() {
                return false;
            }
        }
        let (m, _) = self.mapper();
        layout::inside(layout::connect_button(m.view, self.scale()), x, y)
    }

    /// The Stop button on the display page.
    fn stop_button_at(&self, x: f64, y: f64) -> bool {
        if self.display.is_none() {
            return false;
        }
        let (m, _) = self.mapper();
        layout::inside(layout::connect_button(m.view, self.scale()), x, y)
    }

    /// Save the phone's screen as it is now, turned as shown (Pictures ›
    /// Mirrorly).
    fn screenshot(&mut self) {
        let Some(frame) = self.slot.latest().filter(|_| self.link.is_some()) else {
            self.toast("Nothing to capture yet", 2.0);
            return;
        };
        let (screen, rotation) = {
            let v = self.shared.view.lock().unwrap();
            (v.screen, v.rotation)
        };
        let (w, h) = (screen.0.min(frame.picture.width) & !1, screen.1.min(frame.picture.height) & !1);
        let picture = match self.read_back(&frame.picture, w, h) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("screenshot: {e:#}");
                self.toast("Couldn't take a screenshot", 3.0);
                return;
            }
        };
        let proxy = self.proxy.clone();
        let _ = std::thread::Builder::new().name("screenshot".into()).spawn(move || {
            let r = record::save_screenshot(&picture, rotation)
                .map(|_| "Screenshot saved to Pictures › Mirrorly".to_string())
                .map_err(|e| format!("screenshot: {e:#}"));
            let _ = proxy.send_event(UserEvent::Saved(r));
        });
    }

    /// The top-left `w` x `h` of a decoded picture, read back as NV12.
    fn read_back(&self, pic: &video::DecodedPicture, w: u32, h: u32) -> Result<record::Nv12> {
        use windows::Win32::Graphics::{
            Direct3D11::{D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING},
            Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC},
        };
        let (device, ctx, lock) = self.gpu.as_ref().context("no GPU yet")?;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            ..Default::default()
        };
        let _g = lock.lock();
        unsafe {
            let mut staging = None;
            device.CreateTexture2D(&desc, None, Some(&mut staging))?;
            let staging = staging.context("staging texture")?;
            // The decoder's texture array: one subresource per slice.
            let area = D3D11_BOX { left: 0, top: 0, front: 0, right: w, bottom: h, back: 1 };
            ctx.CopySubresourceRegion(&staging, 0, 0, 0, 0, &pic.texture, pic.index, Some(&area));
            let mut map = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut map))?;
            let (wu, hu, pitch) = (w as usize, h as usize, map.RowPitch as usize);
            let mut data = vec![0u8; wu * hu * 3 / 2];
            let src = map.pData as *const u8;
            // Y rows, then the interleaved UV rows (which follow the luma
            // plane at h rows of pitch).
            for row in 0..hu + hu / 2 {
                std::ptr::copy_nonoverlapping(src.add(row * pitch), data.as_mut_ptr().add(row * wu), wu);
            }
            ctx.Unmap(&staging, 0);
            Ok(record::Nv12 { data, width: w, height: h })
        }
    }

    /// Start or stop recording the phone's screen (Videos › Mirrorly).
    fn toggle_recording(&mut self) {
        if self.recording.is_some() {
            self.stop_recording();
            return;
        }
        if self.link.is_none() {
            self.toast("Connect your iPhone first", 3.0);
            return;
        }
        let (screen, rotation) = {
            let v = self.shared.view.lock().unwrap();
            (v.screen, v.rotation)
        };
        let proxy = self.proxy.clone();
        let started = record::start(screen.0, screen.1, rotation, move |r| {
            let r = r
                .map(|(_, secs)| format!("Recording saved ({}:{:02}) to Videos › Mirrorly", secs as u64 / 60, secs as u64 % 60))
                .map_err(|e| format!("recording: {e:#}"));
            let _ = proxy.send_event(UserEvent::Saved(r));
        });
        match started {
            Ok(rec) => {
                *self.shared.recording.lock().unwrap() = Some(rec.tx.clone());
                // The file starts at a keyframe: ask for one now.
                if let Some(d) = &self.device {
                    d.keyframes.request();
                }
                self.recording = Some(rec);
                self.toast("Recording. Press F7 or the stop button to finish.", 3.0);
            }
            Err(e) => {
                tracing::warn!("recording: {e:#}");
                self.toast("Couldn't start recording", 3.0);
            }
        }
        self.set_title();
        self.sync_bar();
    }

    /// Finish the recording (the file is completed on its own thread).
    fn stop_recording(&mut self) {
        let Some(rec) = self.recording.take() else { return };
        self.shared.recording.lock().unwrap().take();
        let _ = rec.tx.send(record::Msg::Stop);
        tracing::info!("recording stopped: {}", rec.path.display());
        self.toast("Saving the recording…", 10.0);
        self.set_title();
        self.sync_bar();
    }

    /// "Use as display" is offered while an iPad is connected.
    fn display_offered(&self) -> bool {
        self.display.is_none() && self.link.is_some() && self.shared.view.lock().unwrap().is_tablet()
    }

    /// The iPad becomes a second monitor. Mirroring it stops (pointless
    /// now); this window says where to point its Safari until the iPad
    /// shows the display, then steps aside to a notification-area icon.
    fn start_display(&mut self) {
        if self.display_setup {
            return;
        }
        // The iPad's own resolution, landscape (the panel is reported portrait).
        let (w, h) = self.shared.view.lock().unwrap().screen;
        // First time: the driver and the firewall rule, as administrator.
        if display::setup::needed() {
            tracing::info!("iPad display: running the one-time setup");
            self.display_setup = true;
            self.toast("Setting up the iPad display…", 120.0);
            let proxy = self.proxy.clone();
            let _ = std::thread::Builder::new().name("display-setup".into()).spawn(move || {
                let r = display::setup::run_elevated((w.max(h), w.min(h))).map_err(|e| format!("{e:#}"));
                let _ = proxy.send_event(UserEvent::DisplaySetup(r));
            });
            return;
        }
        let session = match display::DisplaySession::start(Some((w.max(h), w.min(h)))) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("iPad display: {e:#}");
                self.toast("Couldn't start the display", 4.0);
                return;
            }
        };
        let url = display::DisplaySession::urls().into_iter().next().unwrap_or_else(|| format!("this PC's address, port {}", display::PORT));
        tracing::info!("iPad display started: {url}");
        let proxy = self.proxy.clone();
        match tray::Tray::new(self.hwnd, "Mirrorly: iPad display", move |a| {
            let _ = proxy.send_event(UserEvent::Tray(a));
        }) {
            Ok(t) => self.tray = Some(t),
            Err(e) => tracing::warn!("{e:#}"),
        }
        self.display = Some(session);
        self.display_hidden = false;
        // While the iPad is still connected, Mirrorly opens the page there
        // itself; mirroring stops once it shows. Otherwise it's up to the
        // user (the window says where to go).
        match self.device.as_ref().filter(|_| self.link.is_some()) {
            Some(d) => {
                d.open_url(format!("{url}#auto"));
                self.opening = Some(Opening::Typing(Instant::now()));
                self.toast("Opening it in Safari on your iPad…", 30.0);
            }
            None => self.set_connected(false),
        }
        self.shared.chrome.lock().unwrap().display = Some(url);
        self.set_bar(false);
        self.slot.request_redraw();
    }

    /// Done opening the page on the iPad (or giving up): stop mirroring it.
    fn opened(&mut self, ok: bool) {
        if self.opening.take().is_none() {
            return;
        }
        if ok {
            self.toast_until = Some(Instant::now());
        } else {
            self.toast("Open the address below in Safari on your iPad", 6.0);
        }
        self.release_all();
        self.set_connected(false);
    }

    /// Opening the page on the iPad: once it connects, tap it once (full
    /// screen and the wake lock need a tap), then stop mirroring.
    fn advance_opening(&mut self) {
        let Some(opening) = self.opening else { return };
        let viewers = self.display.as_ref().map_or(0, |d| d.viewers());
        let center = input::geom::to_u16(0.5);
        match opening {
            Opening::Typing(since) if viewers > 0 => {
                tracing::info!("iPad display: the page is up after {} ms; tapping for full screen", since.elapsed().as_millis());
                self.send(HidEvent::Touch { x: center, y: center });
                self.opening = Some(Opening::Tapped(Instant::now(), false));
            }
            Opening::Typing(since) if since.elapsed() > OPEN_TIMEOUT => {
                tracing::warn!("iPad display: the page did not connect; leaving it to the user");
                self.opened(false);
            }
            Opening::Tapped(at, false) if at.elapsed() >= TAP_HOLD => {
                self.send(HidEvent::Release { x: center, y: center });
                self.opening = Some(Opening::Tapped(at, true));
            }
            Opening::Tapped(at, true) if at.elapsed() >= TAP_SETTLE => self.opened(true),
            _ => {}
        }
    }

    /// Back to mirroring: the display stops, the window returns.
    fn stop_display(&mut self) {
        if self.display.take().is_none() {
            return;
        }
        self.opening = None;
        tracing::info!("iPad display stopped");
        self.tray = None;
        self.shared.chrome.lock().unwrap().display = None;
        if let Some(w) = &self.window {
            w.set_visible(true);
            w.focus_window();
        }
        self.display_hidden = false;
        self.set_connected(true);
    }

    /// The iPad shows the display: hide the window (the icon stays).
    fn check_display(&mut self) {
        if self.display_hidden || self.display.as_ref().is_none_or(|d| d.viewers() == 0) {
            return;
        }
        self.display_hidden = true;
        self.set_bar(false);
        if let Some(w) = &self.window {
            w.set_visible(false);
        }
        if let Some(t) = &self.tray {
            t.notify("Your iPad is a display", "Mirrorly's icon here stops it.");
        }
    }

    /// Drag what was copied on the iPhone out of the window (runs the
    /// system's drag loop until the drop).
    fn drag_shelf(&mut self) {
        let files: Vec<_> = self.shelf_files.iter().filter(|f| f.exists()).cloned().collect();
        if files.is_empty() {
            self.toast("Copy it on the iPhone again", 3.0);
            return;
        }
        match drag::drag_files(self.hwnd, &files) {
            Ok(dropped) => tracing::info!("dragged {} file(s) out (dropped: {dropped})", files.len()),
            Err(e) => {
                tracing::warn!("drag out failed: {e:#}");
                self.toast("Couldn't start the drag", 3.0);
            }
        }
        // The drag loop swallowed the button release and pointer moves.
        self.cursor_inside = false;
        self.bar_cursor = None;
        self.update_hover();
    }

    /// A short message at the bottom of the screen.
    fn toast(&mut self, text: impl Into<String>, secs: f64) {
        self.shared.chrome.lock().unwrap().toast = Some(text.into());
        self.toast_until = Some(Instant::now() + Duration::from_secs_f64(secs));
        self.slot.request_redraw();
    }

    /// Put the Windows clipboard on the iPhone's. Text and images go when
    /// they are copied (`paste` false); files only when Ctrl+V asks
    /// (`paste` true; they can be large). Returns true if Mirrorly is doing
    /// the paste, so the V must not also go to the phone.
    fn send_windows_clipboard(&mut self, paste: bool) -> bool {
        let seq = winclip::sequence();
        if seq == self.clip_sent || self.link.is_none() {
            return false;
        }
        if self.device.as_ref().is_none_or(|d| !d.wants_connection()) {
            return false;
        }
        let content = winclip::read(self.hwnd);
        // The same text or image again, with no iPhone copy in between: it
        // is already there.
        let hash = match &content {
            Some(winclip::WinClip::Text(t)) => Some(hash_of(t.as_bytes())),
            Some(winclip::WinClip::Image(b)) => Some(hash_of(b)),
            _ => None,
        };
        if hash.is_some() && hash == self.clip_sent_hash {
            self.clip_sent = seq;
            return false;
        }
        let Some(d) = self.device.as_ref() else { return false };
        match content {
            Some(winclip::WinClip::Text(t)) => {
                d.push_clipboard(PushContent::Text(t), paste);
                (self.clip_sent, self.clip_sent_hash) = (seq, hash);
                paste
            }
            Some(winclip::WinClip::Image(png)) => {
                d.push_clipboard(PushContent::Png(png), paste);
                (self.clip_sent, self.clip_sent_hash) = (seq, hash);
                paste
            }
            Some(winclip::WinClip::Files(files)) if paste => {
                self.clip_sent_hash = None;
                let what = if files.len() == 1 { "1 file".to_string() } else { format!("{} files", files.len()) };
                tracing::info!("sending {what} copied in Explorer to the iPhone");
                d.paste_files(files);
                self.clip_sent = seq;
                self.toast(format!("Sending {what} to iPhone…"), 120.0);
                true
            }
            Some(winclip::WinClip::Files(_)) => false,
            // Empty, private (password managers), or nothing the iPhone takes.
            None => {
                self.clip_sent = seq;
                false
            }
        }
    }

    /// Ctrl+V in Mirrorly: if the Windows clipboard is newer than the
    /// iPhone's and not there yet (a file copy, or a copy made a moment
    /// ago), send it and paste. True if the V was handled.
    fn paste_from_windows(&mut self) -> bool {
        let windows_newer = match (self.win_copy_at, self.phone_copy_at) {
            (Some(w), Some(p)) => w > p,
            (Some(_), None) => true,
            // Copied before Mirrorly started: unknown which is newer.
            (None, _) => false,
        };
        windows_newer && self.send_windows_clipboard(true)
    }

    fn clipboard(&mut self, ev: ClipEvent) {
        match ev {
            ClipEvent::PhoneCopied => {
                self.phone_copy_at = Some(Instant::now());
                // The iPhone's clipboard moved on: the next Windows copy
                // goes over even if it repeats the last one.
                self.clip_sent_hash = None;
            }
            ClipEvent::PhoneText(text) => {
                // Onto the Windows clipboard; that change is ours, not a copy.
                if let Some(seq) = winclip::write_text(self.hwnd, &text) {
                    self.clip_sent = seq;
                    self.clip_sent_hash = Some(hash_of(text.as_bytes()));
                }
            }
            ClipEvent::Pushed { pasted } => {
                // Mirrorly pressed Command for the paste; let go of it if the
                // user's Ctrl is already up.
                if pasted && !self.keys_down.contains(&0xE3) {
                    self.send(HidEvent::Key { usage: 0xE3, down: false });
                }
            }
            ClipEvent::Fetching(n) => {
                let what = if n == 1 { "1 item".to_string() } else { format!("{n} items") };
                self.toast(format!("Copying {what} from iPhone…"), 120.0);
            }
            ClipEvent::Copied(files) => {
                // Also onto the Windows clipboard, as Explorer would copy them
                // (and a photo as a picture); decoding runs off this thread.
                self.clip_writing = true;
                let (proxy, list) = (self.proxy.clone(), files.clone());
                let _ = std::thread::Builder::new().name("clipboard".into()).spawn(move || {
                    let seq = winclip::write_files(&list);
                    let _ = proxy.send_event(UserEvent::ClipWritten(seq));
                });
                let shelf = shelf_for(&files);
                self.toast(format!("{} copied · drag it from the bar", shelf.label), 4.0);
                self.shared.chrome.lock().unwrap().shelf = Some(shelf);
                self.shelf_files = files;
                self.slot.request_redraw();
            }
            ClipEvent::Sent { pasted, saved, place } => {
                // The paste released Command; press it again if the user is
                // still holding Ctrl.
                for usage in [0xE3, 0xE7] {
                    if self.keys_down.contains(&usage) {
                        self.send(HidEvent::Key { usage, down: true });
                    }
                }
                let count = |n: usize| if n == 1 { "1 file".to_string() } else { format!("{n} files") };
                let text = match (pasted, saved, place) {
                    (0, s, Some(place)) if s > 0 => format!("Saved {} to {place}", count(s)),
                    (p, _, _) if p > 0 => format!("Pasted {} on iPhone", count(p)),
                    _ => "On the iPhone's clipboard: touch and hold, then Paste".into(),
                };
                self.toast(text, if saved > 0 { 6.0 } else { 3.0 });
            }
            ClipEvent::UrlOpened(Ok(())) => tracing::info!("iPad display: the address is in Safari"),
            ClipEvent::UrlOpened(Err(e)) => {
                tracing::warn!("iPad display: opening Safari failed: {e}");
                self.opened(false);
            }
            ClipEvent::Failed(why) => {
                let w = why.to_ascii_lowercase();
                let text = if w.contains("folders") || w.contains("too large") {
                    why
                } else if w.contains("no longer has") {
                    "The iPhone's clipboard changed; copy it again".into()
                } else {
                    "Couldn't reach the iPhone's clipboard".into()
                };
                self.toast(text, 5.0);
            }
        }
    }

    /// Files dropped on the window go to the iPhone.
    fn send_dropped(&mut self) {
        let files = std::mem::take(&mut self.dropped);
        let Some(d) = self.device.as_ref().filter(|_| self.link.is_some()) else {
            self.toast("Connect your iPhone first", 3.0);
            return;
        };
        tracing::info!("sending {} dropped file(s) to the iPhone", files.len());
        let what = if files.len() == 1 { "1 file".to_string() } else { format!("{} files", files.len()) };
        d.paste_files(files);
        self.toast(format!("Sending {what} to iPhone…"), 120.0);
    }

    fn quit(&mut self, el: &ActiveEventLoop) {
        if let Some(rec) = self.recording.take() {
            // Finish the file before the process ends.
            self.shared.recording.lock().unwrap().take();
            let _ = rec.tx.send(record::Msg::Stop);
            if let Some(t) = rec.thread {
                let _ = t.join();
            }
        }
        self.display = None;
        self.tray = None;
        self.release_all();
        self.slot.quit();
        el.exit();
    }

    /// Turn the picture a quarter turn; touches follow through the same
    /// mapping, and the window takes the new shape.
    fn rotate(&mut self, clockwise: bool) {
        let rotation = {
            let mut v = self.shared.view.lock().unwrap();
            v.rotation = if clockwise { v.rotation.next() } else { v.rotation.prev() };
            v.rotation
        };
        tracing::info!("view rotated to {}°", rotation.degrees());
        self.release_all();
        self.apply_size();
        self.slot.request_redraw();
    }

    fn toggle_fullscreen(&mut self) {
        if let Some(w) = self.window.clone() {
            let full = w.fullscreen().is_some();
            if !full {
                self.set_bar(false);
            }
            w.set_fullscreen((!full).then_some(winit::window::Fullscreen::Borderless(None)));
            if full {
                self.apply_size();
            }
        }
    }

    /// F9: tap the phone `PROBE_TAPS` times where the mouse points, each on a
    /// still screen, and time every tap until its response frame is
    /// presented (results in the overlay and the log). Point it at
    /// something that visibly reacts to a touch, e.g. a Calculator key.
    fn start_probe(&self) {
        let (m, _) = self.mapper();
        let Some(p) = m.to_panel(self.cursor.0, self.cursor.1, false) else {
            tracing::warn!("latency probe: put the mouse over the phone picture first");
            return;
        };
        let Some(dev) = &self.device else { return };
        let (sender, shared, slot) = (dev.sender(), self.shared.clone(), self.slot.clone());
        let (x, y) = (input::geom::to_u16(p.0), input::geom::to_u16(p.1));
        tracing::info!("latency probe: {PROBE_TAPS} taps at ({x}, {y})");
        shared.overlay.store(true, Relaxed);
        let _ = std::thread::Builder::new().name("probe".into()).spawn(move || {
            for _ in 0..PROBE_TAPS {
                std::thread::sleep(PROBE_GAP);
                let t = now_us();
                shared.view.lock().unwrap().indicator = Some(Indicator { panel: p, down_us: t, released_us: None });
                sender.send(t, HidEvent::Touch { x, y });
                slot.request_redraw();
                std::thread::sleep(PROBE_HOLD);
                sender.send(now_us(), HidEvent::Release { x, y });
                if let Some(i) = shared.view.lock().unwrap().indicator.as_mut() {
                    i.released_us = Some(now_us());
                }
            }
            tracing::info!("latency probe done");
        });
    }

    /// The App Switcher swipe for this device and view: from the bottom of
    /// what is shown, further on an iPad (0.45, measured) than an iPhone
    /// (0.30).
    fn app_switcher(&self) -> HidEvent {
        let v = self.shared.view.lock().unwrap();
        let travel = if v.is_tablet() { 0.45 } else { 0.30 };
        HidEvent::AppSwitcher { side: v.rotation.bottom_side(), travel }
    }

    /// Lift everything the phone thinks is held (focus loss, disconnect).
    fn release_all(&mut self) {
        if let Some(ev) = self.gesture.up(None) {
            self.send(ev);
        }
        for usage in std::mem::take(&mut self.keys_down) {
            self.send(HidEvent::Key { usage, down: false });
        }
        self.shared.view.lock().unwrap().indicator = None;
        self.slot.request_redraw();
    }

    fn mapper(&self) -> (input::geom::Mapper, input::Side) {
        let v = self.shared.view.lock().unwrap();
        let m = v.mapper();
        // Home / App Switcher swipes start at the bottom of what is shown.
        (m, v.rotation.bottom_side())
    }

    /// The hover bar's controls, in its own client pixels.
    fn toolbar(&self) -> Option<layout::Toolbar> {
        let shelf = self.shared.chrome.lock().unwrap().shelf.is_some();
        let display = self.display_offered();
        self.bar.as_ref().filter(|b| b.is_shown()).map(|b| b.toolbar(shelf, display))
    }

    /// Show or hide the bar (animated). Like macOS, it sits above the phone
    /// screen and never covers it, so the top edge stays free for Control
    /// Center; the mirror window itself never changes size for it.
    fn set_bar(&mut self, show: bool) {
        let full = self.window.as_ref().is_some_and(|w| w.fullscreen().is_some());
        let display = self.display.is_some();
        if let Some(b) = self.bar.as_mut() {
            b.show(show && !full && !display);
        }
        self.sync_bar();
    }

    /// Keep the bar on top of the mirror window's client area.
    fn place_bar(&mut self) {
        let Some(rect) = client_screen_rect(self.hwnd) else { return };
        let top = work_area_top(self.hwnd);
        let scale = self.scale();
        if let Some(b) = self.bar.as_mut() {
            b.place(rect, top, scale);
        }
        self.sync_bar();
    }

    /// Redraw the bar if what it shows changed.
    fn sync_bar(&mut self) {
        let display = self.display_offered();
        let st = {
            let c = self.shared.chrome.lock().unwrap();
            let hover = c.hover.filter(|h| !matches!(h, Control::Connect | Control::Phone(_) | Control::Forget(_) | Control::StopDisplay));
            bar::state_of(&c, hover, display, self.recording.is_some())
        };
        if let Some(b) = self.bar.as_mut() {
            b.sync(&st);
        }
    }

    /// Mouse on the hover bar: hover, a control, or a drag that moves the
    /// window.
    fn bar_input(&mut self, el: &ActiveEventLoop, input: bar::BarInput) {
        match input {
            bar::BarInput::Move(x, y) => self.bar_cursor = Some((x, y)),
            bar::BarInput::Leave => self.bar_cursor = None,
            bar::BarInput::Down(x, y) => {
                self.bar_cursor = Some((x, y));
                match self.toolbar().and_then(|t| t.hit(x, y)) {
                    Some(c) => self.control(el, c),
                    None => drag_window(self.hwnd),
                }
            }
        }
        self.update_hover();
    }

    /// Bar on while the pointer is over the window or the bar (or a finger
    /// is down), off a moment after it leaves; track the control under the
    /// pointer.
    fn update_hover(&mut self) {
        if self.cursor_inside || self.bar_cursor.is_some() || self.gesture.active() {
            self.hide_bar_at = None;
            self.set_bar(true);
        } else if self.hide_bar_at.is_none() {
            self.hide_bar_at = Some(Instant::now() + BAR_LINGER);
        }
        let hover = self
            .bar_cursor
            .and_then(|(x, y)| self.toolbar().and_then(|t| t.hit(x, y)))
            .or_else(|| {
                (self.cursor_inside && self.connect_button_at(self.cursor.0, self.cursor.1)).then_some(Control::Connect)
            })
            .or_else(|| {
                (self.cursor_inside && self.stop_button_at(self.cursor.0, self.cursor.1)).then_some(Control::StopDisplay)
            })
            .or_else(|| self.cursor_inside.then(|| self.picker_at(self.cursor.0, self.cursor.1)).flatten());
        let changed = {
            let mut c = self.shared.chrome.lock().unwrap();
            std::mem::replace(&mut c.hover, hover) != hover
        };
        if changed {
            self.slot.request_redraw();
        }
        self.sync_bar();
    }

    fn set_indicator(&self, f: impl FnOnce(&mut Option<Indicator>)) {
        f(&mut self.shared.view.lock().unwrap().indicator);
        self.slot.request_redraw();
    }

    /// Mouse wheel: a short finger drag, so lists scroll as on the phone.
    fn wheel(&mut self, notches: f64) {
        self.scroll_pending += notches;
        if Instant::now() < self.scroll_busy_until || self.scroll_pending == 0.0 {
            return;
        }
        let (m, _) = self.mapper();
        let start = m.to_panel(self.cursor.0, self.cursor.1, false);
        let dy_px = self.scroll_pending * SCROLL_PER_NOTCH * m.view.h;
        let end = m.to_panel(self.cursor.0, self.cursor.1 + dy_px, true);
        self.scroll_pending = 0.0;
        let (Some(s), Some(e)) = (start, end) else { return };
        self.scroll_busy_until = Instant::now() + SCROLL_BUSY;
        self.send(HidEvent::Scroll {
            x: input::geom::to_u16(s.0),
            y: input::geom::to_u16(s.1),
            dx: e.0 - s.0,
            dy: e.1 - s.1,
        });
    }
}

impl ApplicationHandler<UserEvent> for App {
    /// Finish a file drop; timers: hide the bar a moment after the pointer
    /// left, clear transfer messages.
    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        use winit::event_loop::ControlFlow;
        if !self.dropped.is_empty() {
            self.send_dropped();
        }
        let now = Instant::now();
        if self.hide_bar_at.is_some_and(|t| now >= t) {
            self.hide_bar_at = None;
            if !self.cursor_inside && self.bar_cursor.is_none() && !self.gesture.active() {
                self.set_bar(false);
                self.update_hover();
            }
        }
        if self.toast_until.is_some_and(|t| now >= t) {
            self.toast_until = None;
            self.shared.chrome.lock().unwrap().toast = None;
            self.slot.request_redraw();
        }
        // The bar shows what is current (copied files, Connect).
        self.sync_bar();
        // Bar animation: about 120 steps a second while it runs.
        let animating = self.bar.as_mut().is_some_and(|b| b.tick());
        let frame = animating.then(|| now + Duration::from_millis(8));
        // The display page waits for the iPad to show up.
        self.advance_opening();
        self.check_display();
        let display_poll = match self.opening {
            // The tap's timing needs a closer look.
            Some(Opening::Tapped(..)) => Some(now + Duration::from_millis(10)),
            _ => (self.display.is_some() && (!self.display_hidden || self.opening.is_some())).then(|| now + DISPLAY_POLL),
        };
        el.set_control_flow(match [self.hide_bar_at, self.toast_until, frame, display_poll].into_iter().flatten().min() {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }

    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_none()
            && let Err(e) = self.start(el)
        {
            tracing::error!("startup failed: {e:#}");
            el.exit();
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, ev: UserEvent) {
        let ev = match ev {
            UserEvent::Device(ev) => ev,
            UserEvent::Bar(input) => {
                self.bar_input(el, input);
                return;
            }
            UserEvent::WinClipboard => {
                // Mirrorly's own writes (iPhone copies) are not Windows copies.
                if self.clip_writing || winclip::sequence() == self.clip_sent {
                    return;
                }
                self.win_copy_at = Some(Instant::now());
                self.send_windows_clipboard(false);
                return;
            }
            UserEvent::ClipWritten(seq) => {
                self.clip_writing = false;
                if let Some(seq) = seq {
                    self.clip_sent = seq;
                    self.clip_sent_hash = None;
                }
                return;
            }
            UserEvent::Saved(r) => {
                match r {
                    Ok(text) => self.toast(text, 4.0),
                    Err(e) => {
                        tracing::warn!("{e}");
                        self.toast("Couldn't save it", 4.0);
                    }
                }
                return;
            }
            UserEvent::DisplaySetup(r) => {
                self.display_setup = false;
                match r {
                    Ok(true) => {
                        tracing::info!("iPad display setup done");
                        self.toast_until = Some(Instant::now());
                        self.start_display();
                    }
                    Ok(false) => self.toast("Using the iPad as a display needs administrator permission once", 5.0),
                    Err(e) => {
                        tracing::warn!("iPad display setup: {e}");
                        self.toast("Couldn't set up the iPad display", 5.0);
                    }
                }
                return;
            }
            UserEvent::Tray(action) => {
                match action {
                    tray::TrayAction::StopDisplay => self.stop_display(),
                    tray::TrayAction::Quit => self.quit(el),
                }
                return;
            }
            UserEvent::Phones(list) => {
                self.shared.chrome.lock().unwrap().phones = list;
                self.update_hover();
                self.slot.request_redraw();
                return;
            }
        };
        // Several iPhones plugged in and none chosen: stop and show the picker.
        if let DeviceEvent::Disconnected(r) = &ev
            && r.contains(device::conn::SEVERAL_ON_USB)
            && let Some(d) = &self.device
        {
            d.set_connected(false);
            self.shared.chrome.lock().unwrap().disconnected = true;
        }
        {
            let mut c = self.shared.chrome.lock().unwrap();
            if let Some(page) = status::describe(&ev) {
                c.status = page;
            }
            if let Some(w) = c.welcome.as_mut() {
                w.on_event(&ev);
            }
        }
        self.slot.request_redraw();
        match ev {
            DeviceEvent::Status(_) => {}
            DeviceEvent::Clipboard(c) => self.clipboard(c),
            DeviceEvent::Connected(info) => {
                // First connection done: the welcome checklist retires.
                if self.shared.chrome.lock().unwrap().welcome.take().is_some() {
                    onboarding::mark_done();
                }
                let screen_changed = {
                    let mut v = self.shared.view.lock().unwrap();
                    let changed = v.screen != (info.screen_w, info.screen_h);
                    v.screen = (info.screen_w, info.screen_h);
                    changed
                };
                if screen_changed {
                    self.apply_size();
                }
                let link = match info.transport {
                    device::Transport::Usb => "USB",
                    device::Transport::Wifi => "Wi-Fi",
                };
                *self.shared.transport.lock().unwrap() = Some(link);
                self.link = Some(link);
                self.phone = Some(info.name.clone());
                self.set_title();
                // Copied on Windows while the iPhone was away: hand it over.
                if self.win_copy_at > self.phone_copy_at {
                    self.send_windows_clipboard(false);
                }
            }
            DeviceEvent::Disconnected(_) => {
                // The stream ended: finish the recording with what it has.
                self.stop_recording();
                *self.shared.transport.lock().unwrap() = None;
                self.link = None;
                self.gesture = Gesture::default();
                self.keys_down.clear();
                self.set_title();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.quit(el),
            WindowEvent::Resized(s) => {
                self.shared.view.lock().unwrap().window = (s.width, s.height);
                self.slot.request_redraw();
                self.place_bar();
            }
            WindowEvent::Moved(_) => self.place_bar(),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.shared.chrome.lock().unwrap().scale = scale_factor;
                self.shared.view.lock().unwrap().bezel = BEZEL * scale_factor;
                self.place_bar();
            }
            WindowEvent::Focused(false) => self.release_all(),
            // Our own shelf being dragged out passes over the window first:
            // those files came from the iPhone and must not go back.
            WindowEvent::HoveredFile(path) if is_from_iphone(&path) => {}
            WindowEvent::DroppedFile(path) if is_from_iphone(&path) => {}
            WindowEvent::HoveredFile(_) => {
                let mut c = self.shared.chrome.lock().unwrap();
                if !c.drop_hover {
                    c.drop_hover = true;
                    drop(c);
                    self.slot.request_redraw();
                }
            }
            WindowEvent::HoveredFileCancelled => {
                self.shared.chrome.lock().unwrap().drop_hover = false;
                self.slot.request_redraw();
            }
            WindowEvent::DroppedFile(path) => {
                self.shared.chrome.lock().unwrap().drop_hover = false;
                self.slot.request_redraw();
                self.dropped.push(path);
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::CursorEntered { .. } => {
                self.cursor_inside = true;
                self.update_hover();
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor_inside = false;
                if self.gesture.active()
                    && let Some(ev) = self.gesture.up(None)
                {
                    self.send(ev);
                }
                self.update_hover();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                self.cursor_inside = true;
                if self.gesture.active() {
                    let (m, _) = self.mapper();
                    if let Some(p) = m.to_panel(position.x, position.y, true) {
                        if let Some(ev) = self.gesture.moved(p) {
                            self.send(ev);
                        }
                        self.set_indicator(|i| {
                            if let Some(i) = i {
                                i.panel = p;
                            }
                        });
                    }
                }
                self.update_hover();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let notches = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(p) => p.y / 120.0,
                };
                if self.modifiers.control_key() {
                    let n = SIZES.len();
                    self.size_level = if notches > 0.0 { (self.size_level + 1).min(n - 1) } else { self.size_level.saturating_sub(1) };
                    self.apply_size();
                } else {
                    self.wheel(notches);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let (m, bottom) = self.mapper();
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        if self.stop_button_at(self.cursor.0, self.cursor.1) {
                            self.stop_display();
                            return;
                        }
                        if self.connect_button_at(self.cursor.0, self.cursor.1) {
                            self.set_connected(true);
                            return;
                        }
                        if let Some(c) = self.picker_at(self.cursor.0, self.cursor.1) {
                            self.control(el, c);
                            return;
                        }
                        // Nothing to touch while disconnected.
                        if self.shared.chrome.lock().unwrap().disconnected {
                            return;
                        }
                        if let Some(p) = m.to_panel(self.cursor.0, self.cursor.1, false) {
                            // The indicator goes up before the event is even
                            // queued: it is the perceived-latency trick.
                            let t = now_us();
                            self.set_indicator(|i| *i = Some(Indicator { panel: p, down_us: t, released_us: None }));
                            let ev = self.gesture.down(p, bottom);
                            self.send(ev);
                            self.update_hover();
                        }
                    }
                    (MouseButton::Left, ElementState::Released) => {
                        let p = m.to_panel(self.cursor.0, self.cursor.1, true);
                        if let Some(ev) = self.gesture.up(p) {
                            self.send(ev);
                        }
                        let t = now_us();
                        self.set_indicator(|i| {
                            if let Some(i) = i {
                                i.released_us = Some(t);
                            }
                        });
                        self.update_hover();
                    }
                    (MouseButton::Right, ElementState::Pressed) => self.send(HidEvent::Button { button: Button::Home }),
                    _ => {}
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else { return };
                let down = event.state == ElementState::Pressed;
                let ctrl = self.modifiers.control_key();
                // Mac-style shortcuts (Ctrl stands in for Command), plus F-keys.
                let shortcut: Option<&str> = match code {
                    KeyCode::Digit1 if ctrl => Some("home"),
                    KeyCode::Digit2 if ctrl => Some("switcher"),
                    KeyCode::Digit3 if ctrl => Some("spotlight"),
                    KeyCode::F1 => Some("home"),
                    KeyCode::F2 => Some("switcher"),
                    KeyCode::F3 => Some("lock"),
                    KeyCode::F4 => Some("voldown"),
                    KeyCode::F5 => Some("volup"),
                    KeyCode::F6 => Some("screenshot"),
                    KeyCode::F7 => Some("record"),
                    KeyCode::F9 => Some("probe"),
                    KeyCode::F10 => Some("stats"),
                    KeyCode::F11 => Some("fullscreen"),
                    KeyCode::F12 if self.modifiers.shift_key() => Some("rotate-ccw"),
                    KeyCode::F12 => Some("rotate"),
                    _ => None,
                };
                if let Some(id) = shortcut {
                    if down && !event.repeat {
                        match id {
                            "home" => self.send(HidEvent::Button { button: Button::Home }),
                            "switcher" => self.send(self.app_switcher()),
                            // Ctrl is already held on the phone as Command:
                            // add Space for Command-Space (Spotlight).
                            "spotlight" => {
                                self.send(HidEvent::Key { usage: 0x2C, down: true });
                                self.send(HidEvent::Key { usage: 0x2C, down: false });
                            }
                            "lock" => self.send(HidEvent::Button { button: Button::Lock }),
                            "voldown" => self.send(HidEvent::Button { button: Button::VolumeDown }),
                            "volup" => self.send(HidEvent::Button { button: Button::VolumeUp }),
                            "screenshot" => self.screenshot(),
                            "record" => self.toggle_recording(),
                            "probe" => self.start_probe(),
                            "stats" => {
                                let on = !self.shared.overlay.load(Relaxed);
                                self.shared.overlay.store(on, Relaxed);
                                self.slot.request_redraw();
                            }
                            "fullscreen" => self.toggle_fullscreen(),
                            "rotate" => self.rotate(true),
                            "rotate-ccw" => self.rotate(false),
                            _ => {}
                        }
                    }
                    return;
                }
                // iOS repeats held keys itself, like with a hardware keyboard.
                if event.repeat {
                    return;
                }
                // Ctrl+V: paste what was copied on Windows, if that is newer.
                if code == KeyCode::KeyV && down && self.modifiers.control_key() && self.paste_from_windows() {
                    return;
                }
                if let Some(usage) = hid_usage(code) {
                    if down {
                        self.keys_down.insert(usage);
                    } else if !self.keys_down.remove(&usage) {
                        return; // release of a key pressed before we had focus
                    }
                    self.send(HidEvent::Key { usage, down });
                }
            }
            _ => {}
        }
    }
}

fn hash_of(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// A file Mirrorly fetched from the iPhone's clipboard.
fn is_from_iphone(path: &std::path::Path) -> bool {
    // Windows paths compare case-insensitively.
    let lower = |p: &std::path::Path| p.to_string_lossy().to_lowercase();
    lower(path).starts_with(&lower(&device::clipboard::inbox()))
}

/// The bar chip for files fetched from the iPhone: the name for one file,
/// else a count ("3 photos").
fn shelf_for(files: &[std::path::PathBuf]) -> Shelf {
    let ext = |f: &std::path::PathBuf| f.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let is_video = |f: &std::path::PathBuf| matches!(ext(f).as_str(), "mov" | "mp4");
    let is_photo = |f: &std::path::PathBuf| matches!(ext(f).as_str(), "jpg" | "jpeg" | "png" | "heic" | "gif" | "webp" | "tiff");
    let media = files.iter().all(|f| is_photo(f) || is_video(f));
    let n = files.len();
    let label = if n == 1 && !media {
        files[0].file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "1 file".into())
    } else if files.iter().all(is_photo) {
        if n == 1 { "Photo".into() } else { format!("{n} photos") }
    } else if files.iter().all(is_video) {
        if n == 1 { "Video".into() } else { format!("{n} videos") }
    } else if media {
        format!("{n} items")
    } else {
        format!("{n} files")
    };
    Shelf { label, media }
}

/// Window class of the Mirrorly window, so a second launch can find it.
const WINDOW_CLASS: &str = "MirrorlyWindow";

/// One Mirrorly at a time: two would both hold the iPhone (disconnecting
/// one left the other streaming, and the camera stayed busy). A second
/// launch brings the running window forward instead.
fn already_running() -> bool {
    use windows::{
        Win32::{
            Foundation::{ERROR_ALREADY_EXISTS, GetLastError},
            System::Threading::CreateMutexW,
            UI::WindowsAndMessaging::{FindWindowW, IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow},
        },
        core::{HSTRING, w},
    };
    unsafe {
        // Held (never closed) for the life of the process.
        let Ok(_mutex) = CreateMutexW(None, false, w!(r"Local\Mirrorly.SingleInstance")) else { return false };
        if GetLastError() != ERROR_ALREADY_EXISTS {
            return false;
        }
        if let Ok(h) = FindWindowW(&HSTRING::from(WINDOW_CLASS), None) {
            if IsIconic(h).as_bool() {
                let _ = ShowWindow(h, SW_RESTORE);
            }
            let _ = SetForegroundWindow(h);
        }
        true
    }
}

/// The window's client area in screen pixels.
fn client_screen_rect(hwnd: isize) -> Option<windows::Win32::Foundation::RECT> {
    use windows::Win32::{
        Foundation::{HWND, POINT, RECT},
        Graphics::Gdi::ClientToScreen,
        UI::WindowsAndMessaging::GetClientRect,
    };
    unsafe {
        let h = HWND(hwnd as _);
        let mut r = RECT::default();
        GetClientRect(h, &mut r).ok()?;
        let mut p = POINT { x: 0, y: 0 };
        ClientToScreen(h, &mut p).ok().ok()?;
        Some(RECT { left: p.x, top: p.y, right: p.x + r.right, bottom: p.y + r.bottom })
    }
}

/// Move the window with the mouse, from a press on the bar's background.
fn drag_window(hwnd: isize) {
    use windows::Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::{
            Input::KeyboardAndMouse::ReleaseCapture,
            WindowsAndMessaging::{SC_MOVE, SendMessageW, WM_SYSCOMMAND},
        },
    };
    // SC_MOVE | HTCAPTION: the system's move loop, as if the caption were
    // dragged (the bar follows through WindowEvent::Moved).
    unsafe {
        let _ = ReleaseCapture();
        SendMessageW(HWND(hwnd as _), WM_SYSCOMMAND, Some(WPARAM((SC_MOVE | 2) as usize)), Some(LPARAM(0)));
    }
}

/// Top of the work area (excluding the taskbar) of the window's monitor.
fn work_area_top(hwnd: isize) -> i32 {
    use windows::Win32::{
        Foundation::HWND,
        Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow},
    };
    unsafe {
        let mon = MonitorFromWindow(HWND(hwnd as _), MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(mon, &mut mi).as_bool() { mi.rcWork.top } else { 0 }
    }
}

/// Writes frame records to a CSV on its own thread, so file I/O never sits
/// in the present path.
fn spawn_csv_logger(path: &std::path::Path) -> Result<mpsc::Sender<metrics::FrameRecord>> {
    use std::io::Write;
    let mut out = std::io::BufWriter::new(std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?);
    writeln!(out, "{}", metrics::FrameRecord::CSV_HEADER)?;
    let (tx, rx) = mpsc::channel::<metrics::FrameRecord>();
    std::thread::Builder::new().name("csv".into()).spawn(move || {
        while let Ok(r) = rx.recv() {
            if writeln!(out, "{}", r.csv()).is_err() {
                return;
            }
        }
        let _ = out.flush();
    })?;
    tracing::info!("writing per-frame timings to {}", path.display());
    Ok(tx)
}

/// Log to `%LOCALAPPDATA%\Mirrorly\logs\<name>`, or to a console when
/// asked (or in debug builds, which have one).
fn init_logging(console: bool, name: &str) {
    let filter = || tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    if console && !cfg!(debug_assertions) {
        unsafe {
            let _ = windows::Win32::System::Console::AllocConsole();
        }
    }
    if console || cfg!(debug_assertions) {
        tracing_subscriber::fmt().with_env_filter(filter()).init();
        return;
    }
    let dir = device::paths::data_dir().join("logs");
    let file = std::fs::create_dir_all(&dir).and_then(|_| std::fs::File::create(dir.join(name)));
    match file {
        Ok(f) => tracing_subscriber::fmt().with_env_filter(filter()).with_ansi(false).with_writer(std::sync::Mutex::new(f)).init(),
        Err(_) => tracing_subscriber::fmt().with_env_filter(filter()).init(),
    }
    std::panic::set_hook(Box::new(|info| tracing::error!("panic: {info}")));
}

fn main() -> Result<()> {
    let _ = epoch();
    unsafe { timeBeginPeriod(1) };
    let args = Args::parse();
    // The elevated copy that sets up the iPad display: only that, then exit.
    if let Some(size) = &args.setup_display {
        init_logging(args.console, "display-setup.log");
        tracing::info!("Mirrorly {}: iPad display setup ({size})", env!("CARGO_PKG_VERSION"));
        let code = match display::setup::perform(size) {
            Ok(()) => 0,
            Err(e) => {
                tracing::error!("iPad display setup failed: {e:#}");
                1
            }
        };
        std::process::exit(code);
    }
    // Before logging: a second copy must not truncate the running one's log.
    if already_running() {
        return Ok(());
    }
    init_logging(args.console, "Mirrorly.log");
    tracing::info!("Mirrorly {} starting", env!("CARGO_PKG_VERSION"));

    let event_loop = EventLoop::<UserEvent>::with_user_event().build().context("event loop")?;
    let mut app = App {
        args,
        proxy: event_loop.create_proxy(),
        shared: Arc::new(Shared::default()),
        slot: Arc::new(FrameSlot::default()),
        window: None,
        hwnd: 0,
        device: None,
        gesture: Gesture::default(),
        cursor: (0.0, 0.0),
        cursor_inside: false,
        keys_down: HashSet::new(),
        modifiers: Default::default(),
        phone: None,
        link: None,
        size_level: DEFAULT_SIZE,
        bar: None,
        bar_cursor: None,
        hide_bar_at: None,
        scroll_busy_until: Instant::now(),
        scroll_pending: 0.0,
        shelf_files: Vec::new(),
        dropped: Vec::new(),
        toast_until: None,
        clip_sent: 0,
        clip_sent_hash: None,
        win_copy_at: None,
        phone_copy_at: None,
        clip_writing: false,
        display: None,
        tray: None,
        display_hidden: false,
        display_setup: false,
        opening: None,
        recording: None,
        gpu: None,
        threads: Vec::new(),
    };
    event_loop.run_app(&mut app)?;
    app.slot.quit();
    Ok(())
}
