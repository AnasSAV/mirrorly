//! Developer Disk Image for iOS 27: installed as a Cryptex1 over `cryptexd`
//! (the PersonalizedDMG image-mounter route is for older releases).
//!
//! The payloads come from the community DeveloperDiskImage repository that
//! pymobiledevice3 also uses, so no Xcode is needed. Apple's files are
//! downloaded to a local cache at runtime and never redistributed by us.
//!
//! The download is pinned: a commit of that repository and the SHA-256 of
//! every payload. Upstream (or anything else between this build and GitHub)
//! therefore can't swap the bytes under it, and a cache entry that doesn't
//! match is fetched again instead of installed. To move to a newer DDI, bump
//! `COMMIT` and the hashes below together.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use idevice::cryptexd::{Cryptex1Assets, InstalledCryptex, install_ddi, installed_ddi};

use crate::conn::Tunnel;

const REPO: &str = "https://raw.githubusercontent.com/doronz88/DeveloperDiskImage";
/// The commit these payloads (and hashes) were taken from, 2026-11-19.
const COMMIT: &str = "a9ae4c1c47105d195d860badf04c0e2a835b99d8";
const DIR: &str = "PersonalizedImages/Xcode_iOS_DDI_Cryptex";

/// Every payload, with the SHA-256 it must have.
const FILES: [(&str, &str); 5] = [
    ("BuildManifest.plist", "537391eb2f37e604fc693f422c1568af97220067853e663391ba3e8258569d6e"),
    ("Image.dmg", "500d4139f7963650e6c796d950267ed38909e7d183ad26bba073257529bfe173"),
    ("Image.dmg.trustcache", "2719aa3f1c99245432512116fb8a56d55ec224c706e545772d4c17c1233ac475"),
    ("Image.dmg.cryptex_info", "0180ee20ce5daa7978fe38f24c742361726789ea6af82c3c1d2afc480734a531"),
    ("Image.dmg.root_hash", "5f3281961396be5fe008e11641ffc25c7db24c11d3789ff0c4ff0fa6d47b94ca"),
];

pub fn cache_dir() -> PathBuf {
    crate::paths::data_dir().join("ddi-cryptex")
}

pub async fn installed(t: &mut Tunnel) -> Result<Option<InstalledCryptex>> {
    Ok(installed_ddi(&mut t.adapter, &mut t.handshake).await?)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether the file is there and is exactly the pinned payload.
fn verified(path: &Path, want: &str) -> bool {
    std::fs::read(path).is_ok_and(|b| sha256_hex(&b) == want)
}

/// Download the cryptex payloads (if not cached) and install them.
pub async fn install(t: &mut Tunnel) -> Result<InstalledCryptex> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir)?;
    for (name, want) in FILES {
        let path = dir.join(name);
        if verified(&path, want) {
            continue;
        }
        // Missing, stale or tampered with: fetch it again; never install what
        // is in the cache when it doesn't match.
        let _ = std::fs::remove_file(&path);
        tracing::info!("downloading DDI payload {name}");
        // curl.exe ships with Windows 10+; good enough for a spike.
        let status = std::process::Command::new("curl.exe")
            .args(["-fL", "--retry", "3", "--proto", "=https", "-o"])
            .arg(&path)
            .arg(format!("{REPO}/{COMMIT}/{DIR}/{name}"))
            .status()
            .context("running curl.exe")?;
        if !status.success() {
            let _ = std::fs::remove_file(&path);
            bail!("download of {name} failed ({status})");
        }
        if !verified(&path, want) {
            let _ = std::fs::remove_file(&path);
            bail!("{name} does not match the pinned SHA-256; refusing to install it");
        }
    }
    let assets = Cryptex1Assets::load(&dir)
        .await
        .context("loading cryptex DDI payloads")?;
    tracing::info!("requesting a TSS signature for the DDI and installing it");
    Ok(install_ddi(&mut t.adapter, &mut t.handshake, &assets).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests_are_256_bits_of_hex() {
        let d = sha256_hex(b"abc");
        assert_eq!(d.len(), 64);
        assert_eq!(d, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert!(FILES.iter().all(|(_, want)| want.len() == 64 && want.bytes().all(|c| c.is_ascii_hexdigit())));
    }
}
