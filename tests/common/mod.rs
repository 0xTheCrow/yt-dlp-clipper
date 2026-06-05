//! Shared test fixtures: small media files generated once via the ffmpeg CLI.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

const SIZE: &str = "320x240";
pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;
pub const FPS: f64 = 30.0;
pub const DURATION_SECS: f64 = 3.0;

/// Run ffmpeg to produce `name` in the temp dir if it isn't already there.
fn generate(name: &str, encode_args: &[&str]) -> PathBuf {
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

/// H.264 video + Opus audio in MKV — mirrors a YouTube download whose Opus
/// audio MP4 can't hold via stream copy.
pub fn h264_with_opus() -> PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        generate(
            "fixture_h264_opus.mkv",
            &["-c:v", "libx264", "-c:a", "libopus", "-shortest"],
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

/// AV1 video (no audio) in mp4 — the codec YouTube most often serves.
pub fn av1() -> PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        generate(
            "fixture_av1.mp4",
            &["-map", "0:v", "-c:v", "libaom-av1", "-cpu-used", "8", "-b:v", "200k"],
        )
    })
    .clone()
}

/// Read a single ffprobe field (one value per line).
pub fn ffprobe(path: &std::path::Path, entries: &str) -> String {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", entries])
        .args(["-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(path)
        .output()
        .expect("ffprobe must be installed to run these tests");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn probe_duration(path: &std::path::Path) -> f64 {
    ffprobe(path, "format=duration").trim().parse().unwrap_or(0.0)
}

pub fn probe_stream_types(path: &std::path::Path) -> Vec<String> {
    ffprobe(path, "stream=codec_type")
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
