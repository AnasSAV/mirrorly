//! Copy the FFmpeg DLLs (LGPL shared build) next to the executable so it runs
//! without touching PATH.

use std::path::PathBuf;

fn main() {
    embed_resources();
    copy_ffmpeg_dlls();
}

/// Icon (resource 1, also used as the window icon) and the version info
/// shown in Explorer's file properties.
fn embed_resources() {
    let icon = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../packaging/mirrorly.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let mut res = winresource::WindowsResource::new();
    res.set_icon_with_id(icon.to_str().unwrap(), "1")
        .set("ProductName", "Mirrorly")
        .set("FileDescription", "Mirrorly")
        .set("CompanyName", "AnasSAV")
        .set("LegalCopyright", "Copyright (c) 2026 AnasSAV. MIT License.")
        .set("OriginalFilename", "Mirrorly.exe");
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed the icon/version resource: {e}");
    }
}

fn copy_ffmpeg_dlls() {
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    let Ok(ffmpeg) = std::env::var("FFMPEG_DIR") else {
        println!("cargo:warning=FFMPEG_DIR not set; FFmpeg DLLs not copied");
        return;
    };
    let bin = PathBuf::from(ffmpeg).join("bin");
    // OUT_DIR = target/<profile>/build/app-<hash>/out
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let Some(profile_dir) = out.ancestors().nth(3) else { return };
    let Ok(entries) = std::fs::read_dir(&bin) else {
        println!("cargo:warning=no FFmpeg bin dir at {}", bin.display());
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")) {
            let dst = profile_dir.join(p.file_name().unwrap());
            let stale = std::fs::metadata(&dst)
                .and_then(|d| Ok(d.len() != e.metadata()?.len()))
                .unwrap_or(true);
            if stale {
                let _ = std::fs::copy(&p, &dst);
            }
            println!("cargo:rerun-if-changed={}", p.display());
        }
    }
}
