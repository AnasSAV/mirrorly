//! Copy the FFmpeg DLLs (LGPL shared build) next to the executable so it runs
//! without touching PATH.

use std::path::PathBuf;

fn main() {
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
