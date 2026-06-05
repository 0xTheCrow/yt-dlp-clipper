mod common;

use yank::decoder::Decoder;

const FRAME_SECS: f64 = 1.0 / common::FPS;

fn open(path: &std::path::Path) -> Decoder {
    Decoder::open(path.to_str().unwrap()).expect("decoder should open the fixture")
}

#[test]
fn reports_dimensions_and_duration() {
    let dec = open(&common::h264_with_audio());
    assert_eq!(dec.width, common::WIDTH);
    assert_eq!(dec.height, common::HEIGHT);
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
    assert_eq!(img.size, [common::WIDTH as usize, common::HEIGHT as usize]);
    assert_eq!(img.pixels.len(), (common::WIDTH * common::HEIGHT) as usize);
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
    dec.step_backward().expect("previous frame");
    let after = dec.current_secs();
    assert!(after < before, "did not move backward: {before} -> {after}");
    assert!(
        (before - after - FRAME_SECS).abs() < 0.01,
        "stepped back {} not one frame ({FRAME_SECS})",
        before - after
    );
}

#[test]
fn decodes_vp9() {
    let mut dec = open(&common::vp9());
    assert!(dec.step_forward().is_some(), "vp9 first frame should decode");
    assert!(dec.seek_secs(1.5).is_some(), "vp9 seek should decode");
}

#[test]
fn decodes_av1() {
    let mut dec = open(&common::av1());
    assert!(dec.step_forward().is_some(), "av1 first frame should decode");
    assert!(dec.seek_secs(1.5).is_some(), "av1 seek should decode");
}
