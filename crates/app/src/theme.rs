//! Titanium & Ice: Mirrorly's colours, from its logo, in a light and a dark
//! version. Windows' app mode picks one, and the window follows it live.
//!
//! Ice itself is too pale for text on a light page, so light mode draws
//! accent text in a deeper ice. Buttons stay true Ice with dark labels in
//! both. The window's traffic lights, the record button and the "plugged
//! in" dot keep their usual red, yellow and green: those mean something.

use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

#[derive(Debug)]
pub struct Theme {
    pub light: bool,
    /// The frame around the phone's screen, like the edge of the phone.
    pub frame: [f32; 3],
    /// Behind a page shown instead of the phone's screen.
    pub page: D2D1_COLOR_F,
    pub text: D2D1_COLOR_F,
    pub text_soft: D2D1_COLOR_F,
    pub text_faint: D2D1_COLOR_F,
    /// Button and chip fill, and what is written on it.
    pub accent: D2D1_COLOR_F,
    pub on_accent: D2D1_COLOR_F,
    /// Accent as text, icons and outlines on the page.
    pub accent_ink: D2D1_COLOR_F,
    /// A row on a page, and that row under the pointer.
    pub row: D2D1_COLOR_F,
    pub row_hover: D2D1_COLOR_F,
    /// A checklist step that is done, and its tick.
    pub done: D2D1_COLOR_F,
    pub on_done: D2D1_COLOR_F,
    /// A checklist step still to come: its circle.
    pub outline: D2D1_COLOR_F,
    /// Over the picture while files are dragged onto it.
    pub drop_scrim: D2D1_COLOR_F,
    /// Short messages: the pill and its text.
    pub toast: D2D1_COLOR_F,
    pub on_toast: D2D1_COLOR_F,
    /// The hover bar: body, the line above the window, icons, icon under the
    /// pointer.
    pub bar: D2D1_COLOR_F,
    pub bar_line: D2D1_COLOR_F,
    pub bar_icon: D2D1_COLOR_F,
    pub bar_hover: D2D1_COLOR_F,
}

const fn hex(rgb: u32) -> D2D1_COLOR_F {
    alpha(rgb, 1.0)
}

const fn alpha(rgb: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((rgb >> 16) & 255) as f32 / 255.0,
        g: ((rgb >> 8) & 255) as f32 / 255.0,
        b: (rgb & 255) as f32 / 255.0,
        a,
    }
}

const fn rgb(rgb: u32) -> [f32; 3] {
    let c = hex(rgb);
    [c.r, c.g, c.b]
}

/// The logo's Ice, and what reads on it.
const ICE: u32 = 0x8DC2D6;
const ON_ICE: u32 = 0x14191D;

pub static DARK: Theme = Theme {
    light: false,
    frame: rgb(0x1A1F24),
    page: hex(0x14181C),
    text: hex(0xE9F0F4),
    text_soft: hex(0xA3B3BE),
    text_faint: hex(0x6F808D),
    accent: hex(ICE),
    on_accent: hex(ON_ICE),
    accent_ink: hex(ICE),
    row: alpha(0xFFFFFF, 0.06),
    row_hover: alpha(0xFFFFFF, 0.12),
    done: hex(0x37525F),
    on_done: hex(0xD2EDF5),
    outline: hex(0x4A5863),
    drop_scrim: alpha(0x14181C, 0.86),
    toast: alpha(0x23282E, 0.95),
    on_toast: hex(0xE9F0F4),
    bar: hex(0x262D33),
    bar_line: alpha(0x000000, 0.35),
    bar_icon: alpha(0xE9F0F4, 0.95),
    bar_hover: alpha(0xFFFFFF, 0.16),
};

pub static LIGHT: Theme = Theme {
    light: true,
    frame: rgb(0xC9D3DA),
    page: hex(0xF3F6F8),
    text: hex(0x20272D),
    text_soft: hex(0x56646F),
    text_faint: hex(0x84929C),
    accent: hex(ICE),
    on_accent: hex(ON_ICE),
    accent_ink: hex(0x2F6F88),
    row: alpha(0x2C3339, 0.06),
    row_hover: alpha(0x2C3339, 0.11),
    done: hex(0x37525F),
    on_done: hex(0xD2EDF5),
    outline: hex(0xAEBBC4),
    drop_scrim: alpha(0xF3F6F8, 0.90),
    toast: alpha(0xFFFFFF, 0.97),
    on_toast: hex(0x20272D),
    bar: hex(0xE4EAEE),
    bar_line: alpha(0x2C3339, 0.14),
    bar_icon: hex(0x2C3339),
    bar_hover: alpha(0x2C3339, 0.10),
};

pub fn of(light: bool) -> &'static Theme {
    if light { &LIGHT } else { &DARK }
}

/// The theme for Windows' current mode. `MIRROR_THEME=light|dark` overrides
/// it, to try one without changing the system's.
pub fn for_system(system_light: bool) -> &'static Theme {
    match std::env::var("MIRROR_THEME").as_deref() {
        Ok("light") => &LIGHT,
        Ok("dark") => &DARK,
        _ => of(system_light),
    }
}

impl Default for &'static Theme {
    fn default() -> Self {
        &DARK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG contrast between two opaque colours.
    fn contrast(a: D2D1_COLOR_F, b: D2D1_COLOR_F) -> f32 {
        fn lum(c: D2D1_COLOR_F) -> f32 {
            let f = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
            0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b)
        }
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn text_is_readable_in_both_themes() {
        for t in [&DARK, &LIGHT] {
            assert!(contrast(t.text, t.page) >= 7.0, "main text, light {}", t.light);
            assert!(contrast(t.text_soft, t.page) >= 4.5, "soft text, light {}", t.light);
            assert!(contrast(t.accent_ink, t.page) >= 4.5, "accent text, light {}", t.light);
            assert!(contrast(t.on_accent, t.accent) >= 4.5, "button label, light {}", t.light);
            assert!(contrast(t.on_done, t.done) >= 4.5, "tick, light {}", t.light);
        }
    }

    #[test]
    fn picks_by_mode() {
        assert!(of(true).light);
        assert!(!of(false).light);
    }
}
