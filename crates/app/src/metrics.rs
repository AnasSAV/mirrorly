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
    fn percentiles() {
        let mut v = vec![1000, 2000, 3000, 4000, 100_000];
        let (p50, p95) = pct50_95(&mut v);
        assert_eq!(p50, 3.0);
        assert_eq!(p95, 100.0);
    }
}
