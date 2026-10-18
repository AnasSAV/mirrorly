//! Network-thread side of the video path: RTP -> access units -> decoder.

use std::sync::mpsc;

use device::{DeviceInfo, KeyframeRequester, MediaSink};
use video::{AccessUnit, DecoderInput, Depacketizer};

pub struct VideoSink {
    depack: Depacketizer,
    out: Vec<AccessUnit>,
    tx: mpsc::Sender<DecoderInput>,
    waiting_key: bool,
}

impl VideoSink {
    pub fn new(tx: mpsc::Sender<DecoderInput>) -> Self {
        Self { depack: Depacketizer::new(), out: Vec::new(), tx, waiting_key: true }
    }
}

impl MediaSink for VideoSink {
    fn session_started(&mut self, _info: &DeviceInfo, _keyframes: KeyframeRequester) {
        self.depack = Depacketizer::new();
        self.waiting_key = true;
        let _ = self.tx.send(DecoderInput::Reset);
    }

    fn rtp(&mut self, seq: u16, ts: u32, marker: bool, payload: &[u8], rx_us: u64) -> bool {
        self.depack.push(seq, ts, marker, payload, rx_us, &mut self.out);
        let mut need_keyframe = false;
        for au in self.out.drain(..) {
            // Never feed the decoder a broken picture or a delta without its
            // reference: drop until the next keyframe, and ask for it now.
            if au.corrupt {
                self.waiting_key = true;
                need_keyframe = true;
                continue;
            }
            if self.waiting_key && !au.key {
                need_keyframe = true;
                continue;
            }
            self.waiting_key = false;
            let _ = self.tx.send(DecoderInput::Au(au));
        }
        need_keyframe
    }

    fn session_ended(&mut self) {
        let _ = self.tx.send(DecoderInput::Reset);
    }
}
