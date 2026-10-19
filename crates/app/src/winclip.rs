//! The Windows clipboard, for pasting PC copies on the iPhone.
//!
//! The mirror window listens for clipboard changes (AddClipboardFormatListener,
//! WM_CLIPBOARDUPDATE, caught in a subclass of its window procedure since
//! winit doesn't surface it) and reads what was copied: text, an image (as
//! PNG), or files (Explorer's CF_HDROP).
//!
//! Content an app marks as private is left alone: password managers set
//! "ExcludeClipboardContentFromMonitorProcessing" (or the older "Clipboard
//! Viewer Ignore"), the same signals Windows' own clipboard history and
//! cloud clipboard honour.

use std::path::PathBuf;

use windows::{
    Win32::{
        Foundation::{HGLOBAL, HWND, LPARAM, LRESULT, WPARAM},
        System::{
            DataExchange::{
                AddClipboardFormatListener, CloseClipboard, GetClipboardData, GetClipboardSequenceNumber,
                IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
            },
            Memory::{GlobalLock, GlobalSize, GlobalUnlock},
            Ole::{CF_DIB, CF_HDROP, CF_UNICODETEXT},
        },
        UI::Shell::{DefSubclassProc, DragQueryFileW, HDROP, SetWindowSubclass},
    },
    core::w,
};

/// Sent to clipboard format listeners when the clipboard changes (WinUser.h).
const WM_CLIPBOARDUPDATE: u32 = 0x031D;
const LISTEN_ID: usize = 0x4D43_4C50; // "MCLP"
/// Larger copies are not sent to the iPhone (they travel as one message).
pub const MAX_TEXT_BYTES: usize = 8 << 20;
pub const MAX_IMAGE_BYTES: usize = 32 << 20;

/// What the Windows clipboard holds, as far as the iPhone is concerned.
#[derive(Debug)]
pub enum WinClip {
    Text(String),
    /// PNG-encoded.
    Image(Vec<u8>),
    /// Files copied in Explorer.
    Files(Vec<PathBuf>),
}

/// Change counter of the Windows clipboard.
pub fn sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

/// Call `on_change` (on the window's thread) whenever the clipboard changes.
pub fn listen(hwnd: isize, on_change: impl Fn() + 'static) -> windows::core::Result<()> {
    unsafe extern "system" fn proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, data: usize) -> LRESULT {
        unsafe {
            if msg == WM_CLIPBOARDUPDATE && data != 0 {
                (*(data as *const Box<dyn Fn()>))();
            }
            DefSubclassProc(h, msg, wp, lp)
        }
    }
    unsafe {
        let h = HWND(hwnd as _);
        // Lives as long as the window (the app's lifetime).
        let boxed: Box<dyn Fn()> = Box::new(on_change);
        let cb: &'static Box<dyn Fn()> = Box::leak(Box::new(boxed));
        let _ = SetWindowSubclass(h, Some(proc), LISTEN_ID, cb as *const Box<dyn Fn()> as usize);
        AddClipboardFormatListener(h)
    }
}

/// The clipboard closes when this goes out of scope.
struct Open;

