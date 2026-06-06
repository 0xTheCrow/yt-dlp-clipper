# Cooper Clipper

A cross-platform (Linux / Windows / macOS) desktop app to pull videos with
[yt-dlp](https://github.com/yt-dlp/yt-dlp) and trim them **frame-accurately** —
saving the full video, a clip, or audio only.

## What it does

- **Download** any yt-dlp-supported URL, with a live progress bar and a
  Resolution menu that only offers the heights the source actually provides.
- **Frame-accurate preview** — scrub, play/pause, and step one frame at a time.
  Seeking lands on the exact frame (seek to the keyframe at or before the
  target, then decode forward), with audio kept in sync as the master clock.
- **Trim and save** with a precise in/out point:
  - **Full** — the whole file, stream-copied when the container can hold the
    source codecs, re-encoded only when it can't.
  - **Clip** — a frame-accurate, sample-accurate cut of the window.
  - **Audio only** — re-encoded to MP3/AAC, or a lossless stream-copy of the
    original audio.
- **Optional downscale** of the saved video (aspect-preserving, never upscales).
- **Format choice** — pick the output container (MP4 / MOV / MKV / WebM);
  streams are copied or transcoded to fit it automatically.
- **Rebindable keyboard shortcuts** (set start, set end, play/pause) and
  persisted settings (UI scale, volume, download directory, theme).
- **Built-in yt-dlp updater** — yt-dlp breaks as sites change, so the app can run
  `yt-dlp -U` from Settings.

## Requirements

### Runtime

The app shells out to two external binaries at runtime:

- **`yt-dlp`** — to fetch metadata and download.
- **`ffmpeg`** — yt-dlp uses it to merge the downloaded video + audio.

It looks for them in this order: a managed copy in its own data directory → a
copy next to the executable (bundled) → your `PATH`. The packaged builds
(AppImage / Windows zip) bundle both, so a from-source build is the main case
where you need them installed yourself.

### Build

System packages needed to compile (these link FFmpeg via Rust bindings):

- FFmpeg **dev** libraries — `libavcodec`, `libavformat`, `libavutil`,
  `libavfilter`, `libavdevice`, `libswscale`, `libswresample`
- `clang` (for bindgen) and `pkg-config`
- ALSA dev headers (`libasound2-dev`) for audio output via cpal
- A [Rust toolchain](https://rustup.rs/) (stable, edition 2021)

> **FFmpeg version:** the bindings cover a wide range (FFmpeg 4.4 through 6.x);
> a distro's stock dev packages in that range work. FFmpeg 7.x dropped APIs this
> build needs. (The Windows cross-build pins a 6.0 ffmpeg specifically — see
> `scripts/make-windows.sh`.)

On Debian/Ubuntu:

```sh
sudo apt-get install \
  libavcodec-dev libavformat-dev libavutil-dev libavfilter-dev \
  libavdevice-dev libswscale-dev libswresample-dev \
  clang pkg-config libasound2-dev
```

On Arch Linux (the `ffmpeg`, `alsa-lib`, and `yt-dlp` packages also provide the
runtime binaries, so this one command covers both build and run):

```sh
sudo pacman -S --needed ffmpeg clang pkgconf alsa-lib yt-dlp rust
```

> Arch's `ffmpeg` tracks the latest release, which may be **7.x** — if so the
> build won't link (see the FFmpeg-version note above). In that case install a
> 6.x `ffmpeg` (e.g. a compat package from the AUR) for building. Using `rustup`
> instead of the `rust` package is fine too.

You'll also want the `yt-dlp` and `ffmpeg` **binaries** on your `PATH` to run it.
On Debian/Ubuntu:

```sh
sudo apt-get install ffmpeg
# yt-dlp: see https://github.com/yt-dlp/yt-dlp#installation
```

## Build & run

```sh
cargo build            # debug build
cargo run              # launch the GUI
cargo run -- <path>    # launch and open a local video file
cargo build --release  # optimized build
cargo test             # unit + integration tests (need ffmpeg + ffprobe)
```

The crate is named `yank`; the build produces a `yank` binary.

## Packaging (distributable builds)

Scripts in `scripts/` produce self-contained bundles that ship `yt-dlp` and
`ffmpeg` alongside the app, so end users don't install anything:

- **Linux AppImage** — `./scripts/make-appimage.sh` → a single
  `yt-dlp-clipper-<ver>-x86_64.AppImage`. Builds the normal dynamic release,
  then bundles the app's `libav*`/`libasound` dependencies plus a static
  `ffmpeg` and the self-contained `yt-dlp_linux`. Needs `libfuse2` only for the
  *output* AppImage to run on a double-click.
- **Windows bundle** — `./scripts/make-windows.sh` cross-compiles from Linux
  (no Windows/Wine needed) via [cargo-xwin](https://github.com/rust-cross/cargo-xwin),
  or `scripts/make-windows.ps1` builds natively on Windows. Either produces
  `yt-dlp-clipper-<ver>-win64.zip` (app exe + `yt-dlp.exe` + `ffmpeg.exe` + DLLs).

The artifact version is read from `version` in `Cargo.toml`. There's also an
off-by-default `static-ffmpeg` cargo feature that links FFmpeg statically (for
CI / dependency-free binaries); see `Cargo.toml` for what it needs.

## License

The default dynamic build links system FFmpeg. The packaged releases bundle a
GPL ffmpeg + x264, making those builds **GPL**.
