# Build a single self-contained Windows folder (zipped) for yt-dlp-clipper.
#
# Mirrors scripts/make-appimage.sh: a normal DYNAMIC release build, with the
# FFmpeg runtime DLLs + a static-ish ffmpeg.exe + the self-contained yt-dlp.exe
# staged next to the app exe (where the resolver finds them) and zipped. A friend
# unzips and runs yt-dlp-clipper.exe with nothing else installed.
#
# Static-linking FFmpeg on Windows means building it from source under MSYS2 — not
# worth it here, so we bundle the DLLs instead (Windows loads them from the exe's
# own folder). The whole bundle is GPL (FFmpeg GPL build + x264).
#
# RUN THIS ON WINDOWS (PowerShell), from the repo root:  .\scripts\make-windows.ps1
#
# One-time prerequisites (install yourself; need admin):
#   - Rust (MSVC toolchain):  https://rustup.rs
#   - Visual Studio Build Tools (MSVC linker)
#   - LLVM/clang (bindgen needs libclang):  winget install LLVM.LLVM
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
$root  = (Get-Location).Path
$build = Join-Path $root 'build\windows'   # per-target subdir; make-appimage.sh uses build/linux
$dist  = Join-Path $build 'yt-dlp-clipper'

# FFmpeg shared-GPL *dev* build — its major MUST match ffmpeg-sys-the-third (pinned
# +ffmpeg-6.0 in Cargo.lock), so we use the archived 6.0 release from gyan's GitHub
# (BtbN's rolling 'latest' no longer carries 6.x). Provides include/ + lib/ (MSVC
# import libs to compile/link against) and bin/ (DLLs + ffmpeg.exe). It's a .7z.
$ffmpegVer  = '6.0'
$ffmpegName = "ffmpeg-$ffmpegVer-full_build-shared"
$ffmpegUrl  = "https://github.com/GyanD/codexffmpeg/releases/download/$ffmpegVer/$ffmpegName.7z"
$ytdlpUrl   = 'https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe'

New-Item -ItemType Directory -Force -Path $build | Out-Null

# --- prereq checks -------------------------------------------------------------
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'cargo not found — install Rust (MSVC) from https://rustup.rs' }
if (-not (Get-Command clang -ErrorAction SilentlyContinue) -and -not $env:LIBCLANG_PATH) {
    Write-Warning 'libclang not found — bindgen will fail. Install LLVM (winget install LLVM.LLVM) or set LIBCLANG_PATH.'
}

# --- fetch FFmpeg dev build + yt-dlp.exe (cached) ------------------------------
$ffmpegArchive = Join-Path $build 'ffmpeg.7z'
if (-not (Test-Path $ffmpegArchive)) {
    Write-Host "==> Downloading FFmpeg $ffmpegVer shared (GPL) dev build"
    Invoke-WebRequest -Uri $ffmpegUrl -OutFile $ffmpegArchive
}
# Windows 10 1803+ ships tar.exe (bsdtar/libarchive), which reads .7z natively.
$ffmpegDir = Join-Path $build $ffmpegName
if (-not (Test-Path $ffmpegDir)) {
    tar -xf $ffmpegArchive -C $build
    if ($LASTEXITCODE -ne 0) { throw 'extracting ffmpeg .7z failed — need Windows 10 1803+ (bundled tar) or 7-Zip' }
}
$ytdlpExe = Join-Path $build 'yt-dlp.exe'
if (-not (Test-Path $ytdlpExe)) {
    Write-Host '==> Downloading yt-dlp.exe'
    Invoke-WebRequest -Uri $ytdlpUrl -OutFile $ytdlpExe
}

# --- build (point ffmpeg-sys at the dev build; need its bin/ on PATH) ----------
Write-Host '==> Building release binary'
$env:FFMPEG_DIR = $ffmpegDir
$env:PATH       = (Join-Path $ffmpegDir 'bin') + ';' + $env:PATH
# Static-link the MSVC CRT so the exe doesn't import VCRUNTIME140.dll (which ships with
# the VC++ redistributable, not Windows) — keeps the "unzip and run" promise on a clean box.
$env:RUSTFLAGS  = ($env:RUSTFLAGS + ' -C target-feature=+crt-static').Trim()
cargo build --release
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

# --- stage one self-contained folder ------------------------------------------
# Only yt-dlp-clipper.exe sits at the bundle root, so it's the obvious thing to
# click. The helper exes go in bin\ (the resolver looks there); the FFmpeg DLLs
# stay at the root because Windows loads them from the exe's own dir at startup.
Write-Host '==> Staging bundle'
if (Test-Path $dist) { Remove-Item -Recurse -Force $dist }
New-Item -ItemType Directory -Force -Path (Join-Path $dist 'bin') | Out-Null
Copy-Item 'target\release\yank.exe' (Join-Path $dist 'yt-dlp-clipper.exe')
Copy-Item $ytdlpExe                 (Join-Path $dist 'bin\yt-dlp.exe')
Copy-Item (Join-Path $ffmpegDir 'bin\ffmpeg.exe') (Join-Path $dist 'bin\ffmpeg.exe')
# FFmpeg runtime DLLs the app links against (Windows loads these from the exe dir).
Copy-Item (Join-Path $ffmpegDir 'bin\*.dll') $dist

# --- zip -----------------------------------------------------------------------
$ver = (Select-String -Path 'Cargo.toml' -Pattern '^version\s*=\s*"(.+)"').Matches[0].Groups[1].Value
$zip = Join-Path $root "yt-dlp-clipper-$ver-win64.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $dist -DestinationPath $zip
Write-Host "==> Done: $zip"
