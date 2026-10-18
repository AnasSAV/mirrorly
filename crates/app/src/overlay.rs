//! Debug overlay text, drawn with Direct2D/DirectWrite straight onto the
//! swapchain back buffer (same D3D11 device, same lock as the video draw).
//! Created lazily the first time the overlay is shown.

use anyhow::Result;
use windows::{
    Win32::Graphics::{
        Direct2D::{
            Common::{D2D_RECT_F, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT},
            D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
            D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_OPTIONS,
            D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1CreateFactory, ID2D1Bitmap1, ID2D1DeviceContext,
            ID2D1Factory1, ID2D1SolidColorBrush,
        },
        Direct3D11::ID3D11Device,
        DirectWrite::{
            DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL, DWriteCreateFactory,
            IDWriteFactory, IDWriteTextFormat,
        },
        Dxgi::{Common::DXGI_FORMAT_B8G8R8A8_UNORM, IDXGIDevice, IDXGISurface, IDXGISwapChain2},
    },
    core::{Interface, w},
};

pub struct Overlay {
    ctx: ID2D1DeviceContext,
    format: IDWriteTextFormat,
    text: ID2D1SolidColorBrush,
    bg: ID2D1SolidColorBrush,
    target: Option<ID2D1Bitmap1>,
    scale: f32,
}

impl Overlay {
    pub fn new(device: &ID3D11Device, scale: f32) -> Result<Self> {
        unsafe {
            let factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, Some(&D2D1_FACTORY_OPTIONS::default()))?;
            let dxgi: IDXGIDevice = device.cast()?;
            let d2d = factory.CreateDevice(&dxgi)?;
            let ctx = d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let format = dw.CreateTextFormat(
                w!("Consolas"),
                None,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                12.5 * scale,
                w!("en-us"),
            )?;
            let text = ctx.CreateSolidColorBrush(&D2D1_COLOR_F { r: 0.6, g: 1.0, b: 0.6, a: 1.0 }, None)?;
            let bg = ctx.CreateSolidColorBrush(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.72 }, None)?;
            Ok(Self { ctx, format, text, bg, target: None, scale })
        }
    }

    /// Must be called before the swapchain buffers are resized: the D2D
    /// target holds a reference to the back buffer.
    pub fn release_target(&mut self) {
        unsafe { self.ctx.SetTarget(None) };
        self.target = None;
    }

    /// Draw `text` in the top-left corner of the current back buffer.
    pub fn draw(&mut self, sc: &IDXGISwapChain2, text: &str) -> Result<()> {
        unsafe {
            if self.target.is_none() {
                let surface: IDXGISurface = sc.GetBuffer(0)?;
                let props = D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                    dpiX: 96.0,
                    dpiY: 96.0,
                    bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                    ..Default::default()
                };
                self.target = Some(self.ctx.CreateBitmapFromDxgiSurface(&surface, Some(&props))?);
            }
            if let Some(t) = &self.target {
                self.ctx.SetTarget(t);
            }
            let wide: Vec<u16> = text.encode_utf16().collect();
            let lines = text.lines().count().max(1) as f32;
            let pad = 6.0 * self.scale;
            let w = 380.0 * self.scale;
            let h = lines * 15.0 * self.scale + 2.0 * pad;
            self.ctx.BeginDraw();
            self.ctx.FillRectangle(&D2D_RECT_F { left: 0.0, top: 0.0, right: w, bottom: h }, &self.bg);
            self.ctx.DrawText(
                &wide,
                &self.format,
                &D2D_RECT_F { left: pad, top: pad, right: w - pad, bottom: h },
                &self.text,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            self.ctx.EndDraw(None, None)?;
        }
        Ok(())
    }
}
