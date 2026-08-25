use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use yt_dlp_clipper::{updater, ytdlp};

/// What a worker thread reports back over the shared channel.
pub(crate) enum Msg {
    Info(ytdlp::VideoInfo),
    Progress { downloaded: u64, total: u64 },
    Downloaded(PathBuf),
    Exported(PathBuf),
    /// Export aborted via Cancel; carries the partial output path to delete.
    ExportCanceled(PathBuf),
    Error(String),
}

/// Where the app-update check stands; drives the Settings button and its caption.
#[derive(Clone)]
pub(crate) enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Available(updater::AvailableRelease),
    Failed(String),
}

/// Background work in flight and what it last reported: the shared worker
/// channel, the export/download cancel flags, and the two update channels that
/// deliberately stand apart from it.
pub(crate) struct Jobs {
    rx: Option<Receiver<Msg>>,
    pub(crate) status: String,
    /// Path of the most recently saved export; drives the "Open folder" button
    /// next to the status line. `None` until a save succeeds.
    pub(crate) saved_path: Option<PathBuf>,
    /// Full text of the last failed operation (yt-dlp fetch/download or export),
    /// shown in a dismissable error panel; `None` once cleared or after a success.
    pub(crate) last_error: Option<String>,
    /// Active download progress as `(downloaded, total)` bytes, if downloading.
    pub(crate) progress: Option<(u64, u64)>,
    /// True while an export (compile + save) runs on the worker thread; drives
    /// an indeterminate progress bar since the encode reports no fraction.
    pub(crate) exporting: bool,
    /// Destination of the in-progress export, polled for its growing size to show
    /// a "X so far" readout next to the bar; `None` when not exporting.
    pub(crate) export_path: Option<PathBuf>,
    /// Cancel flag for the in-flight export; set by the Cancel button, checked by
    /// the encode loop. A fresh flag is created per export.
    pub(crate) export_cancel: Arc<AtomicBool>,
    /// Cancel flag for the in-flight download; set when a new video is loaded,
    /// checked by the yt-dlp read loop so it kills the subprocess. A fresh flag
    /// is created per download.
    pub(crate) download_cancel: Arc<AtomicBool>,
    /// True while a background `yt-dlp -U` is running.
    pub(crate) ytdlp_updating: bool,
    /// Cached `yt-dlp --version` for the Settings panel; fetched lazily, and
    /// cleared after an update so it refetches.
    pub(crate) ytdlp_version: Option<String>,
    /// Carries `yt-dlp -U`'s report. Settings actions get channels of their own:
    /// `spawn` replaces `rx`, which would orphan a download or export reporting
    /// on it.
    ytdlp_update_rx: Option<Receiver<Result<String, String>>>,
    /// Outcome of the last app-update check, shown in Settings.
    pub(crate) update_state: UpdateState,
    update_rx: Option<Receiver<Result<Option<updater::AvailableRelease>, String>>>,
}

impl Default for Jobs {
    fn default() -> Self {
        Self {
            rx: None,
            status: String::new(),
            saved_path: None,
            last_error: None,
            progress: None,
            exporting: false,
            export_path: None,
            export_cancel: Arc::new(AtomicBool::new(false)),
            download_cancel: Arc::new(AtomicBool::new(false)),
            ytdlp_updating: false,
            ytdlp_version: None,
            ytdlp_update_rx: None,
            update_state: UpdateState::Idle,
            update_rx: None,
        }
    }
}

impl Jobs {
    /// Run `work` on a background thread, handing it the channel so it can send
    /// progress updates plus a final terminal message.
    pub(crate) fn spawn<F>(&mut self, work: F)
    where
        F: FnOnce(Sender<Msg>) + Send + 'static,
    {
        let (tx, rx) = channel();
        self.rx = Some(rx);
        thread::spawn(move || work(tx));
    }

    /// True while a fetch, download, or export still holds `rx`. Starting
    /// another would replace the receiver and strand the running one, which
    /// keeps working but can no longer report.
    pub(crate) fn is_worker_busy(&self) -> bool {
        self.rx.is_some()
    }

    /// Take the worker channel for draining. The caller puts it back while the
    /// job is still running.
    pub(crate) fn take_receiver(&mut self) -> Option<Receiver<Msg>> {
        self.rx.take()
    }

    pub(crate) fn restore_receiver(&mut self, rx: Receiver<Msg>) {
        self.rx = Some(rx);
    }

    pub(crate) fn clear_receiver(&mut self) {
        self.rx = None;
    }

    /// Report a completed step, clearing any previous error and the progress bar.
    pub(crate) fn succeed(&mut self, status: String) {
        self.status = status;
        self.last_error = None;
        self.progress = None;
    }

    pub(crate) fn finish_export(&mut self) {
        self.exporting = false;
        self.export_path = None;
    }

    /// Run `yt-dlp -U` on a worker thread.
    pub(crate) fn start_ytdlp_update(&mut self) {
        if self.ytdlp_updating {
            return;
        }
        self.ytdlp_updating = true;
        self.status = "updating yt-dlp…".into();
        let (tx, rx) = channel();
        self.ytdlp_update_rx = Some(rx);
        thread::spawn(move || {
            let _ = tx.send(ytdlp::update().map_err(|e| e.to_string()));
        });
    }

    pub(crate) fn poll_ytdlp_update(&mut self) {
        let Some(rx) = self.ytdlp_update_rx.take() else { return };
        match rx.try_recv() {
            Ok(result) => {
                self.ytdlp_updating = false;
                self.ytdlp_version = None;
                match result {
                    Ok(report) => {
                        self.status = format!("yt-dlp: {report}");
                        self.last_error = None;
                    }
                    Err(error) => self.last_error = Some(error),
                }
            }
            Err(TryRecvError::Empty) => self.ytdlp_update_rx = Some(rx),
            Err(TryRecvError::Disconnected) => self.ytdlp_updating = false,
        }
    }

    /// Query GitHub for a newer release on a worker thread.
    pub(crate) fn start_update_check(&mut self) {
        if matches!(self.update_state, UpdateState::Checking) {
            return;
        }
        self.update_state = UpdateState::Checking;
        let (tx, rx) = channel();
        self.update_rx = Some(rx);
        thread::spawn(move || {
            let _ = tx.send(updater::check().map_err(|e| e.to_string()));
        });
    }

    pub(crate) fn poll_update_check(&mut self) {
        let Some(rx) = self.update_rx.take() else { return };
        match rx.try_recv() {
            Ok(Ok(Some(release))) => self.update_state = UpdateState::Available(release),
            Ok(Ok(None)) => self.update_state = UpdateState::UpToDate,
            Ok(Err(error)) => self.update_state = UpdateState::Failed(error),
            Err(TryRecvError::Empty) => self.update_rx = Some(rx),
            Err(TryRecvError::Disconnected) => {
                self.update_state = UpdateState::Failed("the update check stopped".into())
            }
        }
    }
}
