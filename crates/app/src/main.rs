//! iPhone mirror for Windows: window, presentation and input wiring.
//!
//! Threads: main (winit window + input mapping), network (device crate:
//! tunnel, RTP receive, HID send), decode (FFmpeg d3d11va), present
//! (swapchain). Input goes window -> network directly and never waits on
//! decode or present.

mod gpu;
mod metrics;
mod overlay;
mod present;
mod render;
mod sink;
mod state;

use std::{
    collections::HashSet,
    sync::{Arc, atomic::Ordering::Relaxed, mpsc},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use device::{DeviceEvent, DeviceHandle, RctlMode};
use input::{Button, HidEvent, gesture::Gesture, keys::hid_usage};
use muda::{Menu, MenuEvent, MenuItem};
use video::{D3dLock, FrameSlot};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    keyboard::{KeyCode, PhysicalKey},
    platform::windows::MonitorHandleExtWindows,
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::{Window, WindowId},
};

use crate::state::{Indicator, Shared};

#[derive(Parser, Debug)]
#[command(about = "Mirror and control an iPhone over USB")]
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
    /// Start with the latency overlay shown (F10 toggles it).
    #[arg(long)]
    overlay: bool,
    /// Write one line of stage timestamps per presented frame to this CSV.
    #[arg(long)]
    timings_csv: Option<std::path::PathBuf>,
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
    Menu(String),
}

/// Menu bar "buttons": (id, label).
const BUTTONS: [(&str, &str); 9] = [
    ("home", "Home  (F1)"),
    ("switcher", "App Switcher  (F2)"),
    ("lock", "Lock  (F3)"),
    ("voldown", "Vol −  (F4)"),
    ("volup", "Vol +  (F5)"),
    ("rotate", "Rotate ⟳  (F12, Shift+F12 ⟲)"),
    ("stats", "Stats  (F10)"),
    ("probe", "Latency probe  (F9)"),
    ("fullscreen", "Fullscreen  (F11)"),
];

/// Probe taps: count, pause before each (screen must be still), hold.
const PROBE_TAPS: u32 = 10;
const PROBE_GAP: Duration = Duration::from_millis(1200);
const PROBE_HOLD: Duration = Duration::from_millis(80);

