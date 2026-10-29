//! Self-update against a local stand-in for GitHub: a plain-HTTP server serving a
//! release feed, its `latest.json`, and the build files, so the real check,
//! download, probe, and swap code runs end to end without the network.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use yt_dlp_clipper::updater::{check_from, stage};

const RUNNING_VERSION: &str = "0.1.0";
const RELEASE_TAG: &str = "v99.0.0";
const PASSING_SELF_TEST: &[u8] = b"#!/bin/sh\nexit 0\n";

struct ReleaseServer {
    base_url: String,
    routes: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

impl ReleaseServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        let base_url = format!("http://{}", listener.local_addr().expect("local address"));
        let routes: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::default();
        let served_routes = Arc::clone(&routes);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                respond(stream, &served_routes);
            }
        });
        Self { base_url, routes }
    }

    fn serve(&self, path: &str, body: impl Into<Vec<u8>>) -> String {
        self.routes.lock().unwrap().insert(path.to_string(), body.into());
        format!("{}{path}", self.base_url)
    }
}

fn respond(stream: TcpStream, routes: &Mutex<HashMap<String, Vec<u8>>>) {
    let mut reader = BufReader::new(&stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut header_line = String::new();
    while reader.read_line(&mut header_line).is_ok_and(|length| length > 0)
        && !header_line.trim().is_empty()
    {
        header_line.clear();
    }
    let path = request_line.split_whitespace().nth(1).unwrap_or_default();
    let body = routes.lock().unwrap().get(path).cloned();
    let mut writer = &stream;
    let _ = match body {
        Some(body) => write!(
            writer,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .and_then(|()| writer.write_all(&body)),
        None => writer.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n"),
    };
}

fn scratch_directory(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("create scratch directory");
    directory
}

#[test]
fn a_download_that_does_not_match_its_checksum_is_rejected_and_deleted() {
    let server = ReleaseServer::start();
    let url = server.serve("/payload", b"a tampered build".to_vec());
    let destination = scratch_directory("yt_dlp_clipper_update_mismatch").join("payload");
    let published_sha256 = "0".repeat(64);

    let result = stage::download_to(&url, &destination, &published_sha256, |_, _| {});

    assert!(result.is_err(), "a payload with the wrong checksum was accepted");
    assert!(!destination.exists(), "the rejected payload was left on disk");
}

#[test]
fn a_newer_release_without_a_manifest_is_an_error() {
    let server = ReleaseServer::start();
    let build_url = server.serve("/files/build", PASSING_SELF_TEST);
    let release = serde_json::json!({
        "tag_name": RELEASE_TAG,
        "body": "",
        "html_url": server.serve("/page", Vec::new()),
        "assets": [{ "name": "cooper-clipper-x86_64.AppImage", "browser_download_url": build_url }],
    });
    let feed_url = server.serve("/releases/latest", release.to_string());

    assert!(check_from(&feed_url, RUNNING_VERSION).is_err());
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use yt_dlp_clipper::updater::stage::PendingSwap;
    use yt_dlp_clipper::updater::{probe, swap};

    #[test]
    fn a_published_release_is_found_verified_probed_and_installed() {
        let directory = scratch_directory("yt_dlp_clipper_update_end_to_end");
        let release_directory = directory.join("release");
        std::fs::create_dir_all(&release_directory).unwrap();
        let builds: [(&str, &[u8]); 4] = [
            ("cooper-clipper-x86_64.AppImage", PASSING_SELF_TEST),
            ("cooper-clipper-99.0.0-win64.exe", b"windows build"),
            ("cooper-clipper-99.0.0-macos-arm64.zip", b"macos update"),
            ("cooper-clipper-99.0.0-macos-arm64.dmg", b"macos installer"),
        ];
        for (name, contents) in builds {
            std::fs::write(release_directory.join(name), contents).unwrap();
        }

        let manifest_path = directory.join("latest.json");
        let release_glob =
            |extension: &str| format!("{}/*.{extension}", release_directory.display());
        let platform_key = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
        let manifest_script =
            concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/make-release-manifest.sh");
        let status = Command::new(manifest_script)
            .arg(&manifest_path)
            .args([platform_key.as_str(), &release_glob("AppImage")])
            .args(["windows-x86_64", &release_glob("exe")])
            .args(["macos-aarch64", &release_glob("zip")])
            .status()
            .expect("run make-release-manifest.sh");
        assert!(status.success(), "make-release-manifest.sh failed");

        let server = ReleaseServer::start();
        let mut assets = Vec::new();
        for (name, contents) in builds {
            let url = server.serve(&format!("/files/{name}"), contents);
            assets.push(serde_json::json!({ "name": name, "browser_download_url": url }));
        }
        let manifest_url =
            server.serve("/files/latest.json", std::fs::read(&manifest_path).unwrap());
        assets.push(
            serde_json::json!({ "name": "latest.json", "browser_download_url": manifest_url }),
        );
        let release = serde_json::json!({
            "tag_name": RELEASE_TAG,
            "body": "",
            "html_url": server.serve("/page", Vec::new()),
            "assets": assets,
        });
        let feed_url = server.serve("/releases/latest", release.to_string());

        let available = check_from(&feed_url, RUNNING_VERSION)
            .expect("check the release feed")
            .expect("a newer release is offered");
        let staged = directory.join("staged.AppImage");
        stage::download_to(&available.asset_url, &staged, &available.sha256, |_, _| {})
            .expect("download and verify this platform's build");
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            probe::self_test(&staged, probe::SELF_TEST_TIMEOUT).unwrap(),
            "the downloaded build failed its self-test"
        );

        let installed = directory.join("installed.AppImage");
        std::fs::write(&installed, b"the running build").unwrap();
        let pending = PendingSwap {
            target: installed.clone(),
            backup: directory.join("backup.AppImage"),
            staged,
            from_version: RUNNING_VERSION.into(),
            to_version: available.version,
            launch_attempts: 0,
        };
        swap::apply(&pending).expect("install the update");

        assert_eq!(std::fs::read(&installed).unwrap(), PASSING_SELF_TEST);
        assert_eq!(std::fs::read(&pending.backup).unwrap(), b"the running build");
    }

}
