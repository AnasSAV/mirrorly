# Latency

How latency is measured, and every measurement with the change that
produced it. Newest entries at the bottom of each log.

## What is measured where

```
 mouse ─▶ window event ─▶ HID send ─▶ USB ─▶ iOS input ─▶ app render ─▶ encode
                                                                          │
 photons ◀─ monitor ◀─ DWM / flip ◀─ Present() ◀─ decode ◀─ RTP ◀─ USB ◀──┘
```

| Segment | Tool | Notes |
|---|---|---|
| Last RTP packet → `Present()` returned | overlay (F10), `--timings-csv` | per stage, QPC clock |
| `Present()` → scan-out begins | DXGI frame statistics, overlay + 5 s log line | compositor/vsync wait; not the monitor's own scan-out |
| Tap → phone's response frame arrives / is presented | F9 probe, overlay, log line `tap -> ...` | everything except the monitor; needs a still screen and a target that visibly reacts |
| Glass-to-glass (mouse click → photons) | 240 fps camera, procedure B | the real number |
| Phone screen → PC screen | 240 fps camera, procedure A | mirror lag for video only |
| Finger → phone's own screen | 240 fps camera, procedure C | iOS's own touch latency, for reference |

## Internal tools

- **Overlay**: `F10` (or `--overlay`). Per-stage p50/p95 over the last 2 s,
  fps, frames superseded before they could be shown, lost packets, corrupt
  pictures, pictures skipped while waiting for a keyframe, decode errors,
  keyframe requests, input queue delay, and the last tap results.
- **Probe**: point the mouse at something that changes when touched (a
  Calculator key) and press `F9`. Ten taps, 1.2 s apart, each timed until the
  response frame. Results go to the overlay and the console
  (`tap -> phone frame X ms, -> presented Y ms`). Detection is a heuristic:
  the first frame ≥ 30 ms after touch-down that is clearly larger than the
  idle frames. Moving content (videos, blinking cursors) breaks it.
- **CSV**: `--timings-csv out\t.csv` writes one line per presented frame:
  `rx_first, rx_last, dec_start, dec_end, wake, ready, presented` (µs, same
  clock), written from its own thread.
- **Switches for A/B tests**: `--vsync` (Present with sync interval 1 instead
  of immediately with tearing), `F11` borderless fullscreen (allows
  independent flip), `MIRROR_NAGLE=1` (TCP_NODELAY off), `--rctl
  pmd3|idevice|off`.

## Glass-to-glass procedure (240 fps camera)

Equipment: a phone that records 240 fps slow motion (each frame = 4.17 ms),
the mirror running on the PC, room light that doesn't flicker on camera.
Keep the PC display at its native refresh (144 Hz here), the overlay **off**,
and nothing else heavy running.

**A. Mirror lag (video only).** On the iPhone, open a millisecond stopwatch
(e.g. a web page showing ms). Frame the camera so the iPhone screen and the
mirror window are both in view. Record 10 s. In 10 different frames of the
recording, read both stopwatch values; lag = phone value − PC value. Report
min / median / max.

**B. Click-to-photon (the number this project is judged by).** Open
Calculator on the iPhone. Frame the camera on the mouse (finger visible on
the button) and the mirror window. Click a key in the mirror 10 times, ~1 s
apart. For each click, step through the recording: frame where the mouse
button bottoms out = start; first frame where the key's highlight appears in
the mirror = end. Latency = (end − start) × 4.17 ms. Report min / median /
max. Repeat for each configuration under test.

**C. Phone's own touch latency (reference).** Same, but tap the iPhone
screen directly with a finger and film only the phone: finger contact →
key highlight on the iPhone. This is the floor no mirror can beat; B − C ≈
what mirroring adds.

Resolution is one camera frame (±4 ms); 10 repetitions make the median
meaningful.

## Log

### Internal pipeline (last RTP packet → Present() returned)

