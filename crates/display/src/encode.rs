//! H.264 for Safari's WebCodecs decoder, in hardware.
//!
//! First choice: Intel Quick Sync fed straight from GPU surfaces
//! (`GpuEncoder`): the capture converts into a D3D11 NV12 frame pool that
//! FFmpeg created on the capture's own device, and the encoder reads it
//! there, so the picture never visits the CPU. Otherwise (`Encoder`) NV12
//! from system memory, tried in order: Quick Sync, NVIDIA, Media
//! Foundation. Measured 2026-10-19 on the dev laptop: QSV works; NVENC
//! needs a newer driver than 581.86 (FFmpeg 9 wants NVENC API 13.1); MF
//! works. Settings are latency-first: no B-frames, no look-ahead, one frame
//! in flight, a small VBV, keyframes on request. Output is Annex B with
//! SPS/PPS before every IDR, so a viewer can join at any keyframe.

use std::{ffi::CString, ptr};

use anyhow::{Context, Result, bail};
use ffmpeg_sys_next as ff;
use windows::{
    Win32::Graphics::Direct3D11::{D3D11_BIND_DECODER, ID3D11Device, ID3D11Texture2D},
    core::Interface,
};

const EAGAIN: i32 = 11;
const AVERROR_EAGAIN: i32 = -EAGAIN;
/// Surfaces in the GPU frame pool: one being drawn, the encoder's, spares.
const POOL: i32 = 6;

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap()
}

/// Rate control, colour and latency settings shared by every encoder.
unsafe fn configure(ctx: *mut ff::AVCodecContext, width: u32, height: u32, bitrate: i64) {
    unsafe {
        (*ctx).width = width as i32;
        (*ctx).height = height as i32;
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
    }
}

fn options(name: &str, low_power: bool) -> &'static [(&'static str, &'static str)] {
    match name {
        "h264_qsv" if low_power => &[("preset", "veryfast"), ("async_depth", "1"), ("look_ahead", "0"), ("forced_idr", "1"), ("low_power", "1")],
        "h264_qsv" => &[("preset", "veryfast"), ("async_depth", "1"), ("look_ahead", "0"), ("forced_idr", "1")],
        "h264_nvenc" => &[("preset", "p1"), ("tune", "ull"), ("zerolatency", "1"), ("forced-idr", "1"), ("delay", "0")],
        _ => &[("scenario", "display_remoting"), ("hw_encoding", "1")],
    }
}

unsafe fn set_options(ctx: *mut ff::AVCodecContext, opts: &[(&str, &str)]) {
    for (k, v) in opts {
        let (k, v) = (cstr(k), cstr(v));
        unsafe { ff::av_opt_set((*ctx).priv_data, k.as_ptr(), v.as_ptr(), 0) };
    }
}

/// Hand every finished access unit to `out` with its keyframe flag.
unsafe fn drain(ctx: *mut ff::AVCodecContext, pkt: *mut ff::AVPacket, out: &mut impl FnMut(&[u8], bool)) -> Result<()> {
    unsafe {
        loop {
            let r = ff::avcodec_receive_packet(ctx, pkt);
            if r == AVERROR_EAGAIN {
                return Ok(());
            }
            if r < 0 {
                bail!("avcodec_receive_packet ({r})");
            }
            let p = &*pkt;
            out(std::slice::from_raw_parts(p.data, p.size as usize), p.flags & ff::AV_PKT_FLAG_KEY as i32 != 0);
            ff::av_packet_unref(pkt);
        }
    }
}

fn picture_type(keyframe: bool) -> ff::AVPictureType {
    if keyframe { ff::AVPictureType::AV_PICTURE_TYPE_I } else { ff::AVPictureType::AV_PICTURE_TYPE_NONE }
}

/// NV12 from system memory.
pub struct Encoder {
    ctx: *mut ff::AVCodecContext,
    frame: *mut ff::AVFrame,
    pkt: *mut ff::AVPacket,
    width: usize,
    height: usize,
}

// The encoder is used from one thread at a time.
unsafe impl Send for Encoder {}

