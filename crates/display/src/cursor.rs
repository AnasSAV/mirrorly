//! The mouse pointer. Desktop Duplication leaves it out of the screen image
//! and hands it over separately (its shape when that changes, its position
//! when it moves); it goes to the iPad the same way, as small messages,
//! and the page draws it over the video.

/// One pointer pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Px {
    Clear,
    /// Straight-alpha colour.
    Paint { r: u8, g: u8, b: u8, a: u8 },
    /// Invert what is below (the text I-beam does this).
    Invert,
}

const BLACK: Px = Px::Paint { r: 0, g: 0, b: 0, a: 255 };
const WHITE: Px = Px::Paint { r: 255, g: 255, b: 255, a: 255 };

/// DXGI_OUTDUPL_POINTER_SHAPE_TYPE values.
pub const MONOCHROME: u32 = 1;
pub const COLOR: u32 = 2;
pub const MASKED_COLOR: u32 = 4;

#[derive(Debug)]
pub struct Shape {
    pub width: usize,
    pub height: usize,
    pub px: Vec<Px>,
}

impl Shape {
    /// From GetFramePointerShape's buffer and shape info.
    pub fn decode(kind: u32, width: usize, height: usize, pitch: usize, buf: &[u8]) -> Option<Self> {
        let mut px = Vec::with_capacity(width * height);
        match kind {
            // 1 bit per pixel: the AND mask's rows, then the XOR mask's.
            MONOCHROME => {
                let height = height / 2;
                let bit = |row: usize, x: usize| buf.get(row * pitch + x / 8).map(|b| b & (0x80 >> (x % 8)) != 0);
                for y in 0..height {
                    for x in 0..width {
                        px.push(match (bit(y, x)?, bit(y + height, x)?) {
                            (false, false) => BLACK,
                            (false, true) => WHITE,
                            (true, false) => Px::Clear,
                            (true, true) => Px::Invert,
                        });
                    }
                }
                Some(Self { width, height, px })
            }
            // BGRA. Masked: alpha 0 replaces the screen pixel, alpha 255
            // XORs it (shown as inversion; XOR with black changes nothing).
            COLOR | MASKED_COLOR => {
                for y in 0..height {
                    for x in 0..width {
                        let p = buf.get(y * pitch + x * 4..y * pitch + x * 4 + 4)?;
                        let (b, g, r, a) = (p[0], p[1], p[2], p[3]);
                        px.push(match (kind, a) {
                            (COLOR, 0) => Px::Clear,
                            (COLOR, _) => Px::Paint { r, g, b, a },
                            (_, 0) => Px::Paint { r, g, b, a: 255 },
                            _ if r | g | b == 0 => Px::Clear,
                            _ => Px::Invert,
                        });
                    }
                }
                Some(Self { width, height, px })
            }
            _ => None,
        }
    }
}

/// The shape for the page: `s <width> <height> <paint> <invert>`, each
/// layer RGBA (straight alpha) in base64, or `-` when empty. Inverting
/// pixels are white in their layer (the page blends it as a difference).
/// `s 0 0 - -`: no pointer image.
pub fn shape_message(shape: Option<&Shape>) -> String {
    let Some(shape) = shape.filter(|s| s.width > 0 && s.height > 0) else { return "s 0 0 - -".into() };
    let mut paint = vec![0u8; shape.px.len() * 4];
    let mut invert = vec![0u8; shape.px.len() * 4];
    for (i, px) in shape.px.iter().enumerate() {
        match *px {
            Px::Clear => {}
            Px::Paint { r, g, b, a } => paint[i * 4..i * 4 + 4].copy_from_slice(&[r, g, b, a]),
            Px::Invert => invert[i * 4..i * 4 + 4].copy_from_slice(&[255; 4]),
        }
    }
    let layer = |l: &[u8]| if l.chunks(4).all(|p| p[3] == 0) { "-".to_string() } else { crate::server::base64(l) };
    format!("s {} {} {} {}", shape.width, shape.height, layer(&paint), layer(&invert))
}

/// Where the pointer image's top-left is on the picture: `p <x> <y>`, or
/// `p -` when the pointer isn't on this monitor.
pub fn position_message(at: Option<(i32, i32)>) -> String {
    match at {
        Some((x, y)) => format!("p {x} {y}"),
        None => "p -".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monochrome_masks() {
        // 8x2 pixels: AND rows then XOR rows (one byte each).
        let buf = [0b0011_0000, 0b0011_0000, 0b0101_0000, 0b0101_0000];
        let s = Shape::decode(MONOCHROME, 8, 4, 1, &buf).unwrap();
        assert_eq!((s.width, s.height), (8, 2));
        assert_eq!(&s.px[..4], &[BLACK, WHITE, Px::Clear, Px::Invert]);
    }

    #[test]
    fn messages_for_the_page() {
        let s = Shape { width: 2, height: 1, px: vec![Px::Paint { r: 1, g: 2, b: 3, a: 4 }, Px::Invert] };
        let m = shape_message(Some(&s));
        let parts: Vec<&str> = m.split(' ').collect();
        assert_eq!(&parts[..3], &["s", "2", "1"]);
        assert_eq!(parts[3], crate::server::base64(&[1, 2, 3, 4, 0, 0, 0, 0]));
        assert_eq!(parts[4], crate::server::base64(&[0, 0, 0, 0, 255, 255, 255, 255]));
        let only_paint = Shape { width: 1, height: 1, px: vec![BLACK] };
        assert!(shape_message(Some(&only_paint)).ends_with(" -"), "empty invert layer");
        assert_eq!(shape_message(None), "s 0 0 - -");
        assert_eq!(position_message(Some((-3, 40))), "p -3 40");
        assert_eq!(position_message(None), "p -");
    }

    #[test]
    fn masked_colour() {
        let buf = [9, 9, 9, 0, 0, 0, 0, 255, 255, 255, 255, 255];
        let s = Shape::decode(MASKED_COLOR, 3, 1, 12, &buf).unwrap();
        assert_eq!(s.px, vec![Px::Paint { r: 9, g: 9, b: 9, a: 255 }, Px::Clear, Px::Invert]);
    }
}
