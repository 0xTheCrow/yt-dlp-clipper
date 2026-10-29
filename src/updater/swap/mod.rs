//! Per-platform in-place replacement of the running install. Each platform
//! module exposes the same two functions: `current_install_target()` (called
//! once, when a download is about to start, to fill in `PendingSwap::target`)
//! and `apply(&PendingSwap)` (the actual swap, called once the payload is
//! staged and probed). Everything each needs — target, backup, staged paths —
//! already lives on the `PendingSwap` written to disk before `apply` runs, so
//! a crash mid-swap leaves a record `main.rs`'s startup check can act on.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::{apply, current_install_target};
#[cfg(target_os = "macos")]
pub use macos::{apply, current_install_target, unzip_app};
#[cfg(target_os = "windows")]
pub use windows::{apply, current_install_target};

use crate::updater::stage::PendingSwap;
use anyhow::Result;
use std::path::Path;

/// Swap `pending.backup` back over `pending.target` — the mirror of a normal
/// update, built by calling the same platform `apply` with an inverted
/// `PendingSwap` (its `staged` is the old `backup`). Used when the swapped-in
/// build fails to start cleanly `UPDATE_LAUNCH_ATTEMPT_LIMIT` times running.
/// `discard` is a throwaway path for the inverted call's own "backup" slot —
/// a rollback never needs a backup of the build it's discarding.
pub fn restore_backup(pending: &PendingSwap) -> Result<()> {
    let discard = pending.staged.with_extension("rollback-discard");
    let inverted = PendingSwap {
        target: pending.target.clone(),
        backup: discard,
        staged: pending.backup.clone(),
        from_version: pending.to_version.clone(),
        to_version: pending.from_version.clone(),
        launch_attempts: 0,
    };
    apply(&inverted)
}

/// After a successful `apply` (forward or `restore_backup`), hand control to
/// `target` and end this process. Linux/Windows spawn `target` fresh; macOS's
/// `apply` already spawned a detached helper that relaunches once it sees this
/// process gone, so there's nothing left to spawn here.
pub fn relaunch_and_exit(target: &Path) -> ! {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = crate::launch_lock::spawn(&mut std::process::Command::new(target));
    }
    #[cfg(target_os = "macos")]
    {
        let _ = target;
    }
    std::process::exit(0)
}
