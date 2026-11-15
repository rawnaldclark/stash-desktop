# Fetches the libmpv build the audio engine is developed against (Windows x64) into third_party/mpv/.
# The archive is pinned by SHA-256. It needs 7-Zip (the archive is LZMA-compressed).
#
#   powershell -ExecutionPolicy Bypass -File scripts/fetch-mpv.ps1
#
# This build of libmpv is GPL. Stash desktop is GPL-3.0-or-later, so that is compatible.
$ErrorActionPreference = 'Stop'
$url = 'https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261111/mpv-dev-x86_64-20261111-git-bcde6b93cc.7z'
$sha256 = '273c079b11ea5575822f972756f4c9a71149a5c10130353abeb8586738938452'
$dir = Join-Path (Split-Path -Parent $PSScriptRoot) 'third_party\mpv'
$archive = Join-Path $dir 'mpv-dev.7z'

New-Item -ItemType Directory -Force $dir | Out-Null
if (-not (Test-Path $archive)) { Invoke-WebRequest -Uri $url -OutFile $archive }
$got = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLower()
if ($got -ne $sha256) { throw "mpv-dev.7z has SHA-256 $got, expected $sha256. Delete it and run this again." }

$sevenZip = @("$env:ProgramFiles\7-Zip\7z.exe", "${env:ProgramFiles(x86)}\7-Zip\7z.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $sevenZip) { throw '7-Zip was not found. Install it from https://www.7-zip.org and run this again.' }
& $sevenZip x -y -bso0 -bsp0 "-o$dir" $archive
if (-not (Test-Path (Join-Path $dir 'libmpv-2.dll'))) { throw 'libmpv-2.dll was not in the archive.' }
Write-Host "libmpv is in $dir"
