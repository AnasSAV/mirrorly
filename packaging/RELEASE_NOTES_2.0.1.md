**Mirrorly 2.0.1**: the number pad works, and the Developer Disk Image download is pinned.

### Fixed

- **Number pad:** its keys were ignored, except Enter. With Num Lock on they now type numbers and `/ * - + .`; with it off they move the cursor as on Windows (arrows, Home, End, Page Up and Down, Delete). Thanks to @masumxgithub for reporting it (#4).

### Security

- **Developer Disk Image:** when a phone needs it, Mirrorly downloads it from one fixed version of the community repository and checks each file's SHA-256, instead of taking whatever is newest. A cached copy that doesn't match is downloaded again rather than installed. Thanks to @youtsuhodev (#5).
- **The iPad display's address key** is no longer written to Mirrorly's log (#5).

### Requirements

Windows 10 (1809+) or 11 x64, [Apple Devices](https://apps.microsoft.com/detail/9np83lwlpz9k), and an **iPhone or iPad on iOS / iPadOS 27 or later** with Developer Mode on. For the iPad display: the iPad and PC on the same Wi-Fi (network profile *Private*), and administrator rights once.

### Downloads

| File | |
|---|---|
| `Mirrorly-Setup-2.0.1.exe` | Installer (per-user, no admin rights). Installs over earlier versions. |
| `Mirrorly-2.0.1-portable.zip` | Unzip and run `Mirrorly.exe`. |
| `SHA256SUMS.txt` | Checksums. |

Not code-signed yet: if SmartScreen warns, choose *More info › Run anyway*; with Smart App Control on, Windows blocks it until it is signed. Code signing policy: see the [README](https://github.com/AnasSAV/mirrorly#code-signing-policy) (free code signing provided by SignPath.io, certificate by SignPath Foundation, being set up).

Not affiliated with Apple Inc.
