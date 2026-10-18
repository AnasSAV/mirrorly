//! Latency instrumentation: per-frame stage timings (all on one QPC-backed
//! µs clock), rolling statistics for the overlay, tap-to-response results,
//! and pipeline counters shared with the network and decode threads.

use std::{
    collections::VecDeque,
    fmt::Write as _,
    sync::atomic::{AtomicU64, Ordering::Relaxed},
};

/// Counters bumped by the network (sink) and decode threads.
#[derive(Debug, Default)]
pub struct Counters {
    pub lost_packets: AtomicU64,
    pub corrupt_aus: AtomicU64,
    /// Pictures dropped while waiting for a keyframe.
    pub skipped_until_key: AtomicU64,
    pub decode_errors: AtomicU64,
    pub keyframe_requests: AtomicU64,
}

/// Every timestamp of one presented frame, µs.
#[derive(Debug, Clone, Copy)]
pub struct FrameRecord {
    pub seq: u64,
    pub rx_first: u64,
    pub rx_last: u64,
    pub dec_start: u64,
    pub dec_end: u64,
    /// Present thread woke up for this frame.
    pub wake: u64,
    /// Swapchain waitable signalled (ready to accept a frame).
    pub ready: u64,
    /// Present() returned.
    pub presented: u64,
    pub bytes: usize,
    pub key: bool,
}

impl FrameRecord {
    pub const CSV_HEADER: &'static str =
        "seq,rx_first_us,rx_last_us,dec_start_us,dec_end_us,wake_us,ready_us,presented_us,bytes,key";

    pub fn csv(&self) -> String {
        format!(
            "{},{},{},{},{},{},{},{},{},{}",
            self.seq,
            self.rx_first,
            self.rx_last,
            self.dec_start,
            self.dec_end,
            self.wake,
            self.ready,
            self.presented,
            self.bytes,
            self.key as u8
        )
    }
}

/// One tap and the phone's visible response to it.
#[derive(Debug, Clone, Copy)]
pub struct TapResult {
    pub down_us: u64,
    /// Touch-down -> first packet of the response frame arrived.
    pub to_wire_us: u64,
    /// Touch-down -> the response frame was presented.
    pub to_present_us: u64,
    /// Touch-down -> the response frame was shown at a vblank (DXGI frame
    /// statistics), once known.
    pub to_screen_us: Option<u64>,
}

/// Present() returned -> scan-out of the frame began (from DXGI frame
/// statistics). 0 when the flip happened immediately (tearing).
#[derive(Debug, Clone, Copy)]
pub struct DisplaySample {
    pub at_us: u64,
    pub present_to_vblank_us: u64,
    pub rx_to_vblank_us: u64,
    pub immediate: bool,
}

const WINDOW_US: u64 = 2_000_000;
const TAPS_KEPT: usize = 20;

#[derive(Default)]
pub struct Metrics {
    frames: VecDeque<FrameRecord>,
    pub presented_total: u64,
    /// Decoded frames replaced in the slot before they could be presented.
    pub superseded_total: u64,
    last_seq: u64,
    pub taps: VecDeque<TapResult>,
    display: VecDeque<DisplaySample>,
}

impl Metrics {
    pub fn push_display(&mut self, s: DisplaySample) {
        while self.display.front().is_some_and(|d| s.at_us.saturating_sub(d.at_us) > WINDOW_US) {
            self.display.pop_front();
        }
        self.display.push_back(s);
    }

    /// (p50, p95, n) of Present -> scan-out and of last packet -> scan-out, ms.
    pub fn display_stats(&self) -> ((f64, f64), (f64, f64), usize) {
        let mut a: Vec<u64> = self.display.iter().map(|d| d.present_to_vblank_us).collect();
        let mut b: Vec<u64> = self.display.iter().map(|d| d.rx_to_vblank_us).collect();
        (pct50_95(&mut a), pct50_95(&mut b), a.len())
    }

