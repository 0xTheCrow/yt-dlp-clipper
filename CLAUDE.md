# yt-dlp-clipper

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
  - `export` — Full and Clip both route through one `transcode()` that, per stream, **copies when the chosen container can hold the codec and re-encodes when it can't**, targeting the container's native codecs (H.264/AAC for MP4·MOV·MKV, VP9/Opus for WebM); `video_fits`/`audio_fits` + `container_kind` + `audio_encode_codec` decide. `ExportSpec.compatibility_mode` (on by default; GUI "Compatible" toggle by the Video format dropdown) narrows the MP4/MOV copy rules so a saved file plays on phones/TVs, not just computers: `video_fits` allows only H.264 (HEVC/AV1/MPEG-4 re-encode), `audio_fits` only AAC/MP3 (AC-3/ALAC re-encode), and `video_copyable`'s `pix_copy_safe` guard forces a re-encode of non-8-bit-4:2:0 H.264 (e.g. 10-bit/HDR). With it off, any codec/pixel format the container can hold is copied as-is. It doesn't affect MKV/WebM (inherently non-iOS) or audio-only exports. Full additionally short-circuits to a pure-copy `remux_copy` when every stream already fits. Clip always re-encodes video for a frame-accurate `in`, **and always re-encodes audio** so the cut is sample-accurate (a stream copy can't split a packet, so an exact `in`/`out` needs a re-encode); Full copies audio when it fits. AudioOnly re-encodes to MP3/AAC, or `Original` lossless stream-copy into a container that fits the source codec (packet-granular, since copies can't split a packet). Codec-tag reset on copy; x264 opened with a preset + `qmin/qmax/me_range/gop` unset so it accepts the settings. Clip video has a frame-count backstop so sources with missing frame PTS can't re-encode to EOF, and a clip keeps reading audio past the video's end so interleave lag can't truncate the tail. Audio re-encode (`AudioReenc`) runs `abuffer→atrim→abuffersink` (sink chunked to the encoder frame size); the `atrim` window is computed in samples from the first decoded frame's PTS, trimming the cut to the exact sample, and output PTS are stamped from a running sample counter to stay monotonic. The GUI's Video/Audio format dropdowns (`VideoFormat`, `AudioFormat`) pick the output container. `ExportSpec.scale_height` optionally **downscales** the saved video (aspect-preserving, even dims via `scaled_dims`, never upscales); since a copy can't scale, a downscale forces the re-encode path (incl. routing Full away from `remux_copy`).
  - `ytdlp` — wraps the `yt-dlp` binary (fetch `-J` metadata, download with progress, size estimate). `available_heights` lists the source's distinct video heights (tallest first) and `resolution_selector` builds the `-f` expression (`bestvideo[height<=H]+bestaudio/…`) for the GUI's download Resolution menu; both cap at what the source offers.
  - `audio` — cpal output; decodes+resamples the audio stream on a worker thread; `clock_secs()` is the A/V **master clock**.
- **bin (`src/main.rs`)** — egui/eframe GUI. Layout: top toolbar (URL/fetch/open/from-cache/settings), central video preview (aspect-fit, fills width / full height for portrait), bottom clip controls (timeline, transport, Start/End, save).
  - **Decoding runs on a background thread** (`DecoderHandle`): the UI sends seek/step requests (coalesced to the latest) and receives frames + metadata; it never blocks on a decode. Seeks are **generation-tagged** so superseded mid-drag decodes are dropped and the playhead pins to the released spot.
  - **Playback**: video chases the master clock (audio if available, else wall time).
  - **Keyboard shortcuts** (`Keybinds`/`Bind`): set-start (S), set-end (E), play/pause (Space) by default, all **rebindable** in Settings (click the action, press a new key; Esc cancels; `rebind` swaps to keep keys distinct). Persisted by key *name* (so it doesn't need egui's serde feature). The Set Start/End buttons show their key; the main shortcut handler is suppressed while a text field has focus or Settings is capturing a key.
  - Settings persist via eframe storage (UI scale, volume, download dir, delete-cache-on-exit). Downloads go to a managed cache dir (`eframe::storage_dir("yt-dlp-clipper")/downloads`).
  - **Button icons** are SVGs in `assets/` (`save.svg`, `download.svg`, `settings.svg`), embedded via `egui::include_image!` and rasterized by `egui_extras` (svg feature, resvg) after `install_image_loaders` at startup; `icon_button()` builds an icon+text button, sizing the icon to the button font height so it lines up with the caption. Unicode glyphs render as missing-glyph boxes in egui's bundled fonts, hence SVGs.

## Conventions

