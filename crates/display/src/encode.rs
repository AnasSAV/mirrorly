//! H.264 for Safari's WebCodecs decoder, from NV12 frames, in hardware.
//!
//! Encoders tried in order: Intel Quick Sync (`h264_qsv`), NVIDIA
//! (`h264_nvenc`), Media Foundation (`h264_mf`). Measured 2026-10-19 on the
//! dev laptop: QSV works; NVENC needs a newer driver than 581.86 (FFmpeg 9
//! wants NVENC API 13.1); MF works. Settings are latency-first: no B-frames,
//! no look-ahead, one frame in flight, a small VBV, keyframes on request.
//! Output is Annex B with SPS/PPS before every IDR, so a viewer can join at
//! any keyframe.

use std::{ffi::CString, ptr};

use anyhow::{Result, bail};
use ffmpeg_sys_next as ff;

const EAGAIN: i32 = 11;
const AVERROR_EAGAIN: i32 = -EAGAIN;

pub struct Encoder {
    ctx: *mut ff::AVCodecContext,
    frame: *mut ff::AVFrame,
    pkt: *mut ff::AVPacket,
    width: usize,
    height: usize,
}

// The encoder is used from one thread at a time.
unsafe impl Send for Encoder {}

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap()
}

impl Encoder {
    pub fn new(width: u32, height: u32, bitrate: i64) -> Result<Self> {
        let mut last = String::new();
        for name in ["h264_qsv", "h264_nvenc", "h264_mf"] {
            match unsafe { Self::open(name, width, height, bitrate) } {
                Ok(e) => {
                    tracing::info!("H.264 encoder: {name} ({width}x{height}, {} Mbit/s)", bitrate / 1_000_000);
                    return Ok(e);
                }
                Err(e) => {
                    tracing::info!("encoder {name} unavailable: {e:#}");
                    last = format!("{name}: {e:#}");
                }
            }
        }
        bail!("no hardware H.264 encoder ({last})")
    }

    unsafe fn open(name: &'static str, width: u32, height: u32, bitrate: i64) -> Result<Self> {
        unsafe {
            let n = cstr(name);
            let codec = ff::avcodec_find_encoder_by_name(n.as_ptr());
            if codec.is_null() {
                bail!("not built in");
            }
            match Self::try_open(name, codec, width, height, bitrate, true) {
                // Not every Intel GPU has the low-power (VDENC) encoder.
                Err(_) if name == "h264_qsv" => Self::try_open(name, codec, width, height, bitrate, false),
                r => r,
            }
        }
    }

    unsafe fn try_open(
        name: &'static str,
        codec: *const ff::AVCodec,
        width: u32,
        height: u32,
        bitrate: i64,
        low_power: bool,
    ) -> Result<Self> {
        unsafe {
            let ctx = ff::avcodec_alloc_context3(codec);
            (*ctx).width = width as i32;
            (*ctx).height = height as i32;
            (*ctx).pix_fmt = ff::AVPixelFormat::AV_PIX_FMT_NV12;
            (*ctx).time_base = ff::AVRational { num: 1, den: 1_000_000 };
            (*ctx).framerate = ff::AVRational { num: 60, den: 1 };
            (*ctx).gop_size = 600;
            (*ctx).max_b_frames = 0;
            (*ctx).bit_rate = bitrate;
            (*ctx).rc_max_rate = bitrate * 3 / 2;
            (*ctx).rc_buffer_size = (bitrate / 8) as i32; // ~1/8 s of data: no long bursts
            (*ctx).flags |= ff::AV_CODEC_FLAG_LOW_DELAY as i32;
            (*ctx).color_primaries = ff::AVColorPrimaries::AVCOL_PRI_BT709;
            (*ctx).color_trc = ff::AVColorTransferCharacteristic::AVCOL_TRC_BT709;
            (*ctx).colorspace = ff::AVColorSpace::AVCOL_SPC_BT709;
            (*ctx).color_range = ff::AVColorRange::AVCOL_RANGE_MPEG;
            let opts: &[(&str, &str)] = match name {
                "h264_qsv" if low_power => {
                    &[("preset", "veryfast"), ("async_depth", "1"), ("look_ahead", "0"), ("forced_idr", "1"), ("low_power", "1")]
                }
                "h264_qsv" => &[("preset", "veryfast"), ("async_depth", "1"), ("look_ahead", "0"), ("forced_idr", "1")],
                "h264_nvenc" => &[("preset", "p1"), ("tune", "ull"), ("zerolatency", "1"), ("forced-idr", "1"), ("delay", "0")],
                _ => &[("scenario", "display_remoting"), ("hw_encoding", "1")],
            };
            for (k, v) in opts {
                let (k, v) = (cstr(k), cstr(v));
                ff::av_opt_set((*ctx).priv_data, k.as_ptr(), v.as_ptr(), 0);
            }
            let r = ff::avcodec_open2(ctx, codec, ptr::null_mut());
            if r < 0 {
                let mut c = ctx;
                ff::avcodec_free_context(&mut c);
                bail!("avcodec_open2 failed ({r})");
            }
            if name == "h264_qsv" {
                tracing::info!("Quick Sync low-power encoder: {low_power}");
            }
            Self::finish(ctx, width, height)
        }
    }

