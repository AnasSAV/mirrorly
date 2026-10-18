//! Receiver -> device RTCP: periodic RR+SDES (keeps the session alive), the
//! per-frame receipt (APP name=5), the periodic `RCTL` APP report, and keyframe
//! requests (RR+SDES+PLI+FIR) on loss.
//!
//! Two descriptions of the RCTL body exist and they disagree:
//! - idevice `rtcp::build_rctl`: frame counter / ms clock / highest seq.
//! - pymobiledevice3 (described as byte-exact to Xcode): RTP ts>>8, packets in
//!   the last frame, arrival clock on the 24 kHz media base, packet counter
//!   with a 0xEA61 constant.
//! We don't guess: both are selectable and Phase 0 measures which keeps the
//! encoder healthiest.

use std::sync::{
    Arc,
    atomic::{AtomicU32, AtomicU64, Ordering::Relaxed},
};
use std::time::Duration;

use idevice::{
    core_device::{ReportBlock, build_frame_ack, build_keyframe_request, build_liveness, build_rctl},
    tcp::handle::UdpSocketHandle,
};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RctlMode {
    #[default]
    Pmd3,
    Idevice,
    Off,
}

/// Counters the receive loop updates and the feedback task reads.
#[derive(Default)]
pub struct RxState {
    pub media_ssrc: AtomicU32,
    pub last_ts: AtomicU32,
    pub last_frame_pkts: AtomicU32,
    pub packets: AtomicU64,
    pub frames: AtomicU64,
    pub base_seq: AtomicU32,
    /// Extended highest sequence number (cycles << 16 | seq).
    pub highest_ext_seq: AtomicU32,
}

impl RxState {
    pub fn on_packet(&self, ssrc: u32, seq: u16, ts: u32) {
        let n = self.packets.fetch_add(1, Relaxed);
        self.last_ts.store(ts, Relaxed);
        if n == 0 {
            self.media_ssrc.store(ssrc, Relaxed);
            self.base_seq.store(seq as u32, Relaxed);
            self.highest_ext_seq.store(seq as u32, Relaxed);
            return;
        }
        let cur = self.highest_ext_seq.load(Relaxed);
        let mut cycles = cur >> 16;
        let last = (cur & 0xffff) as u16;
        if seq < last && last - seq > 0x8000 {
            cycles += 1;
        }
        let ext = (cycles << 16) | seq as u32;
        if ext.wrapping_sub(cur) < 0x8000_0000 {
            self.highest_ext_seq.store(ext, Relaxed);
        }
    }
}

enum Msg {
    FrameAck(u32),
    Keyframe,
}

const KEYFRAME_REQUEST_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone)]
pub struct Feedback {
    tx: mpsc::UnboundedSender<Msg>,
    pub rx_state: Arc<RxState>,
    pub keyframe_requests: Arc<AtomicU64>,
}

impl Feedback {
    pub fn frame_ack(&self, rtp_ts: u32) {
        let _ = self.tx.send(Msg::FrameAck(rtp_ts));
    }

    /// Ask the encoder for a keyframe (rate-limited by the feedback task).
    pub fn request_keyframe(&self) {
        self.keyframe_requests.fetch_add(1, Relaxed);
        let _ = self.tx.send(Msg::Keyframe);
    }
}

pub fn spawn(
    video: Arc<UdpSocketHandle>,
    rtcp_port: u16,
    our_ssrc: u32,
    mode: RctlMode,
    frame_acks: bool,
) -> Feedback {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let st = Arc::new(RxState::default());
    let fb = Feedback {
        tx,
        rx_state: st.clone(),
        keyframe_requests: Arc::new(AtomicU64::new(0)),
    };
    tokio::spawn(async move {
        let started = std::time::Instant::now();
        let mut rctl = tokio::time::interval(Duration::from_millis(50));
        let mut rr = tokio::time::interval(Duration::from_secs(1));
        rctl.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut fir_seq: u8 = 0;
        let mut last_keyframe_req: Option<std::time::Instant> = None;
        loop {
            let pkt = tokio::select! {
                m = rx.recv() => match m {
                    None => return,
                    Some(Msg::FrameAck(ts)) => {
                        if !frame_acks { continue; }
                        build_frame_ack(our_ssrc, ts)
                    }
                    Some(Msg::Keyframe) => {
                        // Ask immediately, but not more than every 100 ms while
                        // the loss/decode error persists (no PLI storm).
                        if last_keyframe_req.is_some_and(|t| t.elapsed() < KEYFRAME_REQUEST_INTERVAL) {
                            continue;
                        }
                        last_keyframe_req = Some(std::time::Instant::now());
                        fir_seq = fir_seq.wrapping_add(1);
                        build_keyframe_request(our_ssrc, "", st.media_ssrc.load(Relaxed), &[block(&st)], fir_seq)
                    }
                },
                _ = rctl.tick() => {
                    if st.packets.load(Relaxed) == 0 { continue; }
                    match mode {
                        RctlMode::Off => continue,
                        RctlMode::Idevice => {
                            let clock = started.elapsed().as_millis() as u16;
                            let rel = st.highest_ext_seq.load(Relaxed).wrapping_sub(st.base_seq.load(Relaxed)) as u16;
                            build_rctl(our_ssrc, clock, st.frames.load(Relaxed) as u16, rel)
                        }
                        RctlMode::Pmd3 => rctl_pmd3(our_ssrc, &st),
                    }
                }
                _ = rr.tick() => {
                    if st.packets.load(Relaxed) == 0 { continue; }
                    build_liveness(our_ssrc, "", &[block(&st)])
                }
            };
            if video.send_to(rtcp_port, pkt).await.is_err() {
                return;
            }
        }
    });
    fb
}

fn block(st: &RxState) -> ReportBlock {
    ReportBlock {
        source_ssrc: st.media_ssrc.load(Relaxed),
        highest_seq: st.highest_ext_seq.load(Relaxed),
        ..Default::default()
    }
}

/// RCTL body as documented in pymobiledevice3's `_build_rctl_packet`.
fn rctl_pmd3(our_ssrc: u32, st: &RxState) -> Vec<u8> {
    let ts = st.last_ts.load(Relaxed);
    let w2 = ((ts >> 8) & 0xffff) << 16;
    let w3 = st.last_frame_pkts.load(Relaxed);
    let w4 = ((ts / 24) & 0xffff) << 16; // arrival clock on the 24 kHz base, jitter 0
    let w5 = ((st.packets.load(Relaxed) as u32 & 0xffff) << 16) | 0xea61;
    let mut p = Vec::with_capacity(32);
    p.extend_from_slice(&[0x80, 0xcc, 0x00, 0x07]);
    p.extend_from_slice(&our_ssrc.to_be_bytes());
    p.extend_from_slice(b"RCTL");
    p.extend_from_slice(&0x8500_0004u32.to_be_bytes());
    for w in [w2, w3, w4, w5] {
        p.extend_from_slice(&w.to_be_bytes());
    }
    p
}
