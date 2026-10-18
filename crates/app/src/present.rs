//! Present thread: draws each new decoded frame the moment it is published
//! and presents it immediately. Never queues more than one frame: the slot
//! holds only the newest picture and the swapchain allows one in flight.

use std::{
    collections::VecDeque,
    sync::{Arc, atomic::Ordering::Relaxed, mpsc},
};

use video::{D3dLock, FrameSlot, VideoFrame};
use windows::Win32::{
    Graphics::Dxgi::{
        DXGI_ERROR_FRAME_STATISTICS_DISJOINT, DXGI_FRAME_STATISTICS, DXGI_PRESENT, DXGI_PRESENT_ALLOW_TEARING,
    },
    System::Threading::WaitForSingleObjectEx,
};

use crate::{
    metrics::{DisplaySample, FrameRecord, Metrics, Quality, QualityMeter, TapResult},
    layout,
    ui::{Ui, UiFrame},
    render::{Renderer, Scene},
    state::{Indicator, Shared},
};

/// The real touch response can't arrive sooner than this after touch-down;
/// frames before it are not "the" response.
const MIN_RESPONSE_US: u64 = 30_000;
/// Remove the indicator this long after release even if no changed frame
/// was recognised.
const INDICATOR_FALLBACK_US: u64 = 400_000;

pub struct PresentOpts {
    pub vsync: bool,
    pub dot_radius: f64,
    pub dpi_scale: f32,
    /// Per-frame timing records go here (CSV logger thread), if enabled.
    pub csv: Option<mpsc::Sender<FrameRecord>>,
    /// Converts DXGI's raw QPC timestamps to the app's µs clock.
    pub qpc_to_us: fn(i64) -> u64,
}

/// A Present() waiting for DXGI to report when it reached the screen.
struct Pending {
    present_count: u32,
    presented_us: u64,
    rx_last_us: u64,
    tap_down_us: Option<u64>,
}

/// COM objects are not `Send` in the `windows` crate. The renderer is moved
/// to the present thread once and only used there.
pub struct SendRenderer(pub Renderer);
unsafe impl Send for SendRenderer {}

impl SendRenderer {
    // A by-value method so closures capture the whole wrapper (a field
    // access would capture only the non-Send inner value).
    fn into_inner(self) -> Renderer {
        self.0
    }
}

