//! The mouse pointer, drawn into the picture. Desktop Duplication leaves it
//! out of the screen image and hands it over separately (its shape when
//! that changes, its position when it moves); a monitor used with the mouse
//! needs it drawn.

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

/// BT.709, video range (what the capture converts to).
fn yuv(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let (r, g, b) = (r as i32, g as i32, b as i32);
    let y = 16 + (47 * r + 157 * g + 16 * b + 128) / 256;
    // (Rounded so that greys come out exactly neutral.)
    let u = 128 + (-26 * r - 86 * g + 112 * b + 128).div_euclid(256);
    let v = 128 + (112 * r - 102 * g - 10 * b + 128).div_euclid(256);
    (y.clamp(16, 235) as u8, u.clamp(16, 240) as u8, v.clamp(16, 240) as u8)
}

fn mix(under: u8, over: u8, a: u32) -> u8 {
    ((under as u32 * (255 - a) + over as u32 * a + 127) / 255) as u8
}

/// Draw `shape` with its top-left corner at (x0, y0) into a tightly packed
/// NV12 picture of `w` x `h`.
pub fn draw(nv12: &mut [u8], w: usize, h: usize, shape: &Shape, x0: i32, y0: i32) {
    let (luma, chroma) = nv12.split_at_mut(w * h);
    let at = |sx: i32, sy: i32| -> Px {
        if sx < 0 || sy < 0 || sx >= shape.width as i32 || sy >= shape.height as i32 {
            Px::Clear
        } else {
            shape.px[sy as usize * shape.width + sx as usize]
        }
    };
    let (xs, ys) = (x0.max(0)..(x0 + shape.width as i32).min(w as i32), y0.max(0)..(y0 + shape.height as i32).min(h as i32));
    for y in ys.clone() {
        for x in xs.clone() {
            let d = &mut luma[y as usize * w + x as usize];
            match at(x - x0, y - y0) {
                Px::Clear => {}
                Px::Invert => *d = 251 - (*d).clamp(16, 235),
                Px::Paint { r, g, b, a } => *d = mix(*d, yuv(r, g, b).0, a as u32),
            }
        }
    }
    // Chroma is per 2x2 block: blend in the block's average colour by its
    // average coverage. Inverted pixels count as grey.
    for by in ys.start / 2..(ys.end + 1) / 2 {
        for bx in xs.start / 2..(xs.end + 1) / 2 {
            let (mut cover, mut u, mut v) = (0u32, 0u32, 0u32);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (c, cu, cv) = match at(bx * 2 + dx - x0, by * 2 + dy - y0) {
                    Px::Clear => continue,
                    Px::Invert => (255, 128, 128),
                    Px::Paint { r, g, b, a } => {
                        let (_, cu, cv) = yuv(r, g, b);
                        (a as u32, cu as u32, cv as u32)
                    }
                };
                cover += c;
                u += cu * c;
                v += cv * c;
            }
            if cover == 0 {
                continue;
            }
            let i = by as usize * w + bx as usize * 2;
            if i + 1 < chroma.len() {
                chroma[i] = mix(chroma[i], (u / cover) as u8, cover / 4);
                chroma[i + 1] = mix(chroma[i + 1], (v / cover) as u8, cover / 4);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours() {
        assert_eq!(yuv(255, 255, 255), (235, 128, 128));
        assert_eq!(yuv(0, 0, 0), (16, 128, 128));
    }

    #[test]
    fn monochrome_masks() {
        // 8x2 pixels: AND rows then XOR rows (one byte each).
        let buf = [0b0011_0000, 0b0011_0000, 0b0101_0000, 0b0101_0000];
        let s = Shape::decode(MONOCHROME, 8, 4, 1, &buf).unwrap();
        assert_eq!((s.width, s.height), (8, 2));
        assert_eq!(&s.px[..4], &[BLACK, WHITE, Px::Clear, Px::Invert]);
    }

    #[test]
    fn masked_colour() {
        let buf = [9, 9, 9, 0, 0, 0, 0, 255, 255, 255, 255, 255];
        let s = Shape::decode(MASKED_COLOR, 3, 1, 12, &buf).unwrap();
        assert_eq!(s.px, vec![Px::Paint { r: 9, g: 9, b: 9, a: 255 }, Px::Clear, Px::Invert]);
    }

    #[test]
    fn drawn_into_nv12_and_clipped() {
        let (w, h) = (4, 4);
        let mut pic = vec![16u8; w * h];
        pic.extend(vec![128u8; w * h / 2]);
        let shape = Shape { width: 2, height: 2, px: vec![WHITE, Px::Clear, Px::Invert, WHITE] };
        // Partly off the bottom-right corner: only its top-left pixel lands.
        draw(&mut pic, w, h, &shape, 3, 3);
        assert_eq!(pic[3 * w + 3], 235);
        assert_eq!(pic.iter().take(w * h).filter(|&&y| y != 16).count(), 1);
        draw(&mut pic, w, h, &shape, 0, 0);
        assert_eq!((pic[0], pic[1], pic[w], pic[w + 1]), (235, 16, 235, 235));
        assert_eq!(&pic[w * h..w * h + 2], &[128, 128], "white and grey keep chroma neutral");
    }
}
