# Build Mirrorly release artifacts into dist\:
#   Mirrorly-Setup-<version>.exe   (Inno Setup installer, per-user, no admin)
#   Mirrorly-<version>-portable.zip
#   SHA256SUMS.txt
#
# Run from the repository root:  powershell -ExecutionPolicy Bypass -File packaging\build-release.ps1
# Needs: Rust (MSVC), VS 2022 Build Tools, LLVM, FFmpeg in third_party\ffmpeg,
# cargo-about (cargo install cargo-about --features cli), Inno Setup 6.

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

$version = (Select-String -Path crates\app\Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
Write-Host "Mirrorly $version"

# 1. Release build inside the MSVC environment (bindgen needs its headers).
$vcvars = Get-ChildItem "C:\Program Files*\Microsoft Visual Studio\2022\*\VC\Auxiliary\Build\vcvars64.bat" | Select-Object -First 1 -ExpandProperty FullName
if (-not $vcvars) { throw "VS 2022 Build Tools (vcvars64.bat) not found" }
cmd /c "set PATH=C:\Program Files (x86)\Microsoft Visual Studio\Installer;%PATH% && `"$vcvars`" >nul && set PATH=%USERPROFILE%\.cargo\bin;%PATH% && cargo build --release -p app"
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

# 2. Stage exactly what ships.
$stage = Join-Path $root "dist\stage"
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
$rel = Join-Path $root "target\release"
Copy-Item "$rel\Mirrorly.exe" $stage
# Only the FFmpeg libraries Mirrorly loads (checked with llvm-readobj --coff-imports).
foreach ($lib in 'avcodec', 'avutil', 'swresample') {
    $dll = Get-ChildItem "$rel\$lib-*.dll" | Select-Object -First 1
    if (-not $dll) { throw "$lib DLL missing in target\release" }
    Copy-Item $dll.FullName $stage
}
Copy-Item LICENSE "$stage\LICENSE.txt"
New-Item -ItemType Directory -Force "$stage\licenses" | Out-Null
Copy-Item packaging\licenses\*.txt "$stage\licenses\"
# The Virtual Display Driver (MIT, signed), installed on first "Use as
# display" for an iPad: see packaging\vdd\README.md.
New-Item -ItemType Directory -Force "$stage\vdd" | Out-Null
foreach ($f in 'MttVDD.dll', 'MttVDD.inf', 'mttvdd.cat', 'LICENSE.txt') { Copy-Item "packaging\vdd\$f" "$stage\vdd\" }

$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
cargo about generate -c packaging\about.toml -m crates\app\Cargo.toml packaging\about.hbs -o "$stage\THIRD-PARTY-NOTICES.txt"
if ($LASTEXITCODE -ne 0) { throw "cargo about failed" }

$ffver = (& third_party\ffmpeg\bin\ffmpeg.exe -hide_banner -version | Select-Object -First 1) -replace '^ffmpeg version (\S+).*', '$1'
$commit = if ($ffver -match '-g([0-9a-f]+)') { $Matches[1] } else { 'master' }
@"
FFmpeg in Mirrorly
==================

Mirrorly uses these FFmpeg libraries, unmodified, as separate DLLs:
  $((Get-ChildItem "$stage\av*.dll","$stage\sw*.dll" | ForEach-Object Name) -join ', ')

Version:  FFmpeg $ffver
License:  GNU Lesser General Public License, version 3 (licenses\LGPL-3.0.txt,
          which supplements the GNU GPL v3 in licenses\GPL-3.0.txt).
Binaries: BtbN FFmpeg-Builds, "lgpl-shared" variant
          https://github.com/BtbN/FFmpeg-Builds
Source:   https://github.com/FFmpeg/FFmpeg/tree/$commit
          Build scripts and the third-party libraries those builds include:
          https://github.com/BtbN/FFmpeg-Builds

You may replace these DLLs with your own build of the same FFmpeg version;
Mirrorly loads them by name from its own folder.
"@ | Set-Content -Encoding utf8 "$stage\FFMPEG-NOTICE.txt"

@"
Mirrorly $version
==================

Mirror and control your iPhone from Windows, over USB or Wi-Fi.

Before the first run
  1. Install "Apple Devices" from the Microsoft Store (free). It provides the
     driver Windows needs to talk to an iPhone.
  2. iPhone or iPad with iOS / iPadOS 27 or later. Plug it in, unlock it,
     tap "Trust".
  3. Turn on Developer Mode: Settings > Privacy & Security > Developer Mode
     (if it is missing, start Mirrorly once with the iPhone plugged in; the
     switch then appears).

Using it
  - Click and drag on the screen to tap and swipe. Wheel scrolls.
  - Point at the top of the window for the bar: close, minimize, size,
    rotate, App Switcher, Home. Drag the bar to move the window.
  - Ctrl+1 Home, Ctrl+2 App Switcher, Ctrl+3 Spotlight, right-click Home.
  - Typing goes to the iPhone; Ctrl+C/V/X/A/Z work as Command shortcuts.
  - Copy photos or files on the iPhone, then drag the chip on the bar out to
    Windows. Drop PC files on the window to send them to the iPhone: photos
    are pasted into the open app, documents are saved by name in a Mirrorly
    folder in the Files app.
  - One clipboard: copy text or pictures on the PC and paste on the iPhone
    (Ctrl+V in Mirrorly), copy on the iPhone and paste on the PC. Files
    copied in Explorer go over with Ctrl+V in Mirrorly.
  - The power button on the bar disconnects (the iPhone is left alone until
    you connect again). The disconnected screen lists your iPhones: click
    one to mirror it, or Forget it.
  - After one USB connection, unplug: Mirrorly continues over Wi-Fi (same
    network, Windows network profile "Private"). Plug in to go back to USB.

Logs: %LOCALAPPDATA%\Mirrorly\logs\Mirrorly.log
More: https://github.com/AnasSAV/mirrorly

Mirrorly is not affiliated with Apple Inc. iPhone is a trademark of Apple Inc.
"@ | Set-Content -Encoding utf8 "$stage\README.txt"

# 3. Portable zip.
$zip = Join-Path $root "dist\Mirrorly-$version-portable.zip"
Remove-Item $zip -ErrorAction SilentlyContinue
Compress-Archive -Path "$stage\*" -DestinationPath $zip -CompressionLevel Optimal

# 4. Installer.
$iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "C:\Program Files (x86)\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 (ISCC.exe) not found" }
& $iscc /Q "/DAppVersion=$version" "/DStageDir=$stage" packaging\Mirrorly.iss
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }

# 5. Checksums.
$files = Get-ChildItem dist\Mirrorly-Setup-$version.exe, $zip
$files | ForEach-Object { "{0}  {1}" -f (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower(), $_.Name } |
    Set-Content -Encoding ascii dist\SHA256SUMS.txt
$files | Select-Object Name, @{ n = 'MB'; e = { [math]::Round($_.Length / 1MB, 1) } } | Format-Table -AutoSize
Get-Content dist\SHA256SUMS.txt
