//! Capturing the virtual monitor: DXGI Desktop Duplication on the adapter
//! that drives it. The picture stays on the GPU: when the screen changes,
//! the GPU's video processor converts it to NV12 (BT.709, video range),
//! which is copied on the GPU into the encoder's own surface when it takes
//! GPU frames, or read back for one that doesn't.
//!
//! The mouse pointer is not in the picture. Desktop Duplication reports it
//! separately (its shape when that changes, its position when it moves),
//! and so does `next`: the iPad draws it over the video, so moving the
//! mouse costs no encoding and shows without the video's delay.
//!
//! Desktop Duplication only hands out a frame when the screen or the
//! pointer changed, so a still screen costs nothing.

use std::{mem::ManuallyDrop, time::Instant};

use anyhow::{Context, Result, bail};
use windows::{
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Direct3D11::{
                D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION,
                D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_COLOR_SPACE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
                D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
                D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
                D3D11_VIDEO_USAGE_OPTIMAL_SPEED, D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D,
                D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Texture2D, ID3D11VideoContext,
                ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
            },
            Dxgi::{
                Common::{DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC},
                CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_NOT_FOUND, DXGI_ERROR_WAIT_TIMEOUT,
                DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_POINTER_SHAPE_INFO, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput1,
                IDXGIOutputDuplication, IDXGIResource,
            },
        },
    },
    core::Interface,
};

use crate::cursor;

/// The duplication was lost (secure desktop, mode change): make a new
/// `Capture`.
#[derive(Debug)]
pub struct AccessLost;

impl std::fmt::Display for AccessLost {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("screen capture was interrupted (secure desktop or display change)")
    }
}

impl std::error::Error for AccessLost {}

/// What changed, from one call to `next`.
#[derive(Default)]
pub struct Update {
    /// The screen changed: when Windows handed it over.
    pub picture: Option<Instant>,
    /// The pointer moved, appeared or went (see `Capture::pointer_at`).
    pub pointer_moved: bool,
    /// The pointer has a new shape (None inside: it has none).
    pub shape: Option<Option<cursor::Shape>>,
}

pub struct Capture {
    pub width: u32,
    pub height: u32,
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    /// The last screen image (BGRA).
    screen: ID3D11Texture2D,
    /// `screen` as NV12, made again only when the screen changed: copied to
    /// an encoder surface, or read back.
    nv12: ID3D11Texture2D,
    /// The screen changed since `nv12` was made.
    stale: bool,
    staging: ID3D11Texture2D,
    vctx: ID3D11VideoContext,
    vp: ID3D11VideoProcessor,
    input: ID3D11VideoProcessorInputView,
    output: ID3D11VideoProcessorOutputView,
    /// Top-left of the pointer image; None while it is on another monitor
    /// or hidden.
    pointer_at: Option<(i32, i32)>,
}

// Used from the capture thread only.
unsafe impl Send for Capture {}

fn wide(s: &[u16]) -> String {
    String::from_utf16_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())])
}

/// The DXGI output for a GDI device name, with its adapter.
fn output_for(device: &str) -> Result<(IDXGIAdapter1, IDXGIOutput1)> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        for a in 0.. {
            let adapter = match factory.EnumAdapters1(a) {
                Ok(x) => x,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(e) => return Err(e.into()),
            };
            for o in 0.. {
                let output = match adapter.EnumOutputs(o) {
                    Ok(x) => x,
                    Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                    Err(e) => return Err(e.into()),
                };
                if wide(&output.GetDesc()?.DeviceName) == device {
                    return Ok((adapter, output.cast()?));
                }
            }
        }
    }
    bail!("no DXGI output for {device}")
}

fn texture(device: &ID3D11Device, w: u32, h: u32, format: DXGI_FORMAT, bind: u32, staging: bool) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: if staging { D3D11_USAGE_STAGING } else { D3D11_USAGE_DEFAULT },
        BindFlags: bind,
        CPUAccessFlags: if staging { D3D11_CPU_ACCESS_READ.0 as u32 } else { 0 },
        MiscFlags: 0,
    };
    let mut t = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut t))? };
    t.context("CreateTexture2D")
}

