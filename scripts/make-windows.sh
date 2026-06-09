#!/usr/bin/env bash
# Cross-compile a single self-contained Windows folder (zipped) for yt-dlp-clipper,
# FROM LINUX. The Windows counterpart that builds natively is scripts/make-windows.ps1.
#
# Only yt-dlp-clipper.exe needs a Windows toolchain; yt-dlp.exe and ffmpeg.exe are pure runtime
# binaries we just download and stage next to it. We target x86_64-pc-windows-msvc via
# cargo-xwin (it fetches the MSVC CRT + Windows SDK into a user cache — no Visual Studio,
# no Wine). The app links FFmpeg's libav* at build time, so we point FFMPEG_DIR at the
# same gyan 6.0 gpl-shared dev build the .ps1 uses — its import libs are MSVC, matching
# this target exactly. The whole bundle is GPL (FFmpeg GPL build + x264).
#
# Usage:  ./scripts/make-windows.sh
#
# One-time prerequisites (install yourself — this box has no sudo):
#   rustup target add x86_64-pc-windows-msvc
#   cargo install cargo-xwin
#   sudo apt install lld llvm clang p7zip-full zip   # lld-link + llvm-lib/rc; 7z to unpack ffmpeg
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
BUILD="$ROOT/build/windows"       # per-target subdir; make-appimage.sh uses build/linux
DIST="$BUILD/yt-dlp-clipper"
TARGET="x86_64-pc-windows-msvc"

# FFmpeg shared-GPL *dev* build — its major MUST match ffmpeg-sys-the-third (pinned
# +ffmpeg-6.0 in Cargo.lock), so we use the archived 6.0 release from gyan's GitHub
# (BtbN's rolling 'latest' no longer carries 6.x). Provides include/ + lib/ (MSVC
# import libs to link against) and bin/ (DLLs + ffmpeg.exe). It's a .7z, not a .zip.
FFMPEG_VER='6.0'
FFMPEG_NAME="ffmpeg-${FFMPEG_VER}-full_build-shared"
FFMPEG_URL="https://github.com/GyanD/codexffmpeg/releases/download/${FFMPEG_VER}/${FFMPEG_NAME}.7z"
YTDLP_URL='https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe'

mkdir -p "$BUILD"

# --- prereq checks -------------------------------------------------------------
missing=()
command -v cargo >/dev/null || missing+=("rust toolchain (https://rustup.rs)")
command -v cargo-xwin >/dev/null || missing+=("cargo-xwin (cargo install cargo-xwin)")
command -v lld-link >/dev/null || command -v lld >/dev/null || missing+=("lld (apt install lld)")
command -v clang >/dev/null || missing+=("clang (apt install clang)")
command -v 7z >/dev/null || command -v 7za >/dev/null || missing+=("7z (apt install p7zip-full)")
command -v zip   >/dev/null || missing+=("zip (apt install zip)")
rustup target list --installed 2>/dev/null | grep -qx "$TARGET" \
    || missing+=("rust target $TARGET (rustup target add $TARGET)")
if [ "${#missing[@]}" -ne 0 ]; then
    printf 'Missing prerequisites:\n'; printf '  - %s\n' "${missing[@]}"; exit 1
fi

# --- fetch FFmpeg dev build + yt-dlp.exe (cached) ------------------------------
SEVENZ="$(command -v 7z || command -v 7za)"
FFMPEG_ARCHIVE="$BUILD/ffmpeg.7z"
[ -s "$FFMPEG_ARCHIVE" ] || { echo "==> Downloading FFmpeg $FFMPEG_VER shared (GPL) dev build"; wget -qO "$FFMPEG_ARCHIVE" "$FFMPEG_URL"; }
FFMPEG_DIR="$BUILD/$FFMPEG_NAME"
[ -d "$FFMPEG_DIR" ] || "$SEVENZ" x -y -o"$BUILD" "$FFMPEG_ARCHIVE" >/dev/null
YTDLP_EXE="$BUILD/yt-dlp.exe"
[ -f "$YTDLP_EXE" ] || { echo "==> Downloading yt-dlp.exe"; wget -qO "$YTDLP_EXE" "$YTDLP_URL"; }

# --- build (cross-compile yt-dlp-clipper.exe) -------------------------------------------
# ffmpeg-sys finds the import libs + headers via FFMPEG_DIR. bindgen runs libclang
# directly and won't otherwise know it's targeting Windows, so it needs the target
# triple plus the MSVC/SDK headers cargo-xwin splats into the cache (layout below;
# adjust the -I paths here if a cargo-xwin version splats elsewhere).
echo "==> Cross-compiling release binary for $TARGET"
export FFMPEG_DIR
export XWIN_CACHE_DIR="$BUILD"            # cargo-xwin splats the SDK to $XWIN_CACHE_DIR/xwin
XWIN="$XWIN_CACHE_DIR/xwin"
export BINDGEN_EXTRA_CLANG_ARGS="--target=$TARGET -fms-extensions \
-I$XWIN/crt/include \
-I$XWIN/sdk/include/ucrt \
-I$XWIN/sdk/include/um \
-I$XWIN/sdk/include/shared \
-I$FFMPEG_DIR/include"
# Static-link the MSVC CRT so the exe doesn't import VCRUNTIME140.dll (which ships with
# the VC++ redistributable, not Windows) — keeps the "unzip and run" promise on a clean box.
export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static"
cargo xwin build --release --target "$TARGET"

# --- stage one self-contained folder ------------------------------------------
# Only yt-dlp-clipper.exe sits at the bundle root, so it's the obvious thing to
# click. The helper exes go in bin/ (the resolver looks there); the FFmpeg DLLs
# stay at the root because Windows loads them from the exe's own dir at startup.
echo "==> Staging bundle"
rm -rf "$DIST"; mkdir -p "$DIST/bin"
cp "target/$TARGET/release/yt-dlp-clipper.exe" "$DIST/yt-dlp-clipper.exe"
cp "$YTDLP_EXE"                      "$DIST/bin/yt-dlp.exe"
cp "$FFMPEG_DIR/bin/ffmpeg.exe"      "$DIST/bin/ffmpeg.exe"
# FFmpeg runtime DLLs the app links against (Windows loads these from the exe dir).
cp "$FFMPEG_DIR"/bin/*.dll "$DIST"

# --- zip -----------------------------------------------------------------------
VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
ZIP="$ROOT/yt-dlp-clipper-$VERSION-win64.zip"
rm -f "$ZIP"
( cd "$BUILD" && zip -qr "$ZIP" "yt-dlp-clipper" )
echo "==> Done: $ZIP"