    /// Share of frames flipped immediately (mid-refresh), 0..1.
    pub fn immediate_share(&self) -> f64 {
        if self.display.is_empty() {
            return 0.0;
        }
        self.display.iter().filter(|d| d.immediate).count() as f64 / self.display.len() as f64
    }

    /// Attach the on-screen time to the tap that started at `down_us`.
    pub fn tap_shown(&mut self, down_us: u64, shown_us: u64) -> Option<TapResult> {
        let t = self.taps.iter_mut().rev().find(|t| t.down_us == down_us)?;
        t.to_screen_us = Some(shown_us.saturating_sub(down_us));
        Some(*t)
    }
    pub fn push(&mut self, r: FrameRecord) {
        if self.last_seq != 0 && r.seq > self.last_seq + 1 {
            self.superseded_total += r.seq - self.last_seq - 1;
        }
        self.last_seq = r.seq;
        self.presented_total += 1;
        while self.frames.front().is_some_and(|f| r.presented.saturating_sub(f.presented) > WINDOW_US) {
            self.frames.pop_front();
        }
        self.frames.push_back(r);
    }

    pub fn push_tap(&mut self, t: TapResult) {
        if self.taps.len() == TAPS_KEPT {
            self.taps.pop_front();
        }
        self.taps.push_back(t);
    }

    pub fn fps(&self) -> f64 {
        match (self.frames.front(), self.frames.back()) {
            (Some(a), Some(b)) if b.presented > a.presented => {
                (self.frames.len() - 1) as f64 / ((b.presented - a.presented) as f64 / 1e6)
            }
            _ => 0.0,
        }
    }

    fn stage(&self, f: impl Fn(&FrameRecord) -> u64) -> (f64, f64) {
        let mut v: Vec<u64> = self.frames.iter().map(f).collect();
        pct50_95(&mut v)
    }

    /// Multi-line overlay text.
    pub fn text(&self, c: &Counters, input_queue_us: u64, rotation_deg: u32) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "{:.1} fps   presented {}   superseded {}", self.fps(), self.presented_total, self.superseded_total);
        let _ = writeln!(s, "stage (ms)        p50    p95");
        let rows: [(&str, fn(&FrameRecord) -> u64); 7] = [
            ("frame assembly", |f| f.rx_last - f.rx_first),
            ("rx -> decode", |f| f.dec_start.saturating_sub(f.rx_last)),
            ("decode", |f| f.dec_end - f.dec_start),
            ("decode -> wake", |f| f.wake.saturating_sub(f.dec_end)),
            ("swapchain wait", |f| f.ready.saturating_sub(f.wake)),
            ("render+Present", |f| f.presented.saturating_sub(f.ready)),
            ("rx -> presented", |f| f.presented.saturating_sub(f.rx_last)),
        ];
        for (name, f) in rows {
            let (p50, p95) = self.stage(f);
            let _ = writeln!(s, "{name:<16} {p50:>6.2} {p95:>6.2}");
        }
        let ((d50, d95), (r50, r95), n) = self.display_stats();
        if n > 0 {
            let _ = writeln!(s, "{:<16} {d50:>6.2} {d95:>6.2}   {:.0}% immediate", "Present->scanout", self.immediate_share() * 100.0);
            let _ = writeln!(s, "{:<16} {r50:>6.2} {r95:>6.2}", "rx -> scanout");
        } else {
            let _ = writeln!(s, "Present->scanout (no DXGI frame statistics)");
        }
        let _ = writeln!(
            s,
            "lost pkts {}  corrupt {}  skipped {}  dec err {}  kf req {}",
            c.lost_packets.load(Relaxed),
            c.corrupt_aus.load(Relaxed),
            c.skipped_until_key.load(Relaxed),
            c.decode_errors.load(Relaxed),
            c.keyframe_requests.load(Relaxed)
        );
        let _ = writeln!(s, "input queue {:.2} ms   view {}°", input_queue_us as f64 / 1e3, rotation_deg);
        if let Some(last) = self.taps.back() {
            let mut w: Vec<u64> = self.taps.iter().map(|t| t.to_wire_us).collect();
            let mut p: Vec<u64> = self.taps.iter().map(|t| t.to_present_us).collect();
            let (w50, _) = pct50_95(&mut w);
            let (p50, p95) = pct50_95(&mut p);
            let _ = writeln!(
                s,
                "tap->phone frame {:.1} ms (p50 {:.1}, n={})",
                last.to_wire_us as f64 / 1e3,
                w50,
                self.taps.len()
            );
            let _ = writeln!(
                s,
                "tap->presented   {:.1} ms (p50 {:.1}, p95 {:.1})",
                last.to_present_us as f64 / 1e3,
                p50,
                p95
            );
            let mut sc: Vec<u64> = self.taps.iter().filter_map(|t| t.to_screen_us).collect();
            let (s50, _) = pct50_95(&mut sc);
            match last.to_screen_us {
                Some(v) => {
                    let _ = write!(s, "tap->on screen   {:.1} ms (p50 {:.1}, n={})", v as f64 / 1e3, s50, sc.len());
                }
                None => {
                    let _ = write!(s, "tap->on screen   (waiting for DXGI statistics)");
                }
            }
        } else {
            let _ = write!(s, "tap->presented   (click, or F9 for 10 probe taps)");
        }
        s
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Quality {
    #[default]
    Good,
    Fair,
    Poor,
}