- **No magic numbers** — hoist literals to named `const`s, or use a library constant (e.g. `ffmpeg::ffi::AV_TIME_BASE`).
- **Comments describe what the code does**, not why something *wasn't* done; drop dead code rather than commenting around it.
- **No narrative comments** — a comment must stand on its own to a reader with zero history of how the code came to be. Don't narrate the development process or refer to it: no "the collapse happens a frame before the menu opens", no "we tried X but", no temporal/conversational framing ("now", "still", "previously", "as noted above"). State the rule the code follows and the invariant it relies on, in the present tense, as if the code had always been this way. E.g. prefer "A pointer press collapses the selection; restore it so the menu acts on it" over "egui clobbers the selection on press a frame before the menu opens, so grab it first".
- **Avoid adding a crate for one small thing** — prefer std or an existing dep (e.g. `eframe::storage_dir` instead of `dirs`). New deps are fine for genuine core needs (cpal for audio, ffmpeg-the-third).
- This machine has no usable `sudo`; system packages must be installed by the user.

## Planned / TODO

### Frame-step forward optimization (not built)
`decoder::step_by`'s forward branch loops `step_forward()` for a net-positive step count, and each call runs `to_image()` to build a full `egui::ColorImage` that's then discarded for all but the last frame. Not a leak (intermediates are freed) and `n` is usually 1–2, but for a large net-forward jump it's wasted decode→convert churn. Optimization: decode *through* the intermediate frames without converting (only `to_image` the final one). Low priority.

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
- **CI is wired** in `.github/workflows/release.yml`: a `v*` tag builds all three bundles and publishes a GitHub release with them attached; `workflow_dispatch` builds them on demand and leaves them in the run's Artifacts (no release). Linux, Windows, and macOS each run on their own native runner (`ubuntu-22.04`, `windows-latest`, `macos-14`). All runners are free for this public repo (no minute cap).
- **Static FFmpeg is wired** as the off-by-default `static-ffmpeg` cargo feature (see `Cargo.toml`): it enables `ffmpeg-the-third`'s `build` + `build-lib-{x264,vpx,opus,mp3lame}` + `build-license-gpl`. x264 (no native H.264 encoder exists) makes the binary **GPL** — accepted. Default builds stay dynamic so the no-sudo dev box keeps working; the static build is for CI and **requires `nasm`/`yasm` plus STATIC libx264/libvpx/libopus/libmp3lame** (FFmpeg's configure links them; `build` does not vendor them) and git-clones FFmpeg at build time. Build: `cargo build --release --features static-ffmpeg`.
- Static libav* removes the app's own FFmpeg dependency, but yt-dlp still needs an `ffmpeg` **CLI** at runtime to merge — bundle that binary too (see below).
- The **`bundle-tools`** cargo feature embeds `yt-dlp` and the `ffmpeg` CLI binary into the exe at compile time and extracts them to a per-user managed dir on first run. Used by the Windows CI job to produce a single `.exe`. It enables `ffmpeg-the-third/static` (link libav* statically) but does **not** imply `static-ffmpeg` — `static-ffmpeg` runs `./configure` via the Rust build script, which works on Linux/macOS but fails on Windows (the build script runs as a Windows process that can't exec shell scripts). Instead, the CI pre-builds FFmpeg 6.0 statically in an MSYS2 shell (where configure works), then sets `FFMPEG_DIR` and `CODEC_LIB_DIR` for the Rust build. Requires `BUNDLE_YTDLP`, `BUNDLE_FFMPEG_CLI`, `FFMPEG_DIR`, and `CODEC_LIB_DIR` at build time.
- Packaging is hand-rolled per-target scripts (`scripts/make-{appimage,windows,macos}.sh`) rather than `cargo-dist` → AppImage (Linux), single `.exe` (Windows), `.dmg` (macOS). **No code signing/notarization** (distributing to friends; users will see Gatekeeper/SmartScreen warnings).

#### Linux AppImage (built)
`scripts/make-appimage.sh` produces a single self-contained `yt-dlp-clipper-<ver>-x86_64.AppImage`. It does **not** need the `static-ffmpeg` feature: it builds the normal **dynamic** release, then `linuxdeploy` bundles the app's `libav*`/`libasound` deps into the bundle (`RUNPATH=$ORIGIN/../lib`). It downloads and bundles a **static ffmpeg** (BtbN, GitHub-hosted — johnvansickle 415s the CI runner) and the self-contained **`yt-dlp_linux`** (the PyInstaller build — *not* the local python zipapp, which needs python3) into `usr/bin/` next to the exe, where the resolver finds them. Inputs: `packaging/yt-dlp-clipper.desktop` + `assets/yt-dlp-clipper.png`.
- **No sudo to build** — `APPIMAGE_EXTRACT_AND_RUN=1` avoids FUSE for the tooling. `libfuse2` is only needed so the *output* AppImage runs on a double-click (else `--appimage-extract-and-run`).
- Caveats: glibc is **not** bundled, so the AppImage runs on systems with glibc ≥ the build box's (build in an older container for wider reach); `libGL`/`libEGL` are `dlopen`ed and intentionally not bundled (present on every desktop).

#### Windows single exe (built natively on Windows in CI with MSVC + vcpkg)
The CI (`windows-2022` runner) builds a single self-contained `yt-dlp-clipper-<ver>-win64.exe`; whole binary is GPL (x264). The `bundle-tools` feature bakes `yt-dlp.exe` and a static `ffmpeg.exe` CLI (gyan `full_build`, no DLL deps) into the binary at compile time; static libav* links mean no DLL deps from the app either. The result is a truly standalone exe. First run extracts the embedded tool binaries to the per-user managed dir (`storage_dir/bin/`), where they persist and can be updated by `yt-dlp -U`.
- **CI build**: static FFmpeg + codecs come from **vcpkg** (`ffmpeg[avcodec,avformat,avfilter,swscale,swresample,gpl,x264,vpx,dav1d,opus,mp3lame]:x64-windows-static`, cached via `VCPKG_DEFAULT_BINARY_CACHE`). `dav1d` is the software **AV1 decoder** — without it the only AV1 decoder is FFmpeg's native HW-only one, which can't software-decode YouTube's AV1 streams and spams "Missing Sequence Header". `ffmpeg-sys-the-third`'s **vcpkg path** (`vcpkg::find_package("ffmpeg")`, selected because no `FFMPEG_DIR` is set and the target is MSVC) links libav* + the codec deps and, for static builds, **emits the Windows system libs itself** (`ole32`, `secur32`, `ws2_32`, `bcrypt`, `user32`) — so `build.rs` needs nothing Windows-specific (it only embeds the tool binaries). `VCPKGRS_TRIPLET=x64-windows-static` points the vcpkg crate at the static triplet; `-C target-feature=+crt-static` links the CRT statically (matching the triplet) so the exe needs no VCRedist. `LIBCLANG_PATH` points at the runner's LLVM for `ffmpeg-sys-the-third`'s bindgen pass. The app also drops the default `device`/avdevice ffmpeg-the-third feature (nothing opens a capture device).
- **Why MSVC + vcpkg, not MinGW cross-compile**: the prior approach cross-compiled a hand-built static FFmpeg from Linux with MinGW, which hit GNU `ld` single-pass link-ordering, Ubuntu-mingw `_FORTIFY_SOURCE`/`libssp`, and a hand-maintained transitive-dep list — all toolchain artifacts, not app problems. MSVC's linker resolves order-independently, has none of the fortify/libssp issues, and vcpkg supplies static FFmpeg with correct dependency metadata, so the whole class of failures disappears.
- `scripts/make-windows.sh` — **cross-compiles from Linux** (dev convenience, produces a zip bundle, not a single exe) for `x86_64-pc-windows-msvc` via `cargo-xwin`. Uses gyan 6.0 shared build DLLs. Prereqs: `rustup target add x86_64-pc-windows-msvc`, `cargo install cargo-xwin`, and (sudo) `lld llvm clang p7zip-full zip`.

#### macOS .dmg (built — must run on a Mac)
`scripts/make-macos.sh` produces a self-contained `Cooper Clipper.app` and a `yt-dlp-clipper-<ver>-macos-<arch>.dmg` for the **host arch** (arm64 or x86_64 — no universal binary; the CI matrix covers both). It can't be cross-compiled from Linux the way Windows is: Apple's SDK isn't freely redistributable (so there's no cargo-xwin equivalent), and `cpal` links CoreAudio from that SDK. Like the AppImage, it builds the normal **dynamic** release and uses **`dylibbundler`** (the macOS counterpart to `linuxdeploy`) to copy the app's `libav*` dylibs into `Contents/libs` and rewrite their install names to `@executable_path/../libs`. A static `ffmpeg` CLI (osxexperts) + the self-contained `yt-dlp_macos` are staged in `Contents/MacOS/` next to the exe, where the resolver finds them. The `.icns` is generated from `assets/yt-dlp-clipper.png` (`sips`+`iconutil`); a minimal `Info.plist` is emitted inline.
- **Build against `ffmpeg@6`**, not brew's default 7.x (same 6.0 pin reason as Windows): the script sets `PKG_CONFIG_PATH` to `brew --prefix ffmpeg@6`. Prereqs (no sudo, just brew): `brew install ffmpeg@6 pkg-config dylibbundler` + Xcode command-line tools.
- **Unsigned/unnotarized**, so first launch needs right-click → Open (or `xattr -dr com.apple.quarantine` the `.app`).
</content>
