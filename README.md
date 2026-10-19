<p align="center">
  <img src="docs/assets/banner.svg" alt="Mirrorly: your iPhone on your Windows PC" width="100%">
</p>

<p align="center">
  <a href="../../releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/AnasSAV/mirrorly?style=flat-square&color=4C61FA&label=release"></a>
  <a href="../../releases"><img alt="Downloads" src="https://img.shields.io/github/downloads/AnasSAV/mirrorly/total?style=flat-square&color=7B4DF2"></a>
  <img alt="Windows 10 and 11" src="https://img.shields.io/badge/Windows-10%20%7C%2011-0078D4?style=flat-square">
  <img alt="iOS 17.4 or later" src="https://img.shields.io/badge/iOS-17.4%2B-111111?style=flat-square">
  <img alt="Built with Rust" src="https://img.shields.io/badge/built%20with-Rust-B7410E?style=flat-square">
  <a href="LICENSE"><img alt="MIT license" src="https://img.shields.io/badge/license-MIT-22A06B?style=flat-square"></a>
</p>

<p align="center">
  <a href="#download"><b>Download</b></a> ·
  <a href="#get-started">Get started</a> ·
  <a href="#using-mirrorly">Using it</a> ·
  <a href="#drag-and-drop-files">Drag &amp; drop</a> ·
  <a href="#usb-and-wi-fi">USB &amp; Wi-Fi</a> ·
  <a href="#performance">Performance</a> ·
  <a href="#faq">FAQ</a> ·
  <a href="#for-developers">Developers</a>
</p>

---

**Mirrorly** puts your iPhone's screen in a window on your PC and lets you use
it with your mouse and keyboard: tap, swipe, scroll, type, and move photos and
files between the two by dragging them. It works over a USB cable or your
Wi-Fi, needs no Mac, no jailbreak and no account, and adds only about **1 ms**
of delay on the PC.

> Everything stays between your PC and your iPhone. Nothing is uploaded, and
> Mirrorly has no servers, accounts or telemetry.

## Highlights

<table>
  <tr>
    <td width="33%" valign="top">
      <h3>⚡ Low latency</h3>
      Hardware video decoding straight into the window, with no buffering and
      no copies. The PC adds about 1 ms; the rest is the iPhone itself.
    </td>
    <td width="33%" valign="top">
      <h3>🖱️ Full control</h3>
      Click to tap, drag to swipe, scroll with the wheel, and type with your
      keyboard. Ctrl shortcuts work like ⌘ on the iPhone.
    </td>
    <td width="33%" valign="top">
      <h3>📂 Drag &amp; drop, one clipboard</h3>
      Drag photos and files between the iPhone and Windows. Copy on one,
      paste on the other: text, pictures and files.
    </td>
  </tr>
  <tr>
    <td valign="top">
      <h3>📶 USB and Wi-Fi</h3>
      Connect by cable once, then unplug: it carries on over Wi-Fi. Plug in
      again and it's back on USB in about a second.
    </td>
    <td valign="top">
      <h3>🪟 Looks like a Mac</h3>
      Just the phone screen in a rounded window. A bar above it holds the
      buttons and never covers Control Center.
    </td>
    <td valign="top">
      <h3>🧭 Guided setup</h3>
      A welcome screen walks you through the first connection and ticks off
      each step as it happens.
    </td>
  </tr>
</table>

<p align="center">
  <img src="docs/assets/window.svg" alt="The Mirrorly window: window buttons, the copied-files chip, Rotate, App Switcher and Home on a bar above the screen" width="100%">
</p>

## Download

Get the latest version from **[Releases](../../releases/latest)**.

| File | What it is |
|---|---|
| **`Mirrorly-Setup-x.y.z.exe`** | Installer. No administrator rights needed; adds a Start-menu shortcut and an uninstaller. Recommended. |
| `Mirrorly-x.y.z-portable.zip` | Unzip anywhere and run `Mirrorly.exe`. Nothing is installed. |
| `SHA256SUMS.txt` | Checksums to verify your download. |

> [!NOTE]
> Mirrorly isn't code-signed yet, so Windows SmartScreen may warn the first
> time. Choose **More info › Run anyway**, or check the file against
> `SHA256SUMS.txt` first.

### Requirements

