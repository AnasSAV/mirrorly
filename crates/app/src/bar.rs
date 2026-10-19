//! The hover bar, in a window of its own just above the mirror window, like
//! macOS iPhone Mirroring.
//!
//! It used to be a strip of the mirror window, which grew upward by the
//! bar's height whenever the pointer came near. Every resize showed the old
//! picture at the new size and place for a frame or two before the next
//! Present (a visible flicker), and it could not be animated. As its own
//! layered window (per-pixel alpha, drawn with Direct2D into a DIB) the bar
//! fades and slides in, the mirror window never changes size, and the video
//! swapchain is left alone.
//!
//! The window is owned by the mirror window (stays above it, minimises with
//! it) and never takes activation, so keyboard input stays with the mirror.
//! Its mouse input is handed to the app as [`BarInput`]. It follows the
//! mirror window from inside the mirror's own window procedure (a subclass),
//! because while the window is being dragged the system's move loop runs
//! and the app's "moved" events only arrive when the drag ends.

use std::{
    cell::Cell,
    time::{Duration, Instant},
};

use anyhow::Result;
use input::geom::Rect;
use windows::{
    Win32::{
        Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::{
            Direct2D::{
                Common::{D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT},
                D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_RENDER_TARGET_PROPERTIES,
                D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT, D2D1CreateFactory,
                ID2D1DCRenderTarget, ID2D1Factory, ID2D1SolidColorBrush,
            },
            DirectWrite::{
                DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
                DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory,
                IDWriteFactory, IDWriteTextFormat,
            },
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Gdi::{
                AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC,
                CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC, HGDIOBJ, SelectObject,
            },
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent},
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, GetWindowRect, HTCLIENT,
                IDC_ARROW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos, WINDOWPOS,
                WM_WINDOWPOSCHANGED,
                LoadCursorW, MA_NOACTIVATE, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, SetCursor, SetWindowLongPtrW,
                ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WM_LBUTTONDOWN, WM_MOUSEACTIVATE, WM_MOUSEMOVE,
                WM_SETCURSOR, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
            },
        },
    },
    core::{BOOL, HSTRING, PCWSTR, w},
};

use crate::{
    layout::{self, Control, Toolbar},
    state::Shelf,
};

/// Fade + slide in, and out (a little quicker).
const SHOW: Duration = Duration::from_millis(180);
const HIDE: Duration = Duration::from_millis(140);
/// How far the bar travels while it fades, logical px.
const SLIDE: f64 = 7.0;
/// The bar reaches this far down over the mirror window's dark frame, so it
/// meets the window without a seam at the rounded corners (logical px; less
/// than the frame, so the screen's top edge stays uncovered).
pub const OVERLAP: f64 = 5.0;

/// Sent after TrackMouseEvent(TME_LEAVE) when the pointer leaves (WinUser.h).
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Mouse input on the bar, in its client pixels.
#[derive(Debug, Clone, Copy)]
pub enum BarInput {
    Move(f64, f64),
    Leave,
    Down(f64, f64),
}

/// What the bar shows; it is redrawn when this changes.
#[derive(Debug, Clone, PartialEq)]
pub struct BarState {
    pub hover: Option<Control>,
    pub shelf: Option<String>,
    pub shelf_media: bool,
    pub disconnected: bool,
    /// An iPad is connected: offer "Use as display".
    pub display: bool,
}

struct Proc {
    on_input: Box<dyn Fn(BarInput)>,
    tracking: bool,
}

/// Shared with the mirror window's subclass: the bar, and where it sits
/// relative to the mirror window's top-left.
struct Follow {
    bar: HWND,
    offset: Cell<(i32, i32)>,
}

const FOLLOW_ID: usize = 0x4D42_4152; // "MBAR"

