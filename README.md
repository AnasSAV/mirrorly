# iphone_screen_mirror

Low-latency iPhone screen mirroring and control for Windows, in Rust
("scrcpy for iPhone"). USB first, Wi-Fi later.

**Status: USB and Wi-Fi working (Phases 1–3).** Phase 0 findings:
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
| F12 / Shift+F12 | Rotate the view clockwise / counter-clockwise (the phone always streams portrait; rotate for landscape apps) |
| F10 | Latency overlay (per-stage timings, drops, tap results) |
| F9 | Latency probe: 10 timed taps where the mouse points (aim at something that reacts, e.g. a Calculator key) |
| F11 | Borderless fullscreen |

A white dot marks a click the instant it happens and disappears when the
phone's response arrives. The console shows connection status and a stats
line every 5 s. Options: `--vsync`, `--overlay`, `--timings-csv FILE`,
`--rctl pmd3|idevice|off`, `--no-auto-ddi`, `--no-wifi`. Diagnostics via
environment: `MIRROR_TOUCH=uhs|indigo` (force a touch path),
`MIRROR_NAGLE=1`, `MIRROR_DXGI_TRACE=1`. How latency is measured, and
every result: [LATENCY.md](LATENCY.md).

## Wi-Fi

1. Connect the iPhone by USB once and start the app. It creates a Wi-Fi
   pairing record automatically and silently (the phone already trusts this PC
   over USB); you'll see "Setting up Wi-Fi (one time)…" in the title.
2. Unplug. The app looks for the phone on the network (Bonjour,
   `_remotepairing._tcp`) and continues over Wi-Fi in about 2 s. Plug the cable
   in again and it switches back to USB by itself: USB is always preferred.
3. The title shows the transport; the light in the top-right corner of the
   picture shows link quality (green < 10 ms jitter, amber < 25 ms, red above
   that or after any loss / keyframe request in the last 2 s).

`--no-wifi` disables all of this. Pairing records live in
`%LOCALAPPDATA%\iphone-mirror\wifi\` (delete the files to forget the phone).

### Windows requirements for Wi-Fi

- **Network profile Private**: Settings → Network & internet → Wi-Fi → (your
  network) → Network profile type → Private. On Public, Windows Firewall drops
  the phone's Bonjour (mDNS, UDP 5353) answers.
- **Firewall**: if Windows asks whether `iphone-mirror.exe` may communicate on
  private networks, allow it (inbound UDP 5353 for discovery; the rest are
  outbound TCP connections to the phone).
- **Same network, no client isolation**: PC and iPhone on the same Wi-Fi.
  Guest networks and many apartment/hotel networks isolate clients from each
  other; mirroring cannot work there.
- **VPN**: a full-tunnel VPN can hide the local network; disconnect it if the
  phone is not found.
- **Wi-Fi power saving off** for the lowest jitter: Device Manager → Network
  adapters → your Wi-Fi adapter → Power Management → untick "Allow the
  computer to turn off this device to save power"; and in Control Panel →
  Power Options → Change plan settings → Advanced → Wireless Adapter Settings
  → Power Saving Mode → Maximum Performance.
- **iPhone**: unlocked, on the same Wi-Fi.

Over Wi-Fi, touches are sent as Indigo digitizer events: iOS ignores the
touchscreen reports used over USB when they arrive over the network
(measured; cause unknown). Latency vs USB is in [LATENCY.md](LATENCY.md):
about +7 ms at the median, with occasional stalls because the tunnel is
TLS over TCP.

## Layout

| Crate | Role |
|---|---|
| `crates/device` | USB (usbmuxd with TCP_NODELAY, CoreDeviceProxy) and Wi-Fi (RemotePairing, Bonjour, TLS-PSK) transports into one tunnel type; Developer Disk Image, media session, RTCP feedback, HID execution, reconnect with USB preferred (network thread) |
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
