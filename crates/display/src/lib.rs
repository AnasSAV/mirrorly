//! iPad as a second monitor for Windows.
//!
//! Windows gets an extra monitor from the Virtual Display Driver (MIT,
//! signed; installed once). Mirrorly captures it (Desktop Duplication),
//! encodes it in hardware (H.264; with Quick Sync straight from GPU
//! surfaces) and serves it
//! to Safari on the iPad over Wi-Fi, where a page decodes it with
//! WebCodecs. It is used like any other monitor: with the PC's mouse and
//! keyboard.
//!
//! No touch: Windows silently drops injected touch input whenever an
//! administrator window (Task Manager, installers) is in the foreground,
//! anywhere on screen, so it can't be relied on.
//!
//! Feasibility, 2026-10-19 (iPad 10th generation, iPadOS 27, Intel UHD /
//! Quick Sync): 2360x1640 at 60 fps, Safari decodes it smoothly.

mod capture;
mod cursor;
mod encode;
pub mod screen;
mod server;
pub mod setup;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};

pub use screen::{VirtualScreen, detach as monitor_off, driver_present, is_attached as monitor_on};

/// Port the iPad's Safari connects to.
pub const PORT: u16 = 8765;
/// Target bitrate: sharp text at 2360x1640 without flooding Wi-Fi.
const BITRATE: i64 = 12_000_000;
/// At most 60 frames a second: the iPad's refresh rate.
const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

/// A running display session; dropping it stops everything.
pub struct DisplaySession {
    stop: Arc<AtomicBool>,
    hub: server::Hub,
    pub screen: VirtualScreen,
    worker: Option<thread::JoinHandle<()>>,
}

impl DisplaySession {
    /// Put the virtual monitor on the desktop (at `prefer`, the iPad's
    /// resolution, when the driver offers it), capture it and serve it on
    /// `PORT`. Dropping the session takes the monitor off again.
    pub fn start(prefer: Option<(u32, u32)>) -> Result<Self> {
        let (screen, attached_now) = screen::attach(prefer)?;
        let stop = Arc::new(AtomicBool::new(false));
        let started = server::start(PORT, stop.clone()).and_then(|hub| {
            let (s2, scr, h2) = (stop.clone(), screen.clone(), hub.clone());
            let worker = thread::Builder::new()
                .name("display-capture".into())
                .spawn(move || capture_loop(scr, h2, s2))
                .context("capture thread")?;
            Ok((hub, worker))
        });
        let (hub, worker) = match started {
            Ok(x) => x,
            Err(e) => {
                // Only take off what this attempt put on (another session
                // may be showing it).
                if attached_now {
                    let _ = screen::detach();
                }
                return Err(e);
            }
        };
        tracing::info!("iPad display on {} ({}x{}), serving port {PORT}", screen.device, screen.width, screen.height);
        Ok(Self { stop, hub, screen, worker: Some(worker) })
    }

    /// iPads showing the display right now.
    pub fn viewers(&self) -> usize {
        self.hub.viewer_count()
    }

    /// The addresses an iPad on the same network can open (with the
    /// secret that keeps others on the network out).
    pub fn urls() -> Vec<String> {
        let key = server::key();
        local_ipv4().into_iter().map(|ip| format!("http://{ip}:{PORT}/{key}")).collect()
    }
}

impl Drop for DisplaySession {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        self.hub.close_all();
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        if let Err(e) = screen::detach() {
            tracing::warn!("{e:#}");
        }
    }
}

/// This PC's LAN IPv4 addresses (what the iPad types or is sent to).
fn local_ipv4() -> Vec<std::net::Ipv4Addr> {
    // The address the system would use to reach the LAN: no packet is sent.
    let mut out = Vec::new();
    if let Ok(s) = std::net::UdpSocket::bind("0.0.0.0:0")
        && s.connect("192.168.0.1:9").is_ok()
        && let Ok(std::net::SocketAddr::V4(a)) = s.local_addr()
    {
        out.push(*a.ip());
    }
    out
}

/// Per-stage timings, summarised every few seconds in the log.
#[derive(Default)]
struct Timings {
    capture_to_sent: Vec<u32>,
    /// Pointer, colour conversion and hand-over to the encoder (system
    /// memory: includes waiting for the GPU and the readback).
    convert: Vec<u32>,
    /// The encoder, until its output is sent.
    encode: Vec<u32>,
    frames: u32,
    bytes: u64,
    since: Option<Instant>,
}

impl Timings {
    fn report(&mut self, viewers: usize) {
        let since = *self.since.get_or_insert_with(Instant::now);
        let secs = since.elapsed().as_secs_f64();
        if secs < 5.0 {
            return;
        }
        let p = |v: &mut Vec<u32>, q: f64| {
            v.sort_unstable();
            v.get(((v.len() as f64 - 1.0) * q).round() as usize).map(|x| *x as f64 / 1000.0).unwrap_or(0.0)
        };
        if self.frames > 0 {
            tracing::info!(
                "display: {:.0} fps, {:.1} Mbit/s, {viewers} viewer(s) | capture->sent p50 {:.2} p95 {:.2} ms | convert p50 {:.2} encode p50 {:.2} ms",
                self.frames as f64 / secs,
                self.bytes as f64 * 8.0 / secs / 1e6,
                p(&mut self.capture_to_sent, 0.5),
                p(&mut self.capture_to_sent, 0.95),
                p(&mut self.convert, 0.5),
                p(&mut self.encode, 0.5),
            );
        }
        *self = Self { since: Some(Instant::now()), ..Self::default() };
    }
}

