use crate::STORAGE_APP_ID;
use std::path::{Path, PathBuf};

/// Cross-platform directory for downloaded videos, under eframe's app data dir.
pub(crate) fn managed_cache_dir() -> PathBuf {
    eframe::storage_dir(STORAGE_APP_ID)
        .map(|d| d.join("downloads"))
        .unwrap_or_else(std::env::temp_dir)
}

/// Platform filenames for the bundled tool binaries.
#[cfg(windows)]
const YTDLP_EXE: &str = "yt-dlp.exe";
#[cfg(not(windows))]
const YTDLP_EXE: &str = "yt-dlp";
#[cfg(windows)]
const FFMPEG_EXE: &str = "ffmpeg.exe";
#[cfg(not(windows))]
const FFMPEG_EXE: &str = "ffmpeg";

/// Directory holding the running executable, where a packaged build bundles
/// `yt-dlp`/`ffmpeg` next to the app.
fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

/// Subfolder beside the executable where the Windows bundle stages helper
/// binaries, keeping the app exe alone at the bundle root so it's the only
/// thing a user can click to launch.
const BUNDLED_BIN_SUBDIR: &str = "bin";

/// Locate a bundled tool binary: the `bin/` subfolder beside the exe first
/// (the packaged layout), then directly beside the exe (dev `target/` builds
/// and the AppImage/macOS bundles, which stage tools next to the app).
fn bundled_binary(name: &str) -> Option<PathBuf> {
    let dir = exe_dir()?;
    [dir.join(BUNDLED_BIN_SUBDIR).join(name), dir.join(name)]
        .into_iter()
        .find(|p| p.is_file())
}

/// First directory on `PATH` that contains `name`. Returned as a full path so we
/// invoke it directly rather than letting the OS re-resolve a bare name.
fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// Per-user, owner-only-writable directory for the managed yt-dlp copy. Keeping
/// it non-world-writable stops another local user from swapping the binary we
/// run between seeding and launch.
fn managed_bin_dir() -> Option<PathBuf> {
    let dir = eframe::storage_dir(STORAGE_APP_ID)?.join("bin");
    std::fs::create_dir_all(&dir).ok()?;
    set_owner_only(&dir, 0o755);
    Some(dir)
}

/// On Unix, restrict `path` to the given mode (owner-write, others read/exec);
/// a no-op elsewhere (Windows ACLs already make per-user dirs owner-private).
#[cfg(unix)]
fn set_owner_only(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}
#[cfg(not(unix))]
fn set_owner_only(_path: &Path, _mode: u32) {}

/// Resolve yt-dlp to an absolute path, seeding a writable managed copy so
/// `yt-dlp -U` can update it in place even when the app is installed read-only.
/// Order: managed copy → bundled (`bin/` or next to exe) → PATH.
pub(crate) fn resolve_ytdlp() -> Option<PathBuf> {
    let managed = managed_bin_dir().map(|d| d.join(YTDLP_EXE));
    if let Some(m) = &managed {
        if m.is_file() {
            return Some(m.clone());
        }
    }
    let source = bundled_binary(YTDLP_EXE).or_else(|| find_in_path(YTDLP_EXE))?;
    // Seed the managed copy so updates work; if we can't, run the source binary
    // directly — still by absolute path, never a bare name.
    match &managed {
        Some(m) if std::fs::copy(&source, m).is_ok() => {
            set_owner_only(m, 0o755);
            Some(m.clone())
        }
        _ => Some(source),
    }
}

/// Resolve ffmpeg to an absolute path for `--ffmpeg-location`. No managed copy:
/// ffmpeg never self-updates, so it needn't live in a writable dir.
/// Order: bundled (`bin/` or next to exe) → PATH.
pub(crate) fn resolve_ffmpeg() -> Option<PathBuf> {
    bundled_binary(FFMPEG_EXE).or_else(|| find_in_path(FFMPEG_EXE))
}

/// Total size, in bytes, of the files directly in `dir`.
pub(crate) fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

/// Remove every entry in `dir`, ignoring per-entry errors (e.g. a file still
/// open on Windows).
pub(crate) fn clear_dir(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(&path);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
}
