//! A small MP4 writer for one HEVC video track, fed with the phone's own
//! stream as it arrives: no re-encoding, so recording costs next to
//! nothing and keeps the phone's quality.
//!
//! Annex B access units go into `mdat` as length-prefixed NAL units (the
//! parameter sets and access unit delimiters are left out of the samples:
//! they go once into the `hvcC` record of an `hvc1` sample entry, as
//! QuickTime and Windows want). Sample times come from the stream's RTP
//! timestamps, whose spacing is exact but whose clock rate isn't 90 kHz
//! (about 24 kHz measured, not documented): the rate is measured over the
//! recording from the arrival times, so the length is right and network
//! bursts don't bunch frames up. The index (`moov`) is written at the end;
//! the file starts with a keyframe.
//! A display matrix carries the view's rotation, so players show it the
//! way it was seen.
//!
//! The phone's encoder pads the picture (an iPad's 1640x2360 panel comes
//! as 1648x2384) without saying so in the SPS; the live view crops it
//! itself. Here the SPS gets a conformance window instead, which every
//! decoder honours (container-level cropping is widely ignored).

use std::{
    fs::File,
    io::{BufWriter, Seek, SeekFrom, Write},
    path::Path,
};

use anyhow::{Context, Result, bail};

/// The track's clock (sample times are converted to it).
const TIMESCALE: u32 = 90_000;
/// The movie header's clock (milliseconds).
const MOVIE_TIMESCALE: u32 = 1000;
/// A last frame's duration when there is nothing to measure it by.
const DEFAULT_DURATION: u32 = TIMESCALE / 60;

const NAL_VPS: u8 = 32;
const NAL_SPS: u8 = 33;
const NAL_PPS: u8 = 34;
const NAL_AUD: u8 = 35;

/// The NAL units of an Annex B byte stream (start codes removed).
pub fn nal_units(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (k, &s) in starts.iter().enumerate() {
        let mut end = starts.get(k + 1).map_or(data.len(), |&n| n - 3);
        // A 4-byte start code leaves a zero at the end of the previous unit.
        while end > s && data[end - 1] == 0 && k + 1 < starts.len() {
            end -= 1;
        }
        if end > s {
            out.push(&data[s..end]);
        }
    }
    out
}

fn nal_type(nal: &[u8]) -> u8 {
    (nal[0] >> 1) & 0x3F
}

/// A NAL unit's payload without emulation prevention bytes.
fn rbsp(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// Emulation prevention for an RBSP (a 3 after two zeros where needed).
fn escape(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + 8);
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// Exp-Golomb and plain bits, MSB first.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

/// Bits out, MSB first.
#[derive(Default)]
struct BitWriter {
    out: Vec<u8>,
    bits: usize,
}

impl BitWriter {
    fn bit(&mut self, b: u32) {
        if self.bits.is_multiple_of(8) {
            self.out.push(0);
        }
        if b != 0 {
            *self.out.last_mut().unwrap() |= 0x80 >> (self.bits % 8);
        }
        self.bits += 1;
    }

    fn ue(&mut self, v: u32) {
        let v = v as u64 + 1;
        let len = 64 - v.leading_zeros();
        for _ in 1..len {
            self.bit(0);
        }
        for i in (0..len).rev() {
            self.bit(((v >> i) & 1) as u32);
        }
    }
}

/// The SPS with a conformance window that crops the coded picture to
/// `width` x `height` (top-left kept). None: nothing to crop, or an SPS
/// this can't rewrite (sub-layers, other than 4:2:0 / 4:2:2 / 4:4:4).
pub fn crop_sps(nal: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let r = rbsp(nal.get(2..)?);
    if (r.first()? >> 1) & 7 != 0 {
        return None;
    }
    let mut b = Bits { data: &r, pos: 13 * 8 };
    b.ue()?;
    let chroma = b.ue()?;
    if chroma == 3 {
        b.bit()?;
    }
    let coded_w = b.ue()?;
    let coded_h = b.ue()?;
    let window_at = b.pos;
    if b.bit()? == 1 {
        for _ in 0..4 {
            b.ue()?;
        }
    }
    let rest_at = b.pos;
    // Offsets are in chroma samples.
    let (sub_w, sub_h) = match chroma {
        1 => (2, 2),
        2 => (2, 1),
        3 => (1, 1),
        _ => return None,
    };
    if (coded_w <= width && coded_h <= height) || width == 0 || height == 0 {
        return None;
    }
    let (right, bottom) = (coded_w.saturating_sub(width) / sub_w, coded_h.saturating_sub(height) / sub_h);
    // The RBSP ends in a stop bit and zeros: copy up to it, then end anew.
    let last = r.iter().rposition(|&x| x != 0)?;
    let stop_at = last * 8 + 7 - r[last].trailing_zeros() as usize;
    let mut w = BitWriter::default();
    let mut src = Bits { data: &r, pos: 0 };
    while src.pos < window_at {
        w.bit(src.bit()?);
    }
    w.bit(1);
    for v in [0, right, 0, bottom] {
        w.ue(v);
    }
    src.pos = rest_at;
    while src.pos < stop_at {
        w.bit(src.bit()?);
    }
    w.bit(1);
    while w.bits % 8 != 0 {
        w.bit(0);
    }
    let mut out = nal[..2].to_vec();
    out.extend_from_slice(&escape(&w.out));
    Some(out)
}

impl Bits<'_> {
    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.pos / 8)?;
        let b = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Some(b as u32)
    }

    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        let mut v = 1u64;
        for _ in 0..zeros {
            v = (v << 1) | self.bit()? as u64;
        }
        Some((v - 1) as u32)
    }
}

