//! s1: open the CoreDeviceProxy tunnel over USB, list the services we need,
//! and (with --install-ddi) install the iOS 27 Cryptex1 DDI if it is missing.

use anyhow::Result;
use clap::Parser;
use spike::{
    conn::{NoDelayUsbmux, open_tunnel, usb_device},
    ddi,
};

#[derive(Parser)]
struct Args {
    /// Download and install the Developer Disk Image if none is installed.
    #[arg(long)]
    install_ddi: bool,
    /// Print every RSD service, not just the interesting ones.
    #[arg(long)]
    all: bool,
}

const WANTED: [&str; 4] = [
    "com.apple.coredevice.displayservice",
    "com.apple.coredevice.hid.universalhidservice",
    "com.apple.coredevice.hid.indigo",
    "com.apple.security.cryptexd.remote",
];

#[tokio::main]
async fn main() -> Result<()> {
    spike::init_logging();
    let args = Args::parse();
    let dev = usb_device().await?;
    let started = std::time::Instant::now();
    let mut t = open_tunnel(&NoDelayUsbmux::new(&dev)).await?;
    println!("tunnel: host {} <-> device {} (mtu {})", t.host_ip, t.device_ip, t.mtu);
    for (stage, ms) in &t.timings {
        println!("  {stage:<40} {ms:>8.1} ms");
    }
    println!("  {:<40} {:>8.1} ms", "total", spike::conn::ms(started));

    println!("RSD services: {}", t.handshake.services.len());
    let mut names: Vec<_> = t.handshake.services.keys().cloned().collect();
    names.sort();
    for name in &names {
        if args.all || name.contains("coredevice") || name.contains("hid") || name.contains("cryptex") {
            let s = &t.handshake.services[name];
            println!("  {name} (port {})", s.port);
        }
    }
    for w in WANTED {
        println!("  [{}] {w}", if t.handshake.services.contains_key(w) { "x" } else { " " });
    }

    match ddi::installed(&mut t).await {
        Ok(Some(c)) => println!("DDI cryptex installed: {} {}", c.identifier, c.version),
        Ok(None) if args.install_ddi => {
            let c = ddi::install(&mut t).await?;
            println!("DDI cryptex installed now: {} {}", c.identifier, c.version);
            println!("Re-run s1 to confirm the displayservice now appears.");
        }
        Ok(None) => println!("DDI cryptex NOT installed (re-run with --install-ddi)"),
        Err(e) => println!("could not query cryptexd: {e:#}"),
    }
    Ok(())
}
