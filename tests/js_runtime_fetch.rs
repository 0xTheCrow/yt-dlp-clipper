//! Verifies that `fetch_info` invokes yt-dlp with `--js-runtimes
//! quickjs:<path>` when a bundled QuickJS binary was resolved, so yt-dlp can
//! solve YouTube's nsig/PO-token challenge without an external `deno`/`node`
//! install. A shim stands in for yt-dlp and records the argv `fetch_info`
//! builds — the same code the GUI's Fetch button runs.
//!
//! `set_js_runtime` writes a process-global `OnceLock`, so this lives in its
//! own file (own test binary/process) rather than alongside another test that
//! also calls it — see `tests/js_runtime_download.rs`.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use yt_dlp_clipper::ytdlp;

#[test]
fn fetch_info_passes_js_runtimes_when_qjs_resolved() {
    let dir = std::env::temp_dir().join("yt_dlp_clipper_js_runtime_fetch_test");
    let _ = fs::create_dir_all(&dir);
    let argv_log = dir.join("argv.txt");
    let _ = fs::remove_file(&argv_log);

    // A fake yt-dlp that dumps each arg on its own line, then prints an empty
    // JSON object so `fetch_info`'s parse succeeds.
    let shim = dir.join("fake-yt-dlp");
    fs::write(
        &shim,
        format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {argv_log:?}\necho '{{}}'\n"),
    )
    .unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();

    let qjs = PathBuf::from("/opt/yt-dlp-clipper/qjs-sentinel");
    ytdlp::set_binary(shim);
    ytdlp::set_js_runtime(qjs.clone());

    let _ = ytdlp::fetch_info("https://example.com/v", None);

    let recorded = fs::read_to_string(&argv_log).expect("shim should have recorded yt-dlp's argv");
    let args: Vec<&str> = recorded.lines().collect();
    let i = args
        .iter()
        .position(|a| *a == "--js-runtimes")
        .expect("fetch_info must pass --js-runtimes when a QuickJS binary was resolved");
    assert_eq!(
        args[i + 1],
        format!("quickjs:{}", qjs.display()),
        "--js-runtimes must carry the resolved QuickJS path"
    );
}
