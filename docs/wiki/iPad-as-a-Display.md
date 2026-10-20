# iPad as a display

An iPad can be an extra monitor for your PC, like a second screen on your
desk: drag windows onto it and use it with the PC's mouse and keyboard, at
the iPad's own resolution and 60 frames a second. It shows in Safari:
nothing is installed on the iPad.

## Turn it on

1. Connect the iPad to Mirrorly as usual (USB or Wi-Fi), point at the window
   and click the **monitor button** on the bar. It only appears for iPads.
2. **The first time**, Windows asks for administrator permission once, to
   install the virtual display driver and a firewall rule (see below). The
   installer is unsigned for now, so the prompt says *Unknown publisher*.
3. **Hands off the iPad.** Mirrorly opens Safari there, goes to the display's
   address, taps it into full screen and stops mirroring the iPad, usually in
   a few seconds.

The iPad now shows a new monitor next to your others. Mirrorly hides behind
its icon by the clock (it may be under the **^** arrow): click it and choose
**Stop using iPad as a display** to switch back to mirroring, or **Quit
Mirrorly**.

Where the monitor sits relative to your others is up to you, in *Settings ›
System › Display*; Mirrorly puts it back there every time.

### Opening it yourself

If the iPad isn't connected to Mirrorly when you click the monitor button
(or opening Safari fails), Mirrorly's window shows the address instead,
something like `http://192.168.1.14:8765/k7m2x9qa`. Open it in Safari on the
iPad and tap **Start**. *Share › Add to Home Screen* makes it one tap, full
screen, next time.

## Good to know

| | |
|---|---|
| Resolution | The iPad's own, at 60 fps |
| Delay | About 12–14 ms on the PC from a screen change to the frame leaving (median, with Intel Quick Sync), plus Wi-Fi and the iPad's decoding |
| The pointer | Drawn by the iPad itself: moving the mouse sends a few bytes, not video, so it follows quickly |
| When it's off | The virtual monitor is gone, so windows can't get lost on it |
| Network | The iPad and PC on the same Wi-Fi, with the PC's network profile set to *Private* |

**No touch.** The iPad display is used with the mouse and keyboard. Windows
silently ignores touch input from other apps whenever an administrator window
(Task Manager, an installer) is in front, anywhere on screen, so touch
couldn't be made reliable.

## What it installs

- **[Virtual Display Driver](https://github.com/VirtualDrivers/Virtual-Display-Driver)**
  (MIT license, signed by SignPath), which adds the monitor. Mirrorly ships it
  in a `vdd` folder next to `Mirrorly.exe`. Its settings are in
  `C:\VirtualDisplayDriver`: one monitor, offering your iPad's resolution.
  On a PC with Intel graphics it runs on the Intel GPU, so Quick Sync can
  encode its picture without a copy.
- A **firewall rule**, *Mirrorly iPad display*: Mirrorly may accept
  connections on port 8765, on private and domain networks only.

The setup runs again (one more prompt) only if Mirrorly moved to another
folder, which the firewall rule names, or if the driver needs to move to the
Intel GPU (after upgrading from 0.5).

**Removing the driver:** uninstalling Mirrorly leaves it, because that needs
administrator rights. Remove it in *Device Manager › Display adapters ›
Virtual Display Driver › Uninstall device*, ticking *Attempt to remove the
driver for this device*.

## Privacy

The display is served on your own network only, and only while it is on.
Its address has a secret part, made once on your PC, so others on the same
network can't open it. The picture itself is not encrypted on the network.

## How it works

1. Windows composes the virtual monitor like any other; Desktop Duplication
   hands Mirrorly each change.
2. The GPU converts the picture to NV12 once per change and Intel Quick Sync
   encodes it as H.264 straight from GPU memory. Other GPUs encode from
   system memory instead (NVIDIA, or Windows' own encoder).
3. A small web server in Mirrorly sends the frames to Safari over a
   WebSocket. If Wi-Fi falls behind, frames are skipped up to the next
   keyframe rather than queued, so delay never builds up.
4. Safari decodes in hardware (WebCodecs) and draws the pointer on top, from
   its shape and position, which arrive as separate tiny messages.

The code is in [`crates/display`](https://github.com/AnasSAV/mirrorly/tree/main/crates/display);
see also [[Architecture]].

## Problems?

| Problem | Try |
|---|---|
| Safari can't open the address | Same Wi-Fi? Network profile **Private**? VPN off? |
| "Open the full address that Mirrorly shows" | The address is missing its secret part: copy it whole. |
| The monitor button asks for permission again | Mirrorly was moved or reinstalled elsewhere, or the driver is moving to the Intel GPU. |
| The picture stutters | Move closer to the router, use 5 GHz. A tap on the picture shows the iPad's frame rate. |
| Windows went to the iPad's monitor while it was off | Can't happen from 0.5 on: the monitor only exists while in use. Earlier, use *Settings › System › Display*. |
