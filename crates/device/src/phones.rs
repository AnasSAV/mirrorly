//! The iPhones Mirrorly can connect to: those plugged in now, and those it
//! remembers from an earlier USB connection (reachable over Wi-Fi). Watched
//! on a small thread of its own; a change is reported to the window.

use std::{collections::HashMap, time::Duration};

use idevice::{IdeviceService, lockdown::LockdownClient};

use crate::{
    conn::{NoDelayUsbmux, usb_devices},
    wifi,
};

/// How often USB attachments are checked (one local usbmuxd request).
const POLL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phone {
    pub udid: String,
    pub name: String,
    /// "27.0", when known.
    pub ios_version: String,
    /// Plugged in right now.
    pub usb: bool,
    /// Has a Wi-Fi pairing (can be forgotten).
    pub remembered: bool,
}

/// Watch for iPhones; `on_change` gets the full list whenever it changes
/// (and once at the start).
pub fn spawn_watch(on_change: impl Fn(Vec<Phone>) + Send + 'static) -> std::io::Result<()> {
    std::thread::Builder::new().name("phones".into()).spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
        rt.block_on(async move {
            // Names of plugged-in phones never seen before, looked up once.
            let mut names: HashMap<String, (String, String)> = HashMap::new();
            let mut last: Option<Vec<Phone>> = None;
            loop {
                let list = scan(&mut names).await;
                if last.as_ref() != Some(&list) {
                    on_change(list.clone());
                    last = Some(list);
                }
                tokio::time::sleep(POLL).await;
            }
        });
    })?;
    Ok(())
}

async fn scan(names: &mut HashMap<String, (String, String)>) -> Vec<Phone> {
    let usb = usb_devices().await.unwrap_or_default();
    let mut phones: Vec<Phone> = wifi::known()
        .into_iter()
        .map(|i| Phone {
            usb: usb.iter().any(|d| d.udid == i.udid),
            udid: i.udid,
            name: i.name,
            ios_version: i.ios_version,
            remembered: true,
        })
        .collect();
    for dev in &usb {
        if phones.iter().any(|p| p.udid == dev.udid) {
            continue;
        }
        if !names.contains_key(&dev.udid) {
            let found = lookup_name(dev).await;
            names.insert(dev.udid.clone(), found);
        }
        let (name, ios_version) = names[&dev.udid].clone();
        phones.push(Phone { udid: dev.udid.clone(), name, ios_version, usb: true, remembered: false });
    }
    // Plugged in first, then by name.
    phones.sort_by(|a, b| b.usb.cmp(&a.usb).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    phones
}

/// Name and iOS version of a plugged-in iPhone Mirrorly hasn't met: asked
/// of lockdown without a session (no prompt on the phone). "iPhone" if it
/// won't say (e.g. not trusted yet).
async fn lookup_name(dev: &idevice::usbmuxd::UsbmuxdDevice) -> (String, String) {
    let provider = NoDelayUsbmux::new(dev);
    let value = async |l: &mut LockdownClient, key: &str| match l.get_value(Some(key), None).await {
        Ok(plist::Value::String(s)) => Some(s),
        _ => None,
    };
    let Ok(Ok(mut l)) = tokio::time::timeout(Duration::from_secs(2), LockdownClient::connect(&provider)).await else {
        return ("iPhone".into(), String::new());
    };
    let name = value(&mut l, "DeviceName").await.unwrap_or_else(|| "iPhone".into());
    let version = value(&mut l, "ProductVersion").await.unwrap_or_default();
    (name, version)
}