| | Minimum | Recommended |
|---|---|---|
| **PC** | Windows 10 (1809) x64 | Windows 11 |
| **iPhone** | iOS 17.4, with a passcode set | the latest iOS |
| **Driver** | [Apple Devices](https://apps.microsoft.com/detail/9np83lwlpz9k) from the Microsoft Store (free) | |
| **Wi-Fi** *(optional)* | PC and iPhone on the same network | 5 GHz |

## Get started

```mermaid
flowchart LR
    A["Install<br/><b>Apple Devices</b>"] --> B["Plug in the iPhone<br/>unlock · tap <b>Trust</b>"]
    B --> C["Turn on<br/><b>Developer Mode</b>"]
    C --> D["Open <b>Mirrorly</b>"]
    D --> E(["Your iPhone<br/>on screen 🎉"])
```

1. **Install [Apple Devices](https://apps.microsoft.com/detail/9np83lwlpz9k)**
   from the Microsoft Store. It provides the driver Windows needs to talk to an
   iPhone. The Mirrorly installer offers to open the Store page if it's missing.
2. **Plug in your iPhone, unlock it and tap Trust**, then enter your passcode.
3. **Turn on Developer Mode:** *Settings › Privacy & Security › Developer Mode
   › On*. The iPhone restarts; unlock it and tap **Turn On**.
   If the switch isn't there yet, open Mirrorly once with the iPhone plugged
   in: it asks iOS to show it.
4. **Open Mirrorly.** The first time, a welcome screen ticks off each step as it
   happens.

> [!TIP]
> On the first connection Mirrorly sets up Apple's developer services on the
> iPhone (the same signed disk image Xcode uses). It takes a moment, and may
> happen again after the iPhone restarts.

## Using Mirrorly

### Mouse

| Do this | To |
|---|---|
| **Click** / **drag** on the screen | Tap / swipe |
| **Mouse wheel** | Scroll |
| **Drag up from the very bottom edge** | Go Home (pause before letting go for the App Switcher) |
| **Drag down from the top edge** | Notification Center (left) or Control Center (right) |
| **Right-click** | Go Home |
| **Point at the window** | Show the bar: window buttons, copied files, Disconnect, Rotate, App Switcher, Home |
| **Drag the bar** | Move the window |
| **Ctrl + wheel** or the **green button** | Make the window bigger or smaller |

### Keyboard

Typing goes straight to the iPhone, like a Bluetooth keyboard.

| Keys | Action |
|---|---|
| <kbd>Ctrl</kbd> <kbd>C</kbd> / <kbd>V</kbd> / <kbd>X</kbd> / <kbd>A</kbd> / <kbd>Z</kbd> | Copy, paste, cut, select all, undo (sent as ⌘) |
| <kbd>Ctrl</kbd> <kbd>1</kbd> &nbsp;·&nbsp; <kbd>F1</kbd> | Home |
| <kbd>Ctrl</kbd> <kbd>2</kbd> &nbsp;·&nbsp; <kbd>F2</kbd> | App Switcher |
| <kbd>Ctrl</kbd> <kbd>3</kbd> | Spotlight search |
| <kbd>F3</kbd> | Lock (side button) |
| <kbd>F4</kbd> / <kbd>F5</kbd> | Volume down / up |
| <kbd>F11</kbd> | Fullscreen |
| <kbd>F12</kbd> / <kbd>Shift</kbd> <kbd>F12</kbd> | Rotate the view for landscape apps |

> The iPhone always sends its screen upright, so when an app turns sideways,
> rotate the view with <kbd>F12</kbd> or the rotate button.

### Disconnect and switch iPhones

Click the **power button** on the bar to disconnect. Mirrorly lets go of the
iPhone (the camera and microphone are free again) and stays disconnected,
even if the iPhone is plugged in or on the same Wi-Fi, until you choose to
connect.

The disconnected screen lists every iPhone Mirrorly knows:

| You see | Means |
|---|---|
| 🟢 **Plugged in** | connected by cable; click it to mirror over USB |
| **Wi-Fi, when nearby** | connected to this PC before; click it to find it on your network |
| **Forget** | remove its Wi-Fi pairing (plug it in again to use it) |

To use **another iPhone**, disconnect, plug the other one in and click it.
The first time, unlock it, tap **Trust** and turn on Developer Mode, just
like the first phone. If two iPhones are plugged in at once, Mirrorly asks
which one to mirror.

## Drag and drop files

<p align="center">
  <img src="docs/assets/transfer.svg" alt="iPhone to Windows: copy on the iPhone, Mirrorly fetches it, drag the chip out. Windows to iPhone: drop files on the window; photos are pasted, documents are saved by name." width="100%">
</p>

### iPhone → Windows

1. On the iPhone, **copy** what you want:
   - **Photos:** open a photo › **Share** › **Copy Photo**. To send several,
     tap **Select**, pick them, then **Share › Copy Photos**.
   - **Files:** touch and hold a file › **Copy**.
2. Mirrorly fetches it straight away and shows *"Photo copied · drag it from the
   bar"*.
3. Point at the window and **drag the blue chip** on the bar onto the desktop,
   into an Explorer folder, or into any app that accepts files.

Files keep their original names (`Invoice.pdf` stays `Invoice.pdf`). Photos are
named by date and time.

### Windows → iPhone

Drag files from Explorer and **drop them on the mirrored screen**.

| You drop | What happens on the iPhone |
|---|---|
| **Photos and videos** | Pasted into the app you have open (a chat, a note, an email), as if you pressed ⌘V there. |
| **Documents** (PDF, Word, Excel, text, …) | Saved with their names in **Files › On My iPhone › *app* › Mirrorly**, and also put on the clipboard so you can paste them into a chat. |

> [!TIP]
> iOS doesn't let other programs write to the top level of *On My iPhone*, so
> documents go into a **Mirrorly** folder inside the Files folder of an app
> that shares files (VLC or Chrome, for example). In Files, touch and hold that
> folder and choose **Favorite**: it then shows in the sidebar as **Mirrorly**.

<details>
<summary><b>Limits and details</b></summary>

- Up to 200 MB per drop. Folders aren't sent; drop the files inside them.
- Copied text goes to the Windows clipboard; see [Shared clipboard](#shared-clipboard).
- To paste a photo into a **Files** folder, click an empty spot in the folder
  first so the paste lands there.
- Fetched files are kept in `%LOCALAPPDATA%\Mirrorly\clipboard` until you copy
  something else.

</details>

## Shared clipboard

Copy on one, paste on the other, while Mirrorly is connected.

| You copy | On | Then |
|---|---|---|
| Text or a picture | Windows | Paste on the iPhone: **Ctrl+V** in Mirrorly, or touch and hold › **Paste** |
| Files (in Explorer) | Windows | Press **Ctrl+V** in Mirrorly: photos are pasted, documents saved by name |
| Text | iPhone | **Ctrl+V** anywhere on the PC |
| Photos or files | iPhone | **Ctrl+V** into Explorer, a chat or an email; a photo also pastes as a picture into Paint, Word and the like |

Whichever was copied last wins: copy on the iPhone after the PC, and Ctrl+V
in Mirrorly pastes the iPhone's copy.

> [!NOTE]
> Passwords stay put. Anything a password manager marks as private on the PC
> is never sent to the iPhone, and text that comes from the iPhone is kept out
> of Windows' clipboard history and cloud clipboard. On the iPhone, the paste
> suggestion above the keyboard reads *"Paste from dtpasteboardd"*: that is the
> iOS service Mirrorly hands the copy to.

## USB and Wi-Fi

After the first USB connection, **unplug the cable**: Mirrorly finds the iPhone
on your network and carries on over Wi-Fi. Plug it back in and it switches to
USB by itself.

| | 🔌 USB | 📶 Wi-Fi |
|---|---|---|
| Tap → on screen (median) | **87.5 ms** | 96.3 ms |
| Smoothness | steady | occasional short stall on a busy network |
| Switch-over | plug in → back on USB in ~1.3 s | unplug → on Wi-Fi in ~2.1 s |
| Setup | plug in and trust, once | automatic after the first USB connection |

<details>
<summary><b>Wi-Fi checklist</b></summary>

- PC and iPhone on the **same Wi-Fi**, iPhone unlocked.
- Windows network profile set to **Private**: *Settings › Network & internet ›
  Wi-Fi › (your network) › Network profile type › Private*. If Windows asks
  whether Mirrorly may use private networks, allow it.
- Not a guest, hotel or apartment network that keeps devices apart, and no
  full-tunnel VPN.
- For the smoothest picture: 5 GHz, and Wi-Fi power saving off (*Device
  Manager › Network adapters › your Wi-Fi adapter › Power Management*, untick
  "Allow the computer to turn off this device").
- An amber or red light at the top of the screen means the Wi-Fi link is
  struggling.

</details>

## Performance

<p align="center">
  <img src="docs/assets/latency.svg" alt="Tap to screen: USB 87.5 ms, Wi-Fi 96.3 ms; Mirrorly's own share is about 1 ms. Inside Mirrorly: decode 0.44 ms, draw and present 0.07 ms, hand-offs 0.08 ms." width="100%">
</p>

How it gets there:

| Technique | Effect |
|---|---|
| Hardware HEVC decoding (D3D11) straight into the window's GPU memory | no copies between decoder and screen |
| Flip-model swapchain with tearing and a frame-latency waitable | a new frame is shown immediately (vsync would add ~5.5 ms) |
| One frame in flight, no jitter buffer | nothing waits in a queue |
| Input sent on its own path, never behind video | clicks reach the iPhone at once |
| Window corners from Windows 11 and the shader, not a clip region | keeps direct flips (a clip region measured +9.7 ms) |

Every number above was measured on real hardware. The method and the full
results are in [**LATENCY.md**](LATENCY.md).

## Privacy and security

- **Local only.** Video, input and files go directly between the PC and the
  iPhone, over USB or your own network. There are no servers, accounts,
  analytics or update checks.
- **Apple's own services.** Mirrorly uses the developer services iOS provides
  for Xcode (screen, input, clipboard, files), and only after you tap *Trust*
  on the iPhone. Over Wi-Fi the connection is encrypted (TLS). Nothing on the
  iPhone is modified or jailbroken.
- **Your clipboard.** Copies go only between your PC and your iPhone, while
  connected. Content a password manager marks as private on the PC is never
  sent, and iPhone text is kept out of Windows' clipboard history.
- **Open source.** Every line is in this repository (MIT license).

## Troubleshooting

The window tells you what it is waiting for: *Unlock your iPhone*, *Turn on
Developer Mode*, and so on.

| Problem | Try |
|---|---|
| "Apple Devices is required" | Install Apple Devices from the Microsoft Store, then plug the iPhone in again. |
| Stuck on "Trust this computer" | Unlock the iPhone, tap **Trust**, enter your passcode. |
| Black screen / "iPhone is busy" | Close the Camera, or any app using the camera or microphone. |
| Not found over Wi-Fi | Same network? Network profile **Private**? VPN off? Connect by USB once. |
| Choppy over Wi-Fi | Move closer to the router, use 5 GHz, turn Wi-Fi power saving off. |
| Nothing appears after copying on the iPhone | Copy it again with the iPhone unlocked and Mirrorly connected. |
| Ctrl+V on the iPhone pastes the wrong thing | The newest copy wins: copy it again on the side you want. |
| A dropped photo didn't paste | Open the place you want it first (a chat, a note), then drop again. |
| The camera says it's in use | Click the power button to disconnect; Mirrorly then releases it. |
| Mirrorly connects to the wrong iPhone | Disconnect, then click the right one in the list. |

Still stuck? The log is at `%LOCALAPPDATA%\Mirrorly\logs\Mirrorly.log`; run
`Mirrorly.exe --console` to watch it live, and
[open an issue](../../issues) with it.

To forget an iPhone, disconnect and click **Forget** next to it.

## FAQ

<details>
<summary><b>Do I need a Mac, a jailbreak, or a paid Apple account?</b></summary>

No. Mirrorly talks to the iPhone the way Xcode does, using the services built
into iOS. You only need Developer Mode switched on.
</details>

<details>
<summary><b>Is Developer Mode safe to leave on?</b></summary>

It lets development tools connect to your iPhone *after* it trusts your PC. It
doesn't weaken your passcode or install anything. You can turn it off whenever
you like (*Settings › Privacy & Security*); Mirrorly then stops working until
it's back on.
</details>

<details>
<summary><b>Is there sound?</b></summary>

Not yet: Mirrorly shows the screen only.
</details>

<details>
<summary><b>Why doesn't the window turn by itself when an app goes landscape?</b></summary>

The iPhone always sends its screen upright and doesn't say which way an app is
turned. Use <kbd>F12</kbd> or the rotate button on the bar.
</details>

<details>
<summary><b>Why is the iPhone's response about 90 ms and not faster?</b></summary>

That time is the iPhone itself: noticing the touch, redrawing the app and
encoding the video. Mirrorly's own part is about 1 ms. See
[LATENCY.md](LATENCY.md) for the measurements.
</details>

<details>
<summary><b>Which iPhones work?</b></summary>

It should work with any iPhone on iOS 17.4 or later, the version that
introduced the services Mirrorly uses. It is developed and tested on an
iPhone 11 with iOS 27 on Windows 11; reports from other models are welcome.
</details>

---

## For developers

Mirrorly is written in Rust, with a latency-first pipeline split across four
threads so input never waits on video:

<p align="center">
  <img src="docs/assets/architecture.svg" alt="iPhone services (screen, input, clipboard and files) connect through a USB or Wi-Fi tunnel to Mirrorly's network thread, which feeds the decode and present threads; the main thread sends input." width="100%">
</p>

| Crate | Role |
|---|---|
| [`crates/device`](crates/device) | USB (usbmuxd, CoreDeviceProxy) and Wi-Fi (RemotePairing, Bonjour, TLS-PSK) transports into one tunnel; Developer Disk Image; media session and RTCP feedback; HID execution; clipboard file transfer; reconnect with USB preferred |
| [`crates/video`](crates/video) | RTP/HEVC depacketizer; FFmpeg d3d11va decoding into textures on the app's D3D11 device |
| [`crates/input`](crates/input) | Window → screen coordinate mapping, gestures (touch vs bottom-edge system gesture), key mapping |
| [`crates/app`](crates/app) | Window and chrome (Direct2D), swapchain, zero-copy NV12 render, drag and drop |
| [`spike/`](spike), [`spike-decode/`](spike-decode) | Measurement and protocol probes (`s0`–`s7`) |

Design notes: [FEASIBILITY.md](FEASIBILITY.md) (what the iPhone offers, and
how it was verified) and [LATENCY.md](LATENCY.md) (method and results).

### Build

<details>
<summary><b>Requirements and commands</b></summary>

Windows 10/11 x64 with:

- Rust stable (MSVC): `winget install Rustlang.Rustup`
- Visual Studio 2022 Build Tools with the C++ workload:
  `winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"`
- LLVM (libclang for bindgen): `winget install LLVM.LLVM`
- FFmpeg 9.0 **LGPL shared** dev build unpacked to `third_party/ffmpeg`
  (`include/`, `lib/`, `bin/`), e.g. BtbN's
  `ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip`. Never a GPL build.

```powershell
cmd /c "\"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat\" && cargo build --release"
target\release\Mirrorly.exe --console
```

Release packages (installer, portable zip, checksums) also need
`cargo install cargo-about --features cli` and Inno Setup 6
(`winget install JRSoftware.InnoSetup`):

```powershell
powershell -ExecutionPolicy Bypass -File packaging\build-release.ps1   # -> dist\
```

</details>

<details>
<summary><b>Command-line options and diagnostics</b></summary>

| Option | Effect |
|---|---|
| `--console` | Log to a console window instead of the log file |
| `--overlay` | Start with the latency overlay (<kbd>F10</kbd> toggles it) |
| `--timings-csv FILE` | One line of stage timestamps per presented frame |
| `--vsync` | Present with vsync (slower; for comparison) |
| `--no-wifi` | USB only: no Wi-Fi pairing or fallback |
| `--no-auto-ddi` | Don't install the Developer Disk Image automatically |
| `--rctl pmd3\|idevice\|off` | RTCP rate-control report layout |

Environment: `MIRROR_TOUCH=uhs|indigo`, `MIRROR_NAGLE=1`,
`MIRROR_DXGI_TRACE=1`, `MIRROR_SQUARE=1`. <kbd>F9</kbd> runs a latency probe:
10 timed taps where the mouse points.

</details>

### Built on

- [**idevice**](https://github.com/jkcoxson/idevice): the Rust implementation
  of Apple's device protocols that Mirrorly is built on.
- [**pymobiledevice3**](https://github.com/doronz88/pymobiledevice3): reference
  for the CoreDevice services; its Developer Disk Image repository provides
  the signed images.
- [**FFmpeg**](https://ffmpeg.org) (LGPL) for hardware HEVC decoding, and
  [**winit**](https://github.com/rust-windowing/winit) and
  [**windows-rs**](https://github.com/microsoft/windows-rs) for the window and
  Windows APIs.

## License

Mirrorly is MIT-licensed ([LICENSE](LICENSE)). Releases include FFmpeg
libraries under the LGPL v3 and Rust crates under permissive licenses; see
`THIRD-PARTY-NOTICES.txt` and `FFMPEG-NOTICE.txt` in each release.

<sub>Mirrorly is not affiliated with or endorsed by Apple Inc. iPhone, iOS, Mac
and Xcode are trademarks of Apple Inc.</sub>