impl Quality {
    pub fn color(self) -> [f32; 4] {
        match self {
            Quality::Good => [0.20, 0.85, 0.35, 0.95],
            Quality::Fair => [1.00, 0.70, 0.10, 0.95],
            Quality::Poor => [0.95, 0.20, 0.20, 0.95],
        }
    }
}

/// Connection quality from RFC 3550 interarrival jitter (arrival spacing vs
/// RTP timestamp spacing, so a still screen's lower frame rate is not
/// jitter) plus recent loss / keyframe requests.
#[derive(Debug, Default)]
pub struct QualityMeter {
    jitter_ms: f64,
    prev: Option<(u32, u64)>,
    last_trouble_us: u64,
    seen_trouble: u64,
    better_since_us: Option<u64>,
    pub level: Quality,
}

/// The display stream's RTP clock (measured ~24 kHz in Phase 0).
const RTP_HZ: f64 = 24_000.0;
const TROUBLE_HOLD_US: u64 = 2_000_000;
/// Jitter thresholds (ms). The phone's own encode-time variation already
/// shows ~5 ms over USB, so "good" has to sit above that.
const GOOD_BELOW_MS: f64 = 10.0;
const FAIR_BELOW_MS: f64 = 25.0;
/// A better level must hold this long before the light improves.
const RECOVER_US: u64 = 2_000_000;

impl QualityMeter {
    pub fn jitter_ms(&self) -> f64 {
        self.jitter_ms
    }

