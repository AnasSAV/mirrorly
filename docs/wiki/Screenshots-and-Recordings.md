# Screenshots and recordings

| Press | Or on the bar | You get |
|---|---|---|
| <kbd>F6</kbd> | the camera button | A PNG of the screen in **Pictures › Mirrorly**, at the phone's full resolution |
| <kbd>F7</kbd> | the record button | A recording in **Videos › Mirrorly**; press again to finish |

While recording, the record button is a red stop button and the window's
title ends in *· Recording*. A message says where the file went and how long
it is.

Files are named by the time they were taken (`Mirrorly 2026-10-20
07.21.57.mp4`), and turned the way you have the view turned (<kbd>F12</kbd>),
so a landscape game records in landscape. Set the rotation before you start
recording; turning it during a recording doesn't change the file.

## How recordings work

A recording is **the phone's own video stream**, written to an MP4 file as it
arrives. Nothing is decoded and encoded again, so recording:

- costs next to nothing, and doesn't slow the mirror down;
- keeps exactly the quality the phone sent;
- has the phone's own frame timing: smooth playback, the right length.

The file is HEVC (H.265) video in MP4, like the iPhone's own videos. Windows'
Media Player and Photos play it with the HEVC Video Extensions.

Recordings have **no sound**: Mirrorly shows the screen only. A recording ends
by itself, and is saved, if the phone disconnects or you quit Mirrorly.

## Details, for the curious

- The phone pads its picture for the encoder (an iPad's 1640 × 2360 screen
  arrives as 1648 × 2384) without saying so in the stream. Mirrorly rewrites
  the stream's sequence header (SPS) with the crop, which every player
  honours.
- The stream's timestamps run on a clock that isn't the usual 90 kHz (about
  24 kHz, and not documented); Mirrorly measures its rate over the recording
  against the PC's clock.
- The view's rotation is stored as the track's display matrix.

The writer is a small purpose-made one (`crates/video/src/mp4.rs`), which
saves shipping FFmpeg's 22 MB container library.
