//! Verifies that `download` invokes yt-dlp with `--js-runtimes
//! quickjs:<path>` when a bundled QuickJS binary was resolved, so yt-dlp can
//! solve YouTube's nsig/PO-token challenge without an external `deno`/`node`
//! install. A shim stands in for yt-dlp and records the argv `download`
//! builds — the same code the GUI's Save buttons run.
//!
//! `set_js_runtime` writes a process-global `OnceLock`, so this lives in its
//! own file (own test binary/process) rather than alongside another test that
//! also calls it — see `tests/js_runtime_fetch.rs`.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use yt_dlp_clipper::ytdlp;

#[test]
fn download_passes_js_runtimes_when_qjs_resolved() {
    let dir = std::env::temp_dir().join("yt_dlp_clipper_js_runtime_download_test");
    let _ = fs::create_dir_all(&dir);
    let argv_log = dir.join("argv.txt");
    let _ = fs::remove_file(&argv_log);

    // A fake yt-dlp that dumps each arg on its own line, then exits. It reports
    // no output path, so `download()` returns Err — fine, we only need the argv.
    let shim = dir.join("fake-yt-dlp");
    fs::write(&shim, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {argv_log:?}\nexit 0\n"))
        .unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();

    let qjs = PathBuf::from("/opt/yt-dlp-clipper/qjs-sentinel");
    ytdlp::set_binary(shim);
    ytdlp::set_js_runtime(qjs.clone());

    let cancel = std::sync::atomic::AtomicBool::new(false);
    let _ = ytdlp::download("https://example.com/v", None, None, &dir, &cancel, |_, _| {});

    let recorded = fs::read_to_string(&argv_log).expect("shim should have recorded yt-dlp's argv");
    let args: Vec<&str> = recorded.lines().collect();
    let i = args
        .iter()
        .position(|a| *a == "--js-runtimes")
        .expect("download must pass --js-runtimes when a QuickJS binary was resolved");
    assert_eq!(
        args[i + 1],
        format!("quickjs:{}", qjs.display()),
        "--js-runtimes must carry the resolved QuickJS path"
    );
}
