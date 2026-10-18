First release of **Mirrorly**: see and control your iPhone from Windows.

### Highlights
- **Low latency**: the PC side adds about 1 ms (hardware HEVC decode straight into the window, no copies, no buffering). Tap to response is about 90 ms over USB, almost all of it the iPhone itself.
- **USB and Wi-Fi**: after one USB connection, unplug and keep going over Wi-Fi; plug in again and it switches back to USB automatically.
- **Mac-style window**: just the phone screen; point at it and a bar appears above the screen with close / minimize / size and Home / App Switcher / Rotate (Control Center and Notification Center stay reachable).
- **Guided setup**: a welcome screen walks through the first connection and ticks off each step.
- **Full control**: tap, drag, scroll, type (Ctrl shortcuts work as ⌘), Home and App Switcher gestures, Lock and volume buttons, Spotlight (Ctrl+3).

### Requirements
- Windows 10 (1809+) or 11, x64. Windows 11 recommended.
- [Apple Devices](https://apps.microsoft.com/detail/9np83lwlpz9k) from the Microsoft Store.
- iPhone with iOS 17.4 or later, Developer Mode on (see the README).

### Downloads
- `Mirrorly-Setup-0.1.0.exe` — installer (per-user, no admin rights).
- `Mirrorly-0.1.0-portable.zip` — unzip and run `Mirrorly.exe`.
- `SHA256SUMS.txt` — checksums.

Not code-signed yet: Windows SmartScreen may warn; choose *More info → Run anyway*.

### Known limitations
- The iPhone always streams portrait: rotate the view yourself (F12) for landscape apps.
- Wi-Fi uses a TCP tunnel, so a weak network shows as brief stalls rather than dropped frames.
- Tested on an iPhone 11 with iOS 27 (beta) on Windows 11.

Not affiliated with Apple Inc.
