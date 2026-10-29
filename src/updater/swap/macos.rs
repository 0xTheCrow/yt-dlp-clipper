use crate::launch_lock;
use crate::updater::stage::PendingSwap;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Unverified from the Linux dev/CI sandbox this was written in — mirrors
/// CLAUDE.md's documented macOS swap mechanics exactly, but needs a real
/// macOS run to confirm.

/// Climb from the running executable (`<App>.app/Contents/MacOS/<App>`) to the
/// `.app` bundle root, since that whole directory is the thing self-update
/// replaces, not just the executable inside it.
pub fn current_install_target() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("resolving the running exe's path")?;
    exe.parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .filter(|p| p.extension().is_some_and(|ext| ext == "app"))
        .map(Path::to_path_buf)
        .with_context(|| format!("{} doesn't look like it's inside a .app bundle", exe.display()))
}

/// Extract the release's `.zip` payload to `dest_dir` and return the `.app`
/// bundle inside it. Done as its own step, ahead of `apply`, so the helper
/// script's own swap window is a single `mv` rather than an unzip-in-place.
pub fn unzip_app(zip_path: &Path, dest_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dest_dir)
        .with_context(|| format!("creating {}", dest_dir.display()))?;
    let status = launch_lock::status(
        Command::new("unzip").args(["-q", "-o"]).arg(zip_path).arg("-d").arg(dest_dir),
    )
        .context("running unzip")?;
    if !status.success() {
        bail!("unzip exited with {status}");
    }
    std::fs::read_dir(dest_dir)
        .with_context(|| format!("reading {}", dest_dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "app"))
        .with_context(|| format!("no .app bundle found after unzipping {}", zip_path.display()))
}

/// The target is a directory the running process lives inside, so it can't be
/// replaced from within this process the way a single executable can on
/// Linux/Windows — the swap has to happen after this process has fully
/// exited. Writes a detached `/bin/sh` helper that waits on our pid, then
/// swaps `pending.staged` (already an unzipped `.app`, see `unzip_app`) into
/// `pending.target`, restoring `pending.backup` on failure, and relaunches
/// with `--finish-update`. The caller must exit this process right after
/// `apply` returns — the helper's wait loop is what it's waiting for.
pub fn apply(pending: &PendingSwap) -> Result<()> {
    let pid = std::process::id();
    let helper_path = std::env::temp_dir().join(format!("yt-dlp-clipper-update-helper-{pid}.sh"));
    let script = format!(
        "#!/bin/sh\n\
         while kill -0 {pid} 2>/dev/null; do sleep 0.2; done\n\
         rm -rf \"{target}\"\n\
         if mv \"{staged}\" \"{target}\"; then\n\
         \x20   open \"{target}\" --args --finish-update\n\
         else\n\
         \x20   rm -rf \"{target}\"\n\
         \x20   cp -R \"{backup}\" \"{target}\"\n\
         \x20   open \"{target}\" --args --finish-update\n\
         fi\n\
         rm -f \"$0\"\n",
        pid = pid,
        staged = pending.staged.display(),
        target = pending.target.display(),
        backup = pending.backup.display(),
    );
    std::fs::write(&helper_path, script).context("writing the macOS update helper script")?;
    set_executable(&helper_path)?;
    launch_lock::spawn(
        Command::new("/bin/sh")
            .arg(&helper_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )
        .context("spawning the macOS update helper")?;
    Ok(())
}

fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("marking {} executable", path.display()))
}