    pub fn on_frame(&mut self, rtp_ts: u32, arrival_us: u64, c: &Counters) -> Quality {
        if let Some((pts, parr)) = self.prev {
            let d_media = rtp_ts.wrapping_sub(pts) as i32 as f64 / RTP_HZ * 1000.0;
            let d_arrival = (arrival_us as f64 - parr as f64) / 1000.0;
            let d = (d_arrival - d_media).abs();
            // Ignore discontinuities (stream restart, long idle gaps).
            if d < 1000.0 {
                self.jitter_ms += (d - self.jitter_ms) / 16.0;
            }
        }
        self.prev = Some((rtp_ts, arrival_us));
        let trouble = c.lost_packets.load(Relaxed) + c.corrupt_aus.load(Relaxed) + c.keyframe_requests.load(Relaxed);
        if trouble != self.seen_trouble {
            self.seen_trouble = trouble;
            self.last_trouble_us = arrival_us;
        }
        let recent_trouble = self.last_trouble_us != 0 && arrival_us.saturating_sub(self.last_trouble_us) < TROUBLE_HOLD_US;
        let now = if recent_trouble || self.jitter_ms >= FAIR_BELOW_MS {
            Quality::Poor
        } else if self.jitter_ms >= GOOD_BELOW_MS {
            Quality::Fair
        } else {
            Quality::Good
        };
        // Worse: show at once. Better: only after it has held for a while.
        if now >= self.level {
            self.level = now;
            self.better_since_us = None;
        } else {
            let since = *self.better_since_us.get_or_insert(arrival_us);
            if arrival_us.saturating_sub(since) >= RECOVER_US {
                self.level = now;
                self.better_since_us = None;
            }
        }
        self.level
    }
}

/// (p50, p95) in milliseconds.
pub fn pct50_95(v: &mut [u64]) -> (f64, f64) {
    if v.is_empty() {
        return (0.0, 0.0);
    }
    v.sort_unstable();
    let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize] as f64 / 1000.0;
    (at(0.5), at(0.95))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(seq: u64, presented: u64) -> FrameRecord {
        FrameRecord {
            seq,
            rx_first: presented - 900,
            rx_last: presented - 800,
            dec_start: presented - 700,
            dec_end: presented - 400,
            wake: presented - 300,
            ready: presented - 200,
            presented,
            bytes: 1000,
            key: false,
        }
    }

    #[test]
    fn counts_superseded_and_fps() {
        let mut m = Metrics::default();
        m.push(rec(1, 1_000_000));
        m.push(rec(2, 1_016_667));
        m.push(rec(5, 1_033_333)); // 3 and 4 were never presented
        assert_eq!(m.presented_total, 3);
        assert_eq!(m.superseded_total, 2);
        assert!((m.fps() - 60.0).abs() < 0.5);
    }

    #[test]
    fn window_drops_old_frames() {
        let mut m = Metrics::default();
        m.push(rec(1, 1_000_000));
        m.push(rec(2, 4_000_000));
        assert_eq!(m.frames.len(), 1);
    }

    #[test]
    fn quality_tracks_jitter_and_trouble() {
        let c = Counters::default();
        let mut q = QualityMeter::default();
        // Perfectly paced 30 fps: no jitter.
        for i in 0..50u32 {
            q.on_frame(i * 800, 1_000_000 + i as u64 * 33_333, &c);
        }
        assert_eq!(q.level, Quality::Good);
        assert!(q.jitter_ms() < 0.1);
        // Arrivals wobbling by +-30 ms: poor.
        for i in 50..150u32 {
            let wobble = if i % 2 == 0 { 30_000 } else { 0 };
            q.on_frame(i * 800, 1_000_000 + i as u64 * 33_333 + wobble, &c);
        }
        assert_eq!(q.level, Quality::Poor);
        // A keyframe request marks the link poor at once...
        let mut q = QualityMeter::default();
        q.on_frame(0, 1_000_000, &c);
        c.keyframe_requests.fetch_add(1, Relaxed);
        assert_eq!(q.on_frame(800, 1_033_333, &c), Quality::Poor);
        // ...and it recovers only after the trouble window plus the hold.
        let mut last = Quality::Poor;
        for i in 2..200u32 {
            last = q.on_frame(800 * i, 1_000_000 + i as u64 * 33_333, &c);
            if i < 60 {
                assert_eq!(last, Quality::Poor, "frame {i}");
            }
        }
        assert_eq!(last, Quality::Good);
    }

    #[test]
    fn percentiles() {
        let mut v = vec![1000, 2000, 3000, 4000, 100_000];
        let (p50, p95) = pct50_95(&mut v);
        assert_eq!(p50, 3.0);
        assert_eq!(p95, 100.0);
    }
}
