**Mirrorly 0.5**: use an iPad as a second display.

### New: iPad as a second display

- **One click turns an iPad into an extra monitor for Windows.** With an iPad connected, click the new **monitor button** on the bar (iPads only). Drag windows onto it and use it with the PC's mouse and keyboard, at the iPad's own resolution and 60 fps. Nothing to install on the iPad: it runs in **Safari**.
- **One-time setup, one prompt.** The first time, Windows asks for administrator permission once: Mirrorly installs the bundled [Virtual Display Driver](https://github.com/VirtualDrivers/Virtual-Display-Driver) (MIT, signed) and a firewall rule for itself. After that it starts straight away.
- **Only there while you use it.** The virtual monitor joins the desktop when display mode starts and leaves when it stops, back in the place you put it. Windows can't get lost on an invisible screen.
- **Out of the way.** While the iPad is a display, Mirrorly stops mirroring it and hides behind its icon by the clock: **Stop using iPad as a display** brings the window back.
- **Fast:** hardware capture, GPU colour conversion and hardware H.264 (Intel Quick Sync, NVIDIA or Windows' encoder), about 7 ms on the PC; Safari decodes in hardware. If Wi-Fi falls behind, frames are skipped instead of queued, so the delay never builds up.
- **Private:** the address has a secret part made on your PC, so others on your network can't open it. The picture is not encrypted on the network, and the firewall rule covers private networks only.
- The mouse pointer is drawn into the picture, and the iPad stays awake while it shows the display.

Touch on the iPad isn't used: Windows ignores touch from other apps whenever an administrator window is in front, so it couldn't be relied on.

### Requirements

Windows 10 (1809+) or 11 x64, [Apple Devices](https://apps.microsoft.com/detail/9np83lwlpz9k), and an **iPhone or iPad on iOS / iPadOS 27 or later** with Developer Mode on. For the iPad display: the iPad and PC on the same Wi-Fi (network profile *Private*), and administrator rights once.

### Downloads

| File | |
|---|---|
| `Mirrorly-Setup-0.5.0.exe` | Installer (per-user, no admin rights). Installs over earlier versions. |
| `Mirrorly-0.5.0-portable.zip` | Unzip and run `Mirrorly.exe`. |
| `SHA256SUMS.txt` | Checksums. |

Not code-signed yet: if SmartScreen warns, choose *More info › Run anyway*. The display setup's administrator prompt then shows *Unknown publisher*.

Not affiliated with Apple Inc.
