# Architecture

Mirrorly's one rule is **latency first**: nothing waits in a queue it doesn't
have to, and input never waits on video.

![Architecture](https://raw.githubusercontent.com/AnasSAV/mirrorly/main/docs/assets/architecture.svg?sanitize=true)

## Crates

| Crate | Role |
|---|---|
| [`crates/device`](https://github.com/AnasSAV/mirrorly/tree/main/crates/device) | Everything that talks to the phone: the USB and Wi-Fi transports into one tunnel, the Developer Disk Image, the media session and its RTCP feedback, input (HID), the clipboard and files, reconnecting with USB preferred |
| [`crates/video`](https://github.com/AnasSAV/mirrorly/tree/main/crates/video) | RTP → HEVC access units, hardware decoding (FFmpeg d3d11va) into textures on the app's own D3D11 device, the newest-frame slot, the MP4 writer for recordings |
| [`crates/input`](https://github.com/AnasSAV/mirrorly/tree/main/crates/input) | Window → panel coordinates and rotation, gestures (touches vs the system's edge swipes), key mapping |
| [`crates/display`](https://github.com/AnasSAV/mirrorly/tree/main/crates/display) | The iPad as a display: the virtual monitor, capture, encoding, the web server and the Safari page, the one-time setup |
| [`crates/app`](https://github.com/AnasSAV/mirrorly/tree/main/crates/app) | The window, the bar, Direct2D chrome, the swapchain and renderer, drag and drop, the Windows clipboard, screenshots and recordings |
| [`spike`](https://github.com/AnasSAV/mirrorly/tree/main/spike) | Probes and measurement tools (`s0`…`s10`) |

## Threads

| Thread | Does | Never waits on |
|---|---|---|
| **main** | The winit window, mouse and keyboard → touches, keys and gestures, sent straight to the network thread | decoding or presenting |
| **network** (tokio) | The tunnel; receives RTP, reassembles access units; sends input; clipboard; RTCP | the decoder (it drops to the next keyframe instead of queueing) |
| **decode** | FFmpeg d3d11va: one access unit in, one texture out | presentation (only the newest frame is kept) |
| **present** | Draws the newest frame and the chrome, presents | — |

## The phone's side

On USB, Apple's usbmuxd (from Apple Devices) reaches *lockdown*, which opens
a *CoreDeviceProxy*: a tunnel to the phone's network stack, spoken over a
userspace TCP stack ([idevice](https://github.com/jkcoxson/idevice)). Over
Wi-Fi, a *RemotePairing* tunnel (found with Bonjour, TLS with a pre-shared
key from the first USB connection) leads to the same place. Through it, the
*RemoteServiceDiscovery* lists the CoreDevice services Mirrorly uses:

| Service | For |
|---|---|
| Display streaming | The screen as RTP/HEVC, with RTCP feedback that keeps the phone's encoder steady |
| HID (universal HID service, Indigo) | Touches, keys, buttons, edge swipes |
| Pasteboard | The clipboard, both ways, and file transfer |
| House arrest | Saving documents by name into an app's Files folder |
| App service | Opening Safari for the iPad display |
| Image mounter / cryptex | The signed Developer Disk Image these services need |

How each was found and verified is in
[FEASIBILITY.md](https://github.com/AnasSAV/mirrorly/blob/main/FEASIBILITY.md).

## The video path

RTP packets → access units (network thread) → hardware decode into a D3D11
texture array (decode thread) → a one-frame slot → the renderer samples the
decoder's NV12 texture directly, with no copy, into a flip-model swapchain
with tearing and a frame-latency waitable (present thread).

| Choice | Why |
|---|---|
| Decode on the GPU that drives the window's monitor | Zero copy into the swapchain (hybrid laptops have two) |
| One frame in flight, no jitter buffer | Nothing waits in a queue |
| Present immediately (tearing) | vsync measured +5.5 ms |
| Rounded corners from DWM and the shader, not a window region | A region measured +9.7 ms and no direct flips |
| Input on its own path | Clicks reach the phone at once |

Results: Mirrorly's own share of tap-to-screen is about 1 ms; see
[LATENCY.md](https://github.com/AnasSAV/mirrorly/blob/main/LATENCY.md).

## The iPad display

![iPad display](https://raw.githubusercontent.com/AnasSAV/mirrorly/main/docs/assets/display.svg?sanitize=true)

| Step | How |
|---|---|
| Monitor | Virtual Display Driver (IddCx), attached to the desktop only while in use (`ChangeDisplaySettingsEx`), on the Intel GPU when there is one |
| Capture | DXGI Desktop Duplication on the monitor's adapter |
| Convert | The D3D11 video processor, BGRA → NV12 (BT.709, video range), once per screen change |
| Encode | Intel Quick Sync from a D3D11 frame pool on the capture's own device (no copy to the CPU); else NVIDIA or Media Foundation from system memory |
| Send | A small HTTP/WebSocket server; a viewer that falls behind skips to the next keyframe |
| Show | Safari WebCodecs, hardware decode; the pointer drawn by the page from separate shape and position messages |
| Setup | `Mirrorly.exe --setup-display`, elevated once: driver (pnputil + SetupAPI), settings, firewall rule |

## Recordings

The network thread hands each access unit (already HEVC) to a recorder
thread that writes MP4 (`video::mp4`): parameter sets go into the `hvcC`
record (with the SPS rewritten to crop the encoder's padding), samples are
the length-prefixed slices, times are the RTP timestamps at a rate measured
against the PC's clock, and the view's rotation is the display matrix.

## Where things are kept

`%LOCALAPPDATA%\Mirrorly`: `logs\`, `pairing\` (pair records, since Apple's
usbmuxd on Windows won't save them), `wifi\`, `clipboard\` (fetched files),
`onboarded`, and the iPad display's `display-key.txt`,
`virtual-display.txt` and `display-setup.txt`.
