//! Present thread: draws each new decoded frame the moment it is published
//! and presents it immediately. Never queues more than one frame: the slot
//! holds only the newest picture and the swapchain allows one in flight.

use std::{collections::VecDeque, sync::Arc};

use video::{D3dLock, FrameSlot, VideoFrame};
use windows::Win32::{
    Graphics::Dxgi::{DXGI_PRESENT, DXGI_PRESENT_ALLOW_TEARING},
    System::Threading::WaitForSingleObjectEx,
};

use crate::{
    render::{Renderer, Scene},
    state::Shared,
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
        let mut last_seq = 0u64;
        let mut sizes: VecDeque<usize> = VecDeque::with_capacity(32);
        let mut had_frame = false;
        let mut window_start = now_us();
        let mut window_frames = 0u64;
        let mut rx_to_present: Vec<u64> = Vec::new();
        loop {
            let (frame, is_new, quit) = slot.wait(last_seq);
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
            }

            let (window, scene_view, uv_max, rotation, dot) = {
                let mut v = shared.view.lock().unwrap();
                if let Some(f) = &frame {
                    v.tex = Some((f.picture.tex_width, f.picture.tex_height));
                    if is_new {
                        update_indicator(&mut v.indicator, f, &mut sizes, now_us());
                    }
                }
                let m = v.mapper();
                let dot = v.indicator.map(|ind| {
                    let (x, y) = m.to_window(ind.panel.0, ind.panel.1);
                    (x, y, opts.dot_radius)
                });
                (v.window, m.view, v.uv_max(), v.rotation, dot)
            };

            if let Err(e) = r.resize(window.0, window.1) {
                tracing::warn!("ResizeBuffers failed: {e:#}");
            }
            // Wait until the swapchain can take a frame without blocking.
            unsafe { WaitForSingleObjectEx(r.swapchain.waitable, 100, true) };

            let scene = Scene {
                picture: frame.as_ref().map(|f| (&f.picture.texture, f.picture.index)),
                view: scene_view,
                uv_max,
                rotation,
                dot,
            };
            let presented_at = {
                let _g = lock.lock();
                if let Err(e) = r.draw(&scene) {
                    tracing::warn!("draw failed: {e:#}");
                    continue;
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
                let mut s = shared.stats.lock().unwrap();
                s.presented += 1;
                s.last_present_us = presented_at;
                s.last_rx_to_present_us = presented_at.saturating_sub(f.timing.last_pkt_us);
                window_frames += 1;
                rx_to_present.push(s.last_rx_to_present_us);
            }
            // Until the Phase 2 overlay exists: a stats line every 5 s.
            let elapsed = now_us().saturating_sub(window_start);
            if elapsed >= 5_000_000 {
                rx_to_present.sort_unstable();
                let p = |q: f64| {
                    rx_to_present
                        .get(((rx_to_present.len().max(1) - 1) as f64 * q) as usize)
                        .map(|v| *v as f64 / 1000.0)
                        .unwrap_or(0.0)
                };
                tracing::info!(
                    "presented {window_frames} frames ({:.1} fps); last packet -> Present() p50 {:.2} ms, p95 {:.2} ms",
                    window_frames as f64 / (elapsed as f64 / 1e6),
                    p(0.5),
                    p(0.95)
                );
                window_start = now_us();
                window_frames = 0;
                rx_to_present.clear();
            }
        }
    })
}

/// Hide the local touch indicator when the phone's own response shows up:
/// the first frame that started arriving after touch-down (plus a minimum
/// plausible delay) and is clearly bigger than the recent idle frames.
fn update_indicator(ind: &mut Option<crate::state::Indicator>, f: &VideoFrame, sizes: &mut VecDeque<usize>, now: u64) {
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
    let Some(i) = ind else { return };
    let changed = !f.timing.key && bytes > (median * 3 / 2).max(median + 600);
    let responded = changed && f.timing.first_pkt_us >= i.down_us + MIN_RESPONSE_US;
    let expired = i.released_us.is_some_and(|r| now.saturating_sub(r) > INDICATOR_FALLBACK_US);
    if responded || expired {
        *ind = None;
    }
}
