# Getting started

## What you need

| | |
|---|---|
| **PC** | Windows 10 (1809 or later) or Windows 11, 64-bit |
| **iPhone or iPad** | iOS / iPadOS **27 or later**, with a passcode set |
| **Apple Devices** | Free from the [Microsoft Store](https://apps.microsoft.com/detail/9np83lwlpz9k). It brings the driver Windows needs to talk to an iPhone. iTunes works too. |
| **A cable** | For the first connection. After that Wi-Fi works as well. |

Apple's screen sharing and remote control service only exists from iOS 27
on. On an older version Mirrorly says *Update to iOS 27* (or iPadOS 27).

## Install

Download **Mirrorly-Setup-x.y.z.exe** from the
[latest release](https://github.com/AnasSAV/mirrorly/releases/latest) and run
it. It installs for your user only, without administrator rights, and adds a
Start-menu shortcut and an uninstaller. If Apple Devices is missing, the
installer offers to open its Store page.

Prefer not to install? **Mirrorly-x.y.z-portable.zip** runs from any folder.

> Mirrorly isn't code-signed yet, so SmartScreen may warn the first time:
> choose **More info › Run anyway**. `SHA256SUMS.txt` in the release lets you
> check the download first.

## First connection

Open Mirrorly. The first time, a welcome screen lists these steps and ticks
each one off as it happens.

1. **Plug in the iPhone, unlock it and tap Trust**, then enter its passcode.
   If it once trusted this PC and has forgotten it (after a reset or an
   update), Mirrorly asks it to trust the PC again by itself.
2. **Turn on Developer Mode:** *Settings › Privacy & Security › Developer
   Mode › On*. The iPhone restarts; unlock it and tap **Turn On**.
   If the switch isn't there, keep Mirrorly open with the iPhone plugged in:
   Mirrorly asks iOS to show it.
3. **Wait a moment** the first time: Mirrorly sets up Apple's developer
   services on the iPhone (the same signed disk image Xcode uses). This can
   happen again after the iPhone restarts.

The iPhone's screen then appears in the window. Next: [[Using Mirrorly|Using-Mirrorly]].

## Why Developer Mode?

Mirrorly talks to the iPhone the way Xcode does, through services that iOS
offers to development tools on a computer the iPhone trusts. Developer Mode
switches those services on. It doesn't weaken the passcode or install
anything, and you can turn it off at any time; Mirrorly then stops working
until it's back on.
