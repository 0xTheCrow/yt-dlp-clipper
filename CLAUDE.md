# yank

A cross-platform (Linux/Windows/macOS) desktop app to pull videos with **yt-dlp**
and trim them **frame-accurately**, saving the full video, a clip, or audio-only.

## Build & run

System prerequisites (build-time):
- FFmpeg dev libraries: `libavcodec-dev libavformat-dev libavutil-dev libavfilter-dev libavdevice-dev libswscale-dev libswresample-dev`
- `clang` (for bindgen), `pkg-config`
- `libasound2-dev` (ALSA, for cpal audio)

Runtime: the `yt-dlp` binary, and an `ffmpeg` binary (yt-dlp uses it to merge video+audio).

```
cargo build            # debug
cargo test             # unit + integration tests (need ffmpeg + ffprobe)
cargo run              # launch the GUI
cargo run -- <path>    # launch and open a local video
```

## Architecture

- **lib (`src/lib.rs`)** — reusable, GUI-free modules:
  - `decoder` — frame-accurate decode/seek/step (seek to keyframe ≤ target, decode forward to exact frame). Timestamps in stream time-base; container-duration fallback for WebM.
  - `export` — Full and Clip both route through one `transcode()` that, per stream, **copies when the chosen container can hold the codec and re-encodes when it can't**, targeting the container's native codecs (H.264/AAC for MP4·MOV·MKV, VP9/Opus for WebM); `video_fits`/`audio_fits` + `container_kind` + `audio_encode_codec` decide. Full additionally short-circuits to a pure-copy `remux_copy` when every stream already fits. Clip always re-encodes video for a frame-accurate `in`, **and always re-encodes audio** so the cut is sample-accurate (a stream copy can't split a packet, so an exact `in`/`out` needs a re-encode); Full copies audio when it fits. AudioOnly re-encodes to MP3/AAC, or `Original` lossless stream-copy into a container that fits the source codec (packet-granular, since copies can't split a packet). Codec-tag reset on copy; x264 opened with a preset + `qmin/qmax/me_range/gop` unset so it accepts the settings. Clip video has a frame-count backstop so sources with missing frame PTS can't re-encode to EOF, and a clip keeps reading audio past the video's end so interleave lag can't truncate the tail. Audio re-encode (`AudioReenc`) runs `abuffer→atrim→abuffersink` (sink chunked to the encoder frame size); the `atrim` window is computed in samples from the first decoded frame's PTS, trimming the cut to the exact sample, and output PTS are stamped from a running sample counter to stay monotonic. The GUI's Video/Audio format dropdowns (`VideoFormat`, `AudioFormat`) pick the output container. `ExportSpec.scale_height` optionally **downscales** the saved video (aspect-preserving, even dims via `scaled_dims`, never upscales); since a copy can't scale, a downscale forces the re-encode path (incl. routing Full away from `remux_copy`).
  - `ytdlp` — wraps the `yt-dlp` binary (fetch `-J` metadata, download with progress, size estimate). `available_heights` lists the source's distinct video heights (tallest first) and `resolution_selector` builds the `-f` expression (`bestvideo[height<=H]+bestaudio/…`) for the GUI's download Resolution menu; both cap at what the source offers.
  - `audio` — cpal output; decodes+resamples the audio stream on a worker thread; `clock_secs()` is the A/V **master clock**.
- **bin (`src/main.rs`)** — egui/eframe GUI. Layout: top toolbar (URL/fetch/open/from-cache/settings), central video preview (aspect-fit, fills width / full height for portrait), bottom clip controls (timeline, transport, Start/End, save).
  - **Decoding runs on a background thread** (`DecoderHandle`): the UI sends seek/step requests (coalesced to the latest) and receives frames + metadata; it never blocks on a decode. Seeks are **generation-tagged** so superseded mid-drag decodes are dropped and the playhead pins to the released spot.
  - **Playback**: video chases the master clock (audio if available, else wall time).
  - **Keyboard shortcuts** (`Keybinds`/`Bind`): set-start (S), set-end (E), play/pause (Space) by default, all **rebindable** in Settings (click the action, press a new key; Esc cancels; `rebind` swaps to keep keys distinct). Persisted by key *name* (so it doesn't need egui's serde feature). The Set Start/End buttons show their key; the main shortcut handler is suppressed while a text field has focus or Settings is capturing a key.
  - Settings persist via eframe storage (UI scale, volume, download dir, delete-cache-on-exit). Downloads go to a managed cache dir (`eframe::storage_dir("yank")/downloads`).
  - **Button icons** are SVGs in `assets/` (`save.svg`, `download.svg`, `settings.svg`), embedded via `egui::include_image!` and rasterized by `egui_extras` (svg feature, resvg) after `install_image_loaders` at startup; `icon_button()` builds an icon+text button, sizing the icon to the button font height so it lines up with the caption. Unicode glyphs render as missing-glyph boxes in egui's bundled fonts, hence SVGs.

## Conventions

- **No magic numbers** — hoist literals to named `const`s, or use a library constant (e.g. `ffmpeg::ffi::AV_TIME_BASE`).
- **Comments describe what the code does**, not why something *wasn't* done; drop dead code rather than commenting around it.
- **Avoid adding a crate for one small thing** — prefer std or an existing dep (e.g. `eframe::storage_dir` instead of `dirs`). New deps are fine for genuine core needs (cpal for audio, ffmpeg-the-third).
- This machine has no usable `sudo`; system packages must be installed by the user.