pub fn spawn(
    renderer: SendRenderer,
    slot: Arc<FrameSlot>,
    shared: Arc<Shared>,
    lock: Arc<D3dLock>,
    opts: PresentOpts,
    now_us: fn() -> u64,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name("present".into()).spawn(move || {
        let mut r = renderer.into_inner();
        let mut ui: Option<Ui> = None;
        let mut metrics = Metrics::default();
        let mut last_seq = 0u64;
        let mut sizes: VecDeque<usize> = VecDeque::with_capacity(32);
        let mut had_frame = false;
        let mut pending: VecDeque<Pending> = VecDeque::with_capacity(16);
        let mut last_stats_count = 0u32;
        let mut stats_unavailable = false;
        let trace_dxgi = std::env::var_os("MIRROR_DXGI_TRACE").is_some();
        let mut last_summary = now_us();
        let mut quality = QualityMeter::default();
        let mut last_level = None;
        loop {
            let (frame, is_new, quit) = slot.wait(last_seq);
            let wake = now_us();
            if quit {
                return;
            }
            if frame.is_none() && had_frame {
                // Decoder was reset: its textures are going away.
                r.forget_surfaces();
            }
            had_frame = frame.is_some();
            if let Some(f) = &frame
                && is_new
            {
                last_seq = f.seq;
                let level = quality.on_frame(f.timing.rtp_ts, f.timing.last_pkt_us, &shared.counters);
                if last_level != Some(level) {
                    tracing::info!("connection quality: {level:?} (jitter {:.1} ms)", quality.jitter_ms());
                    last_level = Some(level);
                }
            }

            let mut responded_tap = None;
            let (window, scene_view, uv_max, rotation, dots) = {
                let mut v = shared.view.lock().unwrap();
                if let Some(f) = &frame {
                    v.tex = Some((f.picture.tex_width, f.picture.tex_height));
                    if is_new {
                        responded_tap = update_indicator(&mut v.indicator, f, &mut sizes, wake);
                    }
                }
                let m = v.mapper();
                let mut dots = Vec::with_capacity(2);
                if let Some(ind) = v.indicator {
                    let (x, y) = m.to_window(ind.panel.0, ind.panel.1);
                    dots.push((x, y, opts.dot_radius, [1.0, 1.0, 1.0, 0.85]));
                }
                if frame.is_some() && quality.level != Quality::Good {
                    // Connection-quality light, shown only when the link
                    // degrades: at the top centre (where the phone's notch is).
                    let r = opts.dot_radius * 0.4;
                    let (cx, cy) = (m.view.x + m.view.w / 2.0, m.view.y + opts.dot_radius);
                    dots.push((cx, cy, r, quality.level.color()));
                }
                (v.window, m.view, v.uv_max(), v.rotation, dots)
            };

            if r.size != window {
                if let Some(u) = ui.as_mut() {
                    u.release_target();
                }
                let _g = lock.lock();
                if let Err(e) = r.resize(window.0, window.1) {
                    tracing::warn!("ResizeBuffers failed: {e:#}");
                }
            }
            // Wait until the swapchain can take a frame without blocking.
            unsafe { WaitForSingleObjectEx(r.swapchain.waitable, 100, true) };
            let ready = now_us();

            let show_overlay = shared.overlay.load(Relaxed);
            let overlay_text = show_overlay.then(|| {
                let q = shared.hid_stats.lock().unwrap().as_ref().map(|s| s.last_queue_us.load(Relaxed)).unwrap_or(0);
                let link = shared.transport.lock().unwrap().unwrap_or("-");
                format!(
                    "{link}  link {:?}  jitter {:.1} ms\n{}",
                    quality.level,
                    quality.jitter_ms(),
                    metrics.text(&shared.counters, q, rotation.degrees())
                )
            });
            let scene = Scene {
                picture: frame.as_ref().map(|f| (&f.picture.texture, f.picture.index)),
                view: scene_view,
                uv_max,
                rotation,
                corner_radius: layout::corner_radius(scene_view),
                dots,
            };
            // Window chrome: status or welcome page (no video), drop target,
            // messages, stats. (The hover bar is its own window: bar.rs.)
            let (status, welcome, drop_hover, toast, disconnected, hover, phones) = {
                let c = shared.chrome.lock().unwrap();
                let status = frame.is_none().then(|| c.status.clone());
                let welcome = if frame.is_none() { c.welcome.clone() } else { None };
                let phones = if c.disconnected { c.phones.clone() } else { Vec::new() };
                (status, welcome, c.drop_hover, c.toast.clone(), c.disconnected, c.hover, phones)
            };
            let ui_frame = UiFrame {
                view: scene_view,
                status: status.as_ref().map(|(t, d)| (t.as_str(), d.as_str())),
                welcome: welcome.as_ref(),
                drop_hover,
                toast: toast.as_deref(),
                disconnected,
                hover,
                phones: &phones,
                stats: overlay_text.as_deref(),
            };
            let presented = {
                let _g = lock.lock();
                if let Err(e) = r.draw(&scene) {
                    tracing::warn!("draw failed: {e:#}");
                    continue;
                }
                if !ui_frame.is_empty() {
                    if ui.is_none() {
                        match Ui::new(&r.device, opts.dpi_scale) {
                            Ok(u) => ui = Some(u),
                            Err(e) => tracing::warn!("window chrome unavailable: {e:#}"),
                        }
                    }
                    if let Some(u) = ui.as_mut()
                        && let Err(e) = u.draw(&r.swapchain.sc, &ui_frame)
                    {
                        tracing::warn!("chrome draw failed: {e:#}");
                    }
                }
                let (sync, flags) = if opts.vsync {
                    (1, DXGI_PRESENT(0))
                } else if r.swapchain.tearing {
                    (0, DXGI_PRESENT_ALLOW_TEARING)
                } else {
                    (0, DXGI_PRESENT(0))
                };
                let hr = unsafe { r.swapchain.sc.Present(sync, flags) };
                if hr.is_err() {
                    tracing::warn!("Present: {hr:?}");
                }
                now_us()
            };

            if let Some(f) = &frame
                && is_new
            {
                let t = f.timing;
                let rec = FrameRecord {
                    seq: f.seq,
                    rx_first: t.first_pkt_us,
                    rx_last: t.last_pkt_us,
                    dec_start: t.decode_start_us,
                    dec_end: t.decode_end_us,
                    wake,
                    ready,
                    presented,
                    bytes: t.bytes,
                    key: t.key,
                };
                metrics.push(rec);
                if let Some(tx) = &opts.csv {
                    let _ = tx.send(rec);
                }
                if let Some(down) = responded_tap {
                    let tap = TapResult {
                        down_us: down,
                        to_wire_us: t.first_pkt_us.saturating_sub(down),
                        to_present_us: presented.saturating_sub(down),
                        to_screen_us: None,
                    };
                    metrics.push_tap(tap);
                }
                if let Ok(pc) = unsafe { r.swapchain.sc.GetLastPresentCount() } {
                    if pending.len() == 16 {
                        // Statistics never came for these; report taps without them.
                        if let Some(old) = pending.pop_front() {
                            log_tap(&metrics, old.tap_down_us);
                        }
                    }
                    pending.push_back(Pending {
                        present_count: pc,
                        presented_us: presented,
                        rx_last_us: t.last_pkt_us,
                        tap_down_us: responded_tap,
                    });
                }
            }

            // When did earlier Presents reach the screen? DXGI reports the
            // vblank of the most recently displayed one.
            if !stats_unavailable {
                let mut st = DXGI_FRAME_STATISTICS::default();
                let res = unsafe { r.swapchain.sc.GetFrameStatistics(&mut st) };
                if trace_dxgi {
                    let shown = (opts.qpc_to_us)(st.SyncQPCTime) as i64;
                    let matching = pending.iter().find(|p| p.present_count == st.PresentCount);
                    tracing::info!(
                        "dxgi {:?}: stats.PresentCount {} refresh {} | pending {:?} | vblank - matching Present {:?} us",
                        res.as_ref().map(|_| ()).map_err(|e| e.code()),
                        st.PresentCount,
                        st.PresentRefreshCount,
                        pending.iter().map(|p| p.present_count).collect::<Vec<_>>(),
                        matching.map(|p| shown - p.presented_us as i64)
                    );
                }
                match res {
                    Ok(()) if st.PresentCount != last_stats_count => {
                        last_stats_count = st.PresentCount;
                        let shown = (opts.qpc_to_us)(st.SyncQPCTime);
                        while let Some(p) = pending.front() {
                            if p.present_count > st.PresentCount {
                                break;
                            }
                            let p = pending.pop_front().unwrap();
                            if p.present_count == st.PresentCount {
                                // A vblank *before* Present() returned means the
                                // flip happened immediately, mid-refresh (tearing):
                                // scan-out of the new frame began at once.
                                let immediate = shown < p.presented_us;
                                let scanout = shown.max(p.presented_us);
                                metrics.push_display(DisplaySample {
                                    at_us: scanout,
                                    present_to_vblank_us: scanout - p.presented_us,
                                    rx_to_vblank_us: scanout.saturating_sub(p.rx_last_us),
                                    immediate,
                                });
                                if let Some(down) = p.tap_down_us {
                                    metrics.tap_shown(down, scanout);
                                }
                            }
                            log_tap(&metrics, p.tap_down_us);
                        }
                    }
                    Ok(()) => {}
                    Err(e) if e.code() == DXGI_ERROR_FRAME_STATISTICS_DISJOINT => {}
                    Err(e) => {
                        tracing::info!("DXGI frame statistics unavailable ({e}); Present->vblank not measured");
                        stats_unavailable = true;
                    }
                }
            }

            if now_us().saturating_sub(last_summary) >= 5_000_000 {
                last_summary = now_us();
                let ((d50, d95), (r50, r95), n) = metrics.display_stats();
                tracing::info!(
                    "{:.1} fps | Present->scanout p50 {d50:.2} p95 {d95:.2} ms, {:.0}% immediate flips | last packet->scanout p50 {r50:.2} p95 {r95:.2} ms (n={n})",
                    metrics.fps(),
                    metrics.immediate_share() * 100.0
                );
            }
        }
    })
}

