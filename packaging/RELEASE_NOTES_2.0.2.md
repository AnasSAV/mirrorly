**Mirrorly 2.0.2**: connects again with Apple Devices from the Microsoft Store.

### Fixed

- **Apple Devices from the Microsoft Store:** Mirrorly 2.0.0 and 2.0.1 couldn't connect over USB with it and said *"Apple Mobile Device Service is not running"*. Since 2.0, Mirrorly checks that the program answering on Apple's local port really is Apple's. It recognised only the Windows service that iTunes and Apple Mobile Device Support install, while the Store app runs the same thing from its app package. Both are recognised now. Thanks to @dnmp5mwzyt-bot for the detailed report (#11).

### Requirements

Windows 10 (1809+) or 11 x64, [Apple Devices](https://apps.microsoft.com/detail/9np83lwlpz9k), and an **iPhone or iPad on iOS / iPadOS 27 or later** with Developer Mode on. For the iPad display: the iPad and PC on the same Wi-Fi (network profile *Private*), and administrator rights once.

### Downloads

| File | |
|---|---|
| `Mirrorly-Setup-2.0.2.exe` | Installer (per-user, no admin rights). Installs over earlier versions. |
| `Mirrorly-2.0.2-portable.zip` | Unzip and run `Mirrorly.exe`. |
| `SHA256SUMS.txt` | Checksums. |

Not code-signed yet: if SmartScreen warns, choose *More info › Run anyway*; with Smart App Control on, Windows blocks it until it is signed. Code signing policy: see the [README](https://github.com/AnasSAV/mirrorly#code-signing-policy) (free code signing provided by SignPath.io, certificate by SignPath Foundation, being set up).

Not affiliated with Apple Inc.