impl Encoder {
    pub fn new(width: u32, height: u32, bitrate: i64) -> Result<Self> {
        let mut last = String::new();
        for name in ["h264_qsv", "h264_nvenc", "h264_mf"] {
            match unsafe { Self::open(name, width, height, bitrate) } {
                Ok(e) => {
                    tracing::info!("H.264 encoder: {name}, from system memory ({width}x{height}, {} Mbit/s)", bitrate / 1_000_000);
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

    unsafe fn try_open(name: &'static str, codec: *const ff::AVCodec, width: u32, height: u32, bitrate: i64, low_power: bool) -> Result<Self> {
        unsafe {
            let mut ctx = ff::avcodec_alloc_context3(codec);
            configure(ctx, width, height, bitrate);
            (*ctx).pix_fmt = ff::AVPixelFormat::AV_PIX_FMT_NV12;
            set_options(ctx, options(name, low_power));
            let r = ff::avcodec_open2(ctx, codec, ptr::null_mut());
            if r < 0 {
                ff::avcodec_free_context(&mut ctx);
                bail!("avcodec_open2 failed ({r})");
            }
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
            f.pict_type = picture_type(keyframe);
            let r = ff::avcodec_send_frame(self.ctx, self.frame);
            if r < 0 {
                bail!("avcodec_send_frame ({r})");
            }
            drain(self.ctx, self.pkt, &mut out)
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

/// A surface of the GPU frame pool to draw the next picture into.
pub struct Surface {
    frame: *mut ff::AVFrame,
    pub texture: ID3D11Texture2D,
    /// Slice of `texture` (the pool is one texture array).
    pub index: u32,
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe { ff::av_frame_free(&mut self.frame) };
    }
}

/// Quick Sync reading NV12 straight from D3D11 surfaces.
pub struct GpuEncoder {
    ctx: *mut ff::AVCodecContext,
    pkt: *mut ff::AVPacket,
    /// The D3D11 pool the capture draws into, and the same pool as QSV
    /// surfaces (what the encoder takes).
    d3d_frames: *mut ff::AVBufferRef,
    qsv_frames: *mut ff::AVBufferRef,
}

unsafe impl Send for GpuEncoder {}

impl GpuEncoder {
    /// On the capture's device. Fails where Quick Sync can't share it (no
    /// Intel GPU, or the virtual display on another adapter).
    pub fn new(device: &ID3D11Device, width: u32, height: u32, bitrate: i64) -> Result<Self> {
        unsafe {
            let mut d3d_dev = ff::av_hwdevice_ctx_alloc(ff::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA);
            if d3d_dev.is_null() {
                bail!("av_hwdevice_ctx_alloc(d3d11va)");
            }
            let hw = (*((*d3d_dev).data as *mut ff::AVHWDeviceContext)).hwctx as *mut ff::AVD3D11VADeviceContext;
            // FFmpeg releases this reference with the device context.
            (*hw).device = device.clone().into_raw().cast();
            let r = ff::av_hwdevice_ctx_init(d3d_dev);
            if r < 0 {
                ff::av_buffer_unref(&mut d3d_dev);
                bail!("av_hwdevice_ctx_init(d3d11va) ({r})");
            }
            let mut qsv_dev = ptr::null_mut();
            let r = ff::av_hwdevice_ctx_create_derived(&mut qsv_dev, ff::AVHWDeviceType::AV_HWDEVICE_TYPE_QSV, d3d_dev, 0);
            if r < 0 {
                ff::av_buffer_unref(&mut d3d_dev);
                bail!("Quick Sync on this device ({r})");
            }
            let made = Self::frames(d3d_dev, qsv_dev, width, height).and_then(|(d3d_frames, qsv_frames)| {
                let opened = Self::open(qsv_dev, qsv_frames, width, height, bitrate, true)
                    .or_else(|_| Self::open(qsv_dev, qsv_frames, width, height, bitrate, false));
                match opened {
                    Ok(ctx) => Ok(Self { ctx, pkt: ff::av_packet_alloc(), d3d_frames, qsv_frames }),
                    Err(e) => {
                        let (mut a, mut b) = (d3d_frames, qsv_frames);
                        ff::av_buffer_unref(&mut b);
                        ff::av_buffer_unref(&mut a);
                        Err(e)
                    }
                }
            });
            // The frame contexts and the codec hold their own references.
            ff::av_buffer_unref(&mut qsv_dev);
            ff::av_buffer_unref(&mut d3d_dev);
            let e = made?;
            tracing::info!("H.264 encoder: h264_qsv from GPU surfaces ({width}x{height}, {} Mbit/s)", bitrate / 1_000_000);
            Ok(e)
        }
    }

    /// The D3D11 NV12 pool (one texture array, bound as decoder surfaces the
    /// way FFmpeg allocates Quick Sync surfaces itself; NV12 arrays can't be
    /// render targets), and the same pool as QSV surfaces.
    unsafe fn frames(d3d_dev: *mut ff::AVBufferRef, qsv_dev: *mut ff::AVBufferRef, width: u32, height: u32) -> Result<(*mut ff::AVBufferRef, *mut ff::AVBufferRef)> {
        unsafe {
            let mut d3d_frames = ff::av_hwframe_ctx_alloc(d3d_dev);
            if d3d_frames.is_null() {
                bail!("av_hwframe_ctx_alloc");
            }
            let fc = (*d3d_frames).data as *mut ff::AVHWFramesContext;
            (*fc).format = ff::AVPixelFormat::AV_PIX_FMT_D3D11;
            (*fc).sw_format = ff::AVPixelFormat::AV_PIX_FMT_NV12;
            // Quick Sync surfaces are 16-aligned; the encoder crops to the
            // picture (the capture copies only that part in).
            (*fc).width = width.next_multiple_of(16) as i32;
            (*fc).height = height.next_multiple_of(16) as i32;
            (*fc).initial_pool_size = POOL;
            let hw = (*fc).hwctx as *mut ff::AVD3D11VAFramesContext;
            (*hw).BindFlags = D3D11_BIND_DECODER.0 as u32;
            let r = ff::av_hwframe_ctx_init(d3d_frames);
            if r < 0 {
                ff::av_buffer_unref(&mut d3d_frames);
                bail!("D3D11 frame pool ({r})");
            }
            let mut qsv_frames = ptr::null_mut();
            let r = ff::av_hwframe_ctx_create_derived(&mut qsv_frames, ff::AVPixelFormat::AV_PIX_FMT_QSV, qsv_dev, d3d_frames, 0);
            if r < 0 {
                ff::av_buffer_unref(&mut d3d_frames);
                bail!("the pool as Quick Sync surfaces ({r})");
            }
            Ok((d3d_frames, qsv_frames))
        }
    }

    unsafe fn open(qsv_dev: *mut ff::AVBufferRef, qsv_frames: *mut ff::AVBufferRef, width: u32, height: u32, bitrate: i64, low_power: bool) -> Result<*mut ff::AVCodecContext> {
        unsafe {
            let name = cstr("h264_qsv");
            let codec = ff::avcodec_find_encoder_by_name(name.as_ptr());
            if codec.is_null() {
                bail!("h264_qsv not built in");
            }
            let mut ctx = ff::avcodec_alloc_context3(codec);
            configure(ctx, width, height, bitrate);
            (*ctx).pix_fmt = ff::AVPixelFormat::AV_PIX_FMT_QSV;
            (*ctx).hw_device_ctx = ff::av_buffer_ref(qsv_dev);
            (*ctx).hw_frames_ctx = ff::av_buffer_ref(qsv_frames);
            set_options(ctx, options("h264_qsv", low_power));
            let r = ff::avcodec_open2(ctx, codec, ptr::null_mut());
            if r < 0 {
                ff::avcodec_free_context(&mut ctx);
                bail!("avcodec_open2(h264_qsv, GPU surfaces, low_power {low_power}) ({r})");
            }
            tracing::info!("Quick Sync low-power encoder: {low_power}");
            Ok(ctx)
        }
    }

    /// A free surface to draw the next picture into.
    pub fn surface(&mut self) -> Result<Surface> {
        unsafe {
            let mut frame = ff::av_frame_alloc();
            let r = ff::av_hwframe_get_buffer(self.d3d_frames, frame, 0);
            if r < 0 {
                ff::av_frame_free(&mut frame);
                bail!("no free GPU surface ({r})");
            }
            let raw = (*frame).data[0] as *mut std::ffi::c_void;
            let Some(texture) = ID3D11Texture2D::from_raw_borrowed(&raw).cloned() else {
                ff::av_frame_free(&mut frame);
                bail!("GPU surface without a texture");
            };
            Ok(Surface { frame, texture, index: (*frame).data[1] as usize as u32 })
        }
    }

    /// Encode a surface the capture has drawn into.
    pub fn encode(&mut self, surface: Surface, pts_us: i64, keyframe: bool, mut out: impl FnMut(&[u8], bool)) -> Result<()> {
        unsafe {
            let mut q = ff::av_frame_alloc();
            (*q).format = ff::AVPixelFormat::AV_PIX_FMT_QSV as i32;
            (*q).hw_frames_ctx = ff::av_buffer_ref(self.qsv_frames);
            let r = ff::av_hwframe_map(q, surface.frame, ff::AV_HWFRAME_MAP_READ as i32);
            if r < 0 {
                ff::av_frame_free(&mut q);
                bail!("map the surface for Quick Sync ({r})");
            }
            (*q).pts = pts_us;
            (*q).pict_type = picture_type(keyframe);
            let r = ff::avcodec_send_frame(self.ctx, q);
            // The mapping keeps the D3D11 frame alive as long as needed.
            ff::av_frame_free(&mut q);
            drop(surface);
            if r < 0 {
                bail!("avcodec_send_frame ({r})");
            }
            drain(self.ctx, self.pkt, &mut out).context("Quick Sync")
        }
    }
}

impl Drop for GpuEncoder {
    fn drop(&mut self) {
        unsafe {
            ff::av_packet_free(&mut self.pkt);
            ff::avcodec_free_context(&mut self.ctx);
            ff::av_buffer_unref(&mut self.qsv_frames);
            ff::av_buffer_unref(&mut self.d3d_frames);
        }
    }
}
