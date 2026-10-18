//! s4: HEVC decode latency through FFmpeg's d3d11va hwaccel, per DXGI adapter.
//!
//!   s4_decode out/capture.h265                 (all adapters, paced at 33 ms)
//!   s4_decode out/capture.h265 --adapter 1 --readback
//!   s4_decode out/capture.h265 --sw            (software decode, for scale)
//!
//! "submit" latency = send_packet -> receive_frame returned a D3D11 texture.
//! The GPU may still be decoding at that point, so --readback adds a copy to
//! system memory, which waits for the GPU: an upper bound. Phase 1 measures
//! the real thing with D3D11 queries inside the present path.

use std::{ffi::CString, ptr, time::Instant};

use anyhow::{Result, bail};
use clap::Parser;
use ffmpeg_sys_next as ff;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};

#[derive(Parser)]
struct Args {
    file: String,
    /// DXGI adapter index; omit to test every adapter.
    #[arg(long)]
    adapter: Option<u32>,
    /// Sleep between packets to mimic the live stream (0 = as fast as possible).
    #[arg(long, default_value_t = 33)]
    pace_ms: u64,
    /// Copy every frame to system memory (forces GPU completion).
    #[arg(long)]
    readback: bool,
    /// Software decode instead of d3d11va.
    #[arg(long)]
    sw: bool,
}

const EAGAIN: i32 = 11; // MSVC errno
const AVERROR_EAGAIN: i32 = -EAGAIN;
const AVERROR_EOF: i32 = -((b'E' as i32) | (b'O' as i32) << 8 | (b'F' as i32) << 16 | (b' ' as i32) << 24);

#[link(name = "winmm")]
unsafe extern "system" {
    fn timeBeginPeriod(period_ms: u32) -> u32;
}

fn main() -> Result<()> {
    let args = Args::parse();
    unsafe { timeBeginPeriod(1) };
    let adapters = list_adapters()?;
    for (i, name) in adapters.iter().enumerate() {
        println!("DXGI adapter {i}: {name}");
    }
    let packets = split_access_units(&std::fs::read(&args.file)?)?;
    println!("{} access units parsed from {}", packets.len(), args.file);

    if args.sw {
        run(&packets, None, &args)?;
    } else {
        let which: Vec<u32> = match args.adapter {
            Some(a) => vec![a],
            None => (0..adapters.len() as u32).filter(|i| !adapters[*i as usize].contains("Basic Render")).collect(),
        };
        for a in which {
            println!("\n--- d3d11va on adapter {a}: {}", adapters.get(a as usize).map(String::as_str).unwrap_or("?"));
            if let Err(e) = run(&packets, Some(a), &args) {
                println!("failed: {e:#}");
            }
        }
    }
    Ok(())
}

fn list_adapters() -> Result<Vec<String>> {
    let mut out = Vec::new();
    unsafe {
        let f: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut i = 0;
        while let Ok(a) = f.EnumAdapters1(i) {
            let d = a.GetDesc1()?;
            let len = d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len());
            out.push(String::from_utf16_lossy(&d.Description[..len]));
            i += 1;
        }
    }
    Ok(out)
}

/// Split an Annex-B file into access units with FFmpeg's HEVC parser (offline
/// only; the live path gets AUs straight from the RTP marker bit).
fn split_access_units(data: &[u8]) -> Result<Vec<Vec<u8>>> {
    unsafe {
        let codec = ff::avcodec_find_decoder(ff::AVCodecID::AV_CODEC_ID_HEVC);
        let ctx = ff::avcodec_alloc_context3(codec);
        let parser = ff::av_parser_init(ff::AVCodecID::AV_CODEC_ID_HEVC);
        if parser.is_null() {
            bail!("no HEVC parser");
        }
        let mut out = Vec::new();
        let mut off = 0usize;
        loop {
            let mut buf: *mut u8 = ptr::null_mut();
            let mut size = 0i32;
            let remaining = data.len() - off;
            let (inp, n) = if remaining > 0 { (data.as_ptr().add(off), remaining as i32) } else { (ptr::null(), 0) };
            let used = ff::av_parser_parse2(parser, ctx, &mut buf, &mut size, inp, n, i64::MIN, i64::MIN, 0);
            if used < 0 {
                bail!("parser error {used}");
            }
            off += used as usize;
            if size > 0 {
                out.push(std::slice::from_raw_parts(buf, size as usize).to_vec());
            }
            if remaining == 0 && size == 0 {
                break;
            }
        }
        ff::av_parser_close(parser);
        let mut ctx = ctx;
        ff::avcodec_free_context(&mut ctx);
        Ok(out)
    }
}

unsafe extern "C" fn pick_d3d11(_: *mut ff::AVCodecContext, fmts: *const ff::AVPixelFormat) -> ff::AVPixelFormat {
    let mut p = fmts;
    unsafe {
        while *p != ff::AVPixelFormat::AV_PIX_FMT_NONE {
            if *p == ff::AVPixelFormat::AV_PIX_FMT_D3D11 {
                return *p;
            }
            p = p.add(1);
        }
    }
    ff::AVPixelFormat::AV_PIX_FMT_NONE
}

