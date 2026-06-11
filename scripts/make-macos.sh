#!/usr/bin/env bash
# Build a self-contained "Cooper Clipper.app" (and a .dmg) for macOS.
#
# RUN THIS ON A MAC. The app binary links FFmpeg's libav* and CoreAudio at build
# time, so — like the Linux/Windows builds — it can't be cross-compiled; only the
# yt-dlp/ffmpeg *runtime* binaries are downloaded and staged next to the exe.
#
# Mirrors scripts/make-appimage.sh: builds the normal dynamic release against
# Homebrew's ffmpeg@6 (the 6.x line ffmpeg-the-third 1.2 pins), then uses
# dylibbundler to copy the app's libav*/libasound-equivalent dylibs into the
# bundle and rewrite their install names (the macOS counterpart to linuxdeploy).
# A static ffmpeg CLI + the self-contained yt-dlp are dropped next to the exe in
# Contents/MacOS/, where resolve_ffmpeg()/resolve_ytdlp() find them.
#
# No code signing or notarization — users will see a Gatekeeper warning and must
# right-click → Open (or `xattr -dr com.apple.quarantine` the .app) the first time.
#
# Usage:  ./scripts/make-macos.sh
#
# One-time prerequisites (install yourself):
#   brew install ffmpeg@6 pkg-config dylibbundler
#   xcode-select --install            # clang + the macOS SDK for bindgen
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
BUILD="$ROOT/build/macos"          # per-target subdir; the other scripts use build/{linux,windows}
APP_NAME="Cooper Clipper"
APP="$BUILD/$APP_NAME.app"
MACOS_DIR="$APP/Contents/MacOS"
RES_DIR="$APP/Contents/Resources"
BUNDLE_ID="com.cooper.yt-dlp-clipper"
VERSION="${VERSION:-$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)}"

ARCH="$(uname -m)"                  # arm64 (Apple Silicon) or x86_64 (Intel); host-arch build
case "$ARCH" in
    arm64)  FFMPEG_URL='https://www.osxexperts.net/ffmpeg611arm.zip' ;;  # static ffmpeg, arm64
    x86_64) FFMPEG_URL='https://www.osxexperts.net/ffmpeg611intel.zip' ;; # static ffmpeg, intel
    *) echo "Unsupported arch: $ARCH" >&2; exit 1 ;;
esac
YTDLP_URL='https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos'

# --- prereq checks -------------------------------------------------------------
missing=()
command -v cargo >/dev/null || missing+=("rust toolchain (https://rustup.rs)")
command -v brew  >/dev/null || missing+=("Homebrew (https://brew.sh)")
command -v dylibbundler >/dev/null || missing+=("dylibbundler (brew install dylibbundler)")
FFMPEG6_PREFIX="$(brew --prefix ffmpeg@6 2>/dev/null || true)"
[ -n "$FFMPEG6_PREFIX" ] && [ -d "$FFMPEG6_PREFIX" ] || missing+=("ffmpeg@6 (brew install ffmpeg@6)")
if [ "${#missing[@]}" -ne 0 ]; then
    printf 'Missing prerequisites:\n'; printf '  - %s\n' "${missing[@]}"; exit 1
fi

mkdir -p "$BUILD"

# --- build (link against the pinned ffmpeg 6.x, not brew's default 7.x) --------
# ffmpeg-sys-the-third finds headers + dylibs via pkg-config; point it at ffmpeg@6.
echo "==> Building release binary against ffmpeg@6 ($ARCH)"
export PKG_CONFIG_PATH="$FFMPEG6_PREFIX/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
cargo build --release

# --- fetch runtime binaries (cached) ------------------------------------------
echo "==> Fetching yt-dlp + static ffmpeg (cached in build/)"
[ -f "$BUILD/yt-dlp" ] || curl -fsSL -o "$BUILD/yt-dlp" "$YTDLP_URL"
[ -f "$BUILD/ffmpeg" ] || {
    curl -fsSL -o "$BUILD/ffmpeg.zip" "$FFMPEG_URL"
    unzip -o -j "$BUILD/ffmpeg.zip" -d "$BUILD" ffmpeg >/dev/null
}
chmod +x "$BUILD/yt-dlp" "$BUILD/ffmpeg"

# --- assemble the .app skeleton -----------------------------------------------
echo "==> Assembling $APP_NAME.app"
rm -rf "$APP"
mkdir -p "$MACOS_DIR" "$RES_DIR"
cp "$ROOT/target/release/yt-dlp-clipper" "$MACOS_DIR/$APP_NAME"
cp "$BUILD/yt-dlp"             "$MACOS_DIR/yt-dlp"
cp "$BUILD/ffmpeg"            "$MACOS_DIR/ffmpeg"
chmod +x "$MACOS_DIR/"*

# Icon: convert the existing PNG to a .icns (sips + iconutil ship with macOS).
ICONSET="$BUILD/icon.iconset"
rm -rf "$ICONSET"; mkdir -p "$ICONSET"
for sz in 16 32 64 128 256 512; do
    sips -z "$sz" "$sz"     "$ROOT/assets/yt-dlp-clipper.png" --out "$ICONSET/icon_${sz}x${sz}.png"     >/dev/null
    sips -z $((sz*2)) $((sz*2)) "$ROOT/assets/yt-dlp-clipper.png" --out "$ICONSET/icon_${sz}x${sz}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$RES_DIR/AppIcon.icns"

# Info.plist — minimal but enough for Finder/Gatekeeper to treat it as an app.
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>            <string>$APP_NAME</string>
    <key>CFBundleDisplayName</key>     <string>$APP_NAME</string>
    <key>CFBundleIdentifier</key>      <string>$BUNDLE_ID</string>
    <key>CFBundleVersion</key>         <string>$VERSION</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleExecutable</key>      <string>$APP_NAME</string>
    <key>CFBundleIconFile</key>        <string>AppIcon</string>
    <key>CFBundlePackageType</key>     <string>APPL</string>
    <key>LSMinimumSystemVersion</key>  <string>11.0</string>
    <key>NSHighResolutionCapable</key> <true/>
</dict>
</plist>
PLIST

# --- bundle the app's dynamic libav*/dylib deps into the .app -----------------
# dylibbundler copies every non-system dylib the exe links into Contents/libs and
# rewrites install names to @executable_path/../libs, so the .app needs no Homebrew.
echo "==> Bundling dynamic libraries (dylibbundler)"
dylibbundler -cd -b \
    -x "$MACOS_DIR/$APP_NAME" \
    -d "$APP/Contents/libs" \
    -p "@executable_path/../libs"

# --- package a .dmg -----------------------------------------------------------
echo "==> Building .dmg"
DMG="$ROOT/yt-dlp-clipper-$VERSION-macos-$ARCH.dmg"
rm -f "$DMG"
STAGE="$BUILD/dmg"
rm -rf "$STAGE"; mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"      # drag-to-install target
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null

echo "==> Done:"
echo "    app: $APP"
echo "    dmg: $DMG"