/// What `hvcC` needs from the SPS.
#[derive(Debug, Clone, PartialEq)]
pub struct SpsInfo {
    /// profile_space, tier, profile_idc (one byte), compatibility flags,
    /// constraint flags, level: the general profile_tier_level, as is.
    pub profile_tier_level: [u8; 12],
    pub sub_layers: u8,
    pub temporal_id_nested: bool,
    pub chroma_format_idc: u8,
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
}

/// Read an SPS NAL unit (with its 2-byte header).
pub fn parse_sps(nal: &[u8]) -> Option<SpsInfo> {
    let r = rbsp(nal.get(2..)?);
    let first = *r.first()?;
    let sub_layers_minus1 = (first >> 1) & 7;
    let mut ptl = [0u8; 12];
    ptl.copy_from_slice(r.get(1..13)?);
    let mut info = SpsInfo {
        profile_tier_level: ptl,
        sub_layers: sub_layers_minus1 + 1,
        temporal_id_nested: first & 1 == 1,
        chroma_format_idc: 1,
        bit_depth_luma: 8,
        bit_depth_chroma: 8,
    };
    // Sub-layer profile/level fields would follow; with none, the rest is
    // simple to read (the phone's streams have none).
    if sub_layers_minus1 == 0 {
        let mut b = Bits { data: &r, pos: 13 * 8 };
        let fields = (|| {
            b.ue()?; // sps_seq_parameter_set_id
            let chroma = b.ue()?;
            if chroma == 3 {
                b.bit()?; // separate_colour_plane_flag
            }
            b.ue()?; // width
            b.ue()?; // height
            if b.bit()? == 1 {
                for _ in 0..4 {
                    b.ue()?; // conformance window
                }
            }
            Some((chroma, b.ue()?, b.ue()?))
        })();
        if let Some((chroma, luma, chroma_depth)) = fields {
            info.chroma_format_idc = chroma as u8;
            info.bit_depth_luma = luma as u8 + 8;
            info.bit_depth_chroma = chroma_depth as u8 + 8;
        }
    }
    Some(info)
}