struct App {
    args: Args,
    proxy: EventLoopProxy<UserEvent>,
    shared: Arc<Shared>,
    slot: Arc<FrameSlot>,
    window: Option<Arc<Window>>,
    device: Option<DeviceHandle>,
    _menu: Option<Menu>,
    gesture: Gesture,
    cursor: (f64, f64),
    keys_down: HashSet<u16>,
    modifiers: winit::keyboard::ModifiersState,
    status: String,
    phone: Option<String>,
    sized_for: Option<(u32, u32)>,
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
            let who = self.phone.as_deref().unwrap_or("iPhone");
            w.set_title(&format!("{who} — {}", self.status));
        }
    }

    fn start(&mut self, el: &ActiveEventLoop) -> Result<()> {
        // Size the window to the phone's aspect, 85 % of the monitor height.
        let monitor = el.primary_monitor().or_else(|| el.available_monitors().next());
        let mon_h = monitor.as_ref().map(|m| m.size().height).unwrap_or(1080);
        let h = (mon_h as f64 * 0.85) as u32;
        let w = (h as f64 * 828.0 / 1792.0) as u32;
        let attrs = Window::default_attributes()
            .with_title("iPhone — starting")
            .with_inner_size(PhysicalSize::new(w, h));
        let window = Arc::new(el.create_window(attrs)?);
        let hwnd = match window.window_handle()?.as_raw() {
            RawWindowHandle::Win32(h) => h.hwnd.get(),
            _ => anyhow::bail!("not a Win32 window"),
        };

        let menu = Menu::new();
        for (id, label) in BUTTONS {
            menu.append(&MenuItem::with_id(id, label, true, None))?;
        }
        unsafe { menu.init_for_hwnd(hwnd)? };
        let proxy = self.proxy.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = proxy.send_event(UserEvent::Menu(e.id.0));
        }));

        // GPU on the adapter that drives this window's monitor.
        let hmon = window.current_monitor().map(|m| windows::Win32::Graphics::Gdi::HMONITOR(m.hmonitor() as _));
        let gpu = gpu::Gpu::new(hmon)?;
        tracing::info!("GPU: {} (tearing {})", gpu.adapter_name, gpu.tearing);
        let swapchain = gpu.swapchain(windows::Win32::Foundation::HWND(hwnd as _))?;
        let renderer = render::Renderer::new(gpu.device.clone(), gpu.context.clone(), swapchain)?;
        {
            let size = window.inner_size();
            self.shared.view.lock().unwrap().window = (size.width, size.height);
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
            present::PresentOpts {
                vsync: self.args.vsync,
                dot_radius: 14.0 * dpi,
                dpi_scale: dpi as f32,
                csv,
                qpc_to_us,
            },
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
                rctl,
                hid_settle: Duration::from_millis(500),
                now_us,
            },
        )?;
        *decoder_kf.lock().unwrap() = Some(handle.keyframes.clone());
        *self.shared.hid_stats.lock().unwrap() = Some(handle.hid_stats.clone());
        self.device = Some(handle);
        self._menu = Some(menu);
        self.window = Some(window);
        self.set_title();
        Ok(())
    }

    /// Fit the window to the phone's aspect once per screen size.
    fn fit_window(&mut self, screen: (u32, u32)) {
        if self.sized_for == Some(screen) {
            return;
        }
        self.sized_for = Some(screen);
        let (cw, ch) = self.shared.view.lock().unwrap().content();
        if let Some(w) = &self.window {
            let s = w.inner_size();
            // Keep the longer window side, match the picture's aspect.
            let long = s.width.max(s.height) as f64;
            let size = if ch >= cw {
                PhysicalSize::new((long * cw / ch).round() as u32, long as u32)
            } else {
                PhysicalSize::new(long as u32, (long * ch / cw).round() as u32)
            };
            let _ = w.request_inner_size(size);
        }
    }

    fn action(&mut self, id: &str) {
        match id {
            "home" => self.send(HidEvent::Button { button: Button::Home }),
            "switcher" => self.send(HidEvent::AppSwitcher),
            "lock" => self.send(HidEvent::Button { button: Button::Lock }),
            "voldown" => self.send(HidEvent::Button { button: Button::VolumeDown }),
            "volup" => self.send(HidEvent::Button { button: Button::VolumeUp }),
            "stats" => {
                let on = !self.shared.overlay.load(Relaxed);
                self.shared.overlay.store(on, Relaxed);
                self.slot.request_redraw();
            }
            "probe" => self.start_probe(),
            "fullscreen" => {
                // Borderless fullscreen lets DWM hand the swapchain to the
                // display directly (independent flip) instead of composing.
                if let Some(w) = &self.window {
                    let full = w.fullscreen().is_some();
                    w.set_fullscreen((!full).then_some(winit::window::Fullscreen::Borderless(None)));
                }
            }
            "rotate" | "rotate-ccw" => {
                // Turn the picture a quarter turn; touches follow through
                // the same mapping. Swap the window's shape to match.
                let rotation = {
                    let mut v = self.shared.view.lock().unwrap();
                    v.rotation = if id == "rotate" { v.rotation.next() } else { v.rotation.prev() };
                    v.rotation
                };
                tracing::info!("view rotated to {}°", rotation.degrees());
                self.release_all();
                if let Some(w) = &self.window {
                    let s = w.inner_size();
                    let _ = w.request_inner_size(PhysicalSize::new(s.height, s.width));
                }
                self.slot.request_redraw();
            }
            _ => {}
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

    fn set_indicator(&self, f: impl FnOnce(&mut Option<Indicator>)) {
        f(&mut self.shared.view.lock().unwrap().indicator);
        self.slot.request_redraw();
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_none()
            && let Err(e) = self.start(el)
        {
            tracing::error!("startup failed: {e:#}");
            el.exit();
        }
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, ev: UserEvent) {
        match ev {
            UserEvent::Menu(id) => self.action(&id),
            UserEvent::Device(DeviceEvent::Status(s)) => {
                self.status = s;
                self.set_title();
            }
            UserEvent::Device(DeviceEvent::Connected(info)) => {
                {
                    let mut v = self.shared.view.lock().unwrap();
                    v.screen = (info.screen_w, info.screen_h);
                }
                self.fit_window((info.screen_w, info.screen_h));
                self.phone = Some(format!("{} (iOS {})", info.name, info.ios_version));
                self.status = "connected".into();
                self.set_title();
            }
            UserEvent::Device(DeviceEvent::Disconnected(reason)) => {
                self.gesture = Gesture::default();
                self.keys_down.clear();
                self.status = format!("disconnected: {reason}");
                self.set_title();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.release_all();
                self.slot.quit();
                el.exit();
            }
            WindowEvent::Resized(s) => {
                self.shared.view.lock().unwrap().window = (s.width, s.height);
                self.slot.request_redraw();
            }
            WindowEvent::Focused(false) => self.release_all(),
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
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
            }
            WindowEvent::CursorLeft { .. } if self.gesture.active() => {
                if let Some(ev) = self.gesture.up(None) {
                    self.send(ev);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let (m, upright) = self.mapper();
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        if let Some(p) = m.to_panel(self.cursor.0, self.cursor.1, false) {
                            // The indicator goes up before the event is even
                            // queued: it is the perceived-latency trick.
                            let t = now_us();
                            self.set_indicator(|i| *i = Some(Indicator { panel: p, down_us: t, released_us: None }));
                            let ev = self.gesture.down(p, upright);
                            self.send(ev);
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
                    }
                    (MouseButton::Right, ElementState::Pressed) => self.send(HidEvent::Button { button: Button::Home }),
                    _ => {}
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else { return };
                let down = event.state == ElementState::Pressed;
                let shortcut = match code {
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
                        self.action(id);
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

fn main() -> Result<()> {
    let _ = epoch();
    unsafe { timeBeginPeriod(1) };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();

    let event_loop = EventLoop::<UserEvent>::with_user_event().build().context("event loop")?;
    let mut app = App {
        args,
        proxy: event_loop.create_proxy(),
        shared: Arc::new(Shared::default()),
        slot: Arc::new(FrameSlot::default()),
        window: None,
        device: None,
        _menu: None,
        gesture: Gesture::default(),
        cursor: (0.0, 0.0),
        keys_down: HashSet::new(),
        modifiers: Default::default(),
        status: "starting".into(),
        phone: None,
        sized_for: None,
        threads: Vec::new(),
    };
    event_loop.run_app(&mut app)?;
    app.slot.quit();
    Ok(())
}