## Planned / TODO

### yt-dlp bundling + in-app update (mostly built)
yt-dlp breaks often as sites change, so it must be kept current.
- **Binary resolution + seeding (built)** — `resolve_ytdlp()`/`resolve_ffmpeg()`/`find_in_path()` in `main.rs` resolve to **absolute paths** (managed copy → next-to-exe (bundled) → PATH) at startup and pass them to `ytdlp::set_binary`/`set_ffmpeg` (global `OnceLock<PathBuf>`); every invocation uses the full path, never the bare name. yt-dlp is **seeded into a writable managed dir** (`storage_dir/bin/`, `0755`) so `yt-dlp -U` works even when installed read-only (`.app`/AppImage/`Program Files`). ffmpeg isn't seeded (never self-updates).
- **Security hardening (built)** — the managed `bin/` is owner-only-writable (`set_owner_only`, 0755 on unix; no-op on Windows ACLs) so a local user can't swap the binary we run; download passes **`--ffmpeg-location <resolved ffmpeg>`** (verified in `tests/ffmpeg_location.rs`) so yt-dlp merges with our exact ffmpeg and never searches PATH/CWD for a planted one.
- **Update UI (built)** — `ytdlp::version()`/`update()` (`yt-dlp -U` on a worker → `Msg::YtdlpUpdated`); `ytdlp_version`/`ytdlp_updating` on `App`. Settings shows the version + an Update button; failures surface full text via the **error panel** (`error_panel`, `last_error`), which also shows an **Update yt-dlp** button when `ytdlp::suggests_update` matches yt-dlp's outdated-binary hints. Detecting "newer exists" before updating would need a GitHub HTTP check (a dep we avoid), so `-U` itself is the check (no-op when current).
- **Remaining** — actually ship the `yt-dlp`/`ffmpeg` binaries in the package so resolution finds a next-to-exe copy (today it falls through to PATH); verify the read-only-bundle → managed-copy path on real `.app`/AppImage builds.

### Clip/Full audio for non-MP4 codecs (solved)
Previously "Save clip" stream-copied audio into MP4, so an **Opus** source (common in
large YouTube downloads) produced `track 1: codec frame size is not set` and a
malformed audio track. Now `transcode()` re-encodes any stream the chosen container
can't hold (Opus→AAC, VP9/AV1→H.264) and copies the rest, so MP4/MOV/MKV all produce
valid files from any source. Covered by `clip_transcodes_opus_audio_into_mp4` and
`full_transcodes_vp9_into_mp4` in `tests/export.rs`.

### Packaging (for distribution)
- Build natively per-OS via a GitHub Actions matrix (don't cross-compile — native FFmpeg/ALSA deps).
- **Static FFmpeg is wired** as the off-by-default `static-ffmpeg` cargo feature (see `Cargo.toml`): it enables `ffmpeg-the-third`'s `build` + `build-lib-{x264,vpx,opus,mp3lame}` + `build-license-gpl`. x264 (no native H.264 encoder exists) makes the binary **GPL** — accepted. Default builds stay dynamic so the no-sudo dev box keeps working; the static build is for CI and **requires `nasm`/`yasm` plus STATIC libx264/libvpx/libopus/libmp3lame** (FFmpeg's configure links them; `build` does not vendor them) and git-clones FFmpeg at build time. Build: `cargo build --release --features static-ffmpeg`.
- Static libav* removes the app's own FFmpeg dependency, but yt-dlp still needs an `ffmpeg` **CLI** at runtime to merge — bundle that binary too (see below).
- Package with `cargo-dist` (recommended) → AppImage (Linux), `.dmg` (macOS), `.exe`/`.msi` (Windows). **No code signing/notarization** (distributing to friends; users will see Gatekeeper/SmartScreen warnings). Bundle `yt-dlp` + `ffmpeg` binaries next to the exe.

#### Linux AppImage (built)
`scripts/make-appimage.sh` produces a single self-contained `yt-dlp-clipper-<ver>-x86_64.AppImage`. It does **not** need the `static-ffmpeg` feature: it builds the normal **dynamic** release, then `linuxdeploy` bundles the app's `libav*`/`libasound` deps into the bundle (`RUNPATH=$ORIGIN/../lib`). It downloads and bundles a **static ffmpeg** (johnvansickle) and the self-contained **`yt-dlp_linux`** (the PyInstaller build — *not* the local python zipapp, which needs python3) into `usr/bin/` next to the exe, where the resolver finds them. Inputs: `packaging/yt-dlp-clipper.desktop` + `assets/yt-dlp-clipper.png`.
- **No sudo to build** — `APPIMAGE_EXTRACT_AND_RUN=1` avoids FUSE for the tooling. `libfuse2` is only needed so the *output* AppImage runs on a double-click (else `--appimage-extract-and-run`).
- Caveats: glibc is **not** bundled, so the AppImage runs on systems with glibc ≥ the build box's (build in an older container for wider reach); `libGL`/`libEGL` are `dlopen`ed and intentionally not bundled (present on every desktop).
</content>