/// The `hvcC` box payload for these parameter sets.
pub fn hvcc(vps: &[u8], sps: &[u8], pps: &[u8]) -> Option<Vec<u8>> {
    let info = parse_sps(sps)?;
    let mut c = vec![1u8]; // configurationVersion
    c.extend_from_slice(&info.profile_tier_level);
    c.extend_from_slice(&[0xF0, 0x00]); // min_spatial_segmentation_idc 0
    c.push(0xFC); // parallelismType 0
    c.push(0xFC | (info.chroma_format_idc & 3));
    c.push(0xF8 | (info.bit_depth_luma.saturating_sub(8) & 7));
    c.push(0xF8 | (info.bit_depth_chroma.saturating_sub(8) & 7));
    c.extend_from_slice(&[0, 0]); // avgFrameRate unknown
    // constantFrameRate 0, numTemporalLayers, temporalIdNested,
    // lengthSizeMinusOne 3.
    c.push(((info.sub_layers & 7) << 3) | ((info.temporal_id_nested as u8) << 2) | 3);
    c.push(3);
    for (kind, nal) in [(NAL_VPS, vps), (NAL_SPS, sps), (NAL_PPS, pps)] {
        c.push(0x80 | kind); // array_completeness 1
        c.extend_from_slice(&1u16.to_be_bytes());
        c.extend_from_slice(&(nal.len() as u16).to_be_bytes());
        c.extend_from_slice(nal);
    }
    Some(c)
}

struct Sample {
    offset: u64,
    size: u32,
    /// RTP timestamp, unwrapped, since the first sample.
    rtp: u64,
    /// Arrival, µs since the first sample.
    arrival_us: u64,
    key: bool,
}

pub struct Mp4Writer {
    out: BufWriter<File>,
    /// Where `mdat`'s 64-bit size goes, and where its data starts.
    mdat_size_at: u64,
    pos: u64,
    samples: Vec<Sample>,
    params: [Option<Vec<u8>>; 3],
    width: u16,
    height: u16,
    rotation: u32,
    /// The first sample's arrival (µs) and the last RTP timestamp seen
    /// with its unwrapped value.
    start_us: Option<u64>,
    last_rtp: Option<(u32, u64)>,
}

fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(body.len() + 8);
    b.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
    b.extend_from_slice(kind);
    b.extend_from_slice(body);
    b
}

/// A full box: version and flags first.
fn full(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = vec![version];
    b.extend_from_slice(&flags.to_be_bytes()[1..]);
    b.extend_from_slice(body);
    bx(kind, &b)
}

/// The display matrix for a clockwise rotation of a `w` x `h` picture
/// (16.16 fixed point; the last column 2.30).
fn matrix(rotation: u32, w: u16, h: u16) -> [i32; 9] {
    const ONE: i32 = 0x0001_0000;
    const W: i32 = 0x4000_0000;
    let (w, h) = ((w as i32) << 16, (h as i32) << 16);
    match rotation {
        90 => [0, ONE, 0, -ONE, 0, 0, h, 0, W],
        180 => [-ONE, 0, 0, 0, -ONE, 0, w, h, W],
        270 => [0, -ONE, 0, ONE, 0, 0, 0, w, W],
        _ => [ONE, 0, 0, 0, ONE, 0, 0, 0, W],
    }
}

impl Mp4Writer {
    /// Start a file for a `width` x `height` picture, to be shown turned
    /// `rotation` degrees clockwise (0, 90, 180 or 270).
    pub fn create(path: &Path, width: u32, height: u32, rotation: u32) -> Result<Self> {
        let mut out = BufWriter::new(File::create(path).with_context(|| format!("create {}", path.display()))?);
        let ftyp = bx(b"ftyp", &[b"isom".as_slice(), &0x200u32.to_be_bytes(), b"isom", b"iso2", b"mp41"].concat());
        out.write_all(&ftyp)?;
        // mdat with a 64-bit size (set at the end): recordings can be big.
        out.write_all(&1u32.to_be_bytes())?;
        out.write_all(b"mdat")?;
        let mdat_size_at = ftyp.len() as u64 + 8;
        out.write_all(&0u64.to_be_bytes())?;
        Ok(Self {
            out,
            mdat_size_at,
            pos: mdat_size_at + 8,
            samples: Vec::new(),
            params: [None, None, None],
            width: width as u16,
            height: height as u16,
            rotation,
            start_us: None,
            last_rtp: None,
        })
    }

    /// Frames written so far.
    pub fn frames(&self) -> usize {
        self.samples.len()
    }

