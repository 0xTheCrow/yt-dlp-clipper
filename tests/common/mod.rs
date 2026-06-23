//! Shared test fixtures: small media files generated once via the ffmpeg CLI.
//! Fixtures used by only one test binary live in that test file; this module
//! holds the generator and the inputs more than one binary needs.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// Suppress FFmpeg's internal log output. Pass `--nocapture` to restore it:
/// `cargo test -- --nocapture`
pub fn init() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        let nocapture = std::env::args().any(|a| a == "--nocapture" || a == "--show-output");
        if !nocapture {
            ffmpeg_the_third::util::log::set_level(ffmpeg_the_third::util::log::Level::Quiet);
        }
    });
}

const SIZE: &str = "320x240";
pub const FPS: f64 = 30.0;
pub const DURATION_SECS: f64 = 3.0;

/// Run ffmpeg to produce `name` in the temp dir if it isn't already there.
pub fn generate(name: &str, encode_args: &[&str]) -> PathBuf {
    let path = std::env::temp_dir().join(name);
    if path.exists() {
        return path;
    }
    let lavfi_video = format!("testsrc=duration={DURATION_SECS}:size={SIZE}:rate={FPS}");
    let lavfi_audio = format!("sine=frequency=440:duration={DURATION_SECS}");
    let status = Command::new("ffmpeg")
        .args(["-y", "-f", "lavfi", "-i", &lavfi_video])
        .args(["-f", "lavfi", "-i", &lavfi_audio])
        .args(encode_args)
        .arg(&path)
        .status()
        .expect("ffmpeg must be installed to run these tests");
    assert!(status.success(), "ffmpeg failed to generate {name}");
    path
}

/// H.264 video + AAC audio in mp4.
pub fn h264_with_audio() -> PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        generate(
            "fixture_h264.mp4",
            &["-c:v", "libx264", "-c:a", "aac", "-shortest"],
        )
    })
    .clone()
}

/// VP9 video (no audio) in webm — mirrors a YouTube-style merge codec.
pub fn vp9() -> PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        generate(
            "fixture_vp9.webm",
            &["-map", "0:v", "-c:v", "libvpx-vp9", "-b:v", "200k"],
        )
    })
    .clone()
}
