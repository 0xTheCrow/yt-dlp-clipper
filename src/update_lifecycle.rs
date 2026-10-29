//! Startup handling for an in-progress self-update: increments the
//! launch-attempt counter for a build that IS the swapped-in target, rolls
//! back to the pre-swap backup once `UPDATE_LAUNCH_ATTEMPT_LIMIT` is reached,
//! and clears a stale record left by a swap that never actually took.

use crate::binaries::update_state_path;
use yt_dlp_clipper::updater::stage::{PendingSwap, UPDATE_LAUNCH_ATTEMPT_LIMIT};
use yt_dlp_clipper::updater::swap;

/// Runs once, at the very top of `main`, before anything riskier (GUI/GPU
/// init) so a crash there still counts against the attempt limit. Returns the
/// pending swap when this launch is the freshly-applied build and hasn't hit
/// the attempt limit — the caller threads it through to `App`, which clears it
/// once a frame actually renders (see `App::update`), the simplest signal this
/// process has that startup didn't wedge.
pub(crate) fn handle_startup() -> Option<PendingSwap> {
    let state_path = update_state_path();
    let mut pending = PendingSwap::read(&state_path)?;

    if pending.to_version != env!("CARGO_PKG_VERSION") {
        // The swap never took, or this *is* the rollback build restarted after
        // restoring the backup — either way, a stale record.
        PendingSwap::clear(&state_path, &pending.staged, &pending.backup);
        return None;
    }

    pending.launch_attempts += 1;
    let _ = pending.write(&state_path);

    if pending.launch_attempts >= UPDATE_LAUNCH_ATTEMPT_LIMIT {
        if swap::restore_backup(&pending).is_ok() {
            swap::relaunch_and_exit(&pending.target);
        }
        // Restore itself failed — better to let this build keep trying to
        // start than leave the install in a half-rolled-back state.
        return None;
    }

    Some(pending)
}
