//! Draws the decoder's NV12 texture slice straight to the back buffer (no
//! copy): Y and UV planes are bound as two shader resource views on the
//! decoder's own texture array. Also draws the local touch indicator.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow};
use input::geom::{Rect, Rotation};
use windows::{
    Win32::Graphics::{
        Direct3D::{
            D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP, D3D_SRV_DIMENSION_TEXTURE2DARRAY,
            Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile},
            ID3DBlob,
        },
        Direct3D11::*,
        Dxgi::Common::{DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_UNORM, DXGI_FORMAT_UNKNOWN},
    },
    core::{Interface, PCSTR, s},
};

use crate::gpu::Swapchain;

const SHADERS: &str = r#"
cbuffer Quad : register(b0) {
    float4 corner[4]; // xy = NDC, zw = uv; order TL, TR, BL, BR
    float4 color;
};
struct VsOut { float4 pos : SV_Position; float2 uv : TEXCOORD0; };
VsOut vs_main(uint id : SV_VertexID) {
    VsOut o;
    o.pos = float4(corner[id].xy, 0, 1);
    o.uv = corner[id].zw;
    return o;
}
Texture2DArray<float>  tex_y  : register(t0);
Texture2DArray<float2> tex_uv : register(t1);
SamplerState smp : register(s0);
// Full-range (the stream is yuvj420p) BT.709. The stream does not signal
// its matrix; 709 is an assumption (colors looked right at GATE 1).
float4 ps_video(VsOut i) : SV_Target {
    uint w, h, n;
    tex_y.GetDimensions(w, h, n);
    float2 dx = ddx(i.uv), dy = ddy(i.uv);
    // Texels covered by one screen pixel (rotation-proof via derivatives).
    float footprint = max(length(dx * float2(w, h)), length(dy * float2(w, h)));
    float y;
    float2 c;
    if (footprint <= 1.0) {
        y = tex_y.Sample(smp, float3(i.uv, 0));
        c = tex_uv.Sample(smp, float3(i.uv, 0));
    } else {
        // Downscaling: average four bilinear taps spread over the pixel's
        // footprint (a box filter up to ~4x) instead of aliasing.
        float2 a = 0.25 * (dx + dy), b = 0.25 * (dx - dy);
        float3 p0 = float3(i.uv + a, 0), p1 = float3(i.uv - a, 0);
        float3 p2 = float3(i.uv + b, 0), p3 = float3(i.uv - b, 0);
        y = 0.25 * (tex_y.Sample(smp, p0) + tex_y.Sample(smp, p1) + tex_y.Sample(smp, p2) + tex_y.Sample(smp, p3));
        c = 0.25 * (tex_uv.Sample(smp, p0) + tex_uv.Sample(smp, p1) + tex_uv.Sample(smp, p2) + tex_uv.Sample(smp, p3));
    }
    c -= 0.5;
    float3 rgb = float3(y + 1.5748 * c.y,
                        y - 0.1873 * c.x - 0.4681 * c.y,
                        y + 1.8556 * c.x);
    return float4(saturate(rgb), 1);
}
float4 ps_dot(VsOut i) : SV_Target {
    float d = length(i.uv - 0.5) * 2.0;           // 0 at centre, 1 at the rim
    float edge = saturate((1.0 - d) * 6.0);       // soft outer edge
    float fill = d > 0.72 ? 1.0 : 0.35;           // solid ring, translucent core
    return float4(color.rgb, color.a * edge * fill);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct QuadCb {
    corner: [[f32; 4]; 4],
    color: [f32; 4],
}

pub struct Renderer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    pub swapchain: Swapchain,
    rtv: Option<ID3D11RenderTargetView>,
    vs: ID3D11VertexShader,
    ps_video: ID3D11PixelShader,
    ps_dot: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    blend: ID3D11BlendState,
    cb: ID3D11Buffer,
    /// SRVs per (texture, array slice), created once per decoder surface.
    srvs: HashMap<(usize, u32), (ID3D11ShaderResourceView, ID3D11ShaderResourceView)>,
    pub size: (u32, u32),
}

/// What to draw this frame.
pub struct Scene<'a> {
    pub picture: Option<(&'a ID3D11Texture2D, u32)>,
    /// Where the picture goes, in back-buffer pixels.
    pub view: Rect,
    /// Fraction of the texture that is real content (the rest is encoder
    /// padding and decoder alignment).
    pub uv_max: (f32, f32),
    /// How the picture is turned on screen.
    pub rotation: Rotation,
    /// Touch indicator centre in back-buffer pixels, and its radius.
    pub dot: Option<(f64, f64, f64)>,
}

