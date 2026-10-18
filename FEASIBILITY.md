# Phase 0 — Feasibility (GATE 0)

Date: 2026-10-18. All numbers below were measured on the hardware listed in
§1 with the throwaway binaries in `spike/` and `spike-decode/` (release builds,
1 ms timer resolution, `Instant` = QueryPerformanceCounter).

## Verdict

| # | Question | Answer |
|---|----------|--------|
| 1 | Pair + iOS 17+ tunnel on Windows over USB with `idevice`? | **Yes.** CoreDeviceProxy over Apple's usbmuxd + idevice's userspace TCP/UDP stack. 128–167 ms to open (warm). No TUN driver needed. |
| 2 | Display service + RTP/HEVC stream, playable dump? | **Yes.** 10 s → 315 HEVC access units, 0 loss, 0 decode errors, 832×1808 Main. `out/capture.h265` plays in ffplay. |
| 3 | Live streamed touch / buttons / keyboard? | **Yes, all streamed.** `com.apple.coredevice.hid.universalhidservice` (touch) and `com.apple.coredevice.hid.indigo` (buttons, keyboard). Tap, live 120 Hz drags, Home, typing, ⌘-shortcuts and a live mouse window all verified on the phone. Host-side send cost < 0.35 ms. |
| 3c | Fallbacks (WebDriverAgent, Bluetooth HID)? | **Not needed** — not measured (a streamed path exists). |
| 4 | Licenses | `idevice` + `jktcp` MIT; all 303 resolved crates permissive; FFmpeg LGPL-3.0 (dynamic). No GPL in what we ship. Pane / pymobiledevice3 (GPL-3.0) used for reading only. |

**Recommended input path:** CoreDevice HID over the same tunnel as video —
`universalhidservice` 58-byte touchscreen reports to surface 257 for touch,
`hid.indigo` `IndigoButtonEvent` for buttons and `IndigoKeyboardButtonEvent`
for keys. **Decoder:** FFmpeg d3d11va on the Intel iGPU (the adapter that owns
the panel).

**The one red flag is latency on the phone side**, not ours: tap → first
changed frame leaving the phone measured **87 ms p50** in Calculator (§5.4).
Everything on the PC side measured so far costs a few ms. The 35–70 ms
click-to-photon target is likely not reachable for every app; Phase 2's
camera test has to establish the real glass-to-glass number and how much of
it the phone's own touch→display path already accounts for.

## 1. Test environment

| | |
|---|---|
| PC | Windows 11 Home 10.0.26200, i5-13450HX, Intel UHD (internal 144 Hz panel) + RTX 4050 Laptop (Optimus) |
| Phone | iPhone 11 (`iPhone12,1`, N104AP), iOS **27.0 build 24A5430a** (a seed/beta build), passcode set |
| Apple stack | Apple Mobile Device Service (usbmuxd) on 127.0.0.1:27015 |
| Toolchain | Rust 1.99.1 MSVC, VS 2022 Build Tools, LLVM (libclang for bindgen), FFmpeg 9.0 LGPL shared (BtbN) |
| Crates | `idevice =0.1.68`, `jktcp 0.1.7`, `ffmpeg-sys-next =9.0.1`, `winit =0.30.13`, `windows =0.62.2` |

## 2. Setup findings (things the brief didn't anticipate)

- **Rust was not installed**, nor MSVC build tools. Installed via winget.
- **Developer Mode was off** (both lockdown's AMFI domain and the image
  mounter reported `false`). `s0_probe --reveal-devmode` (AMFI
  `reveal_developer_mode_option_in_ui`) made the switch appear; the user
  enabled it by hand.
- **iOS 27 installs the Developer Disk Image as a Cryptex1 over `cryptexd`**,
  not via the PersonalizedDMG / image-mounter route used on iOS 17–26.
  `idevice::cryptexd::install_ddi` does it: payloads from the community
  `doronz88/DeveloperDiskImage` repo (`PersonalizedImages/Xcode_iOS_DDI_Cryptex`),
  signed per-device by Apple's TSS server. Installed:
  `com.apple.MobileAsset.DDI 27.1.5228.8`. Without it the RSD list has 64
  services and no display/HID; with it, 85 services including
  `displayservice`, `hid.indigo`, `hid.universalhidservice`.
  Like Xcode's, the DDI is expected not to survive a reboot (not yet verified
  on this phone) — the app must check and reinstall.
