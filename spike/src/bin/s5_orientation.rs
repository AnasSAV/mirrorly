//! s5: does SpringBoard's `getInterfaceOrientation` follow the frontmost
//! app's rotation? Polls it and prints every change with the poll cost.
//!
//!   s5_orientation --seconds 30 --every-ms 250

use std::time::{Duration, Instant};

use anyhow::Result;
use clap::Parser;
use idevice::{IdeviceService, springboardservices::SpringBoardServicesClient};
use spike::conn::{NoDelayUsbmux, usb_device};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value_t = 30.0)]
    seconds: f64,
    #[arg(long, default_value_t = 250)]
    every_ms: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    spike::init_logging();
    let args = Args::parse();
    let dev = usb_device().await?;
    let provider = NoDelayUsbmux::new(&dev);
    let t = Instant::now();
    let mut sb = SpringBoardServicesClient::connect(&provider).await?;
    println!("springboardservices connected in {:.0} ms", t.elapsed().as_secs_f64() * 1e3);

    let start = Instant::now();
    let mut last = None;
    let mut costs = Vec::new();
    while start.elapsed().as_secs_f64() < args.seconds {
        let a = Instant::now();
        let o = sb.get_interface_orientation().await?;
        costs.push(a.elapsed().as_micros() as u64);
        let s = format!("{o:?}");
        if last.as_ref() != Some(&s) {
            println!("[{:>6.2} s] {s}", start.elapsed().as_secs_f64());
            last = Some(s);
        }
        tokio::time::sleep(Duration::from_millis(args.every_ms)).await;
    }
    costs.sort_unstable();
    println!(
        "{} polls; cost p50 {:.2} ms, max {:.2} ms",
        costs.len(),
        costs[costs.len() / 2] as f64 / 1e3,
        *costs.last().unwrap() as f64 / 1e3
    );
    Ok(())
}
