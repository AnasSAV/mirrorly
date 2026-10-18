//! s2: start the display stream, depacketize RTP/HEVC on the marker bit, keep
//! the encoder fed with RTCP feedback, and dump the elementary stream.
//!
//!   s2_video --seconds 10 --out out/capture.h265
//!   s2_video --out - | ffplay -fflags nobuffer -flags low_delay -framedrop -

use std::{collections::BTreeMap, io::Write, path::PathBuf, sync::atomic::Ordering::Relaxed, time::Duration};

use anyhow::{Context, Result};
use clap::Parser;
use idevice::core_device::{RtpPacket, is_rtcp};
use spike::{
    conn::{NoDelayUsbmux, open_tunnel, usb_device},
    depack::Depacketizer,
    feedback::{self, RctlMode},
    media, now_us,
};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value_t = 10.0)]
    seconds: f64,
    /// Output Annex-B file, or "-" for stdout (live playback through ffplay).
    #[arg(long, default_value = "out/capture.h265")]
    out: String,
    #[arg(long, value_enum, default_value_t = RctlMode::Pmd3)]
    rctl: RctlMode,
    /// Disable the per-frame receipt (APP name=5).
    #[arg(long)]
    no_frame_acks: bool,
    /// Never send PLI/FIR (to see how the encoder behaves without them).
    #[arg(long)]
    no_keyframe_requests: bool,
}

const VIDEO_PT: u8 = 100;

