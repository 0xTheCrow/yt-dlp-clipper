mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use yt_dlp_clipper::export::{export, AudioFormat, ExportSpec, Mode};

/// H.264 video + Opus audio in MKV — mirrors a YouTube download whose Opus
/// audio MP4 can't hold via stream copy.
fn h264_with_opus() -> PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        common::generate(
            "fixture_h264_opus.mkv",
            &["-c:v", "libx264", "-c:a", "libopus", "-shortest"],
        )
    })
    .clone()
}

/// Read a single ffprobe field (one value per line).
fn ffprobe(path: &Path, entries: &str) -> String {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", entries])
        .args(["-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(path)
        .output()
        .expect("ffprobe must be installed to run these tests");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn probe_duration(path: &Path) -> f64 {
    ffprobe(path, "format=duration").trim().parse().unwrap_or(0.0)
}

fn probe_stream_types(path: &Path) -> Vec<String> {
    ffprobe(path, "stream=codec_type")
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn run(mode: Mode, out_name: &str, start: f64, end: f64) -> std::path::PathBuf {
    run_from(common::h264_with_audio(), mode, out_name, start, end)
}

fn run_from(
    input: std::path::PathBuf,
    mode: Mode,
    out_name: &str,
    start: f64,
    end: f64,
) -> std::path::PathBuf {
    let out = std::env::temp_dir().join(out_name);
    let _ = std::fs::remove_file(&out);
    export(&ExportSpec {
        input: input.to_string_lossy().into_owned(),
        output: out.to_string_lossy().into_owned(),
        start_secs: start,
        end_secs: end,
        mode,
        scale_height: None,
    })
    .expect("export should succeed");
    out
}

fn codec_of(path: &std::path::Path, kind: &str) -> String {
    let names = ffprobe(path, "stream=codec_type,codec_name");
    // ffprobe prints codec_type then codec_name per stream, one value per line.
    let lines: Vec<&str> = names.lines().map(|s| s.trim()).collect();
    lines
        .windows(2)
        .find(|w| w[1] == kind || w[0] == kind)
        .map(|w| if w[0] == kind { w[1].to_string() } else { w[0].to_string() })
        .unwrap_or_default()
}

#[test]
fn full_keeps_video_and_audio_for_whole_file() {
    let out = run(Mode::Full, "export_full.mp4", 0.0, common::DURATION_SECS);
    let types = probe_stream_types(&out);
    assert!(types.contains(&"video".to_string()), "missing video: {types:?}");
    assert!(types.contains(&"audio".to_string()), "missing audio: {types:?}");
    assert!(
        (probe_duration(&out) - common::DURATION_SECS).abs() < 0.2,
        "duration {} not near {}",
        probe_duration(&out),
        common::DURATION_SECS
    );
}

#[test]
fn clip_trims_to_window_and_keeps_audio() {
    let out = run(Mode::Clip, "export_clip.mp4", 1.0, 3.0);
    let types = probe_stream_types(&out);
    assert!(types.contains(&"video".to_string()), "missing video: {types:?}");
    assert!(types.contains(&"audio".to_string()), "missing audio: {types:?}");
    let dur = probe_duration(&out);
    assert!((dur - 2.0).abs() < 0.2, "clip duration {dur} not near 2.0");
}

#[test]
fn audio_only_copy_has_no_video_stream() {
    let out = run(Mode::AudioOnly(AudioFormat::Original), "export_audio.m4a", 1.0, 3.0);
    let types = probe_stream_types(&out);
    assert_eq!(types, vec!["audio".to_string()], "expected audio only: {types:?}");
    let dur = probe_duration(&out);
    assert!((dur - 2.0).abs() < 0.2, "audio duration {dur} not near 2.0");
}

#[test]
fn audio_only_mp3_reencodes_to_window() {
    let out = run(Mode::AudioOnly(AudioFormat::Mp3), "export_audio.mp3", 1.0, 3.0);
    let types = probe_stream_types(&out);
    assert_eq!(types, vec!["audio".to_string()], "expected audio only: {types:?}");
    let codec = ffprobe(&out, "stream=codec_name");
    assert!(codec.trim().starts_with("mp3"), "expected mp3, got {codec:?}");
    let dur = probe_duration(&out);
    assert!((dur - 2.0).abs() < 0.3, "audio duration {dur} not near 2.0");
}

#[test]
fn full_into_mkv_copies_streams() {
    let out = run(Mode::Full, "export_full.mkv", 0.0, common::DURATION_SECS);
    assert_eq!(codec_of(&out, "video"), "h264", "video should be copied");
    assert_eq!(codec_of(&out, "audio"), "aac", "audio should be copied");
}

#[test]
fn full_transcodes_vp9_into_mp4() {
    // VP9 can't live in MP4 via copy, so Full must re-encode the video to H.264.
    let out = run_from(common::vp9(), Mode::Full, "export_vp9_full.mp4", 0.0, common::DURATION_SECS);
    assert_eq!(codec_of(&out, "video"), "h264", "vp9 should be transcoded to h264");
}

#[test]
fn clip_transcodes_into_webm() {
    // WebM holds neither H.264 nor AAC, so the clip must become VP9 + Opus.
    let out = run(Mode::Clip, "export_clip.webm", 1.0, 3.0);
    assert_eq!(codec_of(&out, "video"), "vp9", "clip video should be vp9");
    assert_eq!(codec_of(&out, "audio"), "opus", "clip audio should be opus");
    let dur = probe_duration(&out);
    assert!((dur - 2.0).abs() < 0.3, "clip duration {dur} not near 2.0");
}

#[test]
fn clip_transcodes_opus_audio_into_mp4() {
    // MP4 can't hold Opus via copy, so the clip's audio must become AAC.
    let out = run_from(h264_with_opus(), Mode::Clip, "export_opus_clip.mp4", 1.0, 3.0);
    assert_eq!(codec_of(&out, "video"), "h264", "clip video should be h264");
    assert_eq!(codec_of(&out, "audio"), "aac", "opus should be transcoded to aac");
    let dur = probe_duration(&out);
    assert!((dur - 2.0).abs() < 0.3, "clip duration {dur} not near 2.0");
}

/// Duration of the first audio stream, in seconds (sample-accurate trimming
/// makes this match the requested window, unlike packet-granular trimming).
fn audio_stream_duration(path: &std::path::Path) -> f64 {
    ffprobe(path, "stream=codec_type,duration")
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .windows(2)
        .find(|w| w[0] == "audio")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(0.0)
}

#[test]
fn clip_audio_is_sample_accurate() {
    // 0.5..2.0 is exactly 1.5s; with sample-accurate trimming the audio track
    // lands on the window to well within a single packet (~23ms).
    let out = run(Mode::Clip, "acc_clip.mp4", 0.5, 2.0);
    let dur = audio_stream_duration(&out);
    assert!((dur - 1.5).abs() < 0.01, "clip audio duration {dur} not ~1.5");
}

#[test]
fn audio_only_aac_is_sample_accurate() {
    let out = run(Mode::AudioOnly(AudioFormat::Aac), "acc.m4a", 0.5, 2.0);
    let dur = audio_stream_duration(&out);
    assert!((dur - 1.5).abs() < 0.01, "aac duration {dur} not ~1.5");
}

/// Export with a downscale target and return the output's video dimensions.
fn run_scaled(mode: Mode, out_name: &str, height: u32) -> (u32, u32) {
    let input = common::h264_with_audio();
    let out = std::env::temp_dir().join(out_name);
    let _ = std::fs::remove_file(&out);
    export(&ExportSpec {
        input: input.to_string_lossy().into_owned(),
        output: out.to_string_lossy().into_owned(),
        start_secs: 0.5,
        end_secs: 1.5,
        mode,
        scale_height: Some(height),
    })
    .expect("export should succeed");
    let dims = ffprobe(&out, "stream=width,height");
    let nums: Vec<u32> = dims.lines().filter_map(|l| l.trim().parse().ok()).collect();
    (nums[0], nums[1])
}

#[test]
fn clip_downscales_to_target_height() {
    // Fixture is 320x240; 120 lines keeps the 4:3 aspect at 160x120.
    let (w, h) = run_scaled(Mode::Clip, "scaled_clip.mp4", 120);
    assert_eq!((w, h), (160, 120), "clip should downscale to 160x120");
}

#[test]
fn full_downscale_reencodes_from_copyable_source() {
    // h264-in-mp4 would normally remux-copy; a downscale forces a re-encode.
    let (w, h) = run_scaled(Mode::Full, "scaled_full.mp4", 120);
    assert_eq!((w, h), (160, 120), "full should downscale to 160x120");
}
