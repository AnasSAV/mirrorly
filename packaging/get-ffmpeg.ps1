# Download the FFmpeg build named in packaging\ffmpeg.txt into
# third_party\ffmpeg, refusing it unless its SHA-256 and the hashes of the
# files Mirrorly uses match. Used by CI and by hand.
#
#   powershell -ExecutionPolicy Bypass -File packaging\get-ffmpeg.ps1
#   ... -Update    print the file hashes of a new url/sha256 for ffmpeg.txt

param([switch]$Update)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$pin = @{}
foreach ($line in Get-Content (Join-Path $PSScriptRoot 'ffmpeg.txt')) {
    if ($line -match '^\s*([^#=][^=]*)=(.+)$') { $pin[$Matches[1].Trim()] = $Matches[2].Trim() }
}
foreach ($k in 'url', 'sha256') { if (-not $pin[$k]) { throw "ffmpeg.txt has no $k" } }

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("mirrorly-ffmpeg-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $tmp | Out-Null
try {
    $zip = Join-Path $tmp 'ffmpeg.zip'
    $ProgressPreference = 'SilentlyContinue'
    Write-Host "Downloading $($pin.url)"
    Invoke-WebRequest $pin.url -OutFile $zip -UseBasicParsing
    $got = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    if ($got -ne $pin.sha256.ToLower()) { throw "FFmpeg archive SHA-256 is $got, expected $($pin.sha256): refusing it" }
    Expand-Archive $zip -DestinationPath (Join-Path $tmp 'x')
    $src = (Get-ChildItem (Join-Path $tmp 'x') -Directory | Select-Object -First 1).FullName

    if ($Update) {
        foreach ($f in 'avcodec-*.dll', 'avutil-*.dll', 'swresample-*.dll', 'ffmpeg.exe') {
            $file = Get-ChildItem (Join-Path $src "bin\$f") | Select-Object -First 1
            "bin/$($file.Name)=$((Get-FileHash $file.FullName -Algorithm SHA256).Hash.ToLower())"
        }
        return
    }
    foreach ($k in $pin.Keys | Where-Object { $_ -like 'bin/*' }) {
        $file = Join-Path $src $k
        if (-not (Test-Path $file)) { throw "$k is missing from the archive" }
        $h = (Get-FileHash $file -Algorithm SHA256).Hash.ToLower()
        if ($h -ne $pin[$k].ToLower()) { throw "$k SHA-256 is $h, expected $($pin[$k])" }
    }

    $dest = Join-Path $root 'third_party\ffmpeg'
    New-Item -ItemType Directory -Force (Split-Path $dest -Parent) | Out-Null
    if (Test-Path $dest) { Remove-Item $dest -Recurse -Force }
    Move-Item $src $dest
    Write-Host "FFmpeg $(Split-Path $src -Leaf) is in third_party\ffmpeg"
}
finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
