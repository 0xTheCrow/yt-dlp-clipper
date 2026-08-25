mod common;

use std::path::PathBuf;
use std::sync::OnceLock;
use yt_dlp_clipper::decoder::{audio_duration_secs, Decoder};

const FRAME_SECS: f64 = 1.0 / common::FPS;
const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;

fn open(path: &std::path::Path) -> Decoder {
    common::init();
    Decoder::open(path.to_str().unwrap()).expect("decoder should open the fixture")
}

/// AV1 video (no audio) in mp4 — the codec YouTube most often serves.
fn av1() -> PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        common::generate(
            "fixture_av1.mp4",
            &["-map", "0:v", "-c:v", "libaom-av1", "-cpu-used", "8", "-b:v", "200k"],
        )
    })
    .clone()
}

#[test]
fn reports_dimensions_and_duration() {
    let dec = open(&common::h264_with_audio());
    assert_eq!(dec.width, WIDTH);
    assert_eq!(dec.height, HEIGHT);
    assert!(
        (dec.duration_secs() - common::DURATION_SECS).abs() < 0.2,
        "duration {} not near {}",
        dec.duration_secs(),
        common::DURATION_SECS
    );
}

#[test]
fn step_forward_yields_full_frames() {
    let mut dec = open(&common::h264_with_audio());
    let img = dec.step_forward().expect("first frame");
    assert_eq!(img.size, [WIDTH as usize, HEIGHT as usize]);
    assert_eq!(img.pixels.len(), (WIDTH * HEIGHT) as usize);
}

#[test]
fn step_forward_advances_exactly_one_frame() {
    let mut dec = open(&common::h264_with_audio());
    dec.step_forward().expect("frame 0");
    let t0 = dec.current_secs();
    dec.step_forward().expect("frame 1");
    let dt = dec.current_secs() - t0;
    assert!((dt - FRAME_SECS).abs() < 0.005, "frame delta {dt} != {FRAME_SECS}");
}

#[test]
fn seek_lands_near_target() {
    let mut dec = open(&common::h264_with_audio());
    dec.seek_secs(2.0).expect("seek frame");
    assert!((dec.current_secs() - 2.0).abs() < FRAME_SECS);
}

#[test]
fn step_backward_goes_back_one_frame() {
    let mut dec = open(&common::h264_with_audio());
    dec.seek_secs(2.0).expect("seek frame");
    let before = dec.current_secs();
    dec.step_by(-1).expect("previous frame");
    let after = dec.current_secs();
    assert!(after < before, "did not move backward: {before} -> {after}");
    assert!(
        (before - after - FRAME_SECS).abs() < 0.01,
        "stepped back {} not one frame ({FRAME_SECS})",
        before - after
    );
}

/// VP9 in webm (like the vp9() fixture below) has a single keyframe for the
/// whole clip, so backward stepping repeatedly re-decodes from frame 0.
/// Nominal frame-duration arithmetic drifts against the real, rounded pts
/// spacing on this codec, which used to strand the playhead on the second
/// step back: the recomputed target landed back on the same current frame
/// instead of the true previous one, and every further step repeated that.
#[test]
fn repeated_step_backward_keeps_moving_on_sparse_keyframes() {
    let mut dec = open(&common::vp9());
    dec.seek_secs(2.9).expect("seek frame");
    let mut before = dec.current_secs();
    for i in 0..5 {
        let img = dec.step_by(-1);
        let after = dec.current_secs();
        assert!(img.is_some(), "step {i} produced no frame");
        assert!(after < before, "step {i} got stuck: {before} -> {after}");
        before = after;
    }
}

/// A burst of rapid backward taps coalesces (see `decoder_thread`) into one
/// large `step_by(-n)`. If `n` crosses more keyframes than the one nearest
/// the current position holds frames for, the search must keep walking to
/// earlier keyframes rather than clamping to the start of that one GOP.
#[test]
fn large_backward_jump_crosses_keyframe_boundaries() {
    let mut dec = open(&common::h264_short_gop());
    dec.seek_secs(2.5).expect("seek frame");
    let before = dec.current_secs();
    dec.step_by(-20).expect("frame 20 steps back");
    let after = dec.current_secs();
    let expected = before - 20.0 * FRAME_SECS;
    assert!(
        (after - expected).abs() < 0.05,
        "expected near {expected:.4} (20 frames back from {before:.4}), got {after:.4}"
    );
}

/// A backward jump larger than the whole video must clamp to the first
/// frame, not error or hang walking past the start.
#[test]
fn backward_jump_beyond_video_length_clamps_to_start() {
    let mut dec = open(&common::vp9());
    dec.seek_secs(1.0).expect("seek frame");
    dec.step_by(-1000).expect("clamped first frame");
    assert!(dec.current_secs().abs() < 0.05, "expected to clamp to 0, got {}", dec.current_secs());
}

#[test]
fn decodes_vp9() {
    let mut dec = open(&common::vp9());
    assert!(dec.step_forward().is_some(), "vp9 first frame should decode");
    assert!(dec.seek_secs(1.5).is_some(), "vp9 seek should decode");
}

#[test]
fn decodes_av1() {
    let mut dec = open(&av1());
    assert!(dec.step_forward().is_some(), "av1 first frame should decode");
    assert!(dec.seek_secs(1.5).is_some(), "av1 seek should decode");
}

#[test]
fn open_rejects_a_source_with_no_video_stream() {
    common::init();
    for input in [common::opus_audio_only(), common::aac_audio_only()] {
        assert!(
            Decoder::open(input.to_str().unwrap()).is_err(),
            "{} has no video stream, so the video decoder must not open it",
            input.display()
        );
    }
}

#[test]
fn audio_duration_matches_the_source_without_a_stream_duration() {
    common::init();
    let secs = audio_duration_secs(common::opus_audio_only().to_str().unwrap())
        .expect("webm audio-only duration");
    assert!(
        (secs - common::DURATION_SECS).abs() < 0.2,
        "duration {secs} not near {}",
        common::DURATION_SECS
    );
}

#[test]
fn audio_duration_matches_the_source_carrying_a_stream_duration() {
    common::init();
    let secs = audio_duration_secs(common::aac_audio_only().to_str().unwrap())
        .expect("m4a audio-only duration");
    assert!(
        (secs - common::DURATION_SECS).abs() < 0.2,
        "duration {secs} not near {}",
        common::DURATION_SECS
    );
}

/// The decoder thread only reports a source as audio-only when this succeeds,
/// so a video-only file must fall through to the real decode error instead.
#[test]
fn audio_duration_errors_without_an_audio_stream() {
    common::init();
    assert!(audio_duration_secs(common::vp9().to_str().unwrap()).is_err());
}
