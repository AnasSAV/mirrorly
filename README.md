# iphone_screen_mirror

Low-latency iPhone screen mirroring and control for Windows, in Rust
("scrcpy for iPhone"). USB first, Wi-Fi later.

**Status: Phase 1 (USB MVP) under test.** Phase 0 findings:
[FEASIBILITY.md](FEASIBILITY.md). The code in `spike/` and `spike-decode/` is
throwaway measurement tooling; the app lives in `crates/`.

## Run

```powershell
target\release\iphone-mirror.exe            # add --vsync to present with vsync
```

| Input | On the phone |
|---|---|
| Left click / drag | Tap / swipe (touch) |
| Drag starting at the very bottom edge | Home; drag up and pause for the App Switcher |
| Right click | Home |
| Keyboard | Types into the focused field; Ctrl+C/V/X/A/Z act as ⌘ shortcuts |
| F1 / F2 / F3 / F4 / F5 (or the menu bar) | Home / App Switcher / Lock / Volume − / Volume + |
| F12 | Flip the touch mapping for landscape video |

A white dot marks a click the instant it happens and disappears when the
phone's response arrives. The console shows connection status and a stats
line every 5 s.

## Layout

| Crate | Role |
|---|---|
| `crates/device` | usbmuxd (TCP_NODELAY), CoreDeviceProxy tunnel, Developer Disk Image, media session, RTCP feedback, HID execution, reconnect (network thread) |
| `crates/video` | RTP/HEVC depacketizer (marker-bit), FFmpeg d3d11va decode into textures on the app's D3D11 device (decode thread) |
| `crates/input` | Window → panel coordinate mapping, gesture state (touch vs bottom-edge system gesture), key mapping |
| `crates/app` | Window, DXGI flip-model swapchain, zero-copy NV12 render, present thread, menu bar |

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

The app and `spike-decode` use FFmpeg through bindgen, which needs the MSVC
environment for the C headers:

```powershell
cmd /c "\"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat\" && cargo build --release"
cargo build --release -p spike            # s0..s3, no FFmpeg needed
```

The build copies the FFmpeg DLLs next to `iphone-mirror.exe`.

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