- **The coded picture is 832×1808** while the answer's `CustomWidth/Height`
  is 832×1792 and the panel is 828×1792. The SPS carries no cropping that
  ffprobe applies. Phase 1 coordinate mapping must use the real content rect
  (to be pinned down by checking the SPS conformance window / pixels).

## 3. Q1 — Tunnel (USB)

`s1_tunnel`:

| Stage | Cold (first run) | Warm |
|---|---|---|
| lockdown session + CoreDeviceProxy handshake | 318 ms | 100–131 ms |
| userspace TCP connect to RSD | 2.5 ms | 2.4–5.8 ms |
| RSD handshake | 27.6 ms | 21–33 ms |
| **total** | **348 ms** | **128–167 ms** |

Tunnel: host `fd..::2` ↔ device `fd..::1`, MTU 16000. Pairing record comes
from usbmuxd (the phone had already trusted this PC).

Latency-relevant detail: idevice's `UsbmuxdAddr` opens the 127.0.0.1:27015
socket **without TCP_NODELAY**, and every byte (video in, HID out) rides that
one socket. The spike's `NoDelayUsbmux` provider opens it with NODELAY; keep
that in Phase 1.

## 4. Q2 — Video

Session setup mirrors Device Hub: audio `startmediastream` first, then video
on the same `clientSessionID`. Video start: 200–906 ms.

Negotiated `streamConfig` (`out/answer.plist`): HEVC PT 100, **no SRTP**
(cipher suite 0), `Framerate 60`, `RXMaxBitrate 6 000 000`,
`RateAdaptationEnabled true`, **`IsltrpEnabled true`** (idevice's offer
asks for LTR), `TilesPerFrame 1`, `RTCPTimeoutEnabled true`.

`s2_video --seconds 10` (mostly idle home screen):

| Metric | Value |
|---|---|
| RTP packets / loss / reorder | 1771 / 0 / 0 |
| Access units | 315 (1 keyframe, 0 corrupt) |
| Average rate | 31.5 fps, 1.50 Mbit/s |
| Frame interval (p50 / p95 / max) | 32.9 / 51.3 / 53.2 ms |
| Frame assembly, first→last packet (p50 / p95 / max) | **0.37 / 0.89 / 3.2 ms** |
| Frame size p50 / max | 8.1 KB / 88.9 KB (keyframe) |
| RTP clock | ≈ 24 kHz (measured 24 214 Hz) |
| ffmpeg decode of dump | 315/315 frames, 0 errors |

Observed: **the encoder sends ~30 fps when the screen is still and switches to
60 fps when it changes** (frame spacing drops to ~16 ms during animation).

Depacketizer: the spike's `depack.rs` closes an access unit on the RTP
marker bit. idevice's `HevcDepacketizer` closes it only when the next frame's
first packet arrives (adds up to one frame interval) and on loss waits for 128
further packets before skipping the gap; don't use it in the hot path.

RTCP feedback (`feedback.rs`): RR+SDES every 1 s, per-frame receipt (APP
name=5) on every marker, an `RCTL` APP report every 50 ms, and RR+SDES+PLI+FIR
on loss. Two published layouts of `RCTL` disagree (idevice vs pymobiledevice3);
both are implemented. Over USB with zero loss neither changed tap latency
(§5.4). pymobiledevice3's is the default because its authors tie it to
encoder behaviour under motion — to be verified under motion in Phase 2.

## 5. Q3 — Input

### 5.1 Where it comes from

Pane does not implement input itself: it runs pymobiledevice3's
`ScreenStreamServer` and POSTs one HTTP request per touch from a browser.
pymobiledevice3 `remote/core_device/hid_service.py` and idevice
`services/core_device/hid.rs` both implement the same services (idevice's is
MIT and is what the spike uses):

| What | Service | Message |
|---|---|---|
| Touch | `com.apple.coredevice.hid.universalhidservice` | `Request {send: {_0: report, _1: serviceID}}`, 58-byte report ID 0x09 to surface **257** (mainTouchscreen); state 0xC2 contact / 0x02 release; X/Y u16 normalized 0–65535 |
| Drag | same | stream of CONTACT reports, then one RELEASE — no separate begin/end |
| Buttons | `com.apple.coredevice.hid.indigo` | `IndigoButtonEvent {usagePage, usageCode, state}`; Consumer page 0x0C: Home 0x40, Lock 0x30, Vol+ 0xE9, Vol− 0xEA, Mute 0xE2, Siri 0xCF |
| Keys (path A) | `hid.indigo` | `IndigoKeyboardButtonEvent {usageCode, state}` per key (HID page 0x07) |
| Keys (path B) | `universalhidservice` | `createService` virtual keyboard, then full-bitmap reports |