    /// Add one access unit (Annex B) with its RTP timestamp, which arrived
    /// at `time_us`. Until the first keyframe with its parameter sets,
    /// frames are skipped.
    pub fn write(&mut self, au: &[u8], rtp_ts: u32, time_us: u64, key: bool) -> Result<()> {
        let nals = nal_units(au);
        for nal in &nals {
            let slot = match nal_type(nal) {
                NAL_VPS => 0,
                NAL_SPS => 1,
                NAL_PPS => 2,
                _ => continue,
            };
            // The first set is the file's (the stream's don't change).
            if self.params[slot].is_none() {
                let set = match slot {
                    1 => crop_sps(nal, self.width as u32, self.height as u32).unwrap_or_else(|| nal.to_vec()),
                    _ => nal.to_vec(),
                };
                self.params[slot] = Some(set);
            }
        }
        if self.samples.is_empty() && !(key && self.params.iter().all(Option::is_some)) {
            return Ok(());
        }
        let offset = self.pos;
        let mut size = 0u32;
        for nal in nals.iter().filter(|n| !n.is_empty() && !matches!(nal_type(n), NAL_VPS | NAL_SPS | NAL_PPS | NAL_AUD)) {
            self.out.write_all(&(nal.len() as u32).to_be_bytes())?;
            self.out.write_all(nal)?;
            size += 4 + nal.len() as u32;
        }
        if size == 0 {
            return Ok(());
        }
        self.pos += size as u64;
        let start = *self.start_us.get_or_insert(time_us);
        let rtp = match self.last_rtp {
            Some((last, total)) => total + rtp_ts.wrapping_sub(last) as u64,
            None => 0,
        };
        self.last_rtp = Some((rtp_ts, rtp));
        self.samples.push(Sample { offset, size, rtp, arrival_us: time_us.saturating_sub(start), key });
        Ok(())
    }

    /// Each sample's time on the track clock: RTP spacing, at the rate the
    /// arrival times show over the whole recording. Arrival times when the
    /// RTP timestamps don't make sense (no span, an implausible rate).
    fn times(&self) -> Vec<u64> {
        let last = self.samples.last().map_or((0, 0), |s| (s.rtp, s.arrival_us));
        let rate = if last.1 > 0 { last.0 as f64 * 1e6 / last.1 as f64 } else { 0.0 };
        if (1_000.0..=1_000_000.0).contains(&rate) {
            self.samples.iter().map(|s| (s.rtp as f64 * TIMESCALE as f64 / rate) as u64).collect()
        } else {
            self.samples.iter().map(|s| s.arrival_us * TIMESCALE as u64 / 1_000_000).collect()
        }
    }

    /// Each sample's duration from the next one's time.
    fn durations(&self) -> Vec<u32> {
        let t = self.times();
        let mut d: Vec<u32> = t.windows(2).map(|w| w[1].saturating_sub(w[0]).clamp(1, TIMESCALE as u64) as u32).collect();
        let last = d.last().copied().unwrap_or(DEFAULT_DURATION);
        if !self.samples.is_empty() {
            d.push(last);
        }
        d
    }

    /// Write the index and close the file. Returns the length in seconds.
    pub fn finish(mut self) -> Result<f64> {
        if self.samples.is_empty() {
            bail!("nothing was recorded");
        }
        let [Some(vps), Some(sps), Some(pps)] = &self.params else { bail!("no parameter sets") };
        let hvcc = hvcc(vps, sps, pps).context("unreadable SPS")?;
        let durations = self.durations();
        let total: u64 = durations.iter().map(|&d| d as u64).sum();
        let moov = self.moov(&hvcc, &durations, total);
        self.out.write_all(&moov)?;
        let mdat_size = self.pos - (self.mdat_size_at - 8);
        self.out.seek(SeekFrom::Start(self.mdat_size_at))?;
        self.out.write_all(&mdat_size.to_be_bytes())?;
        self.out.flush()?;
        Ok(total as f64 / TIMESCALE as f64)
    }

