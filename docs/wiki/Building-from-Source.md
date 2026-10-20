# Building from source

Mirrorly is Rust, Windows only (x64).

## Tools

| Tool | Install | Why |
|---|---|---|
| Rust stable (MSVC) | `winget install Rustlang.Rustup` | The compiler |
| Visual Studio 2022 Build Tools, C++ workload | `winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"` | Linker, and the headers bindgen reads |
| LLVM | `winget install LLVM.LLVM` | libclang, for bindgen (the path is in `.cargo/config.toml`) |
| FFmpeg 9.0, **LGPL shared** dev build | BtbN's [`ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip`](https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip), unpacked so that `third_party/ffmpeg` holds `bin/`, `include/`, `lib/` | HEVC decoding, H.264 encoding. Never a GPL build. |

## Build and run

The FFmpeg bindings are generated at build time and need the MSVC
environment, so build from a *Developer* prompt, or through `vcvars64.bat`:

```powershell
cmd /c "\"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat\" && cargo build --release"
target\release\Mirrorly.exe --console
```

The build copies FFmpeg's DLLs next to the executable. Release builds keep
debug info (`[profile.release] debug = 1`) for profiling.

## Check before a pull request

The same as CI (see [[Releases and CI|Releases-and-CI]]):

```powershell
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Anything touching the device, video or input needs a run with a real iPhone
or iPad too: the unit tests cover parsing, layout and maths, not the phone.

> **Smart App Control** (Windows 11) sometimes blocks a freshly built test
> program ("An Application Control policy has blocked this file", and cargo
> reports *never executed*). Run the tests again; it's not a test failure.

## Options and diagnostics

| Option | Effect |
|---|---|
| `--console` | Log to a console window instead of the log file |
| `--overlay` | Start with the latency overlay (<kbd>F10</kbd> toggles it) |
| `--timings-csv FILE` | One line of stage timestamps per presented frame |
| `--vsync` | Present with vsync (slower; for comparison) |
| `--no-wifi` | USB only: no Wi-Fi pairing or fallback |
| `--no-auto-ddi` | Don't install the Developer Disk Image automatically |
| `--rctl pmd3\|idevice\|off` | RTCP rate-control report layout |

| Environment | Effect |
|---|---|
| `RUST_LOG` | Log filter, e.g. `info,display=debug` |
| `MIRROR_TOUCH=uhs\|indigo` | Which input service carries touches |
| `MIRROR_NAGLE=1` | Leave Nagle's algorithm on (comparison) |
| `MIRROR_DXGI_TRACE=1` | Trace presentation timing |
| `MIRROR_SQUARE=1` | No rounded window corners |
| `MIRROR_DISPLAY_SYSMEM=1` | iPad display: encode from system memory (comparison) |

<kbd>F9</kbd> runs a latency probe: ten timed taps where the mouse points,
results in the overlay and the log.

## Probes

`spike/` holds the measurement and protocol probes Mirrorly was built from
(`s0`…`s10`), each a small binary, e.g.:

```powershell
cargo run --release -p spike --bin s9_display -- serve     # the iPad display on its own
cargo run --release -p spike --bin s10_mp4 -- in.hevc out.mp4 90 1640 2360
```

[FEASIBILITY.md](https://github.com/AnasSAV/mirrorly/blob/main/FEASIBILITY.md)
records what each verified.
