//! Background video receiver for tools that need the stream running (it opens
//! the HID auth gate) but don't write it out. Keeps RTCP feedback flowing and
//! reports every completed access unit.

use std::sync::{Arc, atomic::Ordering::Relaxed};

use idevice::core_device::{RtpPacket, is_rtcp};
use tokio::sync::mpsc;

use crate::{
    depack::Depacketizer,
    feedback::{self, Feedback, RctlMode},
    media::MediaSession,
    now_us,
};

#[derive(Debug, Clone, Copy)]
pub struct AuEvent {
    pub first_pkt_us: u64,
    pub last_pkt_us: u64,
    pub bytes: usize,
    pub key: bool,
    pub corrupt: bool,
}

pub fn spawn(m: &MediaSession, rctl: RctlMode) -> (mpsc::UnboundedReceiver<AuEvent>, Option<Feedback>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let fb = m
        .rtcp_port
        .map(|p| feedback::spawn(m.video.clone(), p, m.our_ssrc, rctl, true));
    let video = Arc::clone(&m.video);
    let fb2 = fb.clone();
    tokio::spawn(async move {
        let mut depack = Depacketizer::new();
        let mut aus = Vec::new();
        let mut waiting_key = true;
        let mut last_req = 0u64;
        while let Ok(dg) = video.recv().await {
            let now = now_us();
            if is_rtcp(&dg.data) {
                continue;
            }
            let Some(pkt) = RtpPacket::parse(&dg.data) else { continue };
            if pkt.payload_type != 100 {
                continue;
            }
            if let Some(fb) = &fb2 {
                fb.rx_state.on_packet(pkt.ssrc, pkt.sequence_number, pkt.timestamp);
                if pkt.marker {
                    fb.frame_ack(pkt.timestamp);
                }
            }
            depack.push(&pkt, now, &mut aus);
            for au in aus.drain(..) {
                if let Some(fb) = &fb2 {
                    fb.rx_state.frames.fetch_add(1, Relaxed);
                    fb.rx_state.last_frame_pkts.store(au.packets, Relaxed);
                }
                let bad = au.corrupt || (waiting_key && !au.key);
                if bad {
                    waiting_key = true;
                    if now - last_req > 100_000 {
                        if let Some(fb) = &fb2 {
                            fb.request_keyframe();
                        }
                        last_req = now;
                    }
                } else {
                    waiting_key = false;
                }
                let _ = tx.send(AuEvent {
                    first_pkt_us: au.first_pkt_us,
                    last_pkt_us: au.last_pkt_us,
                    bytes: au.data.len(),
                    key: au.key,
                    corrupt: au.corrupt,
                });
            }
        }
    });
    (rx, fb)
}