/// Log a tap once its numbers are final.
fn log_tap(metrics: &Metrics, down: Option<u64>) {
    let Some(down) = down else { return };
    if let Some(t) = metrics.taps.iter().rev().find(|t| t.down_us == down) {
        tracing::info!(
            "tap -> phone frame {:.1} ms, -> presented {:.1} ms, -> on screen {}",
            t.to_wire_us as f64 / 1e3,
            t.to_present_us as f64 / 1e3,
            t.to_screen_us.map(|v| format!("{:.1} ms", v as f64 / 1e3)).unwrap_or_else(|| "n/a".into())
        );
    }
}

/// Hide the local touch indicator when the phone's own response shows up:
/// the first frame that started arriving after touch-down (plus a minimum
/// plausible delay) and is clearly bigger than the recent idle frames.
/// Returns the touch-down time when this frame is that response.
fn update_indicator(ind: &mut Option<Indicator>, f: &VideoFrame, sizes: &mut VecDeque<usize>, now: u64) -> Option<u64> {
    let bytes = f.timing.bytes;
    let mut sorted: Vec<usize> = sizes.iter().copied().collect();
    sorted.sort_unstable();
    let median = sorted.get(sorted.len() / 2).copied().unwrap_or(bytes);
    if !f.timing.key {
        if sizes.len() == 30 {
            sizes.pop_front();
        }
        sizes.push_back(bytes);
    }
    let i = (*ind)?;
    let changed = !f.timing.key && bytes > (median * 3 / 2).max(median + 600);
    let responded = changed && f.timing.first_pkt_us >= i.down_us + MIN_RESPONSE_US;
    let expired = i.released_us.is_some_and(|r| now.saturating_sub(r) > INDICATOR_FALLBACK_US);
    if responded || expired {
        *ind = None;
    }
    responded.then_some(i.down_us)
}