impl Open {
    fn new(hwnd: isize) -> Option<Self> {
        // Another app may hold it for a moment.
        for _ in 0..5 {
            if unsafe { OpenClipboard(Some(HWND(hwnd as _))) }.is_ok() {
                return Some(Open);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

fn available(format: u32) -> bool {
    unsafe { IsClipboardFormatAvailable(format).is_ok() }
}

/// The bytes of a clipboard format (must be open).
fn bytes(format: u32) -> Option<Vec<u8>> {
    unsafe {
        let h = GetClipboardData(format).ok()?;
        let g = HGLOBAL(h.0);
        let p = GlobalLock(g) as *const u8;
        if p.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(p, GlobalSize(g)).to_vec();
        let _ = GlobalUnlock(g);
        Some(v)
    }
}

/// Read the clipboard: `None` when it is empty, private, or holds nothing
/// the iPhone can take.
pub fn read(hwnd: isize) -> Option<WinClip> {
    unsafe {
        let private = [
            RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")),
            RegisterClipboardFormatW(w!("Clipboard Viewer Ignore")),
        ];
        let png = RegisterClipboardFormatW(w!("PNG"));
        let _open = Open::new(hwnd)?;
        if private.iter().any(|&f| f != 0 && available(f)) {
            return None;
        }
        if available(CF_HDROP.0 as u32) {
            let h = GetClipboardData(CF_HDROP.0 as u32).ok()?;
            let drop = HDROP(h.0);
            let n = DragQueryFileW(drop, u32::MAX, None);
            let files = (0..n)
                .filter_map(|i| {
                    let len = DragQueryFileW(drop, i, None) as usize;
                    let mut buf = vec![0u16; len + 1];
                    let got = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
                    (got > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..got])))
                })
                .collect::<Vec<_>>();
            return (!files.is_empty()).then_some(WinClip::Files(files));
        }
        if png != 0 && available(png) {
            if let Some(b) = bytes(png).filter(|b| b.starts_with(b"\x89PNG") && b.len() <= MAX_IMAGE_BYTES) {
                return Some(WinClip::Image(b));
            }
        }
        if available(CF_DIB.0 as u32) {
            if let Some(b) = bytes(CF_DIB.0 as u32).and_then(|d| dib_to_png(&d)).filter(|b| b.len() <= MAX_IMAGE_BYTES) {
                return Some(WinClip::Image(b));
            }
        }
        if available(CF_UNICODETEXT.0 as u32) {
            let b = bytes(CF_UNICODETEXT.0 as u32)?;
            let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
            let text = String::from_utf16_lossy(&units[..end]);
            return (!text.is_empty() && text.len() <= MAX_TEXT_BYTES).then_some(WinClip::Text(text));
        }
        None
    }
}

/// A CF_DIB (BITMAPINFOHEADER, optional masks, pixels) as PNG. Handles the
/// 24- and 32-bit uncompressed forms screenshots and apps put there.
pub fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |o: usize| dib.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let i32_at = |o: usize| u32_at(o).map(|v| v as i32);
    let header = u32_at(0)? as usize;
    let width = i32_at(4)?;
    let height = i32_at(8)?;
    let bpp = dib.get(14..16).map(|b| u16::from_le_bytes([b[0], b[1]]))?;
    let compression = u32_at(16)?;
    const BI_RGB: u32 = 0;
    const BI_BITFIELDS: u32 = 3;
    if width <= 0 || height == 0 || !(bpp == 24 || bpp == 32) {
        return None;
    }
    // BI_BITFIELDS with a plain BITMAPINFOHEADER: three masks follow it.
    let masks = if compression == BI_BITFIELDS && header == 40 { 12 } else { 0 };
    if compression != BI_RGB && compression != BI_BITFIELDS {
        return None;
    }
    if compression == BI_BITFIELDS {
        // Only the usual BGRA layout. The masks follow a 40-byte header, or
        // are its next fields in the larger V4/V5 headers: same offset.
        if (u32_at(40)?, u32_at(44)?, u32_at(48)?) != (0x00FF_0000, 0x0000_FF00, 0x0000_00FF) {
            return None;
        }
    }
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    let stride = (w * bpp as usize / 8).div_ceil(4) * 4;
    let pixels = dib.get(header + masks..)?;
    if pixels.len() < stride * h {
        return None;
    }
    let bottom_up = height > 0;
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in 0..h {
        let src = if bottom_up { h - 1 - row } else { row };
        let line = &pixels[src * stride..src * stride + stride];
        for x in 0..w {
            let p = &line[x * bpp as usize / 8..];
            let a = if bpp == 32 { p[3] } else { 255 };
            rgba.extend_from_slice(&[p[2], p[1], p[0], a]);
        }
    }
    // 32-bit DIBs usually leave alpha at 0, meaning "no alpha".
    if bpp == 32 && rgba.chunks_exact(4).all(|px| px[3] == 0) {
        rgba.chunks_exact_mut(4).for_each(|px| px[3] = 255);
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().ok()?;
        writer.write_image_data(&rgba).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dib(width: i32, height: i32, bpp: u16, pixels: &[u8]) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&40u32.to_le_bytes());
        d.extend_from_slice(&width.to_le_bytes());
        d.extend_from_slice(&height.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&bpp.to_le_bytes());
        d.extend_from_slice(&[0u8; 24]); // BI_RGB, sizes, resolution, colours
        d.extend_from_slice(pixels);
        d
    }

    fn decode(png_bytes: &[u8]) -> (u32, u32, Vec<u8>) {
        let dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
        let mut r = dec.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size()];
        let info = r.next_frame(&mut buf).unwrap();
        (info.width, info.height, buf[..info.buffer_size()].to_vec())
    }

    #[test]
    fn bottom_up_24_bit_with_row_padding() {
        // 1x2, bottom row first: blue (BGR 255,0,0) then red; rows padded to 4.
        let px = [255, 0, 0, 0, 0, 0, 255, 0];
        let (w, h, rgba) = decode(&dib_to_png(&dib(1, 2, 24, &px)).unwrap());
        assert_eq!((w, h), (1, 2));
        assert_eq!(rgba, [255, 0, 0, 255, 0, 0, 255, 255], "top row red, then blue");
    }

    #[test]
    fn zero_alpha_32_bit_becomes_opaque() {
        let px = [10, 20, 30, 0];
        let (_, _, rgba) = decode(&dib_to_png(&dib(1, 1, 32, &px)).unwrap());
        assert_eq!(rgba, [30, 20, 10, 255]);
    }

    #[test]
    fn unsupported_forms_are_skipped() {
        assert!(dib_to_png(&dib(1, 1, 8, &[0; 4])).is_none());
        assert!(dib_to_png(&dib(4, 4, 32, &[0; 8])).is_none(), "truncated");
        assert!(dib_to_png(&[1, 2, 3]).is_none());
    }
}

