//! Runs a staged build's own `--self-test` before it ever replaces the working
//! install, so a wrong-arch asset, an unmet glibc floor, or a corrupt download
//! is caught on the user's actual machine while the old build can still run.

use crate::launch_lock;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// How long the staged build gets to exit 0 before it's treated as hung.
pub const SELF_TEST_TIMEOUT: Duration = Duration::from_secs(10);
/// How often to poll the child for exit, balancing responsiveness against
/// spinning the CPU on a tight loop.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

const SELF_TEST_FLAG: &str = "--self-test";

/// Run `payload --self-test` and wait up to `timeout` for it to exit.
/// `Ok(true)` means it exited 0 in time; `Ok(false)` means it exited non-zero
/// or had to be killed for hanging past `timeout` — either way, the caller
/// must not swap this payload in. Signals on **exit code, not stdout**: a
/// future `windows_subsystem = "windows"` would silently break a stdout probe.
pub fn self_test(payload: &Path, timeout: Duration) -> std::io::Result<bool> {
    let mut child = launch_lock::spawn(Command::new(payload).arg(SELF_TEST_FLAG))?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status.success());
        }
        if Instant::now() >= deadline {
            kill_and_reap(&mut child);
            return Ok(false);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn shim(name: &str, script: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("yt_dlp_clipper_self_test_probe");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join(name);
        fs::write(&path, script).unwrap();
        launch_lock::await_launches_in_progress();
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    #[cfg(unix)]
    fn passes_when_self_test_exits_zero() {
        let path = shim("passes.sh", "#!/bin/sh\nexit 0\n");
        assert!(self_test(&path, Duration::from_secs(2)).unwrap());
    }

    #[test]
    #[cfg(unix)]
    fn fails_when_self_test_exits_nonzero() {
        let path = shim("fails.sh", "#!/bin/sh\nexit 1\n");
        assert!(!self_test(&path, Duration::from_secs(2)).unwrap());
    }

    #[test]
    #[cfg(unix)]
    fn fails_when_self_test_hangs_past_the_timeout() {
        let path = shim("hangs.sh", "#!/bin/sh\nexec tail -f /dev/null\n");
        assert!(!self_test(&path, Duration::from_millis(200)).unwrap());
    }
}
