//! Where Mirrorly keeps its files: `%LOCALAPPDATA%\Mirrorly`.

use std::path::PathBuf;

pub const APP_DIR: &str = "Mirrorly";
/// Folder name used before the app had a name (Phase 1-3 builds).
const OLD_APP_DIR: &str = "iphone-mirror";

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = base.join(APP_DIR);
    let old = base.join(OLD_APP_DIR);
    // Keep Wi-Fi pairings and the cached disk image from earlier builds.
    if !dir.exists() && old.exists() {
        let _ = std::fs::rename(&old, &dir);
    }
    dir
}
