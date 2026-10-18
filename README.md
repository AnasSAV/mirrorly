# iphone_screen_mirror

Low-latency iPhone screen mirroring and control for Windows, in Rust
("scrcpy for iPhone"). USB first, Wi-Fi later.

**Status: Phase 0 (feasibility spike) done — see [FEASIBILITY.md](FEASIBILITY.md).**
The code in `spike/` and `spike-decode/` is throwaway measurement tooling, not
the app.

## Requirements

Phone:
- iOS 17.4 or later (tested on iOS 27.0), passcode set
- Developer Mode on: *Settings → Privacy & Security → Developer Mode*. If the
  switch is missing, run `s0_probe --reveal-devmode` with the phone plugged in.
- Plugged in by USB, unlocked, and **Trust This Computer** accepted

PC (Windows 10/11 x64):
- [Apple Devices](https://apps.microsoft.com/detail/9np83lwlpz9k) (provides
  Apple Mobile Device Service / usbmuxd)
- Rust (stable, MSVC target): `winget install Rustlang.Rustup`
- Visual Studio 2022 Build Tools with the C++ workload:
  `winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"`
- LLVM, for `ffmpeg-sys-next`'s bindgen: `winget install LLVM.LLVM`
- FFmpeg 9.0 **LGPL shared** dev build unpacked to `third_party/ffmpeg`
  (`include/`, `lib/`, `bin/`), e.g. BtbN's
  `ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip`. `.cargo/config.toml` points
  `FFMPEG_DIR` and `LIBCLANG_PATH` at it. Do not use a GPL build.

## Build

```powershell
cargo build --release -p spike            # s0..s3, no FFmpeg needed
# s4 needs the MSVC environment for bindgen's C headers:
cmd /c "\"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat\" && cargo build --release -p spike-decode"
```

## First run on a new phone

```powershell
target\release\s0_probe.exe                 # sees the phone? paired? Developer Mode?
target\release\s1_tunnel.exe --install-ddi  # opens the tunnel, installs the Developer Disk Image
```

The Developer Disk Image is downloaded from the community DeveloperDiskImage
repository and personalized for the phone by Apple's signing server (as Xcode
does). Expect to install it again after the phone reboots (`s1_tunnel`
reports whether it is present).

## Spike tools

| Binary | Purpose |
|---|---|
| `s0_probe` | usbmuxd, pairing record, iOS version, Developer Mode |
| `s1_tunnel` | CoreDeviceProxy tunnel timings, RSD services, DDI check/install |
| `s2_video` | record the HEVC stream (`--out -` pipes to `ffplay -`) with per-frame timing CSV |
| `s3_input` | tap / live drag / buttons / typing / key chords / live mouse window / latency |
| `s4_decode` | FFmpeg d3d11va decode latency per GPU |

Outputs go to `out/` (gitignored).
