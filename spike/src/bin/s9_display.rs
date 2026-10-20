//! s9: iPad as a second monitor, feasibility. Step 2a: which DXGI adapter
//! and output is the virtual display (Virtual Display Driver), and can
//! Desktop Duplication capture it?
//!
//!   s9_display list
//!   s9_display grab [--seconds 5]      (duplicate the 2360x1640 output)

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use windows::{
    Win32::Graphics::{
        Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
        Direct3D11::{D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device},
        Dxgi::{
            CreateDXGIFactory1, DXGI_ERROR_NOT_FOUND, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter1,
            IDXGIFactory1, IDXGIOutput, IDXGIOutput1,
        },
    },
    core::Interface,
};

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Every adapter and its outputs.
    List,
    /// Put the virtual monitor on the desktop.
    On,
    /// Take it off.
    Off,
    /// Run Mirrorly's display session (capture, encode, serve, touch).
    Serve {
        #[arg(long, default_value_t = 1800)]
        seconds: u64,
    },
    /// Duplicate the output whose size matches (default: the iPad's).
    Grab {
        #[arg(long, default_value_t = 2360)]
        width: i32,
        #[arg(long, default_value_t = 1640)]
        height: i32,
        #[arg(long, default_value_t = 5.0)]
        seconds: f64,
    },
}

fn wide(s: &[u16]) -> String {
    String::from_utf16_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())])
}

/// (adapter, output, adapter name, device name, width, height)
fn outputs() -> Result<Vec<(IDXGIAdapter1, IDXGIOutput, String, String, i32, i32)>> {
    let mut out = Vec::new();
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut a = 0;
        loop {
            let adapter = match factory.EnumAdapters1(a) {
                Ok(x) => x,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(e) => return Err(e.into()),
            };
            let name = wide(&adapter.GetDesc1()?.Description);
            let mut o = 0;
            loop {
                let output = match adapter.EnumOutputs(o) {
                    Ok(x) => x,
                    Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                    Err(e) => return Err(e.into()),
                };
                let d = output.GetDesc()?;
                let r = d.DesktopCoordinates;
                out.push((adapter.clone(), output, name.clone(), wide(&d.DeviceName), r.right - r.left, r.bottom - r.top));
                o += 1;
            }
            if o == 0 {
                println!("adapter {a} \"{name}\": no outputs");
            }
            a += 1;
        }
    }
    Ok(out)
}

fn main() -> Result<()> {
    let args = Args::parse();
    spike::init_logging();
    // Physical pixels, as in Mirrorly.
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    match args.cmd {
        Cmd::On => println!("{:?}", display::screen::attach(None)?.0),
        Cmd::Off => display::monitor_off()?,
        Cmd::Serve { seconds } => {
            let session = display::DisplaySession::start(None)?;
            println!("virtual screen: {:?}", session.screen);
            for url in display::DisplaySession::urls() {
                println!("open on the iPad: {url}");
            }
            std::thread::sleep(Duration::from_secs(seconds));
        }
        Cmd::List => {
            for (i, (_, _, adapter, dev, w, h)) in outputs()?.iter().enumerate() {
                println!("{i}: {dev} {w}x{h} on \"{adapter}\"");
            }
        }
        Cmd::Grab { width, height, seconds } => unsafe {
            let all = outputs()?;
            let Some((adapter, output, name, dev, w, h)) = all.into_iter().find(|o| o.4 == width && o.5 == height) else {
                bail!("no output of {width}x{height} (run `list`)");
            };
            println!("duplicating {dev} {w}x{h} on \"{name}\"");
            let mut device: Option<ID3D11Device> = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )?;
            let device = device.context("no D3D11 device")?;
            let output1: IDXGIOutput1 = output.cast()?;
            let dup = output1.DuplicateOutput(&device).context("DuplicateOutput")?;
            let start = Instant::now();
            let (mut frames, mut timeouts) = (0u32, 0u32);
            let mut acquire = Vec::new();
            while start.elapsed() < Duration::from_secs_f64(seconds) {
                let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
                let mut res = None;
                let t = Instant::now();
                match dup.AcquireNextFrame(100, &mut info, &mut res) {
                    Ok(()) => {
                        acquire.push(t.elapsed().as_micros() as u64);
                        if info.LastPresentTime != 0 {
                            frames += 1;
                        }
                        dup.ReleaseFrame()?;
                    }
                    Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => timeouts += 1,
                    Err(e) => return Err(e).context("AcquireNextFrame"),
                }
            }
            println!(
                "{frames} new frames in {seconds} s ({:.1} fps), {timeouts} idle timeouts (an unchanging screen sends nothing)",
                frames as f64 / seconds
            );
        },
    }
    Ok(())
}
