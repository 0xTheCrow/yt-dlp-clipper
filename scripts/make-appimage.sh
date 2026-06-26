#!/usr/bin/env bash
# Build a single self-contained yt-dlp-clipper AppImage for x86-64 Linux.
#
# Bundles the app + a STATIC ffmpeg + the self-contained yt-dlp_linux, and lets
# linuxdeploy pull the app's shared-library deps into the bundle. The result runs
# on any reasonably-recent x86-64 desktop with no system FFmpeg / yt-dlp / python.
#
# No sudo required: APPIMAGE_EXTRACT_AND_RUN lets the AppImage-based tools run
# without FUSE. (Installing libfuse2 is only needed so the *output* AppImage runs
# on a plain double-click instead of with --appimage-extract-and-run.)
#
# Usage:  ./scripts/make-appimage.sh
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
BUILD="$ROOT/build/linux"         # per-target subdir; make-windows.{sh,ps1} use build/windows
APPDIR="$BUILD/AppDir"
export VERSION="${VERSION:-$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)}"
export APPIMAGE_EXTRACT_AND_RUN=1

LINUXDEPLOY_URL="https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage"
YTDLP_URL="https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux"
# GitHub-hosted static build (BtbN, an FFmpeg maintainer; linked from ffmpeg.org).
# This is the runtime ffmpeg CLI yt-dlp merges with; it's independent of the libav*
# the app links, so its version is not pinned to the build-time FFmpeg.
FFMPEG_URL="https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz"

mkdir -p "$BUILD"

echo "==> Building release binary"
cargo build --release

echo "==> Fetching tools + bundled binaries (cached in build/)"
# -nv stays quiet but still prints the URL and any error (a fully-quiet -q hides why
# a fetch failed); --tries lets a transient network blip retry instead of failing CI.
fetch() { wget -nv --tries=3 --timeout=60 --retry-connrefused -O "$1" "$2"; }
[ -f "$BUILD/linuxdeploy.AppImage" ] || fetch "$BUILD/linuxdeploy.AppImage" "$LINUXDEPLOY_URL"
[ -f "$BUILD/yt-dlp" ]               || fetch "$BUILD/yt-dlp" "$YTDLP_URL"
[ -f "$BUILD/ffmpeg" ] || {
    fetch "$BUILD/ffmpeg.tar.xz" "$FFMPEG_URL"
    tar -xf "$BUILD/ffmpeg.tar.xz" -C "$BUILD"
    ffmpeg_bin="$(find "$BUILD" -type f -name ffmpeg | head -1)"
    [ -n "$ffmpeg_bin" ] || { echo "error: no ffmpeg binary in $FFMPEG_URL" >&2; exit 1; }
    cp "$ffmpeg_bin" "$BUILD/ffmpeg"
}
chmod +x "$BUILD/linuxdeploy.AppImage" "$BUILD/yt-dlp" "$BUILD/ffmpeg"

echo "==> Assembling AppDir (app + yt-dlp + ffmpeg next to the exe)"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin"
cp "$ROOT/target/release/yt-dlp-clipper" "$APPDIR/usr/bin/yt-dlp-clipper"
cp "$BUILD/yt-dlp"             "$APPDIR/usr/bin/yt-dlp"
cp "$BUILD/ffmpeg"            "$APPDIR/usr/bin/ffmpeg"
chmod +x "$APPDIR/usr/bin/"*

echo "==> Packing AppImage"
# Pin the output filename (else the appimage plugin derives it from the desktop
# entry's Name=, "Cooper Clipper", and the release workflow's upload glob misses it).
export OUTPUT="yt-dlp-clipper-${VERSION}-x86_64.AppImage"
"$BUILD/linuxdeploy.AppImage" \
    --appdir "$APPDIR" \
    --executable "$APPDIR/usr/bin/yt-dlp-clipper" \
    --desktop-file "$ROOT/packaging/yt-dlp-clipper.desktop" \
    --icon-file "$ROOT/assets/yt-dlp-clipper.png" \
    --output appimage

echo "==> Done: $(ls -1 "$ROOT"/yt-dlp-clipper*.AppImage 2>/dev/null | tail -1)"
