**Mirrorly 0.4**: iPad support.

### New: iPad

- **Mirror and control an iPad**, the same as an iPhone: touch, keyboard, shared clipboard, drag and drop, USB and Wi-Fi.
- **Landscape gestures.** Turn the view with F12 to match the device, and the swipe up from the bottom edge goes Home (pause halfway for the App Switcher) from the bottom of what you see. Ctrl+2 and the App Switcher button follow the view too.
- **iPad screen shape:** the iPad's own resolution from the first frame, and its tighter screen corners.

### Improved

- **Forgotten pairings fix themselves.** If an iPhone or iPad has forgotten this PC (after a reset or an update), Mirrorly asks it to trust the PC again instead of failing quietly.
- **Clear message for older versions.** Apple's screen sharing and control need **iOS / iPadOS 27**; on an older device Mirrorly now says *Update to iOS 27* (or iPadOS 27) instead of retrying.
- Device errors show as readable messages in the log.

### Requirements

Windows 10 (1809+) or 11 x64, [Apple Devices](https://apps.microsoft.com/detail/9np83lwlpz9k), and an **iPhone or iPad on iOS / iPadOS 27 or later** with Developer Mode on. (Earlier notes said iOS 17.4: Apple's service turns out to require 27.)

### Downloads

| File | |
|---|---|
| `Mirrorly-Setup-0.4.0.exe` | Installer (per-user, no admin rights). Installs over earlier versions. |
| `Mirrorly-0.4.0-portable.zip` | Unzip and run `Mirrorly.exe`. |
| `SHA256SUMS.txt` | Checksums. |

Not code-signed yet: if SmartScreen warns, choose *More info › Run anyway*.

Not affiliated with Apple Inc.