**Auth gate:** reports are silently dropped unless a displayservice media
stream is running; iOS needs a settle delay after stream start (the spike
waits 500 ms; pymobiledevice3 reports an iPhone 11 needed ≥ 0.4 s). Mirroring
always streams, so this costs nothing at runtime beyond start-up.

HID surfaces on this phone (`s3_input list`): 257 touchscreen, 512 keyboard,
1026 mainScreenButtons, 1280 avpCustom, 1281 touchscreenGesture.

### 5.2 Proof on the phone (user-verified)

| Test | Result |
|---|---|
| `drag` R→L and L→R across the home screen, 37 CONTACT samples each, generated and sent live at **120 Hz achieved**, then RELEASE | pages flipped both ways, smooth |
| `button home` | worked |
| `type "Hello from Indigo 123!\n" --via indigo` | exact text, capitals and symbols correct |
| `type "Hello from UHS 456?\n" --via uhs` | exact text; on-screen keyboard did **not** hide |
| `keys gui+a gui+c down enter gui+v` | select all / copy / paste worked |
| `live` window (mouse = finger, right click = Home, keyboard; Ctrl → ⌘) | "everything felt smooth, nothing got stuck" |

Recommendation: keyboard via **Indigo** (no service registration, no 300 ms
attach wait, natural down/up events). Not yet tested: the bottom-edge
swipe-up-to-Home gesture, which Pane reports iOS does not accept from injected
touches (Pane works around it by pressing Home).

### 5.3 Host-side cost

| Path | per call (p50 / p95 / max) |
|---|---|
| touch CONTACT during 120 Hz drag | 0.11–0.16 / 0.21–0.27 / 0.32 ms |
| keyboard report (Indigo / UHS) | 0.11 / 0.25 / 1.0 ms, 0.09 / 0.20 / 0.26 ms |
| live window: winit event → send start | 0.03 / 0.07 / 0.18 ms (1371 events, 0 coalesced) |
| live window: send call | 0.07 / 0.13 / 0.26 ms |

Sends are fire-and-forget XPC messages (idevice `send_object(.., false)`).

### 5.4 Input → changed frame on the wire (phone-side latency)

`s3_input latency`: on a still screen, send CONTACT and time until the first
packet of the first frame that is clearly bigger than the idle frames (still
frames are ~300–700 B). This covers HID delivery + iOS input handling + app
render + encode + transport; it excludes our decode and present.

| Scenario | n | Result |
|---|---|---|
| Calculator, tap a key, hold 80 ms | 10 | **84.6 / 87.3 / 89.0 ms** (min / p50 / max) |
| same, hold 250 ms | 5 | 85.7 / 86.5 / 89.3 ms |
| same, hold 20 ms | 5 | 85.7 / 87.7 / 96.4 ms |
| same, `--rctl idevice` | 6 | 85.3 / 86.5 / 90.8 ms |
| same, `--rctl off` | 6 | 81.6 / 87.1 / 101.1 ms |
| Home screen, move after finger-down (2 clean trials) | 2 | 51.2, 52.4 ms |

The changed frame does not move with the hold time, so it is triggered by
touch-down, ~87 ms after we send it, very consistently. The home-screen
drag hint (~51 ms) suggests part of Calculator's 87 ms is app animation
timing, but n=2 is not evidence. The other drag trials were invalid: the
long finger-down most likely put the home screen into icon-edit (jiggle) mode.
The automated method's limits are why Phase 2 uses the camera procedure.

## 6. Decoder choice — FFmpeg d3d11va

`s4_decode out/capture.h265` (packets paced at 33 ms like the live stream;
`thread_count = 1`, `AV_CODEC_FLAG_LOW_DELAY`):

| | frames / errors | frames held by decoder | send → D3D11 frame returned p50 / p95 | + forced readback to RAM p50 / p95 |
|---|---|---|---|---|
| Intel UHD (adapter 0) | 315 / 0 | **0** | **0.43 / 0.69 ms** | 5.6 / 7.3 ms |
| RTX 4050 (adapter 1) | 315 / 0 | 0 | 0.35 / 0.62 ms | 4.5 / 7.8 ms |
| software | 315 / 0 | 0 | 2.65 / 3.97 ms | — |