fn run(packets: &[Vec<u8>], adapter: Option<u32>, args: &Args) -> Result<()> {
    unsafe {
        let codec = ff::avcodec_find_decoder(ff::AVCodecID::AV_CODEC_ID_HEVC);
        let mut ctx = ff::avcodec_alloc_context3(codec);
        (*ctx).flags |= ff::AV_CODEC_FLAG_LOW_DELAY as i32;
        // Frame threading adds a frame of delay per thread: never.
        (*ctx).thread_count = 1;
        let mut hw: *mut ff::AVBufferRef = ptr::null_mut();
        if let Some(a) = adapter {
            let dev = CString::new(a.to_string())?;
            let r = ff::av_hwdevice_ctx_create(&mut hw, ff::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA, dev.as_ptr(), ptr::null_mut(), 0);
            if r < 0 {
                bail!("av_hwdevice_ctx_create(d3d11va, {a}) = {r}");
            }
            (*ctx).hw_device_ctx = ff::av_buffer_ref(hw);
            (*ctx).get_format = Some(pick_d3d11);
        }
        let r = ff::avcodec_open2(ctx, codec, ptr::null_mut());
        if r < 0 {
            bail!("avcodec_open2 = {r}");
        }

        let pkt = ff::av_packet_alloc();
        let frame = ff::av_frame_alloc();
        let sw = ff::av_frame_alloc();
        let t0 = Instant::now();
        let us = || t0.elapsed().as_micros() as u64;
        let mut sent_at = vec![0u64; packets.len()];
        let mut submit = Vec::new();
        let mut total = Vec::new();
        let mut depth_max = 0usize;
        let mut received = 0usize;
        let mut first_fmt = None;
        let mut errors = 0;
        let mut send_errors = 0;

        let mut drain = |sent: usize, sent_at: &[u64], flushing: bool| -> Result<()> {
            loop {
                let r = ff::avcodec_receive_frame(ctx, frame);
                if r == AVERROR_EAGAIN || r == AVERROR_EOF {
                    return Ok(());
                }
                if r < 0 {
                    errors += 1;
                    return Ok(());
                }
                let t_out = us();
                let idx = (*frame).pts.clamp(0, sent_at.len() as i64 - 1) as usize;
                first_fmt.get_or_insert((*frame).format);
                if !flushing {
                    depth_max = depth_max.max(sent - received - 1);
                }
                received += 1;
                submit.push(t_out - sent_at[idx]);
                if args.readback && adapter.is_some() {
                    ff::av_frame_unref(sw);
                    if ff::av_hwframe_transfer_data(sw, frame, 0) < 0 {
                        bail!("hwframe transfer failed");
                    }
                    total.push(us() - sent_at[idx]);
                }
                ff::av_frame_unref(frame);
            }
        };

        for (i, p) in packets.iter().enumerate() {
            if args.pace_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(args.pace_ms));
            }
            ff::av_new_packet(pkt, p.len() as i32);
            ptr::copy_nonoverlapping(p.as_ptr(), (*pkt).data, p.len());
            (*pkt).pts = i as i64;
            sent_at[i] = us();
            let r = ff::avcodec_send_packet(ctx, pkt);
            ff::av_packet_unref(pkt);
            if r < 0 && r != AVERROR_EAGAIN {
                send_errors += 1;
            }
            drain(i + 1, &sent_at, false)?;
        }
        ff::avcodec_send_packet(ctx, ptr::null());
        drain(packets.len(), &sent_at, true)?;

        let fmt = first_fmt.unwrap_or(-1);
        let fmt_name = if fmt == ff::AVPixelFormat::AV_PIX_FMT_D3D11 as i32 { "d3d11 texture (hardware)".to_string() } else { format!("pix_fmt {fmt} (NOT d3d11)") };
        println!(
            "frames out {}/{} | send errors {send_errors} | decode errors {errors} | output {fmt_name}",
            submit.len(),
            packets.len()
        );
        println!("decoder pipeline depth (frames held back): max {depth_max}");
        pct("send_packet -> frame returned", &mut submit);
        if !total.is_empty() {
            pct("send_packet -> frame in system memory (readback)", &mut total);
        }

        ff::av_frame_free(&mut (frame as *mut _));
        let mut sw = sw;
        ff::av_frame_free(&mut sw);
        let mut pkt = pkt;
        ff::av_packet_free(&mut pkt);
        ff::avcodec_free_context(&mut ctx);
        if !hw.is_null() {
            ff::av_buffer_unref(&mut hw);
        }
    }
    Ok(())
}

fn pct(name: &str, v: &mut [u64]) {
    if v.is_empty() {
        println!("{name}: no samples");
        return;
    }
    v.sort_unstable();
    let p = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize] as f64 / 1000.0;
    println!("{name}: p50 {:.2} p95 {:.2} p99 {:.2} max {:.2} ms (n={})", p(0.5), p(0.95), p(0.99), p(1.0), v.len());
}
