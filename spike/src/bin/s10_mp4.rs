//! s10: the MP4 writer, checked against FFmpeg. Splits a raw HEVC Annex B
//! file into access units (a new one at each VCL NAL unit that starts a
//! picture) and writes them as the recorder would, at 60 fps.
//!
//!   s10_mp4 <in.hevc> <out.mp4> [rotation] [width height]

use anyhow::{Context, Result};
use video::mp4::{Mp4Writer, nal_units};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (input, output) = (args.get(1).context("input")?, args.get(2).context("output")?);
    let rotation: u32 = args.get(3).map(|r| r.parse()).transpose()?.unwrap_or(0);
    let data = std::fs::read(input)?;

    // Group NAL units into pictures: VCL types 0..=31 with
    // first_slice_segment_in_pic_flag set start one (parameter sets just
    // before it belong to it).
    let mut aus: Vec<(Vec<u8>, bool)> = Vec::new();
    let mut pending: Vec<u8> = Vec::new();
    let mut current: Option<(Vec<u8>, bool)> = None;
    for nal in nal_units(&data) {
        let t = (nal[0] >> 1) & 0x3F;
        let first_slice = t <= 31 && nal.len() > 2 && nal[2] & 0x80 != 0;
        if first_slice {
            if let Some(au) = current.take() {
                aus.push(au);
            }
            current = Some((std::mem::take(&mut pending), (16..=21).contains(&t)));
        }
        let target = match (&mut current, t <= 31) {
            (Some((buf, _)), true) => buf,
            _ => &mut pending,
        };
        target.extend_from_slice(&[0, 0, 0, 1]);
        target.extend_from_slice(nal);
    }
    aus.extend(current);

    // The picture to keep (the writer crops the coded size to it).
    let width: u32 = args.get(4).map(|r| r.parse()).transpose()?.unwrap_or(828);
    let height: u32 = args.get(5).map(|r| r.parse()).transpose()?.unwrap_or(1792);
    let mut w = Mp4Writer::create(std::path::Path::new(output), width, height, rotation)?;
    for (i, (au, key)) in aus.iter().enumerate() {
        // RTP at 24 kHz (as the phone's), arriving on time.
        w.write(au, (i as u32) * 400, i as u64 * 16_667, *key)?;
    }
    let frames = w.frames();
    let secs = w.finish()?;
    println!("{} access units in, {frames} frames written, {secs:.2} s", aus.len());
    Ok(())
}