"Returned" is submission, not GPU completion; the readback column (which
waits for the GPU but adds a 832×1808 copy) is an upper bound. The first frame
costs up to 38 ms (decoder init) — once per session.

Why FFmpeg over Media Foundation: d3d11va drives `ID3D11VideoDecoder`
directly through the GPU driver (no dependency on the Store HEVC extension),
low-delay mode with no frame threading gave **zero frames of decoder delay**,
and it decodes into a texture on a device we can share with the swapchain.
With those numbers there is nothing for MF to win, so it was not
benchmarked. **Use the Intel adapter**: it owns the 144 Hz panel, so decode →
present stays on one GPU and zero-copy holds; the dGPU saves ~0.1 ms but would
force a cross-adapter copy.

Build note: `ffmpeg-sys-next` runs bindgen, so builds need `LIBCLANG_PATH` and
the MSVC environment (`vcvars64.bat`) for the C headers.

## 7. Latency budget (USB), from what was measured

| Stage | Estimate |
|---|---|
| Our input send | < 0.35 ms |
| Phone: input → frame on the wire | 51–87 ms measured (app-dependent; Phase 2 to pin down) |
| RTP arrival of a frame | 0.4 ms p50 |
| Decode | 0.4–6 ms (bounded) |
| Present on 144 Hz flip model | 0–7 ms wait + scanout |
| **Glass-to-glass (estimate)** | **≈ 60–100 ms** |

Our pipeline adds roughly 5–15 ms; the phone dominates. Phase 2 must measure
the phone's own touch → its own screen with the same camera, so "mirror lag"
and "iOS app lag" are reported separately. Phone-side levers to try in
Phase 2 (each needs measurement, none is known to help): LTR off
(`IsltrpEnabled` — pymobiledevice3 turns it off), the codec feature string,
the 30→60 fps idle ramp, and the per-frame receipt timing.

## 8. Licenses

| Component | License | Implication |
|---|---|---|
| `idevice` 0.1.68, `jktcp` 0.1.7 | MIT | none |
| all 303 resolved crates | MIT / Apache-2.0 / BSD / ISC / Zlib / Unlicense / WTFPL (`ffmpeg-sys-next`) / CDLA-Permissive-2.0 (`webpki-roots`) | all permissive; keep notices |
| FFmpeg 9.0 BtbN `lgpl-shared` | **LGPL-3.0** (`--enable-version3`), dynamically linked | ship the DLLs unmodified with notice + source offer; allow DLL replacement; no static linking. Use a build without `--enable-version3` if LGPL-2.1 is preferred. Never use the installed Gyan "full" build (GPL). |
| Pane, pymobiledevice3 | GPL-3.0 | read for protocol facts only; no code copied. Constants taken from them (e.g. the 14-byte displayservice NAL trailer, the RCTL field layout) are wire-format facts. |
| Apple Developer Disk Image | Apple | downloaded at runtime to `%LOCALAPPDATA%`, never redistributed |

## 9. Recommendations for Phase 1

1. Keep the brief's crate layout. `device` owns usbmuxd-with-NODELAY, tunnel,
   DDI check/install, media session, RTCP feedback; `video` owns the
   marker-bit depacketizer + d3d11va; `input` owns HID reports.
2. One tunnel carries everything. HID uses its own XPC connections, so input
   never queues behind video at our level; jktcp's single adapter task is
   shared, and it's event-driven (immediate flush plus 1 ms tick).
3. Map coordinates against the content rect of the 832×1808 picture.
4. Keyboard via Indigo; Ctrl → GUI for ⌘ shortcuts (verified).
5. Bottom-edge Home gesture: test first; if iOS rejects injected edge swipes
   (as Pane reports), use Pane's trick — keep the edge drag local and press
   Home / double-Home.
6. Reinstall the DDI automatically after a phone reboot.

## 10. Reproduce

```
target\release\s0_probe.exe [--reveal-devmode]
target\release\s1_tunnel.exe [--install-ddi]
target\release\s2_video.exe --seconds 10 --out out\capture.h265
target\release\s3_input.exe list | tap X Y | drag X1 Y1 X2 Y2 --ms 300 --hz 120
target\release\s3_input.exe button home | type "text" --via indigo | keys gui+a gui+c
target\release\s3_input.exe live
target\release\s3_input.exe latency X Y -n 10 --threshold 1200 [--drag-dx N]
set PATH=third_party\ffmpeg\bin;%PATH% && target\release\s4_decode.exe out\capture.h265 [--readback|--sw]
```
