**Mirrorly 2.0**: a security release, a bar laid out like Windows, and a choice of where dropped documents go.

### Security

Every lead from a security review of Mirrorly's code (Cloudflare's security-audit skill) is fixed:

- **Wi-Fi: the iPhone has to prove it's yours.** When reconnecting over Wi-Fi, Mirrorly didn't check that the device answering was really your paired iPhone, so another device on the network could have posed as it and received your PC's clipboard and keystrokes. The iPhone now signs every Wi-Fi connection with the key it gave when it was paired over USB, and anything that can't is refused. *Plug each iPhone or iPad in once after updating:* its Wi-Fi pairing is renewed then, and until that it isn't used over Wi-Fi.
- **iPad display setup:** the one-time administrator setup now writes into a folder only administrators can change, instead of one any account on the PC could prepare.
- **USB:** Mirrorly connects only when Apple Mobile Device Service is what answers on its local port.
- **Your clipboard:** copies an app keeps out of Windows' clipboard history or cloud clipboard (as password managers do) are no longer sent to the phone.
- **Files from the iPhone** are marked as coming from another computer, as downloads are; names Windows would run (`.exe`, `.lnk`, `.url`…) get `.txt` added, and hidden characters that could disguise a name are removed.
- **Limits** on what a phone or the network can make Mirrorly hold: copies from the phone (100 items, 2 GB), photo sizes, video frames.
- **FFmpeg** is one pinned build, checked by SHA-256 when it is downloaded and when a release is packaged.

### New

- **The bar** has the window buttons on the right, as in Windows, and the tools on the left. When you copy something on the iPhone, the chip to drag it out always has room.
- **Settings:** the gear on the bar (or a right-click on the bar) chooses which app on the phone keeps the documents you drop on it.

### Fixed

- **iPad display from the Home Screen:** the bottom of the desktop (the taskbar) was cut off. It now fits.

### Requirements

Windows 10 (1809+) or 11 x64, [Apple Devices](https://apps.microsoft.com/detail/9np83lwlpz9k), and an **iPhone or iPad on iOS / iPadOS 27 or later** with Developer Mode on. For the iPad display: the iPad and PC on the same Wi-Fi (network profile *Private*), and administrator rights once.

### Downloads

| File | |
|---|---|
| `Mirrorly-Setup-2.0.0.exe` | Installer (per-user, no admin rights). Installs over earlier versions. |
| `Mirrorly-2.0.0-portable.zip` | Unzip and run `Mirrorly.exe`. |
| `SHA256SUMS.txt` | Checksums. |

Not code-signed yet: if SmartScreen warns, choose *More info › Run anyway*; with Smart App Control on, Windows blocks it until it is signed. Code signing policy: see the [README](https://github.com/AnasSAV/mirrorly#code-signing-policy) (free code signing provided by SignPath.io, certificate by SignPath Foundation, being set up).

Not affiliated with Apple Inc.
