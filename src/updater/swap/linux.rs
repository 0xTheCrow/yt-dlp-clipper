use crate::launch_lock;
use crate::updater::stage::PendingSwap;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The AppImage runtime sets `$APPIMAGE` to the path of the outer `.AppImage`
/// file being run — `current_exe()` instead resolves inside the temporary
/// squashfs mount, which isn't what needs replacing. Unset means this isn't
/// running from an AppImage at all (a raw dev build), so there's nothing to
/// self-update.
pub fn current_install_target() -> Result<PathBuf> {
    std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .context("not running from an AppImage ($APPIMAGE is unset) — nothing to self-update")
}

/// Back up the running AppImage, then swap the staged one into its place.
pub fn apply(pending: &PendingSwap) -> Result<()> {
    std::fs::copy(&pending.target, &pending.backup).with_context(|| {
        format!("backing up {} to {}", pending.target.display(), pending.backup.display())
    })?;
    launch_lock::await_launches_in_progress();
    atomic_replace(&pending.staged, &pending.target)
}

/// `rename()` first — POSIX-atomic, and the currently-executing AppImage keeps
/// its inode, so the running process is never reading a half-written file.
/// Falls back to copy-then-rename through a same-directory temp file when
/// `staged` and `target` don't share a filesystem (`rename` fails with
/// `EXDEV`): `staged` lives under this app's storage dir while `target`
/// (`$APPIMAGE`) can be anywhere the user put it, so they aren't guaranteed to
/// be co-located the way `backup` and `target` are handled above.
fn atomic_replace(staged: &Path, target: &Path) -> Result<()> {
    if std::fs::rename(staged, target).is_ok() {
        return Ok(());
    }
    let tmp = target.with_extension("update-tmp");
    std::fs::copy(staged, &tmp)
        .with_context(|| format!("copying {} to {}", staged.display(), tmp.display()))?;
    launch_lock::await_launches_in_progress();
    std::fs::rename(&tmp, target)
        .with_context(|| format!("renaming {} to {}", tmp.display(), target.display()))?;
    let _ = std::fs::remove_file(staged);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_backs_up_the_old_target_and_installs_the_staged_payload() {
        let dir = std::env::temp_dir().join("yt_dlp_clipper_linux_swap_apply");
        let _ = std::fs::create_dir_all(&dir);
        let pending = PendingSwap {
            target: dir.join("target.AppImage"),
            backup: dir.join("backup.AppImage"),
            staged: dir.join("staged.AppImage"),
            from_version: "0.1.5".into(),
            to_version: "0.2.0".into(),
            launch_attempts: 0,
        };
        std::fs::write(&pending.target, b"old build").unwrap();
        std::fs::write(&pending.staged, b"new build").unwrap();
        let _ = std::fs::remove_file(&pending.backup);

        apply(&pending).unwrap();

        assert_eq!(std::fs::read(&pending.target).unwrap(), b"new build");
        assert_eq!(std::fs::read(&pending.backup).unwrap(), b"old build");
        assert!(!pending.staged.exists(), "the fast rename path should consume staged");
    }
}
