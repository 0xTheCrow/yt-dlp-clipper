use crate::binaries::managed_cache_dir;
use crate::cache::{cache_thumbnails, is_video_file, CacheEntry, CacheThumb};
use crate::keybinds::Bind;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::thread;

/// The cached-downloads grid: what it lists and the worker decoding thumbnails.
#[derive(Default)]
pub(crate) struct CacheBrowser {
    pub(crate) is_open: bool,
    pub(crate) entries: Vec<CacheEntry>,
    rx: Option<Receiver<CacheThumb>>,
}

impl CacheBrowser {
    /// List the cached videos and kick off background thumbnail decoding.
    pub(crate) fn open(&mut self) {
        let dir = managed_cache_dir();
        let mut paths = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if is_video_file(&path) {
                    paths.push(path);
                }
            }
        }
        paths.sort();
        self.entries = paths
            .iter()
            .map(|p| CacheEntry {
                path: p.clone(),
                name: p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                size: std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
                duration: None,
                tex: None,
            })
            .collect();

        let (tx, rx) = channel();
        self.rx = Some(rx);
        thread::spawn(move || cache_thumbnails(paths, tx));
        self.is_open = true;
    }

    /// Close the grid, dropping the listing and the thumbnail channel so the
    /// worker's remaining sends go nowhere.
    pub(crate) fn close(&mut self) {
        self.is_open = false;
        self.entries.clear();
        self.rx = None;
    }

    /// Attach thumbnails that the worker has finished decoding.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let mut ready = Vec::new();
        let mut done = false;
        if let Some(rx) = &self.rx {
            loop {
                match rx.try_recv() {
                    Ok(item) => ready.push(item),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        done = true;
                        break;
                    }
                }
            }
        }
        for thumb in ready {
            let tex = ctx.load_texture(
                format!("thumb:{}", thumb.path.display()),
                thumb.image,
                egui::TextureOptions::LINEAR,
            );
            if let Some(entry) = self.entries.iter_mut().find(|e| e.path == thumb.path) {
                entry.tex = Some(tex);
                entry.duration = Some(thumb.duration_secs);
            }
        }
        if done {
            self.rx = None;
        }
    }
}

/// Transient interface state: panel visibility and in-progress interactions.
/// Nothing here persists — `Settings` owns everything that survives a restart.
pub(crate) struct UiState {
    /// Scale being edited in Settings; applied to `Settings::ui_scale` only on Apply.
    pub(crate) pending_scale: f32,
    pub(crate) is_settings_open: bool,
    /// In Settings, the action whose next keypress is being captured, if any.
    pub(crate) rebinding: Option<Bind>,
    /// Per nav action (skip back/fwd, step back/fwd) the input time at which a
    /// held key may next fire; `0.0` means "not held" so the next press fires now.
    pub(crate) nav_repeat_at: [f64; 4],
    pub(crate) cache: CacheBrowser,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            pending_scale: crate::app::settings::DEFAULT_UI_SCALE,
            is_settings_open: false,
            rebinding: None,
            nav_repeat_at: [0.0; 4],
            cache: CacheBrowser::default(),
        }
    }
}