impl Renderer {
    pub fn new(device: ID3D11Device, context: ID3D11DeviceContext, swapchain: Swapchain) -> Result<Self> {
        unsafe {
            let vs_code = compile(s!("vs_main"), s!("vs_5_0"))?;
            let psv_code = compile(s!("ps_video"), s!("ps_5_0"))?;
            let psd_code = compile(s!("ps_dot"), s!("ps_5_0"))?;
            let mut vs = None;
            device.CreateVertexShader(blob(&vs_code), None, Some(&mut vs))?;
            let mut ps_video = None;
            device.CreatePixelShader(blob(&psv_code), None, Some(&mut ps_video))?;
            let mut ps_dot = None;
            device.CreatePixelShader(blob(&psd_code), None, Some(&mut ps_dot))?;

            let mut sampler = None;
            device.CreateSamplerState(
                &D3D11_SAMPLER_DESC {
                    Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    MaxLOD: f32::MAX,
                    ..Default::default()
                },
                Some(&mut sampler),
            )?;

            let mut bd = D3D11_BLEND_DESC::default();
            bd.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: true.into(),
                SrcBlend: D3D11_BLEND_SRC_ALPHA,
                DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_ONE,
                DestBlendAlpha: D3D11_BLEND_ZERO,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
            };
            let mut blend = None;
            device.CreateBlendState(&bd, Some(&mut blend))?;

            let mut cb = None;
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: std::mem::size_of::<QuadCb>() as u32,
                    Usage: D3D11_USAGE_DYNAMIC,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut cb),
            )?;

            let mut r = Self {
                device,
                context,
                swapchain,
                rtv: None,
                vs: vs.unwrap(),
                ps_video: ps_video.unwrap(),
                ps_dot: ps_dot.unwrap(),
                sampler: sampler.unwrap(),
                blend: blend.unwrap(),
                cb: cb.unwrap(),
                srvs: HashMap::new(),
                size: (0, 0),
            };
            r.create_rtv()?;
            Ok(r)
        }
    }

    fn create_rtv(&mut self) -> Result<()> {
        unsafe {
            let back: ID3D11Texture2D = self.swapchain.sc.GetBuffer(0)?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            back.GetDesc(&mut desc);
            self.size = (desc.Width, desc.Height);
            let mut rtv = None;
            self.device.CreateRenderTargetView(&back, None, Some(&mut rtv))?;
            self.rtv = rtv;
        }
        Ok(())
    }

    pub fn resize(&mut self, w: u32, h: u32) -> Result<()> {
        if (w, h) == self.size || w == 0 || h == 0 {
            return Ok(());
        }
        self.rtv = None;
        unsafe {
            self.context.OMSetRenderTargets(None, None);
            self.swapchain.sc.ResizeBuffers(0, w, h, DXGI_FORMAT_UNKNOWN, self.swapchain.flags)?;
        }
        self.create_rtv()
    }

    /// The decoder was recreated: its textures are gone.
    pub fn forget_surfaces(&mut self) {
        self.srvs.clear();
    }

    pub fn draw(&mut self, scene: &Scene) -> Result<()> {
        let rtv = self.rtv.clone().ok_or_else(|| anyhow!("no render target"))?;
        let (bw, bh) = (self.size.0 as f64, self.size.1 as f64);
        unsafe {
            let ctx = &self.context.clone();
            ctx.ClearRenderTargetView(&rtv, &[0.0, 0.0, 0.0, 1.0]);
            ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: bw as f32,
                Height: bh as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            ctx.IASetInputLayout(None);
            ctx.VSSetShader(&self.vs, None);
            ctx.VSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));

            if let Some((tex, index)) = scene.picture {
                let (y, uv) = self.srv_pair(tex, index)?;
                // Each displayed corner samples the panel point it shows.
                let uvs = DISPLAY_CORNERS.map(|(u, v)| {
                    let (x, y) = scene.rotation.picture_to_panel(u, v);
                    (x as f32 * scene.uv_max.0, y as f32 * scene.uv_max.1)
                });
                self.upload(quad(scene.view, bw, bh, uvs), [0.0; 4])?;
                ctx.OMSetBlendState(None, None, 0xffff_ffff);
                ctx.PSSetShader(&self.ps_video, None);
                ctx.PSSetShaderResources(0, Some(&[Some(y), Some(uv)]));
                ctx.Draw(4, 0);
                ctx.PSSetShaderResources(0, Some(&[None, None]));
            }

            if let Some((x, y, r)) = scene.dot {
                let rect = Rect { x: x - r, y: y - r, w: 2.0 * r, h: 2.0 * r };
                let uvs = DISPLAY_CORNERS.map(|(u, v)| (u as f32, v as f32));
                self.upload(quad(rect, bw, bh, uvs), [1.0, 1.0, 1.0, 0.85])?;
                ctx.OMSetBlendState(&self.blend, Some(&[0.0; 4]), 0xffff_ffff);
                ctx.PSSetShader(&self.ps_dot, None);
                ctx.Draw(4, 0);
            }
        }
        Ok(())
    }

    fn upload(&self, corner: [[f32; 4]; 4], color: [f32; 4]) -> Result<()> {
        unsafe {
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(&self.cb, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m))?;
            *(m.pData as *mut QuadCb) = QuadCb { corner, color };
            self.context.Unmap(&self.cb, 0);
        }
        Ok(())
    }

    fn srv_pair(&mut self, tex: &ID3D11Texture2D, index: u32) -> Result<(ID3D11ShaderResourceView, ID3D11ShaderResourceView)> {
        let key = (tex.as_raw() as usize, index);
        if let Some(p) = self.srvs.get(&key) {
            return Ok(p.clone());
        }
        // A new decoder means new textures; don't let stale entries pile up.
        if self.srvs.len() > 64 {
            self.srvs.clear();
        }
        let make = |format| -> Result<ID3D11ShaderResourceView> {
            let desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
                Format: format,
                ViewDimension: D3D_SRV_DIMENSION_TEXTURE2DARRAY,
                Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2DArray: D3D11_TEX2D_ARRAY_SRV {
                        MostDetailedMip: 0,
                        MipLevels: 1,
                        FirstArraySlice: index,
                        ArraySize: 1,
                    },
                },
            };
            let mut srv = None;
            unsafe { self.device.CreateShaderResourceView(tex, Some(&desc), Some(&mut srv)) }
                .context("CreateShaderResourceView on the decoder texture (needs BIND_SHADER_RESOURCE)")?;
            Ok(srv.unwrap())
        };
        let pair = (make(DXGI_FORMAT_R8_UNORM)?, make(DXGI_FORMAT_R8G8_UNORM)?);
        self.srvs.insert(key, pair.clone());
        Ok(pair)
    }
}

