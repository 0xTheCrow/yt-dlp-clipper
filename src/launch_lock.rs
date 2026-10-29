//! Keeps an executable the app just wrote from failing to run with ETXTBSY
//! ("Text file busy"). A launched child holds a copy of every file its parent has
//! open until its own `exec` completes, and running a file that any process still
//! holds open for writing fails. Launches hold this lock shared for the length
//! of `spawn`, which std returns from only once the child has exec'd and closed
//! those copies; a writer that closes an executable and then calls
//! `await_launches_in_progress` therefore knows no child still holds it.

use std::io;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::{PoisonError, RwLock};

static LAUNCH_LOCK: RwLock<()> = RwLock::new(());

/// `Command::spawn`, registered as a launch in progress until it returns.
pub fn spawn(command: &mut Command) -> io::Result<Child> {
    let _launching = LAUNCH_LOCK.read().unwrap_or_else(PoisonError::into_inner);
    command.spawn()
}

/// `Command::output` through `spawn`: stdin closed, stdout and stderr captured.
pub fn output(command: &mut Command) -> io::Result<Output> {
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    spawn(command)?.wait_with_output()
}

/// `Command::status` through `spawn`.
pub fn status(command: &mut Command) -> io::Result<ExitStatus> {
    spawn(command)?.wait()
}

/// Call after closing a file that will be executed. Returns once every launch
/// that started while the file was open has exec'd, so none still holds it.
pub fn await_launches_in_progress() {
    drop(LAUNCH_LOCK.write().unwrap_or_else(PoisonError::into_inner));
}
