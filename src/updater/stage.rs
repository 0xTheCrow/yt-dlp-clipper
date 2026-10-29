//! Streams a release asset to disk, and the on-disk record of an in-progress
//! swap (`update-state.json`) that survives a restart so the commit/rollback
//! protocol can pick up where a crash or a fresh launch left off.
//!
//! Every path here is a caller-supplied argument, not something this module
//! resolves itself — the bin crate owns the app's storage layout (`storage_dir`
//! from `eframe`), the same way `ytdlp::download` takes its destination `dir`
//! rather than computing one.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use ureq::Agent;

/// GitHub answers API requests that omit a User-Agent with 403; release assets
/// are served straight from GitHub's own CDN, which is equally strict.
const DOWNLOAD_USER_AGENT: &str = concat!("yt-dlp-clipper/", env!("CARGO_PKG_VERSION"));
const DOWNLOAD_REQUEST_TIMEOUT_SECS: u64 = 300;

/// After this many launches of the swapped-in build without a clean commit, the
/// backup is restored and the app relaunches on the old build instead.
pub const UPDATE_LAUNCH_ATTEMPT_LIMIT: u32 = 2;

/// Chunk size for the download's read/write loop; large enough to keep syscall
/// overhead low, small enough that `on_progress` still reports smoothly.
const DOWNLOAD_CHUNK_BYTES: usize = 64 * 1024;

/// Download `url` to `dest`, reporting `(downloaded, total)` bytes as it goes.
/// `total` is `0` when the server doesn't report `Content-Length`, matching
/// `ytdlp::download`'s progress-callback convention.
pub fn download_to(url: &str, dest: &Path, mut on_progress: impl FnMut(u64, u64)) -> Result<()> {
    let agent: Agent = Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(DOWNLOAD_REQUEST_TIMEOUT_SECS)))
        .build()
        .into();
    let mut response = agent
        .get(url)
        .header("User-Agent", DOWNLOAD_USER_AGENT)
        .call()
        .with_context(|| format!("downloading {url}"))?;
    let total = response.body().content_length().unwrap_or(0);

    let mut file = File::create(dest)
        .with_context(|| format!("creating {}", dest.display()))?;
    let mut reader = response.body_mut().as_reader();
    let mut buf = [0u8; DOWNLOAD_CHUNK_BYTES];
    let mut downloaded = 0u64;
    loop {
        let read = reader.read(&mut buf).context("reading the update download")?;
        if read == 0 {
            break;
        }
        file.write_all(&buf[..read]).context("writing the update download to disk")?;
        downloaded += read as u64;
        on_progress(downloaded, total);
    }
    Ok(())
}

/// Record of an in-progress or just-applied binary/bundle swap, persisted as
/// `update-state.json` so the commit/rollback protocol survives a crash and a
/// restart. `target`/`backup`/`staged` are absolute paths; `target` is what the
/// swap replaced, `backup` is the pre-swap copy, `staged` is the downloaded
/// (and on Linux/Windows, already-applied) payload kept until commit so a
/// rollback can retry without re-downloading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingSwap {
    pub target: PathBuf,
    pub backup: PathBuf,
    pub staged: PathBuf,
    pub from_version: String,
    pub to_version: String,
    pub launch_attempts: u32,
}

impl PendingSwap {
    /// Read `update-state.json` at `state_path`, if present and parseable. A
    /// missing or corrupt file just means "no swap in flight" — never a reason
    /// to block startup.
    pub fn read(state_path: &Path) -> Option<PendingSwap> {
        let text = std::fs::read_to_string(state_path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Write `update-state.json`, replacing any existing one. Called before the
    /// swap (so a crash mid-swap still has a record to roll back from) and
    /// again after each `launch_attempts` increment.
    pub fn write(&self, state_path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self).context("serializing update state")?;
        std::fs::write(state_path, text)
            .with_context(|| format!("writing {}", state_path.display()))
    }

    /// Remove the state file, the staged payload, and the backup — called once
    /// the swapped-in build has proven itself (first successful frame) or once
    /// a mismatched `to_version` shows the swap never took.
    pub fn clear(state_path: &Path, staged: &Path, backup: &Path) {
        let _ = std::fs::remove_file(state_path);
        remove_path(staged);
        remove_path(backup);
    }
}

fn remove_path(path: &Path) {
    if path.is_dir() {
        let _ = std::fs::remove_dir_all(path);
    } else {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn swap(dir: &Path) -> PendingSwap {
        PendingSwap {
            target: dir.join("target"),
            backup: dir.join("backup"),
            staged: dir.join("staged"),
            from_version: "0.1.5".into(),
            to_version: "0.2.0".into(),
            launch_attempts: 0,
        }
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = std::env::temp_dir().join("yt_dlp_clipper_pending_swap_round_trip");
        let _ = std::fs::create_dir_all(&dir);
        let state_path = dir.join("update-state.json");

        let mut original = swap(&dir);
        original.launch_attempts = 1;
        original.write(&state_path).unwrap();

        let read_back = PendingSwap::read(&state_path).expect("state file should parse");
        assert_eq!(read_back, original);
    }

    #[test]
    fn missing_state_file_reads_as_none() {
        let dir = std::env::temp_dir().join("yt_dlp_clipper_pending_swap_missing");
        let state_path = dir.join("does-not-exist.json");
        assert!(PendingSwap::read(&state_path).is_none());
    }

    #[test]
    fn clear_removes_state_staged_and_backup() {
        let dir = std::env::temp_dir().join("yt_dlp_clipper_pending_swap_clear");
        let _ = std::fs::create_dir_all(&dir);
        let state_path = dir.join("update-state.json");
        let pending = swap(&dir);
        pending.write(&state_path).unwrap();
        std::fs::write(&pending.staged, b"payload").unwrap();
        std::fs::write(&pending.backup, b"backup").unwrap();

        PendingSwap::clear(&state_path, &pending.staged, &pending.backup);

        assert!(!state_path.exists());
        assert!(!pending.staged.exists());
        assert!(!pending.backup.exists());
    }

    #[test]
    fn reaches_the_launch_attempt_limit_after_repeated_failures() {
        let dir = std::env::temp_dir().join("yt_dlp_clipper_pending_swap_attempts");
        let _ = std::fs::create_dir_all(&dir);
        let mut pending = swap(&dir);
        for _ in 0..UPDATE_LAUNCH_ATTEMPT_LIMIT {
            assert!(pending.launch_attempts < UPDATE_LAUNCH_ATTEMPT_LIMIT);
            pending.launch_attempts += 1;
        }
        assert!(pending.launch_attempts >= UPDATE_LAUNCH_ATTEMPT_LIMIT);
    }
}
