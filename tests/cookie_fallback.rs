//! `fetch_info_signing_in_if_needed` against a fake yt-dlp that behaves by URL:
//! `/public` fetches without cookies, `/age-restricted…` asks to sign in unless
//! given `--cookies`, and `/missing` fails for a reason cookies can't fix. Each
//! case logs its calls to its own file, one argv per line.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use yt_dlp_clipper::launch_lock;
use yt_dlp_clipper::ytdlp::{self, CookieSource};

const FAKE_YTDLP: &str = r#"#!/bin/sh
for url; do :; done
case_name="${url##*/}"
printf '%s\n' "$*" >> "$(dirname "$0")/$case_name.calls"
case "$case_name" in
    age-restricted*)
        case " $* " in
            *" --cookies "*) ;;
            *) echo "ERROR: [youtube] abc: Sign in to confirm your age. \
                Use --cookies-from-browser or --cookies for the authentication." >&2
               exit 1 ;;
        esac ;;
    missing)
        echo "ERROR: [youtube] abc: Video unavailable. \
            This video has been removed by the uploader" >&2
        exit 1 ;;
esac
printf '{"title": "%s", "formats": []}' "$case_name"
"#;

fn fake_ytdlp_directory() -> &'static Path {
    static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();
    DIRECTORY.get_or_init(|| {
        let directory = std::env::temp_dir().join("yt_dlp_clipper_cookie_fallback");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let fake = directory.join("yt-dlp");
        fs::write(&fake, FAKE_YTDLP).unwrap();
        launch_lock::await_launches_in_progress();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        ytdlp::set_binary(fake);
        directory
    })
}

fn calls_for(case_name: &str) -> Vec<String> {
    let log = fake_ytdlp_directory().join(format!("{case_name}.calls"));
    fs::read_to_string(log).unwrap_or_default().lines().map(str::to_string).collect()
}

fn cookies_file() -> CookieSource {
    CookieSource::File(PathBuf::from("/home/someone/cookies.txt"))
}

fn fetch(case_name: &str, cookies: Option<&CookieSource>) -> anyhow::Result<ytdlp::FetchedVideo> {
    fake_ytdlp_directory();
    ytdlp::fetch_info_signing_in_if_needed(&format!("https://example.com/{case_name}"), cookies)
}

#[test]
fn a_public_video_is_fetched_once_without_cookies() {
    let fetched = fetch("public", Some(&cookies_file())).expect("a public video fetches");

    assert_eq!(fetched.sign_in_cookies, None);
    let calls = calls_for("public");
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(!calls[0].contains("--cookies"), "a public video was sent cookies: {calls:?}");
}

#[test]
fn a_video_that_asks_to_sign_in_is_retried_with_cookies() {
    let fetched = fetch("age-restricted", Some(&cookies_file())).expect("the retry signs in");

    assert_eq!(fetched.sign_in_cookies, Some(cookies_file()));
    let calls = calls_for("age-restricted");
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(!calls[0].contains("--cookies"), "the first attempt sent cookies: {calls:?}");
    assert!(calls[1].contains("--cookies /home/someone/cookies.txt"), "{calls:?}");
}

#[test]
fn signing_in_without_configured_cookies_reports_the_error() {
    let error = fetch("age-restricted-no-cookies", None).err().expect("nothing to sign in with");

    assert!(ytdlp::suggests_cookies(&error.to_string()), "{error}");
    assert_eq!(calls_for("age-restricted-no-cookies").len(), 1);
}

#[test]
fn a_failure_cookies_cannot_fix_is_not_retried() {
    let error = fetch("missing", Some(&cookies_file())).err().expect("the video is gone");

    assert!(error.to_string().contains("Video unavailable"), "{error}");
    assert_eq!(calls_for("missing").len(), 1);
}