/// Runs inside the mirror window's procedure: move the bar along with
/// every move of the window, as it happens.
unsafe extern "system" fn follow_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, data: usize) -> LRESULT {
    unsafe {
        if msg == WM_WINDOWPOSCHANGED && data != 0 {
            let pos = &*(lp.0 as *const WINDOWPOS);
            if (pos.flags & SWP_NOMOVE).0 == 0 {
                let f = &*(data as *const Follow);
                let (dx, dy) = f.offset.get();
                let _ = SetWindowPos(f.bar, None, pos.x + dx, pos.y + dy, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
        DefSubclassProc(hwnd, msg, wp, lp)
    }
}

pub struct Bar {
    hwnd: HWND,
    owner: HWND,
    follow: Box<Follow>,
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    size: (i32, i32),
    rt: ID2D1DCRenderTarget,
    fonts: Fonts,
    brush: Option<ID2D1SolidColorBrush>,
    scale: f64,
    /// Top-left of the bar when fully shown, screen px.
    at: (i32, i32),
    /// 0 hidden .. 1 shown, and where it is heading.
    shown: f64,
    anim: Option<(f64, f64, Instant, Duration)>,
    drawn: Option<BarState>,
}

struct Fonts {
    icons: IDWriteTextFormat,
    light_glyph: IDWriteTextFormat,
    chip: IDWriteTextFormat,
    chip_icon: IDWriteTextFormat,
}

// Segoe Fluent Icons / MDL2 code points (as in ui.rs).
const GLYPH_HOME: &str = "\u{E80F}";
const GLYPH_SWITCHER: &str = "\u{E7C4}";
const GLYPH_ROTATE: &str = "\u{E7AD}";
const GLYPH_POWER: &str = "\u{E7E8}";
const GLYPH_CONNECT: &str = "\u{E703}";
const GLYPH_PHOTO: &str = "\u{EB9F}";
const GLYPH_DOCUMENT: &str = "\u{E8A5}";
const GLYPH_DISPLAY: &str = "\u{E7F4}";

fn rgba(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

fn rect(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x as f32, top: r.y as f32, right: (r.x + r.w) as f32, bottom: (r.y + r.h) as f32 }
}

unsafe extern "system" fn bar_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Proc;
        let xy = || ((lp.0 & 0xFFFF) as i16 as f64, ((lp.0 >> 16) & 0xFFFF) as i16 as f64);
        match msg {
            // Clicking the bar must not take the keyboard from the mirror.
            WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
            WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
                SetCursor(LoadCursorW(None, IDC_ARROW).ok());
                return LRESULT(1);
            }
            WM_MOUSEMOVE if !p.is_null() => {
                if !(*p).tracking {
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    (*p).tracking = TrackMouseEvent(&mut t).is_ok();
                }
                let (x, y) = xy();
                ((*p).on_input)(BarInput::Move(x, y));
                return LRESULT(0);
            }
            WM_MOUSELEAVE if !p.is_null() => {
                (*p).tracking = false;
                ((*p).on_input)(BarInput::Leave);
                return LRESULT(0);
            }
            WM_LBUTTONDOWN if !p.is_null() => {
                let (x, y) = xy();
                ((*p).on_input)(BarInput::Down(x, y));
                return LRESULT(0);
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
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

impl Bar {
    /// Create the (hidden) bar for the mirror window `owner`.
    pub fn new(owner: isize, scale: f64, on_input: impl Fn(BarInput) + 'static) -> Result<Self> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = w!("MirrorlyBar");
            let wc = WNDCLASSW { lpfnWndProc: Some(bar_proc), hInstance: instance.into(), lpszClassName: class, ..Default::default() };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class,
                w!("Mirrorly bar"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                Some(HWND(owner as _)),
                None,
                Some(instance.into()),
                None,
            )?;
            let proc = Box::into_raw(Box::new(Proc { on_input: Box::new(on_input), tracking: false }));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, proc as isize);
            let owner = HWND(owner as _);
            let follow = Box::new(Follow { bar: hwnd, offset: Cell::new((0, 0)) });
            let _ = SetWindowSubclass(owner, Some(follow_proc), FOLLOW_ID, &*follow as *const Follow as usize);

            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                ..Default::default()
            };
            let rt = factory.CreateDCRenderTarget(&props)?;
            let dc = CreateCompatibleDC(None);
            Ok(Self {
                hwnd,
                owner,
                follow,
                dc,
                bitmap: HBITMAP::default(),
                old: HGDIOBJ::default(),
                size: (0, 0),
                rt,
                fonts: Fonts::new(scale)?,
                brush: None,
                scale,
                at: (0, 0),
                shown: 0.0,
                anim: None,
                drawn: None,
            })
        }
    }

    /// Height of the bar itself (without the overlap), physical px.
    pub fn height(&self) -> f64 {
        (layout::BAR_HEIGHT * self.scale).round()
    }

    /// The bar's controls, in its client pixels.
    pub fn toolbar(&self, shelf: bool, display: bool) -> Toolbar {
        let w = self.size.0.max(1) as f64;
        layout::toolbar(Rect { x: 0.0, y: 0.0, w, h: self.height() }, self.scale, shelf, display)
    }

    /// Sit above the mirror window whose client area is `client` (screen
    /// px), or inside its top when there is no room above (`top_limit`).
    pub fn place(&mut self, client: RECT, top_limit: i32, scale: f64) {
        if (scale - self.scale).abs() > 1e-6 {
            self.scale = scale;
            if let Ok(f) = Fonts::new(scale) {
                self.fonts = f;
            }
            self.drawn = None;
        }
        let h = self.height() as i32;
        let overlap = (OVERLAP * self.scale).round() as i32;
        let w = client.right - client.left;
        let y = (client.top - h).max(top_limit);
        self.at = (client.left, y);
        if self.size != (w, h + overlap) {
            self.resize(w, h + overlap);
            self.drawn = None;
        }
        self.update();
    }

    fn resize(&mut self, w: i32, h: i32) {
        unsafe {
            if !self.bitmap.is_invalid() {
                SelectObject(self.dc, self.old);
                let _ = DeleteObject(self.bitmap.into());
            }
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w.max(1),
                    biHeight: -h.max(1),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            if let Ok(b) = CreateDIBSection(Some(self.dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                self.bitmap = b;
                self.old = SelectObject(self.dc, b.into());
            }
            self.size = (w, h);
        }
    }

    /// Show or hide, animated from wherever it is now.
    pub fn show(&mut self, on: bool) {
        let target = if on { 1.0 } else { 0.0 };
        let heading = self.anim.map(|a| a.1).unwrap_or(self.shown);
        if heading == target {
            return;
        }
        // Keep the remaining time proportional to the distance left.
        let full = if on { SHOW } else { HIDE };
        let dur = full.mul_f64((target - self.shown).abs().max(0.05));
        self.anim = Some((self.shown, target, Instant::now(), dur));
        self.update();
    }

    pub fn is_shown(&self) -> bool {
        self.shown > 0.0 || self.anim.is_some_and(|a| a.1 > 0.0)
    }

    /// Advance the animation; true while it is still running.
    pub fn tick(&mut self) -> bool {
        let Some((from, to, start, dur)) = self.anim else { return false };
        let t = (start.elapsed().as_secs_f64() / dur.as_secs_f64().max(1e-3)).min(1.0);
        // Ease out when appearing, ease in when leaving.
        let e = if to > from { 1.0 - (1.0 - t).powi(3) } else { t * t };
        self.shown = from + (to - from) * e;
        if t >= 1.0 {
            self.shown = to;
            self.anim = None;
        }
        self.update();
        self.anim.is_some()
    }

    /// Redraw if what the bar shows changed.
    pub fn sync(&mut self, state: &BarState) {
        if self.drawn.as_ref() == Some(state) {
            return;
        }
        if self.draw(state).is_ok() {
            self.drawn = Some(state.clone());
            self.update();
        }
    }

    /// Push position and opacity to the window.
    fn update(&mut self) {
        unsafe {
            if self.shown <= 0.0 && self.anim.is_none() {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
                return;
            }
            if self.drawn.is_none() || self.bitmap.is_invalid() {
                return;
            }
            let slide = ((1.0 - self.shown) * SLIDE * self.scale).round() as i32;
            let dst = POINT { x: self.at.0, y: self.at.1 + slide };
            // Where the mirror window's subclass keeps it while dragging.
            let mut owner = RECT::default();
            if GetWindowRect(self.owner, &mut owner).is_ok() {
                self.follow.offset.set((dst.x - owner.left, dst.y - owner.top));
            }
            let size = SIZE { cx: self.size.0, cy: self.size.1 };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: (self.shown.clamp(0.0, 1.0) * 255.0).round() as u8,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&dst),
                Some(&size),
                Some(self.dc),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
    }

    fn color(&mut self, c: D2D1_COLOR_F) -> Result<ID2D1SolidColorBrush> {
        unsafe {
            if self.brush.is_none() {
                self.brush = Some(self.rt.CreateSolidColorBrush(&c, None)?);
            }
            let b = self.brush.clone().unwrap();
            b.SetColor(&c);
            Ok(b)
        }
    }

    fn text(&mut self, s: &str, f: &IDWriteTextFormat, r: Rect, c: D2D1_COLOR_F) -> Result<()> {
        let wide: Vec<u16> = s.encode_utf16().collect();
        let brush = self.color(c)?;
        unsafe { self.rt.DrawText(&wide, f, &rect(r), &brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL) };
        Ok(())
    }

    fn draw(&mut self, st: &BarState) -> Result<()> {
        let s = self.scale;
        let (w, h) = (self.size.0 as f64, self.size.1 as f64);
        let t = self.toolbar(st.shelf.is_some(), st.display);
        unsafe {
            let bounds = RECT { left: 0, top: 0, right: self.size.0, bottom: self.size.1 };
            self.rt.BindDC(self.dc, &bounds)?;
            self.rt.BeginDraw();
            self.rt.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
            // Rounded on top; square where it meets the window below.
            let body = rgba(0.155, 0.155, 0.165, 1.0);
            let r = (12.0 * s) as f32;
            let brush = self.color(body)?;
            let rr = D2D1_ROUNDED_RECT { rect: rect(Rect { x: 0.0, y: 0.0, w, h }), radiusX: r, radiusY: r };
            self.rt.FillRoundedRectangle(&rr, &brush);
            self.rt.FillRectangle(&rect(Rect { x: 0.0, y: h / 2.0, w, h: h / 2.0 }), &brush);
            // A hairline where the bar ends and the window's frame begins.
            let line = self.color(rgba(0.0, 0.0, 0.0, 0.35))?;
            self.rt.FillRectangle(&rect(Rect { x: 0.0, y: t.bar.h - 1.0, w, h: 1.0 }), &line);

            for &(c, cx, cy, r) in &t.lights {
                let col = match c {
                    Control::Close => rgba(1.0, 0.373, 0.341, 1.0),
                    Control::Minimize => rgba(0.996, 0.737, 0.180, 1.0),
                    _ => rgba(0.157, 0.784, 0.251, 1.0),
                };
                let e = D2D1_ELLIPSE { point: windows_numerics::Vector2 { X: cx as f32, Y: cy as f32 }, radiusX: r as f32, radiusY: r as f32 };
                let b = self.color(col)?;
                self.rt.FillEllipse(&e, &b);
                // Like macOS, the symbols appear only while the pointer is
                // over the lights.
                if st.hover.is_some_and(|h| matches!(h, Control::Close | Control::Minimize | Control::Zoom)) {
                    let g = match c {
                        Control::Close => "×",
                        Control::Minimize => "−",
                        _ => "+",
                    };
                    let f = self.fonts.light_glyph.clone();
                    self.text(g, &f, Rect { x: cx - r, y: cy - r - 0.5 * s, w: 2.0 * r, h: 2.0 * r }, rgba(0.25, 0.1, 0.05, 0.85))?;
                }
            }
            for &(c, r) in &t.icons {
                if st.hover == Some(c) {
                    let rr = D2D1_ROUNDED_RECT { rect: rect(r), radiusX: (6.0 * s) as f32, radiusY: (6.0 * s) as f32 };
                    let b = self.color(rgba(1.0, 1.0, 1.0, 0.18))?;
                    self.rt.FillRoundedRectangle(&rr, &b);
                }
                let g = match c {
                    Control::Home => GLYPH_HOME,
                    Control::AppSwitcher => GLYPH_SWITCHER,
                    Control::Power if st.disconnected => GLYPH_CONNECT,
                    Control::Power => GLYPH_POWER,
                    Control::Display => GLYPH_DISPLAY,
                    _ => GLYPH_ROTATE,
                };
                let f = self.fonts.icons.clone();
                self.text(g, &f, r, rgba(1.0, 1.0, 1.0, 0.95))?;
            }
            if let (Some(r), Some(label)) = (t.shelf, &st.shelf) {
                let a = if st.hover == Some(Control::Shelf) { 1.0 } else { 0.85 };
                let rr = D2D1_ROUNDED_RECT { rect: rect(r), radiusX: (r.h / 2.0) as f32, radiusY: (r.h / 2.0) as f32 };
                let b = self.color(rgba(0.30, 0.38, 0.98, a))?;
                self.rt.FillRoundedRectangle(&rr, &b);
                let icon = Rect { x: r.x + 6.0 * s, y: r.y, w: 18.0 * s, h: r.h };
                let glyph = if st.shelf_media { GLYPH_PHOTO } else { GLYPH_DOCUMENT };
                let f = self.fonts.chip_icon.clone();
                self.text(glyph, &f, icon, rgba(1.0, 1.0, 1.0, 1.0))?;
                let lr = Rect { x: icon.x + icon.w + 3.0 * s, y: r.y, w: r.w - icon.w - 18.0 * s, h: r.h };
                let f = self.fonts.chip.clone();
                self.text(label, &f, lr, rgba(1.0, 1.0, 1.0, 1.0))?;
            }
            self.rt.EndDraw(None, None)?;
        }
        Ok(())
    }
}

impl Drop for Bar {
    fn drop(&mut self) {
        unsafe {
            let _ = RemoveWindowSubclass(self.owner, Some(follow_proc), FOLLOW_ID);
            let p = GetWindowLongPtrW(self.hwnd, GWLP_USERDATA) as *mut Proc;
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            if !p.is_null() {
                drop(Box::from_raw(p));
            }
            if !self.bitmap.is_invalid() {
                SelectObject(self.dc, self.old);
                let _ = DeleteObject(self.bitmap.into());
            }
            let _ = DeleteDC(self.dc);
        }
    }
}

impl Fonts {
    fn new(scale: f64) -> Result<Self> {
        unsafe {
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let icon_family = if has_family(&dw, "Segoe Fluent Icons") { w!("Segoe Fluent Icons") } else { w!("Segoe MDL2 Assets") };
            let scale = scale as f32;
            let fmt = |family: PCWSTR, weight: DWRITE_FONT_WEIGHT, size: f32| {
                dw.CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size * scale, w!("en-us"))
            };
            let icons = fmt(icon_family, DWRITE_FONT_WEIGHT_NORMAL, 15.0)?;
            let light_glyph = fmt(w!("Segoe UI"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 9.0)?;
            let chip = fmt(w!("Segoe UI Variable Text"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 12.0)?;
            let chip_icon = fmt(icon_family, DWRITE_FONT_WEIGHT_NORMAL, 11.0)?;
            for f in [&icons, &light_glyph, &chip_icon] {
                f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            }
            chip.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
            chip.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            chip.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
            chip.SetTrimming(&trim, None)?;
            Ok(Self { icons, light_glyph, chip, chip_icon })
        }
    }
}

/// The bar state for the current chrome.
pub fn state_of(c: &crate::state::Chrome, hover: Option<Control>, display: bool) -> BarState {
    let shelf: Option<&Shelf> = c.shelf.as_ref();
    BarState {
        hover,
        shelf: shelf.map(|s| s.label.clone()),
        shelf_media: shelf.is_some_and(|s| s.media),
        disconnected: c.disconnected,
        display,
    }
}