    fn moov(&self, hvcc: &[u8], durations: &[u32], total: u64) -> Vec<u8> {
        let movie_duration = (total * MOVIE_TIMESCALE as u64 / TIMESCALE as u64) as u32;
        let m = matrix(self.rotation, self.width, self.height);
        let matrix_bytes: Vec<u8> = m.iter().flat_map(|v| v.to_be_bytes()).collect();

        let mut mvhd = Vec::new();
        mvhd.extend_from_slice(&[0; 8]); // creation, modification
        mvhd.extend_from_slice(&MOVIE_TIMESCALE.to_be_bytes());
        mvhd.extend_from_slice(&movie_duration.to_be_bytes());
        mvhd.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate 1.0
        mvhd.extend_from_slice(&0x0100u16.to_be_bytes()); // volume 1.0
        mvhd.extend_from_slice(&[0; 10]);
        mvhd.extend_from_slice(&matrix(0, 0, 0).iter().flat_map(|v| v.to_be_bytes()).collect::<Vec<_>>());
        mvhd.extend_from_slice(&[0; 24]); // pre_defined
        mvhd.extend_from_slice(&2u32.to_be_bytes()); // next_track_ID

        let mut tkhd = Vec::new();
        tkhd.extend_from_slice(&[0; 8]);
        tkhd.extend_from_slice(&1u32.to_be_bytes()); // track_ID
        tkhd.extend_from_slice(&[0; 4]);
        tkhd.extend_from_slice(&movie_duration.to_be_bytes());
        tkhd.extend_from_slice(&[0; 8]);
        tkhd.extend_from_slice(&[0; 4]); // layer, alternate_group
        tkhd.extend_from_slice(&[0; 4]); // volume, reserved
        tkhd.extend_from_slice(&matrix_bytes);
        tkhd.extend_from_slice(&((self.width as u32) << 16).to_be_bytes());
        tkhd.extend_from_slice(&((self.height as u32) << 16).to_be_bytes());

        let mut mdhd = Vec::new();
        mdhd.extend_from_slice(&[0; 8]);
        mdhd.extend_from_slice(&TIMESCALE.to_be_bytes());
        mdhd.extend_from_slice(&(total.min(u32::MAX as u64) as u32).to_be_bytes());
        mdhd.extend_from_slice(&0x55C4u16.to_be_bytes()); // language "und"
        mdhd.extend_from_slice(&[0; 2]);

        let mut hdlr = vec![0; 4];
        hdlr.extend_from_slice(b"vide");
        hdlr.extend_from_slice(&[0; 12]);
        hdlr.extend_from_slice(b"Mirrorly\0");

        // The sample entry: hvc1 with its hvcC.
        let mut entry = vec![0; 6];
        entry.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
        entry.extend_from_slice(&[0; 16]);
        entry.extend_from_slice(&self.width.to_be_bytes());
        entry.extend_from_slice(&self.height.to_be_bytes());
        entry.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // 72 dpi
        entry.extend_from_slice(&0x0048_0000u32.to_be_bytes());
        entry.extend_from_slice(&[0; 4]);
        entry.extend_from_slice(&1u16.to_be_bytes()); // frame_count
        let mut name = [0u8; 32];
        name[0] = 4;
        name[1..5].copy_from_slice(b"HEVC");
        entry.extend_from_slice(&name);
        entry.extend_from_slice(&0x0018u16.to_be_bytes()); // depth
        entry.extend_from_slice(&(-1i16).to_be_bytes());
        entry.extend_from_slice(&bx(b"hvcC", hvcc));
        let mut stsd = 1u32.to_be_bytes().to_vec();
        stsd.extend_from_slice(&bx(b"hvc1", &entry));

        // Durations, run-length.
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for &d in durations {
            match runs.last_mut() {
                Some((n, last)) if *last == d => *n += 1,
                _ => runs.push((1, d)),
            }
        }
        let mut stts = (runs.len() as u32).to_be_bytes().to_vec();
        for (n, d) in &runs {
            stts.extend_from_slice(&n.to_be_bytes());
            stts.extend_from_slice(&d.to_be_bytes());
        }
        let keys: Vec<u32> = self.samples.iter().enumerate().filter(|(_, s)| s.key).map(|(i, _)| i as u32 + 1).collect();
        let mut stss = (keys.len() as u32).to_be_bytes().to_vec();
        for k in &keys {
            stss.extend_from_slice(&k.to_be_bytes());
        }
        let mut stsz = vec![0; 4]; // sample_size 0: each its own
        stsz.extend_from_slice(&(self.samples.len() as u32).to_be_bytes());
        for s in &self.samples {
            stsz.extend_from_slice(&s.size.to_be_bytes());
        }
        // One sample per chunk, 64-bit offsets.
        let mut stsc = 1u32.to_be_bytes().to_vec();
        stsc.extend_from_slice(&[1u32, 1, 1].iter().flat_map(|v| v.to_be_bytes()).collect::<Vec<_>>());
        let mut co64 = (self.samples.len() as u32).to_be_bytes().to_vec();
        for s in &self.samples {
            co64.extend_from_slice(&s.offset.to_be_bytes());
        }
        let stbl = bx(
            b"stbl",
            &[
                full(b"stsd", 0, 0, &stsd),
                full(b"stts", 0, 0, &stts),
                full(b"stss", 0, 0, &stss),
                full(b"stsc", 0, 0, &stsc),
                full(b"stsz", 0, 0, &stsz),
                full(b"co64", 0, 0, &co64),
            ]
            .concat(),
        );
        let dinf = bx(b"dinf", &full(b"dref", 0, 0, &[1u32.to_be_bytes().to_vec(), full(b"url ", 0, 1, &[])].concat()));
        let minf = bx(b"minf", &[full(b"vmhd", 0, 1, &[0; 8]), dinf, stbl].concat());
        let mdia = bx(b"mdia", &[full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr), minf].concat());
        let trak = bx(b"trak", &[full(b"tkhd", 0, 3, &tkhd), mdia].concat());
        bx(b"moov", &[full(b"mvhd", 0, 0, &mvhd), trak].concat())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_annex_b() {
        let data = [0, 0, 0, 1, 0x40, 1, 9, 0, 0, 1, 0x42, 1, 0, 0, 0, 1, 0x26, 1, 7];
        let n = nal_units(&data);
        assert_eq!(n, vec![&[0x40, 1, 9][..], &[0x42, 1][..], &[0x26, 1, 7][..]]);
        assert_eq!(n.iter().map(|x| nal_type(x)).collect::<Vec<_>>(), vec![NAL_VPS, NAL_SPS, 19]);
    }

