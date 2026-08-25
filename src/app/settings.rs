use crate::binaries::managed_cache_dir;
use crate::keybinds::{Bind, Keybinds, Shortcut};
use crate::theme::{theme_pref_from_name, theme_pref_name};
use std::path::PathBuf;
use yt_dlp_clipper::ytdlp::CookieSource;

/// Storage keys for the persisted settings. Each is read and written in exactly
/// one place below; the names are on-disk format and must stay stable.
const SCALE_KEY: &str = "ui_scale";
const DOWNLOAD_DIR_KEY: &str = "download_dir";
const OUTPUT_DIR_KEY: &str = "output_dir";
const DELETE_ON_EXIT_KEY: &str = "delete_cache_on_exit";
const OPEN_DIR_ON_SAVE_KEY: &str = "open_dir_on_save";
const COMPATIBILITY_MODE_KEY: &str = "compatibility_mode";
const VOLUME_KEY: &str = "volume";
const THEME_KEY: &str = "theme";
const KEYBINDS_KEY: &str = "keybinds";
const COOKIES_KEY: &str = "cookies_source";

pub(crate) const DEFAULT_UI_SCALE: f32 = 1.0;
const DEFAULT_VOLUME: f32 = 0.5;

/// Settings that persist between sessions via eframe storage.
pub(crate) struct Settings {
    pub(crate) ui_scale: f32,
    /// Light/Dark/System appearance; `System` follows the desktop theme.
    pub(crate) theme: egui::ThemePreference,
    /// Where downloads are saved; `None` uses the managed cache directory.
    pub(crate) download_dir: Option<PathBuf>,
    /// Default folder the export save dialog opens in; `None` uses the system
    /// default. The dialog is always shown either way.
    pub(crate) output_dir: Option<PathBuf>,
    /// Clear the managed cache directory when the app closes.
    pub(crate) delete_cache_on_exit: bool,
    /// Reveal the saved file's folder in the system file manager after a save.
    pub(crate) open_dir_on_save: bool,
    /// When set, a saved MP4/MOV is restricted to broadly-playable codecs
    /// (H.264 8-bit + AAC/MP3), re-encoding anything else; when clear, the
    /// source codec/quality is kept (HEVC/AV1, 10-bit, HDR). On by default.
    pub(crate) compatibility_mode: bool,
    /// Output volume in `0.0..=1.0`, applied to audio playback.
    pub(crate) volume: f32,
    /// Configurable shortcuts for clip + playback actions.
    pub(crate) keybinds: Keybinds,
    /// Where fetch/download pull auth cookies from — a browser's live profile
    /// or an exported `cookies.txt` — so age-gated or members-only videos work.
    /// `None` sends no cookies.
    pub(crate) cookies: Option<CookieSource>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ui_scale: DEFAULT_UI_SCALE,
            theme: egui::ThemePreference::System,
            download_dir: None,
            output_dir: None,
            delete_cache_on_exit: false,
            open_dir_on_save: false,
            compatibility_mode: true,
            volume: DEFAULT_VOLUME,
            keybinds: Keybinds::default(),
            cookies: None,
        }
    }
}

impl Settings {
    pub(crate) fn load(storage: &dyn eframe::Storage) -> Self {
        let defaults = Self::default();
        Self {
            ui_scale: eframe::get_value(storage, SCALE_KEY).unwrap_or(defaults.ui_scale),
            theme: eframe::get_value::<String>(storage, THEME_KEY)
                .map_or(defaults.theme, |name| theme_pref_from_name(&name)),
            download_dir: eframe::get_value(storage, DOWNLOAD_DIR_KEY).flatten(),
            output_dir: eframe::get_value(storage, OUTPUT_DIR_KEY).flatten(),
            delete_cache_on_exit: eframe::get_value(storage, DELETE_ON_EXIT_KEY)
                .unwrap_or(defaults.delete_cache_on_exit),
            open_dir_on_save: eframe::get_value(storage, OPEN_DIR_ON_SAVE_KEY)
                .unwrap_or(defaults.open_dir_on_save),
            compatibility_mode: eframe::get_value(storage, COMPATIBILITY_MODE_KEY)
                .unwrap_or(defaults.compatibility_mode),
            volume: eframe::get_value(storage, VOLUME_KEY).unwrap_or(defaults.volume),
            keybinds: load_keybinds(storage),
            cookies: eframe::get_value(storage, COOKIES_KEY).flatten(),
        }
    }

    pub(crate) fn save(&self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, SCALE_KEY, &self.ui_scale);
        eframe::set_value(storage, THEME_KEY, &theme_pref_name(self.theme).to_owned());
        eframe::set_value(storage, DOWNLOAD_DIR_KEY, &self.download_dir);
        eframe::set_value(storage, OUTPUT_DIR_KEY, &self.output_dir);
        eframe::set_value(storage, DELETE_ON_EXIT_KEY, &self.delete_cache_on_exit);
        eframe::set_value(storage, OPEN_DIR_ON_SAVE_KEY, &self.open_dir_on_save);
        eframe::set_value(storage, COMPATIBILITY_MODE_KEY, &self.compatibility_mode);
        eframe::set_value(storage, VOLUME_KEY, &self.volume);
        save_keybinds(storage, &self.keybinds);
        eframe::set_value(storage, COOKIES_KEY, &self.cookies);
    }

    /// Where downloads land: the configured folder, else the managed cache dir.
    pub(crate) fn effective_download_dir(&self) -> PathBuf {
        self.download_dir.clone().unwrap_or_else(managed_cache_dir)
    }
}

/// Each shortcut persists as (action id, key name, ctrl, shift) so it doesn't
/// depend on egui's serde feature. Keyed by a stable id so reordering or adding
/// actions can't misread a save; unknown ids and absent actions keep their
/// defaults. Falls back to the older ctrl-less (id, name, shift) format.
fn load_keybinds(storage: &dyn eframe::Storage) -> Keybinds {
    let mut keybinds = Keybinds::default();
    if let Some(saved) =
        eframe::get_value::<Vec<(String, String, bool, bool)>>(storage, KEYBINDS_KEY)
    {
        for (id, name, ctrl, shift) in saved {
            if let (Some(bind), Some(key)) = (Bind::from_id(&id), egui::Key::from_name(&name)) {
                keybinds.put(bind, Shortcut { key, ctrl, shift });
            }
        }
    } else if let Some(saved) =
        eframe::get_value::<Vec<(String, String, bool)>>(storage, KEYBINDS_KEY)
    {
        for (id, name, shift) in saved {
            if let (Some(bind), Some(key)) = (Bind::from_id(&id), egui::Key::from_name(&name)) {
                keybinds.put(bind, Shortcut { key, ctrl: false, shift });
            }
        }
    }
    keybinds
}

fn save_keybinds(storage: &mut dyn eframe::Storage, keybinds: &Keybinds) {
    let saved: Vec<(String, String, bool, bool)> = Bind::ALL
        .iter()
        .map(|(bind, _)| {
            let sc = keybinds.shortcut(*bind);
            (bind.id().to_owned(), sc.key.name().to_owned(), sc.ctrl, sc.shift)
        })
        .collect();
    eframe::set_value(storage, KEYBINDS_KEY, &saved);
}