impl Capture {
    /// Duplicate the monitor with this GDI device name.
    pub fn new(device_name: &str) -> Result<Self> {
        let (adapter, output) = output_for(device_name)?;
        unsafe {
            let mut device = None;
            let mut ctx = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )
            .context("D3D11CreateDevice on the virtual display's adapter")?;
            let (device, ctx): (ID3D11Device, ID3D11DeviceContext) = (device.context("device")?, ctx.context("context")?);
            // The encoder's runtime may use the device from its own threads.
            if let Ok(mt) = device.cast::<ID3D11Multithread>() {
                let _ = mt.SetMultithreadProtected(true);
            }
            let dup = output.DuplicateOutput(&device).context("DuplicateOutput")?;
            let d = output.GetDesc()?.DesktopCoordinates;
            let (w, h) = ((d.right - d.left) as u32 & !1, (d.bottom - d.top) as u32 & !1);

            let rt = D3D11_BIND_RENDER_TARGET.0 as u32;
            let screen = texture(&device, w, h, DXGI_FORMAT_B8G8R8A8_UNORM, rt | D3D11_BIND_SHADER_RESOURCE.0 as u32, false)?;
            let nv12 = texture(&device, w, h, DXGI_FORMAT_NV12, rt, false)?;
            let staging = texture(&device, w, h, DXGI_FORMAT_NV12, 0, true)?;

            let vdev: ID3D11VideoDevice = device.cast()?;
            let vctx: ID3D11VideoContext = ctx.cast()?;
            let rate = DXGI_RATIONAL { Numerator: 60, Denominator: 1 };
            let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: rate,
                InputWidth: w,
                InputHeight: h,
                OutputFrameRate: rate,
                OutputWidth: w,
                OutputHeight: h,
                Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
            };
            let venum = vdev.CreateVideoProcessorEnumerator(&content).context("video processor enumerator")?;
            let vp = vdev.CreateVideoProcessor(&venum, 0).context("video processor")?;
            // Input: full-range RGB. Output: BT.709 YCbCr, 16-235 (what the
            // encoder signals in its VUI).
            vctx.VideoProcessorSetStreamColorSpace(&vp, 0, &D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 });
            vctx.VideoProcessorSetOutputColorSpace(&vp, &D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: (1 << 2) | (1 << 4) });
            let mut input = None;
            vdev.CreateVideoProcessorInputView(
                &screen,
                &venum,
                &D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                    FourCC: 0,
                    ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 } },
                },
                Some(&mut input),
            )?;
            let mut output = None;
            vdev.CreateVideoProcessorOutputView(
                &nv12,
                &venum,
                &D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
                },
                Some(&mut output),
            )?;
            tracing::info!("capturing {device_name} ({w}x{h})");
            Ok(Self {
                width: w,
                height: h,
                device,
                ctx,
                dup,
                screen,
                nv12,
                stale: true,
                staging,
                vctx,
                vp,
                input: input.context("input view")?,
                output: output.context("output view")?,
                pointer_at: None,
            })
        }
    }

    /// The D3D11 device the picture lives on (for a GPU encoder).
    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }

    /// Top-left of the pointer image on this monitor, if it is here.
    pub fn pointer_at(&self) -> Option<(i32, i32)> {
        self.pointer_at
    }

    /// Wait up to `timeout_ms` for the screen or the pointer to change, and
    /// take the change in. None: nothing changed.
    pub fn next(&mut self, timeout_ms: u32) -> Result<Option<Update>> {
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res: Option<IDXGIResource> = None;
            match self.dup.AcquireNextFrame(timeout_ms, &mut info, &mut res) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
                Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => return Err(AccessLost.into()),
                Err(e) => return Err(e).context("AcquireNextFrame"),
            }
            let acquired = Instant::now();
            let mut update = Update::default();
            // Pointer-only updates carry no new picture.
            if info.LastPresentTime != 0
                && let Some(res) = &res
            {
                let frame: ID3D11Texture2D = res.cast()?;
                self.ctx.CopySubresourceRegion(&self.screen, 0, 0, 0, 0, &frame, 0, None);
                self.stale = true;
                update.picture = Some(acquired);
            }
            if info.LastMouseUpdateTime != 0 {
                let p = info.PointerPosition;
                let at = p.Visible.as_bool().then_some((p.Position.x, p.Position.y));
                update.pointer_moved = at != self.pointer_at;
                self.pointer_at = at;
            }
            if info.PointerShapeBufferSize > 0 {
                let mut buf = vec![0u8; info.PointerShapeBufferSize as usize];
                let (mut needed, mut shape) = (0u32, DXGI_OUTDUPL_POINTER_SHAPE_INFO::default());
                if self.dup.GetFramePointerShape(buf.len() as u32, buf.as_mut_ptr().cast(), &mut needed, &mut shape).is_ok() {
                    update.shape = Some(cursor::Shape::decode(shape.Type, shape.Width as usize, shape.Height as usize, shape.Pitch as usize, &buf));
                }
            }
            let _ = self.dup.ReleaseFrame();
            Ok(Some(update))
        }
    }

    /// The screen as NV12 in `nv12`, converted only if it changed.
    fn convert(&mut self) -> Result<()> {
        if !self.stale {
            return Ok(());
        }
        unsafe {
            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: ManuallyDrop::new(Some(self.input.clone())),
                ..Default::default()
            };
            let r = self.vctx.VideoProcessorBlt(&self.vp, &self.output, 0, std::slice::from_ref(&stream));
            drop(ManuallyDrop::into_inner(stream.pInputSurface));
            r.context("VideoProcessorBlt")?;
        }
        self.stale = false;
        Ok(())
    }

    /// The current picture, copied on the GPU into slice `index` of an NV12
    /// texture (an encoder's surface; it may be larger than the picture).
    pub fn render_into(&mut self, target: &ID3D11Texture2D, index: u32) -> Result<()> {
        self.convert()?;
        // One subresource per array slice (no mips): both NV12 planes go.
        unsafe { self.ctx.CopySubresourceRegion(target, index, 0, 0, 0, &self.nv12, 0, None) };
        Ok(())
    }

    /// The current picture as NV12 in system memory (Y rows, then UV rows,
    /// tightly packed), for encoders that don't take GPU frames.
    pub fn read_nv12(&mut self, nv12: &mut Vec<u8>) -> Result<()> {
        self.convert()?;
        unsafe {
            self.ctx.CopyResource(&self.staging, &self.nv12);
            let mut map = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut map)).context("Map")?;
            let (w, h, pitch) = (self.width as usize, self.height as usize, map.RowPitch as usize);
            nv12.resize(w * h * 3 / 2, 0);
            let src = map.pData as *const u8;
            // Y plane (h rows), then the interleaved UV plane (h/2 rows).
            for row in 0..h + h / 2 {
                std::ptr::copy_nonoverlapping(src.add(row * pitch), nv12.as_mut_ptr().add(row * w), w);
            }
            self.ctx.Unmap(&self.staging, 0);
        }
        Ok(())
    }
}
