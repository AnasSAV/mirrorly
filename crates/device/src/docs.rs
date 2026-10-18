//! Documents from the PC, saved by name where the Files app shows them.
//!
//! iOS keeps the Files app's own "On My iPhone" storage off limits to
//! developer services (2026-10-18, iOS 27: the CoreDevice file service
//! refuses `group.com.apple.FileProvider.LocalStorage` and
//! `com.apple.DocumentsApp` with ContainerLookupErrorDomain 7 / 2). What is
//! open is the Documents folder of apps that share files with a computer
//! (`UIFileSharingEnabled`, the Finder/iTunes "Files" tab), through
//! house_arrest `VendDocuments`. Apps that also open documents in place show
//! that folder in Files as On My iPhone › <app>. Dropped documents go into a
//! "Mirrorly" folder there, with their names.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use idevice::{
    RsdService,
    afc::{AfcClient, opcode::AfcFopenMode},
    house_arrest::HouseArrestClient,
    installation_proxy::InstallationProxyClient,
    rsd::RsdHandshake,
    tcp::handle::AdapterHandle,
};

const FOLDER: &str = "Mirrorly";

/// Preferred hosts, when installed: general-purpose apps whose Files folder
/// is a natural place for anything.
const PREFERRED: &[&str] = &["org.videolan.vlc-ios", "com.google.chrome.ios"];

/// An app whose Documents folder Files shows.
#[derive(Debug, Clone)]
pub struct Host {
    pub bundle_id: String,
    pub name: String,
}

impl Host {
    /// Where the user finds the files, as Files names it.
    pub fn place(&self) -> String {
        format!("On My iPhone › {} › {FOLDER}", self.name)
    }
}

/// Pick the app to keep dropped documents in.
pub async fn find_host(adapter: &mut AdapterHandle, handshake: &mut RsdHandshake) -> Result<Option<Host>> {
    let mut ip = InstallationProxyClient::connect_rsd(adapter, handshake).await.context("installation_proxy")?;
    let apps = ip.get_apps(Some("User"), None).await?;
    let mut hosts: Vec<Host> = apps
        .iter()
        .filter_map(|(id, info)| {
            let d = info.as_dictionary()?;
            let flag = |k: &str| d.get(k).and_then(|v| v.as_boolean()).unwrap_or(false);
            (flag("UIFileSharingEnabled") && flag("LSSupportsOpeningDocumentsInPlace")).then(|| Host {
                bundle_id: id.clone(),
                name: d
                    .get("CFBundleDisplayName")
                    .or(d.get("CFBundleName"))
                    .and_then(|v| v.as_string())
                    .unwrap_or(id)
                    .to_string(),
            })
        })
        .collect();
    let rank = |h: &Host| PREFERRED.iter().position(|p| *p == h.bundle_id).unwrap_or(PREFERRED.len());
    hosts.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(hosts.into_iter().next())
}

/// Copy `files` into the host app's Documents/Mirrorly, keeping names (a
/// number is added rather than overwrite anything).
pub async fn save(
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    host: &Host,
    files: &[(&Path, Vec<u8>)],
) -> Result<()> {
    let ha = HouseArrestClient::connect_rsd(adapter, handshake).await.context("house_arrest")?;
    let mut afc: AfcClient = ha.vend_documents(host.bundle_id.clone()).await.with_context(|| format!("{} documents", host.name))?;
    let dir = format!("/Documents/{FOLDER}");
    let existing = match afc.list_dir(&dir).await {
        Ok(l) => l,
        Err(_) => {
            afc.mk_dir(&dir).await.context("create the Mirrorly folder")?;
            Vec::new()
        }
    };
    let mut taken: Vec<String> = existing.into_iter().map(|n| n.to_lowercase()).collect();
    for (path, bytes) in files {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or_else(|| anyhow!("no file name"))?;
        let name = free_name(&name, &taken);
        taken.push(name.to_lowercase());
        let target = format!("{dir}/{name}");
        let mut f = afc.open(&target, AfcFopenMode::WrOnly).await.with_context(|| format!("create {target}"))?;
        f.write_entire(bytes).await.with_context(|| format!("write {target}"))?;
        f.close().await?;
        tracing::info!("saved {} ({} B) to {}", name, bytes.len(), host.place());
    }
    Ok(())
}

/// `name`, or `stem (2).ext` and so on if a file by that name exists.
fn free_name(name: &str, taken: &[String]) -> String {
    let free = |n: &str| !taken.contains(&n.to_lowercase());
    if free(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    (2..).map(|n| format!("{stem} ({n}){ext}")).find(|n| free(n)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_never_overwrite() {
        let taken = vec!["report.pdf".to_string(), "report (2).pdf".to_string()];
        assert_eq!(free_name("Report.pdf", &taken), "Report (3).pdf");
        assert_eq!(free_name("other.pdf", &taken), "other.pdf");
    }
}