#[tokio::main]
async fn main() -> Result<()> {
    spike::init_logging();
    let args = Args::parse();
    let to_stdout = args.out == "-";
    std::fs::create_dir_all("out")?;

    let dev = usb_device().await?;
    let mut t = open_tunnel(&NoDelayUsbmux::new(&dev)).await?;
    let m = media::start(&mut t, 1).await?;
    eprintln!(
        "stream started in {:.1} ms; our ssrc {:#x}, device ssrc {:?}, device rtcp port {:?}",
        m.start_ms, m.our_ssrc, m.device_ssrc, m.rtcp_port
    );
    plist::to_file_xml("out/answer.plist", &m.answer).ok();
    let fb = match m.rtcp_port {
        Some(port) => Some(feedback::spawn(m.video.clone(), port, m.our_ssrc, args.rctl, !args.no_frame_acks)),
        None => {
            eprintln!("WARNING: no SourcePort in answer, RTCP feedback disabled (see out/answer.plist)");
            None
        }
    };

    let mut sink: Box<dyn Write> = if to_stdout {
        Box::new(std::io::stdout().lock())
    } else {
        Box::new(std::io::BufWriter::new(std::fs::File::create(&args.out).context("create output")?))
    };
    let mut csv = std::io::BufWriter::new(std::fs::File::create("out/s2_frames.csv")?);
    writeln!(csv, "idx,rtp_ts,first_pkt_us,last_pkt_us,assembly_us,interval_us,packets,bytes,key,params,corrupt,written")?;

    let mut depack = Depacketizer::new();
    let mut aus = Vec::new();
    let mut pt_counts: BTreeMap<u8, u64> = BTreeMap::new();
    let mut rtcp_in = 0u64;
    let mut waiting_key = true;
    let mut last_kf_req_us = 0u64;
    let mut written = 0u64;
    let mut dropped = 0u64;
    let mut bytes_total = 0u64;
    let mut keys = 0u64;
    let mut prev_last_us: Option<u64> = None;
    let mut intervals = Vec::new();
    let mut assembly = Vec::new();
    let mut sizes = Vec::new();
    let mut first_ts: Option<(u32, u64)> = None;
    let mut last_ts: Option<(u32, u64)> = None;
    let mut idx = 0u64;

    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(args.seconds);
    let start_us = now_us();
    loop {
        let dg = match tokio::time::timeout_at(deadline, m.video.recv()).await {
            Err(_) => break,
            Ok(Err(e)) => {
                eprintln!("udp recv error: {e:?}");
                break;
            }
            Ok(Ok(dg)) => dg,
        };
        let rx_us = now_us();
        if is_rtcp(&dg.data) {
            rtcp_in += 1;
            continue;
        }
        let Some(pkt) = RtpPacket::parse(&dg.data) else { continue };
        *pt_counts.entry(pkt.payload_type).or_default() += 1;
        if pkt.payload_type != VIDEO_PT {
            continue;
        }
        if let Some(fb) = &fb {
            fb.rx_state.on_packet(pkt.ssrc, pkt.sequence_number, pkt.timestamp);
        }
        depack.push(&pkt, rx_us, &mut aus);
        if pkt.marker
            && let Some(fb) = &fb
        {
            fb.frame_ack(pkt.timestamp);
        }

        for au in aus.drain(..) {
            idx += 1;
            if let Some(fb) = &fb {
                fb.rx_state.frames.fetch_add(1, Relaxed);
                fb.rx_state.last_frame_pkts.store(au.packets, Relaxed);
            }
            let interval = prev_last_us.map(|p| au.last_pkt_us.saturating_sub(p));
            prev_last_us = Some(au.last_pkt_us);
            if let Some(i) = interval {
                intervals.push(i);
            }
            assembly.push(au.last_pkt_us - au.first_pkt_us);
            sizes.push(au.data.len() as u64);
            first_ts.get_or_insert((au.rtp_ts, au.last_pkt_us));
            last_ts = Some((au.rtp_ts, au.last_pkt_us));
            if au.key {
                keys += 1;
            }

            let write = if au.corrupt {
                waiting_key = true;
                false
            } else if waiting_key && !au.key {
                false
            } else {
                waiting_key = false;
                true
            };
            if !write {
                dropped += 1;
                // Ask again at most every 100 ms while we are still waiting.
                if !args.no_keyframe_requests && rx_us - last_kf_req_us > 100_000 {
                    if let Some(fb) = &fb {
                        fb.request_keyframe();
                    }
                    last_kf_req_us = rx_us;
                }
            } else {
                sink.write_all(&au.data)?;
                if to_stdout {
                    sink.flush()?;
                }
                written += 1;
                bytes_total += au.data.len() as u64;
            }
            writeln!(
                csv,
                "{idx},{},{},{},{},{},{},{},{},{},{},{}",
                au.rtp_ts,
                au.first_pkt_us,
                au.last_pkt_us,
                au.last_pkt_us - au.first_pkt_us,
                interval.map(|i| i.to_string()).unwrap_or_default(),
                au.packets,
                au.data.len(),
                au.key as u8,
                au.params as u8,
                au.corrupt as u8,
                write as u8
            )?;
        }
    }
    sink.flush()?;
    csv.flush()?;
    let wall_s = (now_us() - start_us) as f64 / 1e6;

    let s = &depack.stats;
    eprintln!("\n=== s2 summary ({wall_s:.1} s) ===");
    eprintln!("RTP packets {} (by PT: {pt_counts:?}), RTCP from device {rtcp_in}", s.packets);
    eprintln!(
        "AUs {} | written {written} | dropped {dropped} | keyframes {keys} | corrupt {} | unterminated {}",
        s.aus, s.corrupt_aus, s.unterminated_aus
    );
    eprintln!("loss: gaps {} lost pkts {} late/dup {}", s.gaps, s.lost_packets, s.late_packets);
    if let Some(fb) = &fb {
        eprintln!("keyframe requests sent: {}", fb.keyframe_requests.load(Relaxed));
    }
    eprintln!(
        "avg fps {:.1} | bitrate {:.2} Mbit/s",
        s.aus as f64 / wall_s,
        bytes_total as f64 * 8.0 / wall_s / 1e6
    );
    if let (Some((ts0, us0)), Some((ts1, us1))) = (first_ts, last_ts)
        && us1 > us0
    {
        eprintln!(
            "RTP clock estimate: {:.0} Hz",
            ts1.wrapping_sub(ts0) as f64 / ((us1 - us0) as f64 / 1e6)
        );
    }
    pct("frame interval (last pkt -> last pkt)", &mut intervals, "ms", 1e3);
    pct("frame assembly (first pkt -> last pkt)", &mut assembly, "ms", 1e3);
    pct("frame size", &mut sizes, "KB", 1e3);
    if !to_stdout {
        eprintln!("wrote {} -> play with: ffplay {}", args.out, PathBuf::from(&args.out).display());
    }
    eprintln!("per-frame log: out/s2_frames.csv; device answer: out/answer.plist");

    let mut client = m.client;
    if let Err(e) = client.stop_media_stream().await {
        eprintln!("stop_media_stream failed: {e:?}");
    }
    Ok(())
}

fn pct(name: &str, v: &mut [u64], unit: &str, div: f64) {
    if v.is_empty() {
        eprintln!("{name}: no data");
        return;
    }
    v.sort_unstable();
    let p = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize] as f64 / div;
    eprintln!(
        "{name}: p50 {:.2} p95 {:.2} p99 {:.2} max {:.2} {unit} (n={})",
        p(0.5),
        p(0.95),
        p(0.99),
        p(1.0),
        v.len()
    );
}
