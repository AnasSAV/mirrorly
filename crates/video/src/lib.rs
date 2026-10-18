//! RTP/HEVC -> access units (on the network thread) -> GPU decode (on the
//! decode thread) -> the newest picture in a [`FrameSlot`] for presentation.

pub mod d3d_lock;
pub mod decoder;
pub mod depack;

use std::sync::{Arc, Condvar, Mutex, mpsc};

use windows::Win32::Graphics::Direct3D11::ID3D11Device;

pub use d3d_lock::D3dLock;
pub use decoder::{DecodedPicture, Decoder};
pub use depack::{AccessUnit, DepackStats, Depacketizer};

/// Per-frame timestamps (µs on the app clock) for the latency overlay.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameTiming {
    pub rtp_ts: u32,
    pub first_pkt_us: u64,
    pub last_pkt_us: u64,
    pub decode_start_us: u64,
    pub decode_end_us: u64,
    pub bytes: usize,
    pub key: bool,
}

pub struct VideoFrame {
    pub picture: DecodedPicture,
    pub timing: FrameTiming,
    /// Monotonic frame number, to tell new frames from re-presents.
    pub seq: u64,
}

/// Newest decoded frame, handed from the decode thread to the present
/// thread. Only the latest is kept: presenting never queues behind older
/// frames.
#[derive(Default)]
pub struct FrameSlot {
    inner: Mutex<SlotState>,
    cv: Condvar,
}

#[derive(Default)]
pub struct SlotState {
    pub frame: Option<Arc<VideoFrame>>,
    /// Set by anyone who needs a re-render without a new frame
    /// (resize, overlay change).
    pub redraw: bool,
    pub quit: bool,
}

impl FrameSlot {
    pub fn publish(&self, f: Arc<VideoFrame>) {
        self.inner.lock().unwrap().frame = Some(f);
        self.cv.notify_one();
    }

    pub fn request_redraw(&self) {
        self.inner.lock().unwrap().redraw = true;
        self.cv.notify_one();
    }

    pub fn quit(&self) {
        self.inner.lock().unwrap().quit = true;
        self.cv.notify_one();
    }

    /// Block until there is a frame newer than `seen` (or a redraw / quit
    /// request). Returns the newest frame, whether it is new, and quit.
    pub fn wait(&self, seen: u64) -> (Option<Arc<VideoFrame>>, bool, bool) {
        let mut g = self.inner.lock().unwrap();
        loop {
            let newer = g.frame.as_ref().is_some_and(|f| f.seq > seen);
            if newer || g.redraw || g.quit {
                g.redraw = false;
                return (g.frame.clone(), newer, g.quit);
            }
            g = self.cv.wait(g).unwrap();
        }
    }

    /// Drop the current frame (e.g. after a disconnect) so its surface
    /// returns to the decoder that owns it.
    pub fn clear(&self) {
        self.inner.lock().unwrap().frame = None;
        self.cv.notify_one();
    }
}

pub enum DecoderInput {
    Au(AccessUnit),
    /// New session: drop the decoder and wait for a keyframe.
    Reset,
}

/// COM pointers are not `Send` in the `windows` crate; the device is
/// free-threaded and every context call goes through [`D3dLock`].
struct SendDevice(ID3D11Device);
unsafe impl Send for SendDevice {}

/// Decode thread. Never blocks on presentation; on any decode error it drops
/// frames until the next keyframe and asks for one immediately.
pub fn spawn_decoder(
    device: ID3D11Device,
    lock: Arc<D3dLock>,
    rx: mpsc::Receiver<DecoderInput>,
    slot: Arc<FrameSlot>,
    request_keyframe: impl Fn() + Send + 'static,
    now_us: fn() -> u64,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    let device = SendDevice(device);
    std::thread::Builder::new().name("decode".into()).spawn(move || {
        let device = device;
        let mut decoder: Option<Decoder> = None;
        let mut waiting_key = true;
        let mut seq = 0u64;
        while let Ok(msg) = rx.recv() {
            let au = match msg {
                DecoderInput::Reset => {
                    slot.clear();
                    decoder = None;
                    waiting_key = true;
                    continue;
                }
                DecoderInput::Au(au) => au,
            };
            if waiting_key && !au.key {
                continue;
            }
            if decoder.is_none() {
                match Decoder::new(&device.0, lock.clone()) {
                    Ok(d) => decoder = Some(d),
                    Err(e) => {
                        tracing::error!("cannot create decoder: {e:#}");
                        continue;
                    }
                }
            }
            let start = now_us();
            match decoder.as_mut().unwrap().decode(&au.data) {
                Ok(Some(picture)) => {
                    waiting_key = false;
                    seq += 1;
                    let timing = FrameTiming {
                        rtp_ts: au.rtp_ts,
                        first_pkt_us: au.first_pkt_us,
                        last_pkt_us: au.last_pkt_us,
                        decode_start_us: start,
                        decode_end_us: now_us(),
                        bytes: au.data.len(),
                        key: au.key,
                    };
                    slot.publish(Arc::new(VideoFrame { picture, timing, seq }));
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("decode error: {e:#}; requesting a keyframe");
                    waiting_key = true;
                    request_keyframe();
                }
            }
        }
    })
}
