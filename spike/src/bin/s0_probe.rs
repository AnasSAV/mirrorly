//! s0: is the phone visible over usbmuxd, paired, and in Developer Mode?

use anyhow::Result;
use idevice::{
    IdeviceService, amfi::AmfiClient, lockdown::LockdownClient, mobile_image_mounter::ImageMounter,
    provider::IdeviceProvider,
};
use spike::conn::{NoDelayUsbmux, usb_device, usbmuxd};

#[tokio::main]
async fn main() -> Result<()> {
    spike::init_logging();
    let mut mux = usbmuxd().await?;
    let all = mux.get_devices().await?;
    println!("usbmuxd devices: {}", all.len());
    for d in &all {
        println!("  {} id={} via {:?}", d.udid, d.device_id, d.connection_type);
    }
    let dev = usb_device().await?;
    let provider = NoDelayUsbmux::new(&dev);

    let pairing = provider.get_pairing_file().await;
    println!("pair record from usbmuxd: {}", if pairing.is_ok() { "present" } else { "MISSING" });
    let pairing = pairing?;

    let mut lockdown = LockdownClient::connect(&provider).await?;
    lockdown.start_session(&pairing).await?;
    for key in ["DeviceName", "ProductType", "ProductVersion", "BuildVersion", "HardwareModel"] {
        let v = lockdown.get_value(Some(key), None).await;
        println!("{key:>15}: {}", show(v.ok()));
    }
    // Screen geometry, if lockdown publishes it (iTunes domain).
    for key in ["ScreenWidth", "ScreenHeight", "ScreenScaleFactor"] {
        let v = lockdown.get_value(Some(key), Some("com.apple.mobile.iTunes")).await;
        println!("{key:>15}: {}", show(v.ok()));
    }
    let dev_mode = lockdown
        .get_value(Some("DeveloperModeStatus"), Some("com.apple.security.mac.amfi"))
        .await;
    println!("{:>15}: {} (lockdown amfi domain)", "DeveloperMode", show(dev_mode.ok()));
    match ImageMounter::connect(&provider).await {
        Ok(mut m) => match m.query_developer_mode_status().await {
            Ok(b) => println!("{:>15}: {b} (mobile_image_mounter)", "DeveloperMode"),
            Err(e) => println!("{:>15}: query failed: {e:?}", "DeveloperMode"),
        },
        Err(e) => println!("mobile_image_mounter unavailable: {e:?}"),
    }

    // Only makes Settings -> Privacy & Security -> Developer Mode visible; the
    // user still turns it on (and the phone restarts) by hand.
    if std::env::args().any(|a| a == "--reveal-devmode") {
        let mut amfi = AmfiClient::connect(&provider).await?;
        amfi.reveal_developer_mode_option_in_ui().await?;
        println!("asked iOS to show the Developer Mode switch in Settings");
    }
    Ok(())
}

fn show(v: Option<plist::Value>) -> String {
    match v {
        Some(plist::Value::String(s)) => s,
        Some(plist::Value::Boolean(b)) => b.to_string(),
        Some(other) => format!("{other:?}"),
        None => "(unavailable)".into(),
    }
}
