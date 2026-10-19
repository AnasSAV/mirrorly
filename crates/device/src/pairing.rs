//! Re-pairing when a device has forgotten this PC.
//!
//! Apple's service (usbmuxd) keeps the PC's pair record, the device keeps
//! its half. If the device lost its half (reset, "Reset Location & Privacy",
//! trust revoked) while the PC still has a record, lockdown refuses the
//! session with InvalidHostID ("device does not have pairing file") and
//! nothing shows the Trust prompt, because the PC believes it is paired.
//! Here the PC asks the device to pair again (lockdown `Pair`), which shows
//! Trust on the device, and stores the new record back in usbmuxd so every
//! tool on this PC uses it. Apple's Windows service refuses SavePairRecord
//! (seen 2026-10-19), so then the record is kept in Mirrorly's own folder
//! (`%LOCALAPPDATA%\Mirrorly\pairing`) and preferred from then on.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use idevice::{IdeviceError, IdeviceService, lockdown::LockdownClient, provider::IdeviceProvider};

use crate::conn::{NoDelayUsbmux, usbmuxd};

fn own_path(udid: &str) -> std::path::PathBuf {
    crate::paths::data_dir().join("pairing").join(format!("{udid}.plist"))
}

/// Mirrorly's own pair record for this device, if it keeps one.
pub fn own_record(udid: &str) -> Option<idevice::pairing_file::PairingFile> {
    idevice::pairing_file::PairingFile::read_from_file(own_path(udid)).ok()
}

/// Drop Mirrorly's own record (Apple's may be the valid one again).
/// True if there was one.
pub fn forget_own_record(udid: &str) -> bool {
    std::fs::remove_file(own_path(udid)).is_ok()
}

/// How long to wait for someone to unlock the device and tap Trust.
const WAIT: Duration = Duration::from_secs(180);

/// "iPad", "iPhone"…: readable before the device trusts us.
pub async fn device_class(provider: &NoDelayUsbmux) -> String {
    let Ok(mut l) = LockdownClient::connect(provider).await else { return "device".into() };
    match l.get_value(Some("DeviceClass"), None).await {
        Ok(plist::Value::String(s)) if !s.is_empty() => s,
        _ => "device".into(),
    }
}

/// Ask the device to trust this PC again; `waiting` is told what to show.
pub async fn repair(provider: &NoDelayUsbmux, udid: &str, waiting: impl Fn(&str)) -> Result<()> {
    let mut mux = usbmuxd().await.context("Apple Mobile Device Service")?;
    let buid = mux.get_buid().await.context("system BUID")?;
    // Keep this PC's identity: the old record's host ID, if there is one.
    let host_id = match provider.get_pairing_file().await {
        Ok(p) => p.host_id,
        Err(_) => uuid::Uuid::new_v4().to_string().to_uppercase(),
    };
    let host_name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows PC".into());
    let class = device_class(provider).await;
    let start = Instant::now();
    let record = loop {
        if start.elapsed() > WAIT {
            bail!("nobody tapped Trust on the {class} within {} minutes", WAIT.as_secs() / 60);
        }
        waiting(&class);
        let mut l = LockdownClient::connect(provider).await?;
        // Returns once Trust is tapped (it polls while the dialog is up).
        match tokio::time::timeout(WAIT, l.pair(host_id.clone(), buid.clone(), Some(&host_name))).await {
            Ok(Ok(record)) => break record,
            // Locked: the dialog shows only once it is unlocked.
            Ok(Err(IdeviceError::PasswordProtected)) => tokio::time::sleep(Duration::from_secs(2)).await,
            Ok(Err(IdeviceError::UserDeniedPairing)) => bail!("\"Don't Trust\" was tapped on the {class}"),
            Ok(Err(e)) => return Err(anyhow!(e).context("pairing")),
            Err(_) => bail!("nobody tapped Trust on the {class}"),
        }
    };
    let bytes = record.serialize().context("pair record")?;
    match mux.save_pair_record(udid, bytes.clone()).await {
        Ok(()) => {
            forget_own_record(udid);
            tracing::info!("{class} {udid} paired again with this PC (stored by Apple's service)");
        }
        Err(e) => {
            // Keep it ourselves; Apple's service keeps the old record.
            let path = own_path(udid);
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, &bytes).with_context(|| format!("write {}", path.display()))?;
            tracing::info!("{class} {udid} paired again with this PC (Apple's service refused it: {e}; kept by Mirrorly)");
        }
    }
    Ok(())
}