/// Corner order of the triangle strip: TL, TR, BL, BR, as (u, v) in 0..1.
const DISPLAY_CORNERS: [(f64, f64); 4] = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)];

/// Corners (NDC position + uv) of `rect` in a `bw` x `bh` back buffer, with
/// one texture coordinate per corner (in `DISPLAY_CORNERS` order).
fn quad(rect: Rect, bw: f64, bh: f64, uv: [(f32, f32); 4]) -> [[f32; 4]; 4] {
    let nx = |x: f64| (x / bw * 2.0 - 1.0) as f32;
    let ny = |y: f64| (1.0 - y / bh * 2.0) as f32;
    let (l, t, r, b) = (nx(rect.x), ny(rect.y), nx(rect.x + rect.w), ny(rect.y + rect.h));
    [[l, t, uv[0].0, uv[0].1], [r, t, uv[1].0, uv[1].1], [l, b, uv[2].0, uv[2].1], [r, b, uv[3].0, uv[3].1]]
}

fn compile(entry: PCSTR, target: PCSTR) -> Result<ID3DBlob> {
    unsafe {
        let mut code = None;
        let mut errors = None;
        let r = D3DCompile(
            SHADERS.as_ptr().cast(),
            SHADERS.len(),
            s!("mirror.hlsl"),
            None,
            None,
            entry,
            target,
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut errors),
        );
        if let Err(e) = r {
            let msg = errors
                .map(|b: ID3DBlob| String::from_utf8_lossy(blob(&b)).into_owned())
                .unwrap_or_default();
            return Err(anyhow!("shader compile failed: {e} {msg}"));
        }
        Ok(code.unwrap())
    }
}

fn blob(b: &ID3DBlob) -> &[u8] {
    unsafe { std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()) }
}
