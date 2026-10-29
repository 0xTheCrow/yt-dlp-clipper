//! Fetches, downloads, decodes, and exports a real YouTube video through the
//! app's own yt-dlp wrapper. Ignored by default: it needs the network, and
//! YouTube answers datacenter IPs (CI runners) with a bot check. Run it before
//! a release with `cargo test --test live_youtube -- --ignored`.
//!
//! yt-dlp resolves from `YTDLP_BIN`, then the app's managed copy, then PATH.
//! QuickJS resolves from `QJS_BIN`, then PATH. ffmpeg and ffprobe come from PATH.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use yt_dlp_clipper::decoder::Decoder;
use yt_dlp_clipper::export::{export, AudioFormat, ExportSpec, Mode};
use yt_dlp_clipper::ytdlp;

const VIDEO_URL: &str = "https://www.youtube.com/watch?v=jNQXAC9IVRw";
const STORAGE_APP_ID: &str = "yt-dlp-clipper";
const DOWNLOAD_HEIGHT_CAP: u32 = 360;
const CLIP_START_SECS: f64 = 5.0;
const CLIP_END_SECS: f64 = 10.0;
const DURATION_TOLERANCE_SECS: f64 = 0.5;
const BACKWARD_STEP_FRAMES: i64 = -3;

struct ScratchDirectory(PathBuf);

