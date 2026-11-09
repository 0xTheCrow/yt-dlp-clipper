# yt-dlp-clipper

A desktop app that downloads videos with
[yt-dlp](https://github.com/yt-dlp/yt-dlp) and trims them frame-accurately.
Save the full video, a clip, or just the audio.

## Download

Get a standalone build from the
[Releases page](https://github.com/0xTheCrow/yt-dlp-clipper/releases).
yt-dlp and ffmpeg come bundled, so there's nothing else to install.

## Build from source

You need Rust, `clang`, `pkg-config`, the ALSA headers, and the FFmpeg dev
libraries (version 4.4–6.x; 7.x won't link). To run the app you also need
`yt-dlp` and `ffmpeg` on your `PATH`.

```sh
# Debian/Ubuntu
sudo apt-get install libavcodec-dev libavformat-dev libavutil-dev libavfilter-dev \
  libavdevice-dev libswscale-dev libswresample-dev clang pkg-config libasound2-dev ffmpeg

cargo run              # launch
cargo run -- <path>    # launch and open a local video
```
