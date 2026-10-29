use crate::launch_lock;
use crate::updater::stage::PendingSwap;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Unverified from the Linux dev/CI sandbox this was written in — mirrors
/// CLAUDE.md's documented Windows swap mechanics exactly, but needs a real
/// Windows run to confirm.
pub fn current_install_target() -> Result<PathBuf> {
    std::env::current_exe().context("resolving the running exe's path")
}

/// Windows permits renaming a running exe but not deleting or overwriting it —
/// the same trick `yt-dlp -U` already relies on. Rename the running exe aside,
/// then write the staged payload to the now-free original path; the caller is
/// responsible for relaunching `pending.target` and exiting this process
/// afterward (this function only performs the file swap).
pub fn apply(pending: &PendingSwap) -> Result<()> {
    std::fs::rename(&pending.target, &pending.backup).with_context(|| {
        format!("renaming {} to {}", pending.target.display(), pending.backup.display())
    })?;
    std::fs::copy(&pending.staged, &pending.target).with_context(|| {
        format!("copying {} to {}", pending.staged.display(), pending.target.display())
    })?;
    launch_lock::await_launches_in_progress();
    Ok(())
}
