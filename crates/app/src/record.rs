//! Screen recordings and screenshots of the iPhone.
//!
//! A recording is the phone's own HEVC stream written to MP4 as it arrives
//! (`video::mp4`): no re-encoding, so it costs next to nothing and keeps
//! the phone's quality. The network thread hands each access unit to a
//! recorder thread, which does the file writing. A screenshot is the
//! newest decoded picture, read back from the GPU once and saved as PNG.
//! Both are turned the way the view is, and go to Videos › Mirrorly and
//! Pictures › Mirrorly, named by the time they were taken.

use std::{
    path::{Path, PathBuf},
    sync::mpsc,
};

use anyhow::{Context, Result};
use input::geom::Rotation;

/// To the recorder thread.
pub enum Msg {
    /// One access unit: Annex B data, RTP timestamp, when it arrived (µs),
    /// keyframe.
    Au(Vec<u8>, u32, u64, bool),
    Stop,
}

/// A recording in progress.
pub struct Recording {
    pub tx: mpsc::Sender<Msg>,
    pub path: PathBuf,
    /// The recorder thread (joined on quit, so the file gets finished).
    pub thread: Option<std::thread::JoinHandle<()>>,
}

/// Where Windows keeps the user's pictures or videos, plus "Mirrorly".
fn folder(id: &windows::core::GUID) -> Result<PathBuf> {
    use windows::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{KF_FLAG_CREATE, SHGetKnownFolderPath},
    };
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_CREATE, None).context("known folder")?;
        let path = PathBuf::from(p.to_string().unwrap_or_default());
        CoTaskMemFree(Some(p.0 as *const _));
        let dir = path.join("Mirrorly");
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        Ok(dir)
    }
}

/// "Mirrorly 2026-10-20 07.15.03.ext" in that folder, not yet taken.
fn new_file(id: &windows::core::GUID, ext: &str) -> Result<PathBuf> {
    let dir = folder(id)?;
    let stamp = chrono::Local::now().format("%Y-%m-%d %H.%M.%S");
    let mut path = dir.join(format!("Mirrorly {stamp}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("Mirrorly {stamp} ({n}).{ext}"));
        n += 1;
    }
    Ok(path)
}

/// Start recording a `width` x `height` stream shown at `rotation`. The
/// recorder thread calls `done` with the file and its length when stopped.
pub fn start(width: u32, height: u32, rotation: Rotation, done: impl FnOnce(Result<(PathBuf, f64)>) + Send + 'static) -> Result<Recording> {
    let path = new_file(&windows::Win32::UI::Shell::FOLDERID_Videos, "mp4")?;
    let mut writer = video::mp4::Mp4Writer::create(&path, width, height, rotation.degrees())?;
    let (tx, rx) = mpsc::channel::<Msg>();
    let file = path.clone();
    let thread = std::thread::Builder::new().name("recorder".into()).spawn(move || {
        let mut failed = None;
        while let Ok(Msg::Au(data, rtp, at, key)) = rx.recv() {
            if failed.is_none()
                && let Err(e) = writer.write(&data, rtp, at, key)
            {
                failed = Some(e);
            }
        }
        let r = match failed {
            Some(e) => Err(e),
            None => writer.finish().map(|secs| (file.clone(), secs)),
        };
        if r.is_err() {
            let _ = std::fs::remove_file(&file);
        }
        done(r);
    })?;
    tracing::info!("recording to {}", path.display());
    Ok(Recording { tx, path, thread: Some(thread) })
}

/// An NV12 picture read back from the GPU (Y rows, then UV rows, tightly
/// packed), full-range BT.709 as the phone sends it.
pub struct Nv12 {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// RGB, turned the way the view is.
fn to_rgb(p: &Nv12, rotation: Rotation) -> (Vec<u8>, u32, u32) {
    let (w, h) = (p.width as usize, p.height as usize);
    let (y_plane, uv_plane) = p.data.split_at(w * h);
    let (ow, oh) = match rotation {
        Rotation::R90 | Rotation::R270 => (h, w),
        _ => (w, h),
    };
    let mut out = vec![0u8; ow * oh * 3];
    for y in 0..h {
        for x in 0..w {
            let luma = y_plane[y * w + x] as f32 / 255.0;
            let i = (y / 2) * w + (x / 2) * 2;
            // Neutral chroma is 128.
            let (u, v) = ((uv_plane[i] as f32 - 128.0) / 255.0, (uv_plane[i + 1] as f32 - 128.0) / 255.0);
            let rgb = [luma + 1.5748 * v, luma - 0.1873 * u - 0.4681 * v, luma + 1.8556 * u];
            // Where this pixel lands in the turned picture (clockwise).
            let (ox, oy) = match rotation {
                Rotation::R0 => (x, y),
                Rotation::R90 => (h - 1 - y, x),
                Rotation::R180 => (w - 1 - x, h - 1 - y),
                Rotation::R270 => (y, w - 1 - x),
            };
            let o = (oy * ow + ox) * 3;
            for (k, c) in rgb.iter().enumerate() {
                out[o + k] = (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        }
    }
    (out, ow as u32, oh as u32)
}

fn write_png(path: &Path, rgb: &[u8], w: u32, h: u32) -> Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgb)?;
    Ok(())
}

/// Save a screenshot (on the calling thread: run it off the UI thread).
pub fn save_screenshot(p: &Nv12, rotation: Rotation) -> Result<PathBuf> {
    let path = new_file(&windows::Win32::UI::Shell::FOLDERID_Pictures, "png")?;
    let (rgb, w, h) = to_rgb(p, rotation);
    write_png(&path, &rgb, w, h)?;
    tracing::info!("screenshot {}", path.display());
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2x2: white, black / grey, grey; neutral chroma.
    fn picture() -> Nv12 {
        Nv12 { data: vec![255, 0, 128, 128, 128, 128], width: 2, height: 2 }
    }

    #[test]
    fn colours_and_size() {
        let (rgb, w, h) = to_rgb(&picture(), Rotation::R0);
        assert_eq!((w, h), (2, 2));
        assert_eq!(&rgb[..6], &[255, 255, 255, 0, 0, 0]);
    }

    #[test]
    fn turned_clockwise() {
        // Top row white, black. Turned 90 clockwise, white is top-right.
        let (rgb, w, h) = to_rgb(&Nv12 { data: vec![255, 0, 0, 0, 128, 128], width: 2, height: 2 }, Rotation::R90);
        assert_eq!((w, h), (2, 2));
        assert_eq!(&rgb[3..6], &[255, 255, 255], "top-right");
        assert_eq!(&rgb[..3], &[0, 0, 0]);
    }
}
