//! Developer Disk Image for iOS 27: installed as a Cryptex1 over `cryptexd`
//! (the PersonalizedDMG image-mounter route is for older releases).
//!
//! The payloads come from the community DeveloperDiskImage repository that
//! pymobiledevice3 also uses, so no Xcode is needed. Apple's files are
//! downloaded to a local cache at runtime and never redistributed by us.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use idevice::cryptexd::{Cryptex1Assets, InstalledCryptex, install_ddi, installed_ddi};

use crate::conn::Tunnel;

const REPO_RAW: &str =
    "https://raw.githubusercontent.com/doronz88/DeveloperDiskImage/main/PersonalizedImages/Xcode_iOS_DDI_Cryptex";
const FILES: [&str; 5] = [
    "BuildManifest.plist",
    "Image.dmg",
    "Image.dmg.trustcache",
    "Image.dmg.cryptex_info",
    "Image.dmg.root_hash",
];

pub fn cache_dir() -> PathBuf {
    crate::paths::data_dir().join("ddi-cryptex")
}

pub async fn installed(t: &mut Tunnel) -> Result<Option<InstalledCryptex>> {
    Ok(installed_ddi(&mut t.adapter, &mut t.handshake).await?)
}

/// Download the cryptex payloads (if not cached) and install them.
pub async fn install(t: &mut Tunnel) -> Result<InstalledCryptex> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir)?;
    for name in FILES {
        let path = dir.join(name);
        if path.exists() {
            continue;
        }
        tracing::info!("downloading DDI payload {name}");
        // curl.exe ships with Windows 10+; good enough for a spike.
        let status = std::process::Command::new("curl.exe")
            .args(["-fL", "--retry", "3", "-o"])
            .arg(&path)
            .arg(format!("{REPO_RAW}/{name}"))
            .status()
            .context("running curl.exe")?;
        if !status.success() {
            let _ = std::fs::remove_file(&path);
            bail!("download of {name} failed ({status})");
        }
    }
    let assets = Cryptex1Assets::load(&dir)
        .await
        .context("loading cryptex DDI payloads")?;
    tracing::info!("requesting a TSS signature for the DDI and installing it");
    Ok(install_ddi(&mut t.adapter, &mut t.handshake, &assets).await?)
}
