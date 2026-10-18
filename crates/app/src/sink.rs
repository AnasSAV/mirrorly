//! Network-thread side of the video path: RTP -> access units -> decoder.

use std::sync::{
    Arc,
    atomic::Ordering::Relaxed,
    mpsc,
};

use device::{DeviceInfo, KeyframeRequester, MediaSink};
use video::{AccessUnit, DecoderInput, Depacketizer};

use crate::metrics::Counters;

pub struct VideoSink {
    depack: Depacketizer,
    out: Vec<AccessUnit>,
    tx: mpsc::Sender<DecoderInput>,
    waiting_key: bool,
    shared: Arc<crate::state::Shared>,
    reported_lost: u64,
}

impl VideoSink {
    pub fn new(tx: mpsc::Sender<DecoderInput>, shared: Arc<crate::state::Shared>) -> Self {
        Self { depack: Depacketizer::new(), out: Vec::new(), tx, waiting_key: true, shared, reported_lost: 0 }
    }

    fn c(&self) -> &Counters {
        &self.shared.counters
    }
}

impl MediaSink for VideoSink {
    fn session_started(&mut self, _info: &DeviceInfo, _keyframes: KeyframeRequester) {
        self.depack = Depacketizer::new();
        self.reported_lost = 0;
        self.waiting_key = true;
        let _ = self.tx.send(DecoderInput::Reset);
    }

    fn rtp(&mut self, seq: u16, ts: u32, marker: bool, payload: &[u8], rx_us: u64) -> bool {
        self.depack.push(seq, ts, marker, payload, rx_us, &mut self.out);
        let lost = self.depack.stats.lost_packets;
        if lost != self.reported_lost {
            self.c().lost_packets.fetch_add(lost - self.reported_lost, Relaxed);
            self.reported_lost = lost;
        }
        let mut need_keyframe = false;
        // Reuse the buffer: this runs for every packet.
        let mut aus = std::mem::take(&mut self.out);
        for au in aus.drain(..) {
            // Never feed the decoder a broken picture or a delta without its
            // reference: drop until the next keyframe, and ask for it now.
            if au.corrupt {
                self.c().corrupt_aus.fetch_add(1, Relaxed);
                self.waiting_key = true;
                need_keyframe = true;
                continue;
            }
            if self.waiting_key && !au.key {
                self.c().skipped_until_key.fetch_add(1, Relaxed);
                need_keyframe = true;
                continue;
            }
            self.waiting_key = false;
            let _ = self.tx.send(DecoderInput::Au(au));
        }
        self.out = aus;
        if need_keyframe {
            self.c().keyframe_requests.fetch_add(1, Relaxed);
        }
        need_keyframe
    }

    fn session_ended(&mut self) {
        let _ = self.tx.send(DecoderInput::Reset);
    }
}
