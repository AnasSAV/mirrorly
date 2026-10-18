//! FFmpeg HEVC decode on the GPU (d3d11va) into textures on *our* D3D11
//! device, so the present thread can sample them directly (zero copy).
//!
//! Phase 0 settings kept: `AV_CODEC_FLAG_LOW_DELAY`, `thread_count = 1` (frame
//! threading would hold frames back), output pixel format `AV_PIX_FMT_D3D11`.
//! New here: we supply the device and the frame pool, and add
//! `D3D11_BIND_SHADER_RESOURCE` to the pool textures so they can be read by
//! a pixel shader.

use std::{ptr, sync::Arc};

use anyhow::{Result, bail};
use ffmpeg_sys_next as ff;
use windows::{
    Win32::Graphics::Direct3D11::{
        D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC, ID3D11Device, ID3D11Texture2D,
    },
    core::Interface,
};

use crate::d3d_lock::{D3dLock, ffmpeg_lock, ffmpeg_unlock};

const EAGAIN: i32 = 11; // MSVC errno
const AVERROR_EAGAIN: i32 = -EAGAIN;
const AVERROR_EOF: i32 = -((b'E' as i32) | (b'O' as i32) << 8 | (b'F' as i32) << 16 | (b' ' as i32) << 24);
/// Surfaces beyond the DPB: one on screen, one queued, one spare.
const EXTRA_HW_FRAMES: i32 = 4;

/// A decoded picture: a slice of the decoder's texture array. Holding it
/// keeps the surface out of the decoder's pool.
pub struct DecodedPicture {
    frame: *mut ff::AVFrame,
    pub texture: ID3D11Texture2D,
    pub index: u32,
    /// Coded size (includes the encoder's alignment padding).
    pub width: u32,
    pub height: u32,
    /// Size of the texture itself. FFmpeg aligns its decoder surfaces, so
    /// this can be larger than the coded size; texture coordinates must be
    /// computed against it.
    pub tex_width: u32,
    pub tex_height: u32,
}

// SAFETY: the AVFrame is only unref'd on drop; the texture is a COM object
// used under the shared D3D lock.
unsafe impl Send for DecodedPicture {}
unsafe impl Sync for DecodedPicture {}

impl Drop for DecodedPicture {
    fn drop(&mut self) {
        unsafe { ff::av_frame_free(&mut self.frame) };
    }
}

pub struct Decoder {
    ctx: *mut ff::AVCodecContext,
    pkt: *mut ff::AVPacket,
    frame: *mut ff::AVFrame,
    // Kept alive for the lifetime of the FFmpeg device context's lock pointer.
    _lock: Arc<D3dLock>,
}

unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new(device: &ID3D11Device, lock: Arc<D3dLock>) -> Result<Self> {
        unsafe {
            let hw = ff::av_hwdevice_ctx_alloc(ff::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA);
            if hw.is_null() {
                bail!("av_hwdevice_ctx_alloc(d3d11va) failed");
            }
            let devctx = (*hw).data as *mut ff::AVHWDeviceContext;
            let d3d = (*devctx).hwctx as *mut ff::AVD3D11VADeviceContext;
            // FFmpeg releases this reference when the device context goes.
            (*d3d).device = device.clone().into_raw().cast();
            (*d3d).lock = Some(ffmpeg_lock);
            (*d3d).unlock = Some(ffmpeg_unlock);
            (*d3d).lock_ctx = lock.raw();
            let mut hw = hw;
            let r = ff::av_hwdevice_ctx_init(hw);
            if r < 0 {
                ff::av_buffer_unref(&mut hw);
                bail!("av_hwdevice_ctx_init = {r}");
            }

            let codec = ff::avcodec_find_decoder(ff::AVCodecID::AV_CODEC_ID_HEVC);
            if codec.is_null() {
                ff::av_buffer_unref(&mut hw);
                bail!("FFmpeg has no HEVC decoder");
            }
            let mut ctx = ff::avcodec_alloc_context3(codec);
            (*ctx).hw_device_ctx = hw; // ownership moves to the codec context
            (*ctx).get_format = Some(get_format);
            (*ctx).flags |= ff::AV_CODEC_FLAG_LOW_DELAY as i32;
            (*ctx).thread_count = 1;
            (*ctx).extra_hw_frames = EXTRA_HW_FRAMES;
            let r = ff::avcodec_open2(ctx, codec, ptr::null_mut());
            if r < 0 {
                ff::avcodec_free_context(&mut ctx);
                bail!("avcodec_open2 = {r}");
            }
            Ok(Self { ctx, pkt: ff::av_packet_alloc(), frame: ff::av_frame_alloc(), _lock: lock })
        }
    }

    /// Decode one access unit. Returns the newest picture it produced (with
    /// low-delay mode that is this access unit's picture), or `None`.
    pub fn decode(&mut self, au: &[u8]) -> Result<Option<DecodedPicture>> {
        unsafe {
            if ff::av_new_packet(self.pkt, au.len() as i32) < 0 {
                bail!("av_new_packet failed");
            }
            ptr::copy_nonoverlapping(au.as_ptr(), (*self.pkt).data, au.len());
            let r = ff::avcodec_send_packet(self.ctx, self.pkt);
            ff::av_packet_unref(self.pkt);
            if r < 0 && r != AVERROR_EAGAIN {
                bail!("avcodec_send_packet = {r}");
            }
            let mut newest = None;
            loop {
                let r = ff::avcodec_receive_frame(self.ctx, self.frame);
                if r == AVERROR_EAGAIN || r == AVERROR_EOF {
                    break;
                }
                if r < 0 {
                    bail!("avcodec_receive_frame = {r}");
                }
                if (*self.frame).format != ff::AVPixelFormat::AV_PIX_FMT_D3D11 as i32 {
                    ff::av_frame_unref(self.frame);
                    bail!("decoder fell back to software output (format {})", (*self.frame).format);
                }
                let owned = ff::av_frame_clone(self.frame);
                ff::av_frame_unref(self.frame);
                let raw_tex = (*owned).data[0] as *mut std::ffi::c_void;
                let Some(texture) = ID3D11Texture2D::from_raw_borrowed(&raw_tex).cloned() else {
                    let mut owned = owned;
                    ff::av_frame_free(&mut owned);
                    bail!("decoded frame has no texture");
                };
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                texture.GetDesc(&mut desc);
                newest = Some(DecodedPicture {
                    texture,
                    index: (*owned).data[1] as usize as u32,
                    width: (*owned).width as u32,
                    height: (*owned).height as u32,
                    tex_width: desc.Width,
                    tex_height: desc.Height,
                    frame: owned,
                });
            }
            Ok(newest)
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            ff::av_frame_free(&mut self.frame);
            ff::av_packet_free(&mut self.pkt);
            ff::avcodec_free_context(&mut self.ctx);
        }
    }
}

/// Pick D3D11 output and give the frame pool shader-readable textures.
unsafe extern "C" fn get_format(ctx: *mut ff::AVCodecContext, fmts: *const ff::AVPixelFormat) -> ff::AVPixelFormat {
    unsafe {
        let mut p = fmts;
        while *p != ff::AVPixelFormat::AV_PIX_FMT_NONE {
            if *p == ff::AVPixelFormat::AV_PIX_FMT_D3D11 {
                break;
            }
            p = p.add(1);
        }
        if *p != ff::AVPixelFormat::AV_PIX_FMT_D3D11 {
            return ff::AVPixelFormat::AV_PIX_FMT_NONE;
        }
        let mut frames: *mut ff::AVBufferRef = ptr::null_mut();
        if ff::avcodec_get_hw_frames_parameters(ctx, (*ctx).hw_device_ctx, ff::AVPixelFormat::AV_PIX_FMT_D3D11, &mut frames) < 0 {
            return ff::AVPixelFormat::AV_PIX_FMT_NONE;
        }
        let fc = (*frames).data as *mut ff::AVHWFramesContext;
        let d3d = (*fc).hwctx as *mut ff::AVD3D11VAFramesContext;
        (*d3d).BindFlags |= D3D11_BIND_SHADER_RESOURCE.0 as u32;
        if ff::av_hwframe_ctx_init(frames) < 0 {
            ff::av_buffer_unref(&mut frames);
            return ff::AVPixelFormat::AV_PIX_FMT_NONE;
        }
        if !(*ctx).hw_frames_ctx.is_null() {
            ff::av_buffer_unref(&mut (*ctx).hw_frames_ctx);
        }
        (*ctx).hw_frames_ctx = frames;
        ff::AVPixelFormat::AV_PIX_FMT_D3D11
    }
}