| Date | Build / change | Config | n | p50 | p95 | p99 | Notes |
|---|---|---|---|---|---|---|---|
| 2026-10-18 | Phase 1 MVP (zero-copy d3d11va, flip-discard, waitable, max latency 1) | windowed, immediate + tearing, overlay off | 574 frames | 0.61 ms | 1.17 ms | 1.44 ms | stages: assembly 0.46, rx→decode 0.06, decode 0.44, →present 0.01, wait 0.01, render+Present 0.07 (p50, ms); 0 superseded; max 37.6 ms = first frame (decoder init) |
| 2026-10-18 | same | overlay **on** | ~250 frames | 1.01 ms | 1.75 ms | – | Direct2D text adds ~0.45 ms to render+Present; keep the overlay off for measurements |

### Tap → response (phone side + our pipeline, no monitor)

| Date | Build / change | Tool | n | → phone frame (p50) | → presented (p50) | Notes |
|---|---|---|---|---|---|---|
| 2026-10-18 | Phase 0 spike (no window) | `s3_input latency`, Calculator | 31 | 87.3 ms | – | range 81.6–101.1 ms over hold times and RCTL modes |
| 2026-10-18 | Phase 2 instrumented app, TCP_NODELAY on | F9 probe, Calculator "5" key | 10 | 86.8 ms | **87.5 ms** | presented 79.7–93.5 ms + one first-tap outlier 140.8 ms; frame arrival → presented ≈ 1 ms |
| 2026-10-18 | same, `MIRROR_NAGLE=1` (NODELAY off) | F9 probe, same key | 10 | 86.0 ms | 86.7 ms | 75.7–94.2 ms + first-tap 107.3 ms. **No measurable difference**: loopback ACKs immediately, so Nagle has nothing to hold. NODELAY stays on (free, protects the loaded case) |

### Present() → scan-out (DXGI frame statistics, no camera)

`GetFrameStatistics` reports the vblank (QPC time) of the most recently
displayed Present. A vblank *earlier* than the Present means the flip happened
immediately, mid-refresh (tearing), so scan-out began at once; the metric is
`max(0, vblank − Present())`. Monitor scan-out to a given row (≤ 6.9 ms at
144 Hz, ~3.5 ms average) and pixel response come on top in every mode and are
properties of the monitor. Each row: 4 × 5 s windows, Calculator/home screen
idle (~21 fps), Intel UHD, 144 Hz panel, 390×797 window.

| Date | Config | Present → scan-out p50 / p95 | Immediate flips | Last packet → scan-out p50 / p95 |
|---|---|---|---|---|
| 2026-10-18 | **windowed, immediate + tearing (default)** | **0.00 / 1.9–2.1 ms** | 55–69 % | **0.7–1.0 / 2.6–2.9 ms** |
| 2026-10-18 | windowed, `--vsync` | 5.3–5.6 / 7.9–8.6 ms | 0 % | 6.4–7.4 / 9.3–9.8 ms |
| 2026-10-18 | F11 borderless fullscreen, immediate | 0.7–1.8 / 3.5–7.5 ms | 33–43 % | 1.2–2.7 / 4.4–7.9 ms |
| 2026-10-18 | F11 borderless fullscreen, `--vsync` | 5.4–6.4 / 8.8–9.8 ms | 0 % | 6.5–7.4 / 9.8–10.6 ms |

Conclusions: the windowed flip-model swapchain on this Intel GPU is already
flipped directly (no DWM composition frame) most of the time, so the default
is the fastest mode. Vsync costs ~5.5 ms at p50. Fullscreen buys nothing and
is ~1–1.5 ms worse at p50 here (larger back buffer; fewer direct flips).
Default stays windowed + immediate. `MIRROR_DXGI_TRACE=1` logs the raw
statistics.

### USB vs Wi-Fi (Phase 3, same app session, same Calculator screen)

One mirror session with `--timings-csv`; the cable was unplugged (Wi-Fi run),
then plugged back in (USB run). F9 probe, 10 taps each. PC on 5 GHz Wi-Fi
(Realtek RTL8852BE, Wi-Fi 6), network profile Private. Wi-Fi touches use
Indigo digitizer events, USB touches use universalhidservice reports (see
README: iOS ignores the latter over the network).