impl ScratchDirectory {
    fn create() -> Self {
        let path = std::env::temp_dir()
            .join(format!("yt_dlp_clipper_live_youtube_{}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create scratch directory");
        Self(path)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join(name))
        .find(|path| path.is_file())
}

fn resolve_ytdlp() -> PathBuf {
    let managed_copy = eframe::storage_dir(STORAGE_APP_ID)
        .map(|directory| directory.join("bin").join("yt-dlp"))
        .filter(|path| path.is_file());
    std::env::var_os("YTDLP_BIN")
        .map(PathBuf::from)
        .or(managed_copy)
        .or_else(|| find_in_path("yt-dlp"))
        .expect("yt-dlp not found: set YTDLP_BIN or put yt-dlp on PATH")
}

fn resolve_quickjs() -> PathBuf {
    std::env::var_os("QJS_BIN")
        .map(PathBuf::from)
        .or_else(|| find_in_path("qjs"))
        .expect("QuickJS not found: set QJS_BIN or put qjs on PATH, as the shipped app bundles it")
}

fn ffprobe(path: &Path, arguments: &[&str]) -> String {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-of", "default=noprint_wrappers=1:nokey=1"])
        .args(arguments)
        .arg(path)
        .output()
        .expect("ffprobe must be installed to run this test");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn probe_stream(path: &Path, stream_selector: &str, entry: &str) -> String {
    ffprobe(path, &["-select_streams", stream_selector, "-show_entries", entry])
}

fn probe_duration_secs(path: &Path) -> f64 {
    ffprobe(path, &["-show_entries", "format=duration"]).parse().unwrap_or(0.0)
}

fn export_to(input: &Path, output: &Path, mode: Mode, start_secs: f64, end_secs: f64) {
    export(&ExportSpec {
        input: input.to_string_lossy().into_owned(),
        output: output.to_string_lossy().into_owned(),
        start_secs,
        end_secs,
        mode,
        scale_height: None,
        compatibility_mode: true,
    })
    .unwrap_or_else(|error| panic!("export to {} failed: {error:#}", output.display()));
}

fn assert_duration_near(path: &Path, expected_secs: f64) {
    let actual_secs = probe_duration_secs(path);
    assert!(
        (actual_secs - expected_secs).abs() <= DURATION_TOLERANCE_SECS,
        "{} lasts {actual_secs}s, expected about {expected_secs}s",
        path.display()
    );
}

#[test]
#[ignore]
fn live_youtube_video_fetches_downloads_decodes_and_exports() {
    ffmpeg_the_third::util::log::set_level(ffmpeg_the_third::util::log::Level::Quiet);
    ytdlp::set_binary(resolve_ytdlp());
    ytdlp::set_ffmpeg(find_in_path("ffmpeg").expect("ffmpeg must be on PATH"));
    ytdlp::set_js_runtime(resolve_quickjs());
    let scratch = ScratchDirectory::create();

    let info = ytdlp::fetch_info(VIDEO_URL, None).expect("fetch metadata");
    assert!(!info.title.is_empty(), "metadata has no title");
    assert!(!ytdlp::available_heights(&info).is_empty(), "metadata lists no video heights");

    let selector = ytdlp::resolution_selector(Some(DOWNLOAD_HEIGHT_CAP), true, true);
    let mut progress_report_count = 0;
    let download = ytdlp::download(
        VIDEO_URL, selector.as_deref(), None, &scratch.0, &AtomicBool::new(false),
        |_, _| progress_report_count += 1,
    )
    .expect("download");
    assert!(download.is_file(), "reported download path {} is missing", download.display());
    assert!(progress_report_count > 0, "download reported no progress");
    let downloaded_height: u32 =
        probe_stream(&download, "v:0", "stream=height").parse().unwrap_or(0);
    assert!(
        downloaded_height > 0 && downloaded_height <= DOWNLOAD_HEIGHT_CAP,
        "downloaded height {downloaded_height} is outside 1..={DOWNLOAD_HEIGHT_CAP}"
    );
    let downloaded_audio_codec = probe_stream(&download, "a:0", "stream=codec_name");
    assert!(!downloaded_audio_codec.is_empty(), "download has no audio");

    let mut decoder = Decoder::open(download.to_str().unwrap()).expect("open download");
    let duration_secs = decoder.duration_secs();
    assert!(duration_secs > CLIP_END_SECS, "video is only {duration_secs}s long");
    let frame_secs = 1.0 / decoder.fps();
    decoder.step_forward().expect("first frame");
    let seek_target_secs = duration_secs / 2.0;
    decoder.seek_secs(seek_target_secs).expect("frame at seek target");
    let landed_secs = decoder.current_secs();
    assert!(
        (landed_secs - seek_target_secs).abs() < frame_secs,
        "seek to {seek_target_secs}s landed at {landed_secs}s"
    );
    decoder.step_by(BACKWARD_STEP_FRAMES).expect("frame after stepping backward");
    let stepped_frames = (landed_secs - decoder.current_secs()) / frame_secs;
    assert!(
        (stepped_frames + BACKWARD_STEP_FRAMES as f64).abs() < 0.5,
        "stepping {BACKWARD_STEP_FRAMES} frames moved {stepped_frames} frames back"
    );

    let full = scratch.file("full.mp4");
    export_to(&download, &full, Mode::Full, 0.0, duration_secs);
    assert_eq!(probe_stream(&full, "v:0", "stream=codec_name"), "h264");
    assert_eq!(probe_stream(&full, "a:0", "stream=codec_name"), "aac");
    assert_duration_near(&full, duration_secs);

    let clip = scratch.file("clip.mp4");
    export_to(&download, &clip, Mode::Clip, CLIP_START_SECS, CLIP_END_SECS);
    assert_eq!(probe_stream(&clip, "v:0", "stream=codec_name"), "h264");
    assert_duration_near(&clip, CLIP_END_SECS - CLIP_START_SECS);

    let audio = scratch.file("audio.mp3");
    let audio_only = Mode::AudioOnly(AudioFormat::Mp3);
    export_to(&download, &audio, audio_only, CLIP_START_SECS, CLIP_END_SECS);
    assert_eq!(probe_stream(&audio, "a:0", "stream=codec_name"), "mp3");
    assert!(probe_stream(&audio, "v", "stream=codec_name").is_empty(), "audio export has video");
    assert_duration_near(&audio, CLIP_END_SECS - CLIP_START_SECS);
}
