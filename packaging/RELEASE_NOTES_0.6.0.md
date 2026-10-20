**Mirrorly 0.6**: screenshots and screen recordings, and a faster iPad display that opens by itself.

### New: screenshots and recordings

- **F6** (or the new camera button on the bar) saves a screenshot to **Pictures › Mirrorly**, at full resolution.
- **F7** (or the record button, which turns into a red stop button) records the screen to **Videos › Mirrorly**. A recording is the phone's own video stream written to MP4 as it arrives: nothing is re-encoded, so it costs next to nothing and keeps the phone's quality.
- Both are turned the way the view is turned (F12). Recordings have no sound, and end by themselves if the phone disconnects.

### iPad as a display: faster, and hands-free

- **Opens by itself.** Click the monitor button and keep your hands off the iPad: Mirrorly opens Safari there, goes to the display's address and puts it in full screen, then stops mirroring. (The address still works by hand too.)
- **The pointer is drawn by the iPad.** Moving the mouse no longer needs a video frame, just a few bytes, so the pointer follows after only the network's delay.
- **The picture never leaves the GPU.** With Intel graphics, Quick Sync encodes straight from GPU memory: a screen change now takes about 12–14 ms on the PC instead of ~15.5 ms (median), and the slowest frames improved from up to 26 ms to about 16 ms. On a PC with Intel graphics the virtual monitor now runs on the Intel GPU for this; if you set the display up with 0.5, Mirrorly asks for administrator permission once more to move it.

### Fixed

- A display session that failed to start could switch off a virtual monitor another session was using.
- On a narrow window the bar leaves out its least needed buttons instead of drawing them over the window buttons.

### Requirements

Windows 10 (1809+) or 11 x64, [Apple Devices](https://apps.microsoft.com/detail/9np83lwlpz9k), and an **iPhone or iPad on iOS / iPadOS 27 or later** with Developer Mode on. For the iPad display: the iPad and PC on the same Wi-Fi (network profile *Private*), and administrator rights once. Recordings are HEVC: Windows plays them with the HEVC Video Extensions.

### Downloads

| File | |
|---|---|
| `Mirrorly-Setup-0.6.0.exe` | Installer (per-user, no admin rights). Installs over earlier versions. |
| `Mirrorly-0.6.0-portable.zip` | Unzip and run `Mirrorly.exe`. |
| `SHA256SUMS.txt` | Checksums. |

Not code-signed yet: if SmartScreen warns, choose *More info › Run anyway*. The display setup's administrator prompt then shows *Unknown publisher*.

Not affiliated with Apple Inc.
