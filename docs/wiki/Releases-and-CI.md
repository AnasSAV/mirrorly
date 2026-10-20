# Releases and CI

## Automated checks

| Workflow | Runs | Does |
|---|---|---|
| **CI** (`.github/workflows/ci.yml`) | Every push to `main` and every pull request | Builds the workspace on Windows, runs Clippy with warnings as errors, runs all tests |
| **Package** (`package.yml`) | By hand (*Actions › Package › Run workflow*), and for every `v*` tag | Builds the installer, portable zip and checksums on a clean machine; keeps them as the run's artifacts (publishes nothing) |
| **Wiki** (`wiki.yml`) | Pushes to `main` that change `docs/wiki/` | Publishes `docs/wiki/` to this wiki |
| **Dependabot** (`dependabot.yml`) | Monthly | Proposes updates to the GitHub Actions used |

The Rust crates are pinned on purpose (`idevice`, `windows`,
`ffmpeg-sys-next` follow Apple's protocols and FFmpeg's ABI) and are updated
by hand, with a test on a device.

CI downloads BtbN's FFmpeg 9.0 LGPL shared build once and caches it. The
tests need no phone: they cover the parsing, layout and maths around it.

## Editing the wiki

Edit the pages in [`docs/wiki`](https://github.com/AnasSAV/mirrorly/tree/main/docs/wiki)
and push to `main`: the Wiki workflow publishes them. Edits made on the wiki
website are overwritten by the next publish, so make them in the repository.
Page names are file names (`USB-and-Wi-Fi.md` is the page *USB and Wi-Fi*),
`_Sidebar.md` is the sidebar, and links between pages use `[[Title|Page-Name]]`.

## Making a release

1. **Version:** bump `version` in `crates/app/Cargo.toml` (Mirrorly's version
   is the app crate's).
2. **Notes:** write `packaging/RELEASE_NOTES_x.y.z.md` (the earlier ones show
   the shape: what's new, fixed, requirements, downloads).
3. **README / wiki:** update what changed.
4. **Build** (needs `cargo install cargo-about --features cli` and Inno Setup 6):
   ```powershell
   powershell -ExecutionPolicy Bypass -File packaging\build-release.ps1
   ```
   It builds in the MSVC environment, stages exactly what ships (Mirrorly,
   the FFmpeg DLLs it loads, the display driver, licences and notices), and
   writes `dist\Mirrorly-Setup-x.y.z.exe`, `dist\Mirrorly-x.y.z-portable.zip`
   and `dist\SHA256SUMS.txt`.
5. **Try it:** install the setup over the previous version, connect a phone.
6. **Commit and push**, then publish:
   ```powershell
   gh release create vX.Y.Z dist\Mirrorly-Setup-X.Y.Z.exe dist\Mirrorly-X.Y.Z-portable.zip dist\SHA256SUMS.txt `
     --target main --title "Mirrorly X.Y.Z: …" --notes-file packaging\RELEASE_NOTES_X.Y.Z.md
   ```

## What ships, and its licences

| Part | Licence |
|---|---|
| Mirrorly | MIT |
| FFmpeg (`avcodec`, `avutil`, `swresample` DLLs, unmodified) | LGPL v3; see `FFMPEG-NOTICE.txt` in the package |
| Virtual Display Driver (`vdd\`, signed by SignPath) | MIT; see `vdd\LICENSE.txt` |
| Rust crates | Permissive; listed in `THIRD-PARTY-NOTICES.txt` (cargo-about) |

The driver files are byte-exact copies of the signed release (their catalog
covers the INF's exact bytes; `.gitattributes` stops Git from changing line
endings). Where they come from: [`packaging/vdd/README.md`](https://github.com/AnasSAV/mirrorly/blob/main/packaging/vdd/README.md).
