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

// Release builds are a GUI app: no console window (logs go to a file).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod gpu;
mod layout;
mod metrics;
mod onboarding;
mod present;
mod render;
mod sink;
mod state;
mod status;
mod ui;

use std::{
    collections::HashSet,
    sync::{Arc, atomic::Ordering::Relaxed, mpsc},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use device::{DeviceEvent, DeviceHandle, RctlMode};
use input::{Button, HidEvent, gesture::Gesture, keys::hid_usage};
use video::{D3dLock, FrameSlot};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    keyboard::{KeyCode, PhysicalKey},
    platform::windows::{IconExtWindows, MonitorHandleExtWindows},
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Icon, Window, WindowId},
};

use crate::{
    layout::Control,
    state::{Indicator, Shared},
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
    /// Hover bar: shown above the screen while the pointer is over the
    /// window; `bar_grew_up` remembers whether the window moved up for it.
    bar_grew_up: bool,
    hide_bar_at: Option<Instant>,
    scroll_busy_until: Instant,
    scroll_pending: f64,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl App {
    fn send(&self, ev: HidEvent) {
        if let Some(d) = &self.device {
            d.send(now_us(), ev);
        }
    }

    fn set_title(&self) {
        if let Some(w) = &self.window {
            let t = match (&self.phone, self.link) {
                (Some(p), Some(l)) => format!("Mirrorly — {p} · {l}"),
                (Some(p), None) => format!("Mirrorly — {p}"),
                _ => "Mirrorly".to_string(),
            };
            w.set_title(&t);
        }
    }

    fn scale(&self) -> f64 {
        self.window.as_ref().map(|w| w.scale_factor()).unwrap_or(1.0)
    }

    fn start(&mut self, el: &ActiveEventLoop) -> Result<()> {
        let attrs = Window::default_attributes()
            .with_title("Mirrorly")
            .with_decorations(false)
            .with_resizable(false)
            .with_window_icon(Icon::from_resource(1, None).ok())
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
        let ((cw, ch), b, bar) = {
            let v = self.shared.view.lock().unwrap();
            (v.content(), v.bezel, v.top_bar)
        };
        let f = SIZES[self.size_level];
        let r = input::geom::fit(mw * f - 2.0 * b, mh * f - 2.0 * b, cw, ch);
        let _ = w.request_inner_size(PhysicalSize::new(
            (r.w + 2.0 * b).round() as u32,
            (r.h + 2.0 * b + bar).round() as u32,
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
            Control::AppSwitcher => self.send(HidEvent::AppSwitcher),
            Control::Home => self.send(HidEvent::Button { button: Button::Home }),
        }
    }

    fn quit(&mut self, el: &ActiveEventLoop) {
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

    fn mapper(&self) -> (input::geom::Mapper, bool) {
        let v = self.shared.view.lock().unwrap();
        let m = v.mapper();
        // The bottom-edge gesture is only verified with the picture upright.
        let upright = v.rotation == input::geom::Rotation::R0;
        (m, upright)
    }

    /// The hover bar's controls, when it is shown.
    fn toolbar(&self) -> Option<layout::Toolbar> {
        let bar = self.shared.view.lock().unwrap().bar()?;
        Some(layout::toolbar(bar, self.scale()))
    }

    /// Show or hide the bar. Like macOS, it is a strip above the phone
    /// screen: the window grows upward by its height (so the screen stays
    /// where it is, and its top edge stays free for Control Center), or
    /// downward when there is no room above.
    fn set_bar(&mut self, show: bool) {
        let Some(w) = self.window.clone() else { return };
        let shown = self.shared.view.lock().unwrap().top_bar > 0.0;
        if show == shown || (show && w.fullscreen().is_some()) {
            return;
        }
        let h_bar = (layout::BAR_HEIGHT * w.scale_factor()).round();
        let (Ok(pos), size) = (w.outer_position(), w.inner_size()) else { return };
        let (x, mut y, width) = (pos.x, pos.y, size.width as i32);
        let height;
        if show {
            let room_above = y - work_area_top(self.hwnd) >= h_bar as i32;
            self.bar_grew_up = room_above;
            if room_above {
                y -= h_bar as i32;
            }
            height = size.height as i32 + h_bar as i32;
        } else {
            if self.bar_grew_up {
                y += h_bar as i32;
            }
            height = size.height as i32 - h_bar as i32;
        }
        self.shared.view.lock().unwrap().top_bar = if show { h_bar } else { 0.0 };
        self.shared.chrome.lock().unwrap().toolbar = show;
        use windows::Win32::{
            Foundation::HWND,
            UI::WindowsAndMessaging::{SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos},
        };
        unsafe {
            let _ = SetWindowPos(HWND(self.hwnd as _), None, x, y, width, height, SWP_NOZORDER | SWP_NOACTIVATE);
        }
        self.slot.request_redraw();
    }

    /// Bar on while the pointer is over the window (or a finger is down),
    /// off a moment after it leaves; track the control under the pointer.
    fn update_hover(&mut self) {
        if self.cursor_inside || self.gesture.active() {
            self.hide_bar_at = None;
            self.set_bar(true);
        } else if self.hide_bar_at.is_none() {
            self.hide_bar_at = Some(Instant::now() + BAR_LINGER);
        }
        let hover = self.toolbar().and_then(|t| {
            (self.cursor_inside && t.contains(self.cursor.0, self.cursor.1)).then(|| t.hit(self.cursor.0, self.cursor.1)).flatten()
        });
        let mut c = self.shared.chrome.lock().unwrap();
        if c.hover != hover {
            c.hover = hover;
            drop(c);
            self.slot.request_redraw();
        }
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
    /// Timers: hide the bar a moment after the pointer left.
    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        use winit::event_loop::ControlFlow;
        match self.hide_bar_at {
            Some(t) if Instant::now() >= t => {
                self.hide_bar_at = None;
                if !self.cursor_inside && !self.gesture.active() {
                    self.set_bar(false);
                    self.update_hover();
                }
                el.set_control_flow(ControlFlow::Wait);
            }
            Some(t) => el.set_control_flow(ControlFlow::WaitUntil(t)),
            None => el.set_control_flow(ControlFlow::Wait),
        }
    }

    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_none()
            && let Err(e) = self.start(el)
        {
            tracing::error!("startup failed: {e:#}");
            el.exit();
        }
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, UserEvent::Device(ev): UserEvent) {
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
            }
            DeviceEvent::Disconnected(_) => {
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
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.shared.chrome.lock().unwrap().scale = scale_factor;
                self.shared.view.lock().unwrap().bezel = BEZEL * scale_factor;
            }
            WindowEvent::Focused(false) => self.release_all(),
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
                let (m, upright) = self.mapper();
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        // The hover bar takes the press: a control, or the
                        // bar's background, which moves the window.
                        if let Some(tb) = self.toolbar()
                            && tb.contains(self.cursor.0, self.cursor.1)
                        {
                            match tb.hit(self.cursor.0, self.cursor.1) {
                                Some(c) => self.control(el, c),
                                None => {
                                    if let Some(w) = &self.window {
                                        let _ = w.drag_window();
                                    }
                                }
                            }
                            return;
                        }
                        if let Some(p) = m.to_panel(self.cursor.0, self.cursor.1, false) {
                            // The indicator goes up before the event is even
                            // queued: it is the perceived-latency trick.
                            let t = now_us();
                            self.set_indicator(|i| *i = Some(Indicator { panel: p, down_us: t, released_us: None }));
                            let ev = self.gesture.down(p, upright);
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
                            "switcher" => self.send(HidEvent::AppSwitcher),
                            // Ctrl is already held on the phone as Command:
                            // add Space for Command-Space (Spotlight).
                            "spotlight" => {
                                self.send(HidEvent::Key { usage: 0x2C, down: true });
                                self.send(HidEvent::Key { usage: 0x2C, down: false });
                            }
                            "lock" => self.send(HidEvent::Button { button: Button::Lock }),
                            "voldown" => self.send(HidEvent::Button { button: Button::VolumeDown }),
                            "volup" => self.send(HidEvent::Button { button: Button::VolumeUp }),
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

/// Log to `%LOCALAPPDATA%\Mirrorly\logs\Mirrorly.log`, or to a console when
/// asked (or in debug builds, which have one).
fn init_logging(console: bool) {
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
    let file = std::fs::create_dir_all(&dir).and_then(|_| std::fs::File::create(dir.join("Mirrorly.log")));
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
    init_logging(args.console);
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
        bar_grew_up: false,
        hide_bar_at: None,
        scroll_busy_until: Instant::now(),
        scroll_pending: 0.0,
        threads: Vec::new(),
    };
    event_loop.run_app(&mut app)?;
    app.slot.quit();
    Ok(())
}