// ---------------------------------------------------------------------------
// Writing: what was copied on the iPhone, onto the Windows clipboard.

/// Copy `bytes` into a movable global block and hand it to the clipboard
/// (which must be open and emptied).
fn put(format: u32, bytes: &[u8]) -> bool {
    use windows::Win32::{
        Foundation::{GlobalFree, HANDLE},
        System::{
            DataExchange::SetClipboardData,
            Memory::{GMEM_MOVEABLE, GlobalAlloc},
        },
    };
    unsafe {
        let Ok(g) = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) else { return false };
        let p = GlobalLock(g) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(g));
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(g);
        // On success the clipboard owns the block.
        if SetClipboardData(format, Some(HANDLE(g.0))).is_err() {
            let _ = GlobalFree(Some(g));
            return false;
        }
        true
    }
}

/// Replace the clipboard with these (format, bytes); the new sequence
/// number on success. `hwnd` None: opened from a helper thread.
fn write(hwnd: Option<isize>, items: &[(u32, Vec<u8>)]) -> Option<u32> {
    use windows::Win32::System::DataExchange::EmptyClipboard;
    let open = match hwnd {
        Some(h) => Open::new(h)?,
        None => {
            let mut opened = None;
            for _ in 0..5 {
                if unsafe { OpenClipboard(None) }.is_ok() {
                    opened = Some(Open);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            opened?
        }
    };
    unsafe { EmptyClipboard().ok()? };
    let all = items.iter().all(|(f, b)| put(*f, b));
    drop(open);
    all.then(sequence)
}

/// Text copied on the iPhone. Marked so that the Windows clipboard history
/// and cloud clipboard skip it (it may be a password).
pub fn write_text(hwnd: isize, text: &str) -> Option<u32> {
    let mut utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    utf16.extend_from_slice(&[0, 0]);
    let exclude = unsafe { RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")) };
    write(Some(hwnd), &[(CF_UNICODETEXT.0 as u32, utf16), (exclude, vec![0; 4])])
}

/// Files fetched from the iPhone, as Explorer would copy them; a single
/// photo also as a bitmap (for Paint, Word, chats that take images). Runs
/// on any thread (decoding a photo takes a moment).
pub fn write_files(files: &[PathBuf]) -> Option<u32> {
    let mut items = vec![(CF_HDROP.0 as u32, hdrop(files))];
    // "Preferred DropEffect" = copy, so pasting in Explorer copies them.
    let effect = unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) };
    items.push((effect, 1u32.to_le_bytes().to_vec()));
    if let [one] = files
        && is_image(one)
        && let Some(dib) = image_dib(one)
    {
        items.push((CF_DIB.0 as u32, dib));
    }
    write(None, &items)
}

fn is_image(p: &std::path::Path) -> bool {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "heic" | "gif" | "webp" | "tiff")
}

/// CF_HDROP: a DROPFILES header, then the wide paths, each NUL-terminated,
/// and a final NUL.
fn hdrop(files: &[PathBuf]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&20u32.to_le_bytes()); // pFiles: paths follow the header
    b.extend_from_slice(&[0; 8]); // pt
    b.extend_from_slice(&0u32.to_le_bytes()); // fNC
    b.extend_from_slice(&1u32.to_le_bytes()); // fWide
    for f in files {
        for u in f.as_os_str().to_string_lossy().encode_utf16().chain([0]) {
            b.extend_from_slice(&u.to_le_bytes());
        }
    }
    b.extend_from_slice(&[0, 0]);
    b
}

