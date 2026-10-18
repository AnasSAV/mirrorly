//! Window chrome drawn over the picture with Direct2D/DirectWrite: the hover
//! bar (traffic lights + Home / App Switcher / Rotate), the status page shown
//! when there is no video, and the stats overlay. Same D3D11 device and lock
//! as the video draw; only drawn when something is visible.

use anyhow::Result;
use input::geom::Rect;
use windows::{
    Win32::Graphics::{
        Direct2D::{
            Common::{D2D_RECT_F, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT},
            D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
            D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_FACTORY_OPTIONS,
            D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_ROUNDED_RECT, D2D1CreateFactory, ID2D1Bitmap1,
            ID2D1DeviceContext, ID2D1Factory1, ID2D1SolidColorBrush,
        },
        Direct3D11::ID3D11Device,
        DirectWrite::{
            DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL,
            DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT_CENTER,
            DWRITE_TEXT_ALIGNMENT_LEADING, DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
        },
        Dxgi::{Common::DXGI_FORMAT_B8G8R8A8_UNORM, IDXGIDevice, IDXGISurface, IDXGISwapChain2},
    },
    core::{BOOL, HSTRING, Interface, PCWSTR, w},
};

use crate::{
    layout::{Control, Toolbar},
    onboarding::{STEPS, StepState, Welcome},
};

/// One frame's worth of chrome.
pub struct UiFrame<'a> {
    pub view: Rect,
    /// Hover bar, and the control under the pointer.
    pub toolbar: Option<(Toolbar, Option<Control>)>,
    /// Status page (title, detail) when there is no video.
    pub status: Option<(&'a str, &'a str)>,
    /// First-run checklist (drawn instead of the plain status page).
    pub welcome: Option<&'a Welcome>,
    pub stats: Option<&'a str>,
}

impl UiFrame<'_> {
    pub fn is_empty(&self) -> bool {
        self.toolbar.is_none() && self.status.is_none() && self.stats.is_none()
    }
}

pub struct Ui {
    ctx: ID2D1DeviceContext,
    mono: IDWriteTextFormat,
    title: IDWriteTextFormat,
    body: IDWriteTextFormat,
    icons: IDWriteTextFormat,
    big_icon: IDWriteTextFormat,
    light_glyph: IDWriteTextFormat,
    heading: IDWriteTextFormat,
    step_title: IDWriteTextFormat,
    step_detail: IDWriteTextFormat,
    step_num: IDWriteTextFormat,
    step_icon: IDWriteTextFormat,
    brush: ID2D1SolidColorBrush,
    target: Option<ID2D1Bitmap1>,
    scale: f32,
}

// Segoe Fluent Icons / MDL2 code points.
const GLYPH_HOME: &str = "\u{E80F}";
const GLYPH_SWITCHER: &str = "\u{E7C4}";
const GLYPH_ROTATE: &str = "\u{E7AD}";
const GLYPH_PHONE: &str = "\u{E8EA}";
const GLYPH_CHECK: &str = "\u{E73E}";