    #[test]
    fn removes_emulation_prevention() {
        assert_eq!(rbsp(&[1, 0, 0, 3, 1, 0, 0, 3, 0, 5]), vec![1, 0, 0, 1, 0, 0, 0, 5]);
        assert_eq!(escape(&rbsp(&[1, 0, 0, 3, 1, 0, 0, 3, 0, 5])), vec![1, 0, 0, 3, 1, 0, 0, 3, 0, 5]);
    }

    #[test]
    fn exp_golomb_round_trip() {
        let mut w = BitWriter::default();
        for v in [0, 1, 2, 3, 7, 100, 1648] {
            w.ue(v);
        }
        let mut b = Bits { data: &w.out, pos: 0 };
        assert_eq!([0, 1, 2, 3, 7, 100, 1648].map(|_| b.ue().unwrap()), [0, 1, 2, 3, 7, 100, 1648]);
    }

    #[test]
    fn exp_golomb() {
        // 1 | 010 | 011 | 00100 -> 0, 1, 2, 3
        let data = [0b1010_0110, 0b0100_0000];
        let mut b = Bits { data: &data, pos: 0 };
        assert_eq!([b.ue(), b.ue(), b.ue(), b.ue()], [Some(0), Some(1), Some(2), Some(3)]);
    }

    #[test]
    fn times_follow_rtp_spacing_at_the_measured_rate() {
        let dir = std::env::temp_dir().join(format!("mirrorly-mp4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut w = Mp4Writer::create(&dir.join("t.mp4"), 16, 16, 0).unwrap();
        // RTP at 24 kHz, 400 ticks a frame (60 fps); arrivals bunched up.
        let arrivals = [0u64, 40_000, 41_000, 42_000, 66_667];
        for (i, a) in arrivals.iter().enumerate() {
            w.samples.push(Sample { offset: 0, size: 1, rtp: i as u64 * 400, arrival_us: *a, key: i == 0 });
        }
        assert_eq!(w.durations(), vec![1500, 1500, 1500, 1500, 1500], "even 60 fps at 90 kHz");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rotation_matrices() {
        assert_eq!(matrix(0, 10, 20)[0], 0x10000);
        assert_eq!(matrix(90, 10, 20)[6], 20 << 16, "turned clockwise: moved right by the height");
        assert_eq!(matrix(270, 10, 20)[7], 10 << 16);
    }
}
