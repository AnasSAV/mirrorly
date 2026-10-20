# Troubleshooting

Mirrorly's window says what it is waiting for (*Unlock your iPhone*, *Turn
on Developer Mode*, …). Start there; the tables below cover the rest.

## Connecting

| Problem | Try |
|---|---|
| "Update to iOS 27" / "Update to iPadOS 27" | Apple's screen sharing service needs iOS / iPadOS 27: *Settings › General › Software Update*. |
| "Apple Devices is required" | Install [Apple Devices](https://apps.microsoft.com/detail/9np83lwlpz9k) from the Microsoft Store, then plug the iPhone in again. |
| Stuck on "Trust this computer" | Unlock the iPhone, tap **Trust**, enter the passcode. |
| No Developer Mode switch in Settings | Keep Mirrorly open with the iPhone plugged in: Mirrorly asks iOS to show it. Then *Settings › Privacy & Security › Developer Mode*. |
| Mirrorly connects to the wrong iPhone | Disconnect (power button), then click the right one in the list. |
| Not found over Wi-Fi | Same network? Network profile **Private**? VPN off? Connect by USB once. See [[USB and Wi-Fi\|USB-and-Wi-Fi]]. |

## The picture

| Problem | Try |
|---|---|
| Black screen, or "iPhone is busy" | Close the Camera, or any app using the camera or microphone. |
| The camera says it's in use | Click the power button to disconnect: Mirrorly then releases it. |
| Choppy over Wi-Fi | Move closer to the router, use 5 GHz, turn Wi-Fi power saving off. |
| A landscape app shows sideways | Turn the view with <kbd>F12</kbd>: the iPhone always sends its screen upright. |

## Files and clipboard

| Problem | Try |
|---|---|
| Nothing appears after copying on the iPhone | Copy it again with the iPhone unlocked and Mirrorly connected. |
| Ctrl+V on the iPhone pastes the wrong thing | The newest copy wins: copy it again on the side you want. |
| A dropped photo didn't paste | Open the place you want it first (a chat, a note), then drop again. |
| A document went "somewhere" | *Files › On My iPhone › (an app) › Mirrorly*: see [[Files and clipboard\|Files-and-Clipboard]]. |

## Screenshots, recordings, iPad display

| Problem | Try |
|---|---|
| A recording won't play | It's HEVC: Windows plays it with the HEVC Video Extensions (Microsoft Store). |
| "Nothing to capture yet" | Wait until the phone's screen shows in the window. |
| The iPad display | See its own [[problems table\|iPad-as-a-Display]]. |

## Logs

The log is at `%LOCALAPPDATA%\Mirrorly\logs\Mirrorly.log` (paste that into
File Explorer's address bar). It starts afresh each time Mirrorly starts. The
iPad display's one-time setup writes `display-setup.log` next to it.

To watch the log live, start Mirrorly with `Mirrorly.exe --console`.

Still stuck? [Open an issue](https://github.com/AnasSAV/mirrorly/issues/new/choose)
with the log: the form asks for what helps. The log contains no passwords,
but does contain your device's name.

## Starting over

- **Forget an iPhone's Wi-Fi pairing:** disconnect, click **Forget** next to it.
- **The first-run welcome screen again:** delete
  `%LOCALAPPDATA%\Mirrorly\onboarded`.
- **Everything Mirrorly keeps** (pairings, logs, fetched clipboard files,
  the iPad display's address and position) is in `%LOCALAPPDATA%\Mirrorly`.