fn rgba(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

impl Ui {
    pub fn new(device: &ID3D11Device, scale: f32) -> Result<Self> {
        unsafe {
            let factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, Some(&D2D1_FACTORY_OPTIONS::default()))?;
            let dxgi: IDXGIDevice = device.cast()?;
            let ctx = factory.CreateDevice(&dxgi)?.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let icon_family = if has_family(&dw, "Segoe Fluent Icons") { w!("Segoe Fluent Icons") } else { w!("Segoe MDL2 Assets") };
            let fmt = |family: PCWSTR, weight: DWRITE_FONT_WEIGHT, size: f32| {
                dw.CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size * scale, w!("en-us"))
            };
            let mono = fmt(w!("Consolas"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 12.5)?;
            let title = fmt(w!("Segoe UI Variable Display"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 18.0)?;
            let body = fmt(w!("Segoe UI Variable Text"), DWRITE_FONT_WEIGHT_NORMAL, 13.0)?;
            let icons = fmt(icon_family, DWRITE_FONT_WEIGHT_NORMAL, 15.0)?;
            let big_icon = fmt(icon_family, DWRITE_FONT_WEIGHT_NORMAL, 44.0)?;
            let light_glyph = fmt(w!("Segoe UI"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 9.0)?;
            let heading = fmt(w!("Segoe UI Variable Display"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 21.0)?;
            let step_title = fmt(w!("Segoe UI Variable Text"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 13.5)?;
            let step_detail = fmt(w!("Segoe UI Variable Text"), DWRITE_FONT_WEIGHT_NORMAL, 12.0)?;
            let step_num = fmt(w!("Segoe UI Variable Text"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 11.5)?;
            let step_icon = fmt(icon_family, DWRITE_FONT_WEIGHT_NORMAL, 11.0)?;
            for f in [&title, &body, &icons, &big_icon, &light_glyph, &heading, &step_num, &step_icon] {
                f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            }
            for f in [&mono, &step_title, &step_detail] {
                f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
            }
            body.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
            let brush = ctx.CreateSolidColorBrush(&rgba(1.0, 1.0, 1.0, 1.0), None)?;
            Ok(Self {
                ctx,
                mono,
                title,
                body,
                icons,
                big_icon,
                light_glyph,
                heading,
                step_title,
                step_detail,
                step_num,
                step_icon,
                brush,
                target: None,
                scale,
            })
        }
    }

    /// Must be called before the swapchain buffers are resized: the D2D
    /// target holds a reference to the back buffer.
    pub fn release_target(&mut self) {
        unsafe { self.ctx.SetTarget(None) };
        self.target = None;
    }

    pub fn draw(&mut self, sc: &IDXGISwapChain2, f: &UiFrame) -> Result<()> {
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
            self.ctx.BeginDraw();
            match (f.welcome, f.status) {
                (Some(w), Some((title, detail))) => self.welcome_page(f.view, w, title, detail),
                (None, Some((title, detail))) => self.status_page(f.view, title, detail),
                _ => {}
            }
            if let Some((tb, hover)) = &f.toolbar {
                self.toolbar(tb, *hover);
            }
            if let Some(text) = f.stats {
                self.stats(f.view, f.view.y, text);
            }
            self.ctx.EndDraw(None, None)?;
        }
        Ok(())
    }

    fn color(&self, c: D2D1_COLOR_F) -> &ID2D1SolidColorBrush {
        unsafe { self.brush.SetColor(&c) };
        &self.brush
    }

    fn text(&self, s: &str, format: &IDWriteTextFormat, r: Rect, c: D2D1_COLOR_F) {
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            self.ctx.DrawText(&wide, format, &rect(r), self.color(c), D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }
    }

    fn toolbar(&self, t: &Toolbar, hover: Option<Control>) {
        let s = self.scale as f64;
        unsafe {
            // A title strip of its own, above the screen (like macOS).
            self.ctx.FillRectangle(&rect(t.bar), self.color(rgba(0.155, 0.155, 0.165, 1.0)));
            for &(c, cx, cy, r) in &t.lights {
                let col = match c {
                    Control::Close => rgba(1.0, 0.373, 0.341, 1.0),
                    Control::Minimize => rgba(0.996, 0.737, 0.180, 1.0),
                    _ => rgba(0.157, 0.784, 0.251, 1.0),
                };
                let e = D2D1_ELLIPSE { point: windows_numerics::Vector2 { X: cx as f32, Y: cy as f32 }, radiusX: r as f32, radiusY: r as f32 };
                self.ctx.FillEllipse(&e, self.color(col));
                // Like macOS, the symbols appear only while the pointer is
                // over the lights.
                if hover.is_some_and(|h| matches!(h, Control::Close | Control::Minimize | Control::Zoom)) {
                    let g = match c {
                        Control::Close => "×",
                        Control::Minimize => "−",
                        _ => "+",
                    };
                    let box_ = Rect { x: cx - r, y: cy - r - 0.5 * s, w: 2.0 * r, h: 2.0 * r };
                    self.text(g, &self.light_glyph, box_, rgba(0.25, 0.1, 0.05, 0.85));
                }
            }
            for &(c, r) in &t.icons {
                if hover == Some(c) {
                    let rr = D2D1_ROUNDED_RECT { rect: rect(r), radiusX: (6.0 * s) as f32, radiusY: (6.0 * s) as f32 };
                    self.ctx.FillRoundedRectangle(&rr, self.color(rgba(1.0, 1.0, 1.0, 0.18)));
                }
                let g = match c {
                    Control::Home => GLYPH_HOME,
                    Control::AppSwitcher => GLYPH_SWITCHER,
                    _ => GLYPH_ROTATE,
                };
                self.text(g, &self.icons, r, rgba(1.0, 1.0, 1.0, 0.95));
            }
        }
    }

    fn status_page(&self, view: Rect, title: &str, detail: &str) {
        let s = self.scale as f64;
        let cx = view.x + view.w / 2.0;
        let cy = view.y + view.h * 0.42;
        let w = view.w * 0.8;
        self.text(GLYPH_PHONE, &self.big_icon, Rect { x: cx - 40.0 * s, y: cy - 90.0 * s, w: 80.0 * s, h: 70.0 * s }, rgba(0.6, 0.6, 0.63, 1.0));
        self.text(title, &self.title, Rect { x: cx - w / 2.0, y: cy - 10.0 * s, w, h: 30.0 * s }, rgba(1.0, 1.0, 1.0, 1.0));
        self.text(detail, &self.body, Rect { x: cx - w / 2.0, y: cy + 28.0 * s, w, h: view.h * 0.4 }, rgba(0.62, 0.62, 0.66, 1.0));
    }

    /// First run: what Mirrorly is, and a live checklist of how to connect.
    fn welcome_page(&self, view: Rect, w: &Welcome, now_title: &str, now_detail: &str) {
        let s = self.scale as f64;
        let pad = 26.0 * s;
        let x = view.x + pad;
        let width = view.w - 2.0 * pad;
        let mut y = view.y + view.h * 0.09;

        // App badge: the Mirrorly blue tile with a phone.
        let badge = 58.0 * s;
        let br = Rect { x: view.x + (view.w - badge) / 2.0, y, w: badge, h: badge };
        unsafe {
            let rr = D2D1_ROUNDED_RECT { rect: rect(br), radiusX: (14.0 * s) as f32, radiusY: (14.0 * s) as f32 };
            self.ctx.FillRoundedRectangle(&rr, self.color(rgba(0.30, 0.38, 0.98, 1.0)));
        }
        self.text(GLYPH_PHONE, &self.icons, Rect { y: br.y + 2.0 * s, ..br }, rgba(1.0, 1.0, 1.0, 1.0));
        y += badge + 18.0 * s;

        self.text("Welcome to Mirrorly", &self.heading, Rect { x: view.x, y, w: view.w, h: 30.0 * s }, rgba(1.0, 1.0, 1.0, 1.0));
        y += 32.0 * s;
        self.text(
            "See and control your iPhone from this PC.",
            &self.body,
            Rect { x: view.x + pad, y, w: view.w - 2.0 * pad, h: 22.0 * s },
            rgba(0.62, 0.62, 0.66, 1.0),
        );
        y += 44.0 * s;

        let circle = 22.0 * s;
        let text_x = x + circle + 12.0 * s;
        let text_w = width - circle - 12.0 * s;
        for (i, (state, (title, detail))) in w.states().iter().zip(STEPS).enumerate() {
            let c = Rect { x, y, w: circle, h: circle };
            let e = D2D1_ELLIPSE {
                point: windows_numerics::Vector2 { X: (c.x + circle / 2.0) as f32, Y: (c.y + circle / 2.0) as f32 },
                radiusX: (circle / 2.0) as f32,
                radiusY: (circle / 2.0) as f32,
            };
            let (title_col, detail_col) = match state {
                StepState::Done => {
                    unsafe { self.ctx.FillEllipse(&e, self.color(rgba(0.20, 0.78, 0.35, 1.0))) };
                    self.text(GLYPH_CHECK, &self.step_icon, c, rgba(1.0, 1.0, 1.0, 1.0));
                    (rgba(0.75, 0.75, 0.78, 1.0), rgba(0.48, 0.48, 0.52, 1.0))
                }
                StepState::Active => {
                    unsafe { self.ctx.FillEllipse(&e, self.color(rgba(0.30, 0.47, 1.0, 1.0))) };
                    self.text(&(i + 1).to_string(), &self.step_num, c, rgba(1.0, 1.0, 1.0, 1.0));
                    (rgba(1.0, 1.0, 1.0, 1.0), rgba(0.70, 0.70, 0.74, 1.0))
                }
                StepState::Pending => {
                    unsafe { self.ctx.DrawEllipse(&e, self.color(rgba(0.40, 0.40, 0.44, 1.0)), (1.5 * s) as f32, None) };
                    self.text(&(i + 1).to_string(), &self.step_num, c, rgba(0.55, 0.55, 0.6, 1.0));
                    (rgba(0.62, 0.62, 0.66, 1.0), rgba(0.45, 0.45, 0.49, 1.0))
                }
            };
            self.text(title, &self.step_title, Rect { x: text_x, y: y + 1.0 * s, w: text_w, h: 20.0 * s }, title_col);
            self.text(detail, &self.step_detail, Rect { x: text_x, y: y + 21.0 * s, w: text_w, h: 44.0 * s }, detail_col);
            y += 70.0 * s;
        }

        // What Mirrorly is doing right now.
        y += 6.0 * s;
        self.text(now_title, &self.step_title, Rect { x, y, w: width, h: 20.0 * s }, rgba(0.45, 0.62, 1.0, 1.0));
        self.text(now_detail, &self.step_detail, Rect { x, y: y + 21.0 * s, w: width, h: 60.0 * s }, rgba(0.62, 0.62, 0.66, 1.0));
    }

    fn stats(&self, view: Rect, top: f64, text: &str) {
        let s = self.scale as f64;
        let lines = text.lines().count().max(1) as f64;
        let pad = 6.0 * s;
        let r = Rect { x: view.x, y: top, w: (380.0 * s).min(view.w), h: lines * 15.0 * s + 2.0 * pad };
        unsafe { self.ctx.FillRectangle(&rect(r), self.color(rgba(0.0, 0.0, 0.0, 0.72))) };
        let inner = Rect { x: r.x + pad, y: r.y + pad, w: r.w - 2.0 * pad, h: r.h - pad };
        self.text(text, &self.mono, inner, rgba(0.6, 1.0, 0.6, 1.0));
    }
}

fn rect(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x as f32, top: r.y as f32, right: (r.x + r.w) as f32, bottom: (r.y + r.h) as f32 }
}

fn has_family(dw: &IDWriteFactory, name: &str) -> bool {
    unsafe {
        let mut coll = None;
        if dw.GetSystemFontCollection(&mut coll, false).is_err() {
            return false;
        }
        let Some(coll) = coll else { return false };
        let (mut index, mut exists) = (0u32, BOOL(0));
        coll.FindFamilyName(&HSTRING::from(name), &mut index, &mut exists).is_ok() && exists.as_bool()
    }
}