/// Decode a photo with Windows Imaging (JPEG, PNG; HEIC with the HEIF
/// extension), turned upright per its EXIF orientation, as a bottom-up
/// 32-bit CF_DIB.
fn image_dib(path: &std::path::Path) -> Option<Vec<u8>> {
    use windows::{
        Win32::{
            Foundation::GENERIC_READ,
            Graphics::Imaging::{
                CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICBitmapSource, IWICImagingFactory,
                WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICBitmapTransformOptions,
                WICDecodeMetadataCacheOnDemand,
            },
            System::{
                Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, StructuredStorage::PROPVARIANT},
                Variant::VT_UI2,
            },
        },
        core::{HSTRING, Interface},
    };
    unsafe {
        // Helper threads need COM; harmless if it is already set up.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let decoder = factory
            .CreateDecoderFromFilename(&HSTRING::from(path.as_os_str()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        // EXIF orientation (tag 274): how to turn the stored pixels upright.
        let mut orientation = 1u16;
        if let Ok(q) = frame.GetMetadataQueryReader() {
            for name in [w!("/app1/ifd/{ushort=274}"), w!("/ifd/{ushort=274}")] {
                let mut v = PROPVARIANT::default();
                if q.GetMetadataByName(name, &mut v).is_ok() && v.Anonymous.Anonymous.vt == VT_UI2 {
                    orientation = v.Anonymous.Anonymous.Anonymous.uiVal;
                    break;
                }
            }
        }
        let transform = WICBitmapTransformOptions(match orientation {
            2 => 8,     // mirror horizontally
            3 => 2,     // rotate 180
            4 => 16,    // mirror vertically
            5 => 1 | 8, // transpose
            6 => 1,     // rotate 90 clockwise
            7 => 3 | 8, // transverse
            8 => 3,     // rotate 270 clockwise
            _ => 0,
        });
        let mut source: IWICBitmapSource = frame.cast().ok()?;
        if transform.0 != 0 {
            let rot = factory.CreateBitmapFlipRotator().ok()?;
            rot.Initialize(&source, transform).ok()?;
            source = rot.cast().ok()?;
        }
        let conv = factory.CreateFormatConverter().ok()?;
        conv.Initialize(&source, &GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)
            .ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        conv.GetSize(&mut w, &mut h).ok()?;
        let stride = w as usize * 4;
        let mut px = vec![0u8; stride * h as usize];
        conv.CopyPixels(std::ptr::null(), stride as u32, &mut px).ok()?;
        Some(dib_from_bgra(w, h, &px))
    }
}

/// A bottom-up 32-bit BI_RGB DIB from top-down BGRA rows.
fn dib_from_bgra(w: u32, h: u32, px: &[u8]) -> Vec<u8> {
    let stride = w as usize * 4;
    let mut d = Vec::with_capacity(40 + px.len());
    d.extend_from_slice(&40u32.to_le_bytes());
    d.extend_from_slice(&(w as i32).to_le_bytes());
    d.extend_from_slice(&(h as i32).to_le_bytes()); // positive: bottom-up
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&32u16.to_le_bytes());
    d.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    d.extend_from_slice(&(px.len() as u32).to_le_bytes());
    d.extend_from_slice(&[0; 16]); // resolution, colours
    for row in px.chunks_exact(stride).rev() {
        d.extend_from_slice(row);
    }
    d
}

#[cfg(test)]
mod write_tests {
    use super::*;

    #[test]
    fn hdrop_layout() {
        let b = hdrop(&[PathBuf::from("C:\\a.jpg"), PathBuf::from("C:\\é.pdf")]);
        assert_eq!(&b[..4], &20u32.to_le_bytes());
        assert_eq!(&b[16..20], &1u32.to_le_bytes(), "wide paths");
        let units: Vec<u16> = b[20..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(String::from_utf16(&units).unwrap(), "C:\\a.jpg\0C:\\é.pdf\0\0");
    }

    #[test]
    fn dib_round_trips_through_the_reader() {
        // 1x2 top-down: red over blue; the DIB stores it bottom-up.
        let px = [0, 0, 255, 255, 255, 0, 0, 255];
        let png_bytes = dib_to_png(&dib_from_bgra(1, 2, &px)).unwrap();
        let mut r = png::Decoder::new(std::io::Cursor::new(png_bytes)).read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size()];
        r.next_frame(&mut buf).unwrap();
        assert_eq!(&buf[..8], &[255, 0, 0, 255, 0, 0, 255, 255]);
    }
}
