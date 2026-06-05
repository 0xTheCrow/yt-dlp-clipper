//! Verifies that a download invokes yt-dlp with `--ffmpeg-location` pointing at
//! the resolved ffmpeg, so yt-dlp merges with exactly the binary we shipped and
//! never searches PATH/CWD for one. A shim stands in for yt-dlp and records the
//! argv that the real `download()` builds — the same code the GUI's button runs.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use yank::ytdlp;

#[test]
fn download_passes_ffmpeg_location() {
    let dir = std::env::temp_dir().join("yank_ffmpeg_loc_test");
    let _ = fs::create_dir_all(&dir);
    let argv_log = dir.join("argv.txt");
    let _ = fs::remove_file(&argv_log);

    // A fake yt-dlp that dumps each arg on its own line, then exits. It reports
    // no output path, so `download()` returns Err — fine, we only need the argv.
    let shim = dir.join("fake-yt-dlp");
    fs::write(
        &shim,
        format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {:?}\nexit 0\n", argv_log),
    )
    .unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();

    let ffmpeg = PathBuf::from("/opt/yank/ffmpeg-sentinel");
    ytdlp::set_binary(shim);
    ytdlp::set_ffmpeg(ffmpeg.clone());

    let _ = ytdlp::download("https://example.com/v", Some("bestvideo+bestaudio"), &dir, |_, _| {});

    let recorded = fs::read_to_string(&argv_log).expect("shim should have recorded yt-dlp's argv");
    let args: Vec<&str> = recorded.lines().collect();
    let i = args
        .iter()
        .position(|a| *a == "--ffmpeg-location")
        .expect("download must pass --ffmpeg-location");
    assert_eq!(
        args[i + 1],
        ffmpeg.to_str().unwrap(),
        "--ffmpeg-location must carry the resolved ffmpeg path"
    );
}