/// The encoder in use: from GPU surfaces when Quick Sync can share the
/// capture's device, else from system memory.
enum Enc {
    Gpu(encode::GpuEncoder),
    Cpu(encode::Encoder, Vec<u8>),
}

fn capture_loop(mut screen: VirtualScreen, hub: server::Hub, stop: Arc<AtomicBool>) {
    let epoch = Instant::now();
    let mut timings = Timings::default();
    while !stop.load(Relaxed) {
        // (Re)start capture: at the beginning, and after Windows took it away
        // (secure desktop, display change).
        let mut cap = match capture::Capture::new(&screen.device) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("display capture: {e:#}");
                thread::sleep(Duration::from_secs(1));
                if let Ok(s) = screen::find() {
                    screen = s;
                }
                continue;
            }
        };
        // MIRROR_DISPLAY_SYSMEM=1: the system-memory path, for comparison.
        let gpu = if std::env::var_os("MIRROR_DISPLAY_SYSMEM").is_some() {
            Err(anyhow::anyhow!("MIRROR_DISPLAY_SYSMEM is set"))
        } else {
            encode::GpuEncoder::new(cap.device(), cap.width, cap.height, BITRATE)
        };
        let mut enc = match gpu {
            Ok(g) => Enc::Gpu(g),
            Err(e) => {
                tracing::info!("GPU-surface encoding unavailable ({e:#}); encoding from system memory");
                match encode::Encoder::new(cap.width, cap.height, BITRATE) {
                    Ok(c) => Enc::Cpu(c, Vec::new()),
                    Err(e) => {
                        tracing::error!("display encoder: {e:#}");
                        return;
                    }
                }
            }
        };
        let mut have_picture = false;
        // When the oldest change not yet sent was captured.
        let mut pending: Option<Instant> = None;
        let mut last_sent = Instant::now() - FRAME_INTERVAL;
        hub.want_key.store(true, Relaxed);
        while !stop.load(Relaxed) {
            // Wait for a change; with one pending, only until it is due.
            let wait = match pending {
                Some(_) => FRAME_INTERVAL.saturating_sub(last_sent.elapsed()).as_millis() as u32,
                None => 16,
            };
            match cap.next(wait) {
                Ok(Some(u)) => {
                    // The pointer goes to the iPad at once, outside the video.
                    if let Some(shape) = &u.shape {
                        hub.pointer_shape(&cursor::shape_message(shape.as_ref()));
                    }
                    if u.pointer_moved || u.shape.is_some() {
                        hub.pointer_position(&cursor::position_message(cap.pointer_at()));
                    }
                    if let Some(t) = u.picture {
                        pending.get_or_insert(t);
                        have_picture = true;
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::info!("{e:#}; restarting capture");
                    break;
                }
            }
            let key = hub.want_key.load(Relaxed);
            if hub.viewer_count() == 0 && !key {
                pending = None;
            }
            // Mouse movement alone reports changes far above the iPad's 60 Hz;
            // more frames than that only queue up in Safari. A new viewer on a
            // still screen gets the last picture as a keyframe.
            let due = pending.is_some() && last_sent.elapsed() >= FRAME_INTERVAL;
            if !have_picture || !(due || key) {
                timings.report(hub.viewer_count());
                continue;
            }
            let t0 = pending.take().unwrap_or_else(Instant::now);
            last_sent = Instant::now();
            hub.want_key.store(false, Relaxed);
            let pts = epoch.elapsed().as_micros() as i64;
            let t_conv = Instant::now();
            let mut t_enc = t_conv;
            let mut sent = 0usize;
            let mut emit = |au: &[u8], is_key: bool| {
                sent += au.len();
                hub.broadcast(au, is_key);
            };
            // Draw the pointer, convert to NV12 and encode.
            let r = match &mut enc {
                Enc::Gpu(g) => g.surface().and_then(|s| {
                    cap.render_into(&s.texture, s.index)?;
                    t_enc = Instant::now();
                    g.encode(s, pts, key, &mut emit)
                }),
                Enc::Cpu(c, nv12) => cap.read_nv12(nv12).and_then(|()| {
                    t_enc = Instant::now();
                    c.encode(nv12, pts, key, &mut emit)
                }),
            };
            if let Err(e) = r {
                tracing::warn!("encode: {e:#}");
                break;
            }
            timings.convert.push((t_enc - t_conv).as_micros() as u32);
            timings.encode.push(t_enc.elapsed().as_micros() as u32);
            if sent > 0 {
                timings.capture_to_sent.push(t0.elapsed().as_micros() as u32);
                timings.frames += 1;
                timings.bytes += sent as u64;
            }
            timings.report(hub.viewer_count());
        }
    }
}