    unsafe fn finish(ctx: *mut ff::AVCodecContext, width: u32, height: u32) -> Result<Self> {
        unsafe {
            let frame = ff::av_frame_alloc();
            (*frame).format = ff::AVPixelFormat::AV_PIX_FMT_NV12 as i32;
            (*frame).width = width as i32;
            (*frame).height = height as i32;
            if ff::av_frame_get_buffer(frame, 0) < 0 {
                bail!("av_frame_get_buffer");
            }
            Ok(Self { ctx, frame, pkt: ff::av_packet_alloc(), width: width as usize, height: height as usize })
        }
    }

    /// Encode one NV12 picture (`width*height*3/2` bytes, tightly packed);
    /// every finished access unit goes to `out` with its keyframe flag.
    pub fn encode(&mut self, nv12: &[u8], pts_us: i64, keyframe: bool, mut out: impl FnMut(&[u8], bool)) -> Result<()> {
        unsafe {
            if ff::av_frame_make_writable(self.frame) < 0 {
                bail!("av_frame_make_writable");
            }
            let (w, h) = (self.width, self.height);
            let f = &mut *self.frame;
            for row in 0..h {
                ptr::copy_nonoverlapping(nv12.as_ptr().add(row * w), f.data[0].add(row * f.linesize[0] as usize), w);
            }
            for row in 0..h / 2 {
                ptr::copy_nonoverlapping(nv12.as_ptr().add(w * h + row * w), f.data[1].add(row * f.linesize[1] as usize), w);
            }
            f.pts = pts_us;
            f.pict_type = if keyframe { ff::AVPictureType::AV_PICTURE_TYPE_I } else { ff::AVPictureType::AV_PICTURE_TYPE_NONE };
            let r = ff::avcodec_send_frame(self.ctx, self.frame);
            if r < 0 {
                bail!("avcodec_send_frame ({r})");
            }
            loop {
                let r = ff::avcodec_receive_packet(self.ctx, self.pkt);
                if r == AVERROR_EAGAIN {
                    return Ok(());
                }
                if r < 0 {
                    bail!("avcodec_receive_packet ({r})");
                }
                let p = &*self.pkt;
                out(std::slice::from_raw_parts(p.data, p.size as usize), p.flags & ff::AV_PKT_FLAG_KEY as i32 != 0);
                ff::av_packet_unref(self.pkt);
            }
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            ff::av_frame_free(&mut self.frame);
            ff::av_packet_free(&mut self.pkt);
            ff::avcodec_free_context(&mut self.ctx);
        }
    }
}
