# FAQ

**Do I need a Mac, a jailbreak or a paid Apple account?**
No. Mirrorly talks to the iPhone the way Xcode does, through services built
into iOS. You only need Developer Mode on.

**Is Developer Mode safe to leave on?**
It lets development tools connect to your iPhone, and only *after* it trusts
your PC. It doesn't weaken the passcode or install anything. You can turn it
off whenever you like (*Settings › Privacy & Security*); Mirrorly then stops
working until it's back on.

**Does anything leave my PC or phone?**
No. Video, input, files and the clipboard go directly between the PC and the
phone, over USB or your own network. Mirrorly has no servers, accounts,
analytics or update checks.

**Which iPhones and iPads work?**
Any on **iOS / iPadOS 27 or later**: Apple's screen sharing and control
service refuses older versions. Tested on an iPhone 11 and an iPad (10th
generation) with Windows 11; reports about other models are welcome.

**Is there sound?**
Not yet: Mirrorly shows the screen only, and recordings have no sound.

**Why doesn't the window turn by itself when an app goes landscape?**
The iPhone always sends its screen upright and doesn't say which way an app
is turned. Use <kbd>F12</kbd> or the rotate button.

**Why does a tap take about 90 ms to show?**
That time is the iPhone itself: noticing the touch, drawing the app,
encoding the video. Mirrorly's own part is about 1 ms. See
[LATENCY.md](https://github.com/AnasSAV/mirrorly/blob/main/LATENCY.md).

**Can I touch the iPad when it's a display?**
No: it's used with the PC's mouse and keyboard, like any other monitor.
Windows silently ignores touch from other apps while an administrator
window is in front, which made it unreliable.

**Can two iPhones be mirrored at once?**
No, one at a time. Disconnect and pick another in the list to switch.

**Why isn't Mirrorly signed? SmartScreen warns about it.**
Code-signing certificates cost money every year; it may come later. Each
release lists SHA-256 checksums, and the source is all here to build
yourself (see [[Building from source|Building-from-Source]]).