| Date | Transport | Probe target | Tap → on screen (10 taps) | Median | Notes |
|---|---|---|---|---|---|
| 2026-10-18 | **USB** | Calculator "4"\* | 78.7, 78.7, 82.7, 85.2, 88.7, 89.7, 90.4, 103.1, 106.7, 121.1 ms | **89.2 ms** | link Good |
| 2026-10-18 | **Wi-Fi** | Calculator "5" | 87.4, 88.9, 92.7, 93.6, 96.0, 96.6, 97.0, 102.8, 119.6, 149.1 ms | **96.3 ms** | link Fair, jitter ≈ 10.5 ms |

\* the window shifted between runs, so the USB probe hit the neighbouring
key; same kind of response (a digit appears).

Per-frame (CSV, 1505 Wi-Fi frames / 1683 USB frames):

| | Wi-Fi | USB |
|---|---|---|
| last packet → Present() p50 / p95 | 1.00 / 4.54 ms | 1.09 / 1.65 ms |
| longest gap between frames | 152 ms | 55 ms |
| RFC 3550 jitter (overlay) | ~9–13 ms ("Fair"); spikes to 35 ms during big animations | ~5 ms (encoder variation) |
| tunnel setup | pair-verify 113–221 ms + TLS-PSK 188–354 ms + RSD 46–114 ms | CoreDeviceProxy 59–130 ms + RSD 17–33 ms |
| switch time | unplug → Wi-Fi video ≈ 2.1 s | plug in → USB video ≈ 1.3 s |

Reading: Wi-Fi adds ~7 ms at the median and a much heavier tail. The video
rides inside the TLS-over-TCP tunnel, so a lost Wi-Fi packet is retransmitted
and everything behind it waits (head-of-line blocking): frames then arrive in
a burst (152 ms gap; p95 frame→Present 4.5 ms as the burst is decoded). No
frames are lost, so no keyframe requests fire; the cost is delay, not
corruption. Removing it needs Apple's QUIC tunnel transport, which idevice
0.1.68 does not implement. Nothing on our side buffers.

### Glass-to-glass (camera)

Not measured: no camera available (2026-10-18). Best estimate from the
measurements above: tap → response presented ≈ 87.5 ms median, + Present →
scan-out ≈ 0 ms median, + monitor scan-out/response (~3.5 ms average to a
mid-screen row, plus panel response) ≈ **91–95 ms click-to-photon**, of which
~86 ms is inside the phone.

| Date | Build / change | Config | Procedure | n | min | median | max | Notes |
|---|---|---|---|---|---|---|---|---|
| | | | | | | | | |

## Tuning checklist (Phase 2)

| Item | Status |
|---|---|
| TCP_NODELAY on the usbmuxd socket (carries input and video) | on; A/B measured: no difference (87.5 vs 86.7 ms median tap → presented) |
| Socket buffer sizes | the only kernel socket is loopback TCP to usbmuxd; RTP is userspace (jktcp, unbounded channels). Frame assembly p50 0.46 ms shows no buffering stall; not tuned |
| Coalescing pointer moves | only under backlog (a move is dropped only if a newer one is already queued); 0 coalesced in normal use |
| High refresh output | panel 144 Hz; Present immediately with tearing allowed, max frame latency 1 |
| Timer resolution | `timeBeginPeriod(1)` at start-up (affects timed button holds; the frame and input paths are event-driven) |
| Present mode | measured via DXGI statistics: windowed + immediate/tearing is best (Present → scan-out p50 0 ms); vsync +5.5 ms; fullscreen +1–1.5 ms. Default unchanged |
| Overlay cost | Direct2D text adds ~0.45 ms to render+Present; off by default |
| Encoder (phone side) | the phone's ~86 ms dominates everything; offer knobs (LTR, bitrate, codec features) untested — the only remaining lever |
