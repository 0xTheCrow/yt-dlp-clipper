use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use yank::audio::AudioPlayer;
use yank::decoder::Decoder;
use yank::export::{self, ExportSpec, Mode};
use yank::ytdlp;

/// On-disk identifier for the app-data dir (settings + download cache). Kept
/// generic and stable so renaming the app doesn't orphan persisted state.
const STORAGE_APP_ID: &str = "yt-dlp-clipper";

const SCALE_STORAGE_KEY: &str = "ui_scale";
const DOWNLOAD_DIR_KEY: &str = "download_dir";
const DELETE_ON_EXIT_KEY: &str = "delete_cache_on_exit";
const VOLUME_KEY: &str = "volume";
const KEYBINDS_KEY: &str = "keybinds";
const OUTPUT_DIR_KEY: &str = "output_dir";
const OPEN_DIR_ON_SAVE_KEY: &str = "open_dir_on_save";
const THEME_KEY: &str = "theme";

/// Standard heights offered when downscaling a saved video, tallest first. Only
/// those shorter than the source are shown, so the menu never upscales.
const EXPORT_HEIGHT_LADDER: [u32; 8] = [2160, 1440, 1080, 720, 480, 360, 240, 144];

/// Inner padding applied to every button, enlarging the click target.
const BUTTON_PAD: egui::Vec2 = egui::Vec2::new(10.0, 6.0);

/// Seconds skipped by the skip-back / skip-forward keys.
const SKIP_SECS: f64 = 5.0;
/// Held nav key: how long before auto-repeat begins, then the interval between
/// repeats (seconds). Keeps a held key from firing too fast.
const NAV_REPEAT_DELAY: f64 = 0.3;
const NAV_REPEAT_INTERVAL: f64 = 0.1;

/// Inner margin for text input fields. Vertical padding is kept below
/// `BUTTON_PAD.y` so inputs never stand taller than the buttons beside them.
const INPUT_MARGIN: egui::Margin = egui::Margin {
    left: 8.0,
    right: 8.0,
    top: 4.0,
    bottom: 4.0,
};

/// The floppy-disk "save" icon (embedded at compile time).
fn save_icon() -> egui::ImageSource<'static> {
    egui::include_image!("../assets/save.svg")
}

/// The "download" icon used by the Fetch button (embedded at compile time).
fn download_icon() -> egui::ImageSource<'static> {
    egui::include_image!("../assets/download.svg")
}

/// The gear "settings" icon (embedded at compile time).
fn settings_icon() -> egui::ImageSource<'static> {
    egui::include_image!("../assets/settings.svg")
}

/// A button with a square SVG `icon` to the left of `text`. The icon is sized to
/// the button font's height so it lines up with the caption.
fn icon_button(ui: &mut egui::Ui, icon: egui::ImageSource<'_>, text: &str) -> egui::Response {
    let size = ui.text_style_height(&egui::TextStyle::Button);
    let image = egui::Image::new(icon).fit_to_exact_size(egui::Vec2::splat(size));
    ui.add(egui::Button::image_and_text(image, text))
}

/// Reveal a saved file in the system file manager, selecting it when the
/// platform supports it and otherwise opening its containing folder.
fn reveal_in_file_manager(path: &Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let dir = path.parent().unwrap_or(path);
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

/// Height of a standard button (font height + vertical padding). Used to size
/// manually-laid-out rows so their contents stay vertically centered.
fn button_height(ui: &egui::Ui) -> f32 {
    let text = ui.text_style_height(&egui::TextStyle::Button);
    (text + 2.0 * ui.spacing().button_padding.y).max(ui.spacing().interact_size.y)
}

/// A user-rebindable keyboard action.
/// A bindable shortcut: a key plus whether Shift must be held. Shift lets the
/// arrow keys carry two actions each (skip vs. single-frame step).
#[derive(Clone, Copy, PartialEq)]
struct Shortcut {
    key: egui::Key,
    shift: bool,
}

impl Shortcut {
    const fn plain(key: egui::Key) -> Self {
        Self { key, shift: false }
    }
    const fn shifted(key: egui::Key) -> Self {
        Self { key, shift: true }
    }
    /// Label for the settings button, e.g. "Space" or "Shift+ArrowLeft".
    fn label(self) -> String {
        if self.shift {
            format!("Shift+{}", self.key.name())
        } else {
            self.key.name().to_owned()
        }
    }
}

/// True on the frame `sc` is pressed (with the exact Shift state). For one-shot
/// actions like set-start / play-pause.
fn shortcut_pressed(i: &egui::InputState, sc: Shortcut) -> bool {
    i.key_pressed(sc.key) && i.modifiers.shift == sc.shift
}

/// True while `sc` is held (exact Shift state). Paired with a manual timer for
/// hold-to-repeat navigation.
fn shortcut_down(i: &egui::InputState, sc: Shortcut) -> bool {
    i.key_down(sc.key) && i.modifiers.shift == sc.shift
}

#[derive(Clone, Copy, PartialEq)]
enum Bind {
    SetStart,
    SetEnd,
    PlayPauseClip,
    PlayPause,
    SkipBack,
    SkipForward,
    StepBack,
    StepForward,
}

impl Bind {
    /// All actions, in display order (row-major: each consecutive pair forms one
    /// two-column grid row), with their settings labels.
    const ALL: [(Bind, &'static str); 8] = [
        (Bind::SetStart, "Set start"),
        (Bind::SetEnd, "Set end"),
        (Bind::PlayPauseClip, "Play / pause clip"),
        (Bind::PlayPause, "Play / pause"),
        (Bind::SkipForward, "Skip forward 5s"),
        (Bind::SkipBack, "Skip back 5s"),
        (Bind::StepForward, "Step forward 1 frame"),
        (Bind::StepBack, "Step back 1 frame"),
    ];

    /// Stable identifier for persistence, decoupled from display order so
    /// reordering or adding actions never misreads an older save.
    fn id(self) -> &'static str {
        match self {
            Bind::SetStart => "set_start",
            Bind::SetEnd => "set_end",
            Bind::PlayPauseClip => "play_pause_clip",
            Bind::PlayPause => "play_pause",
            Bind::SkipBack => "skip_back",
            Bind::SkipForward => "skip_forward",
            Bind::StepBack => "step_back",
            Bind::StepForward => "step_forward",
        }
    }

    fn from_id(id: &str) -> Option<Bind> {
        Bind::ALL.iter().map(|(b, _)| *b).find(|b| b.id() == id)
    }
}

/// The configurable shortcuts for the clip and playback actions.
#[derive(Clone, Copy)]
struct Keybinds {
    set_start: Shortcut,
    set_end: Shortcut,
    play_pause_clip: Shortcut,
    play_pause: Shortcut,
    skip_back: Shortcut,
    skip_forward: Shortcut,
    step_back: Shortcut,
    step_forward: Shortcut,
}

impl Default for Keybinds {
    fn default() -> Self {
        Self {
            set_start: Shortcut::plain(egui::Key::S),
            set_end: Shortcut::plain(egui::Key::E),
            play_pause_clip: Shortcut::plain(egui::Key::Space),
            play_pause: Shortcut::shifted(egui::Key::Space),
            skip_back: Shortcut::plain(egui::Key::ArrowLeft),
            skip_forward: Shortcut::plain(egui::Key::ArrowRight),
            step_back: Shortcut::shifted(egui::Key::ArrowLeft),
            step_forward: Shortcut::shifted(egui::Key::ArrowRight),
        }
    }
}

impl Keybinds {
    fn shortcut(&self, bind: Bind) -> Shortcut {
        match bind {
            Bind::SetStart => self.set_start,
            Bind::SetEnd => self.set_end,
            Bind::PlayPauseClip => self.play_pause_clip,
            Bind::PlayPause => self.play_pause,
            Bind::SkipBack => self.skip_back,
            Bind::SkipForward => self.skip_forward,
            Bind::StepBack => self.step_back,
            Bind::StepForward => self.step_forward,
        }
    }

    fn put(&mut self, bind: Bind, sc: Shortcut) {
        match bind {
            Bind::SetStart => self.set_start = sc,
            Bind::SetEnd => self.set_end = sc,
            Bind::PlayPauseClip => self.play_pause_clip = sc,
            Bind::PlayPause => self.play_pause = sc,
            Bind::SkipBack => self.skip_back = sc,
            Bind::SkipForward => self.skip_forward = sc,
            Bind::StepBack => self.step_back = sc,
            Bind::StepForward => self.step_forward = sc,
        }
    }

    /// Bind `sc` to `bind`. If another action already uses `sc`, swap so it takes
    /// `bind`'s old shortcut — keeping every action on a distinct shortcut.
    fn rebind(&mut self, bind: Bind, sc: Shortcut) {
        let old = self.shortcut(bind);
        for (other, _) in Bind::ALL {
            if other != bind && self.shortcut(other) == sc {
                self.put(other, old);
            }
        }
        self.put(bind, sc);
    }
}

/// High-contrast text on top of egui's stock dark/light visuals: body/button
/// text is pushed toward the extreme (near-white on dark, near-black on light)
/// and hovered/active widgets go fully to the extreme.
fn themed_visuals(theme: egui::Theme) -> egui::Visuals {
    let (body, button, extreme) = match theme {
        egui::Theme::Dark => (
            egui::Color32::from_gray(225),
            egui::Color32::from_gray(235),
            egui::Color32::WHITE,
        ),
        egui::Theme::Light => (
            egui::Color32::from_gray(30),
            egui::Color32::from_gray(20),
            egui::Color32::BLACK,
        ),
    };
    let mut visuals = theme.default_visuals();
    visuals.widgets.noninteractive.fg_stroke.color = body;
    visuals.widgets.inactive.fg_stroke.color = button;
    visuals.widgets.hovered.fg_stroke.color = extreme;
    visuals.widgets.active.fg_stroke.color = extreme;
    visuals
}

/// Stable string for persisting the theme preference (egui's enum isn't
/// serialized directly, mirroring how keybinds avoid egui's serde feature).
fn theme_pref_name(pref: egui::ThemePreference) -> &'static str {
    match pref {
        egui::ThemePreference::Dark => "dark",
        egui::ThemePreference::Light => "light",
        egui::ThemePreference::System => "system",
    }
}

fn theme_pref_from_name(name: &str) -> egui::ThemePreference {
    match name {
        "dark" => egui::ThemePreference::Dark,
        "light" => egui::ThemePreference::Light,
        _ => egui::ThemePreference::System,
    }
}

/// Label shown in the Settings theme dropdown.
fn theme_pref_label(pref: egui::ThemePreference) -> &'static str {
    match pref {
        egui::ThemePreference::Dark => "Dark",
        egui::ThemePreference::Light => "Light",
        egui::ThemePreference::System => "Match desktop",
    }
}

/// Register both theme palettes + shared spacing on the context, then activate
/// the chosen preference (egui resolves `System` against the desktop theme).
fn apply_theme(ctx: &egui::Context, pref: egui::ThemePreference) {
    ctx.set_visuals_of(egui::Theme::Dark, themed_visuals(egui::Theme::Dark));
    ctx.set_visuals_of(egui::Theme::Light, themed_visuals(egui::Theme::Light));
    ctx.all_styles_mut(|style| style.spacing.button_padding = BUTTON_PAD);
    ctx.set_theme(pref);
}

/// Cross-platform directory for downloaded videos, under eframe's app data dir.
fn managed_cache_dir() -> PathBuf {
    eframe::storage_dir(STORAGE_APP_ID)
        .map(|d| d.join("downloads"))
        .unwrap_or_else(std::env::temp_dir)
}

/// Platform filenames for the bundled tool binaries.
#[cfg(windows)]
const YTDLP_EXE: &str = "yt-dlp.exe";
#[cfg(not(windows))]
const YTDLP_EXE: &str = "yt-dlp";
#[cfg(windows)]
const FFMPEG_EXE: &str = "ffmpeg.exe";
#[cfg(not(windows))]
const FFMPEG_EXE: &str = "ffmpeg";

/// Directory holding the running executable, where a packaged build bundles
/// `yt-dlp`/`ffmpeg` next to the app.
fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

/// Subfolder beside the executable where the Windows bundle stages helper
/// binaries, keeping the app exe alone at the bundle root so it's the only
/// thing a user can click to launch.
const BUNDLED_BIN_SUBDIR: &str = "bin";

/// Locate a bundled tool binary: the `bin/` subfolder beside the exe first
/// (the packaged layout), then directly beside the exe (dev `target/` builds
/// and the AppImage/macOS bundles, which stage tools next to the app).
fn bundled_binary(name: &str) -> Option<PathBuf> {
    let dir = exe_dir()?;
    [dir.join(BUNDLED_BIN_SUBDIR).join(name), dir.join(name)]
        .into_iter()
        .find(|p| p.is_file())
}

/// First directory on `PATH` that contains `name`. Returned as a full path so we
/// invoke it directly rather than letting the OS re-resolve a bare name.
fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// Per-user, owner-only-writable directory for the managed yt-dlp copy. Keeping
/// it non-world-writable stops another local user from swapping the binary we
/// run between seeding and launch.
fn managed_bin_dir() -> Option<PathBuf> {
    let dir = eframe::storage_dir(STORAGE_APP_ID)?.join("bin");
    std::fs::create_dir_all(&dir).ok()?;
    set_owner_only(&dir, 0o755);
    Some(dir)
}

/// On Unix, restrict `path` to the given mode (owner-write, others read/exec);
/// a no-op elsewhere (Windows ACLs already make per-user dirs owner-private).
#[cfg(unix)]
fn set_owner_only(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}
#[cfg(not(unix))]
fn set_owner_only(_path: &Path, _mode: u32) {}

/// Resolve yt-dlp to an absolute path, seeding a writable managed copy so
/// `yt-dlp -U` can update it in place even when the app is installed read-only.
/// Order: managed copy → bundled (`bin/` or next to exe) → PATH.
fn resolve_ytdlp() -> Option<PathBuf> {
    let managed = managed_bin_dir().map(|d| d.join(YTDLP_EXE));
    if let Some(m) = &managed {
        if m.is_file() {
            return Some(m.clone());
        }
    }
    let source = bundled_binary(YTDLP_EXE).or_else(|| find_in_path(YTDLP_EXE))?;
    // Seed the managed copy so updates work; if we can't, run the source binary
    // directly — still by absolute path, never a bare name.
    match &managed {
        Some(m) if std::fs::copy(&source, m).is_ok() => {
            set_owner_only(m, 0o755);
            Some(m.clone())
        }
        _ => Some(source),
    }
}

/// Resolve ffmpeg to an absolute path for `--ffmpeg-location`. No managed copy:
/// ffmpeg never self-updates, so it needn't live in a writable dir.
/// Order: bundled (`bin/` or next to exe) → PATH.
fn resolve_ffmpeg() -> Option<PathBuf> {
    bundled_binary(FFMPEG_EXE).or_else(|| find_in_path(FFMPEG_EXE))
}

/// Total size, in bytes, of the files directly in `dir`.
fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

/// Remove every entry in `dir`, ignoring per-entry errors (e.g. a file still
/// open on Windows).
fn clear_dir(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(&path);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn main() -> eframe::Result<()> {
    // Resolve the tool binaries to absolute paths up front (seeding yt-dlp into a
    // writable managed copy) so we never invoke a bare name the OS could resolve
    // to a planted binary, and yt-dlp merges with exactly the ffmpeg we shipped.
    if let Some(ytdlp_bin) = resolve_ytdlp() {
        ytdlp::set_binary(ytdlp_bin);
    }
    if let Some(ffmpeg_bin) = resolve_ffmpeg() {
        ytdlp::set_ffmpeg(ffmpeg_bin);
    }

    let cli_path = std::env::args().nth(1);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Cooper Clipper")
            .with_inner_size([960.0, 720.0])
            .with_min_inner_size([800.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        STORAGE_APP_ID,
        options,
        Box::new(move |cc| {
            // Register the SVG (and other) image loaders so `egui::Image` /
            // `include_image!` can rasterize the button icons.
            egui_extras::install_image_loaders(&cc.egui_ctx);
            let mut app = App::default();
            if let Some(storage) = cc.storage {
                if let Some(scale) = eframe::get_value::<f32>(storage, SCALE_STORAGE_KEY) {
                    app.ui_scale = scale;
                    app.pending_scale = scale;
                    cc.egui_ctx.set_zoom_factor(scale);
                }
                app.download_dir = eframe::get_value::<Option<PathBuf>>(storage, DOWNLOAD_DIR_KEY)
                    .flatten();
                app.output_dir = eframe::get_value::<Option<PathBuf>>(storage, OUTPUT_DIR_KEY)
                    .flatten();
                app.delete_cache_on_exit =
                    eframe::get_value(storage, DELETE_ON_EXIT_KEY).unwrap_or(false);
                app.open_dir_on_save =
                    eframe::get_value(storage, OPEN_DIR_ON_SAVE_KEY).unwrap_or(false);
                app.volume = eframe::get_value(storage, VOLUME_KEY).unwrap_or(0.5);
                if let Some(name) = eframe::get_value::<String>(storage, THEME_KEY) {
                    app.theme = theme_pref_from_name(&name);
                }
                // Each shortcut persists as (action id, key name, shift). Keyed by
                // a stable id so reordering/adding actions can't misread a save;
                // unknown ids and absent actions just keep their defaults.
                if let Some(saved) =
                    eframe::get_value::<Vec<(String, String, bool)>>(storage, KEYBINDS_KEY)
                {
                    for (id, name, shift) in saved {
                        if let (Some(bind), Some(key)) =
                            (Bind::from_id(&id), egui::Key::from_name(&name))
                        {
                            app.keybinds.put(bind, Shortcut { key, shift });
                        }
                    }
                }
            }
            apply_theme(&cc.egui_ctx, app.theme);
            if let Some(path) = cli_path {
                app.load_video(PathBuf::from(path));
            }
            Ok(Box::new(app))
        }),
    )
}

enum Msg {
    Info(ytdlp::VideoInfo),
    Progress { downloaded: u64, total: u64 },
    Downloaded(PathBuf),
    Exported(PathBuf),
    /// Export aborted via Cancel; carries the partial output path to delete.
    ExportCanceled(PathBuf),
    Error(String),
    /// Result of a background `yt-dlp -U`: `Ok` carries yt-dlp's report,
    /// `Err` its failure text.
    YtdlpUpdated(Result<String, String>),
}

/// A preview navigation requested by the UI, applied after widget borrows end.
enum Nav {
    Back,
    Forward,
    Seek { secs: f64, released: bool },
}

const CONTROL_PAD: f32 = 6.0;
/// Cache-browser thumbnail cell size, and the pixel width thumbnails decode to.
const CACHE_THUMB_W: f32 = 200.0;
const CACHE_THUMB_H: f32 = 120.0;
const THUMB_DECODE_WIDTH: usize = 320;

/// Format seconds as `M:SS.mmm`.
fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0);
    let minutes = (s / 60.0) as u64;
    let rem = s - (minutes as f64) * 60.0;
    format!("{minutes}:{rem:06.3}")
}

/// Dropdown label for an audio export format.
fn audio_format_label(format: export::AudioFormat) -> &'static str {
    match format {
        export::AudioFormat::Mp3 => "MP3",
        export::AudioFormat::Aac => "AAC (.m4a)",
        export::AudioFormat::Original => "Original (lossless)",
    }
}

/// Compact duration as `M:SS`.
fn fmt_duration(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Make a string safe to use as a filename across platforms.
fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .take(100)
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    if trimmed.is_empty() {
        "video".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Format a byte count as MB/GB (decimal units).
fn fmt_size(bytes: u64) -> String {
    const MB: f64 = 1_000_000.0;
    const GB: f64 = 1_000_000_000.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else {
        format!("{:.1} MB", b / MB)
    }
}

enum DecodeRequest {
    Seek { secs: f64, gen: u64 },
    StepForward { gen: u64 },
    StepBackward { gen: u64 },
}

enum DecodeEvent {
    Opened { width: u32, height: u32, duration_secs: f64, fps: f64 },
    /// `gen` echoes the request that produced this frame, so the UI can ignore
    /// frames from superseded seeks (e.g. mid-drag decodes after a release).
    Frame { image: egui::ColorImage, secs: f64, gen: u64 },
    Error(String),
}

/// Owns the `Decoder` on a worker thread so decoding never blocks the UI. The
/// decoder is created and used entirely on that thread (it never crosses a
/// thread boundary); only frames and metadata are sent back.
struct DecoderHandle {
    req_tx: Sender<DecodeRequest>,
    event_rx: Receiver<DecodeEvent>,
    width: u32,
    height: u32,
    duration_secs: f64,
    fps: f64,
    current_secs: f64,
    ready: bool,
    /// Monotonic request id; each request returns the gen it was tagged with.
    gen: AtomicU64,
}

impl DecoderHandle {
    fn spawn(path: String) -> Self {
        let (req_tx, req_rx) = channel();
        let (event_tx, event_rx) = channel();
        thread::spawn(move || decoder_loop(path, req_rx, event_tx));
        Self {
            req_tx,
            event_rx,
            width: 0,
            height: 0,
            duration_secs: 0.0,
            fps: 30.0,
            current_secs: 0.0,
            ready: false,
            gen: AtomicU64::new(1),
        }
    }

    fn next_gen(&self) -> u64 {
        self.gen.fetch_add(1, Ordering::Relaxed)
    }

    fn seek_secs(&self, secs: f64) -> u64 {
        let gen = self.next_gen();
        let _ = self.req_tx.send(DecodeRequest::Seek { secs, gen });
        gen
    }
    fn step_forward(&self) -> u64 {
        let gen = self.next_gen();
        let _ = self.req_tx.send(DecodeRequest::StepForward { gen });
        gen
    }
    fn step_backward(&self) -> u64 {
        let gen = self.next_gen();
        let _ = self.req_tx.send(DecodeRequest::StepBackward { gen });
        gen
    }
}

/// Open the file, emit metadata + first frame, then serve decode requests.
/// Pending requests are coalesced to the latest so a fast drag never backlogs.
fn decoder_loop(path: String, req_rx: Receiver<DecodeRequest>, event_tx: Sender<DecodeEvent>) {
    let mut dec = match Decoder::open(&path) {
        Ok(dec) => dec,
        Err(e) => {
            let _ = event_tx.send(DecodeEvent::Error(e.to_string()));
            return;
        }
    };
    let opened = DecodeEvent::Opened {
        width: dec.width,
        height: dec.height,
        duration_secs: dec.duration_secs(),
        fps: dec.fps(),
    };
    if event_tx.send(opened).is_err() {
        return;
    }
    if let Some(image) = dec.step_forward() {
        let _ = event_tx.send(DecodeEvent::Frame { image, secs: dec.current_secs(), gen: 0 });
    }

    while let Ok(first) = req_rx.recv() {
        // Fold the first request and any already-queued ones. Seeks collapse to
        // the latest (a fast drag never backlogs); steps accumulate into a net
        // frame delta so rapid/held stepping isn't dropped — and a multi-frame
        // backward jump becomes a single seek rather than one seek per frame.
        let mut seek: Option<f64> = None;
        let mut steps: i64 = 0;
        let mut gen = 0;
        let mut req = Some(first);
        while let Some(r) = req {
            match r {
                DecodeRequest::Seek { secs, gen: g } => {
                    seek = Some(secs);
                    steps = 0;
                    gen = g;
                }
                DecodeRequest::StepForward { gen: g } => {
                    steps += 1;
                    gen = g;
                }
                DecodeRequest::StepBackward { gen: g } => {
                    steps -= 1;
                    gen = g;
                }
            }
            req = req_rx.try_recv().ok();
        }

        let mut image = None;
        if let Some(secs) = seek {
            image = dec.seek_secs(secs);
        }
        if steps != 0 {
            image = dec.step_by(steps);
        }
        if let Some(image) = image {
            if event_tx
                .send(DecodeEvent::Frame { image, secs: dec.current_secs(), gen })
                .is_err()
            {
                return;
            }
        }
    }
}

/// One cached video shown in the cache browser.
struct CacheEntry {
    path: PathBuf,
    name: String,
    size: u64,
    duration: Option<f64>,
    tex: Option<egui::TextureHandle>,
}

/// What the thumbnail worker sends back per file.
struct CacheThumb {
    path: PathBuf,
    image: egui::ColorImage,
    duration_secs: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downscale_preserves_aspect_and_is_in_bounds() {
        let src = egui::ColorImage {
            size: [640, 480],
            pixels: vec![egui::Color32::WHITE; 640 * 480],
        };
        let out = downscale_thumb(&src, 320);
        assert_eq!(out.size, [320, 240]);
        assert_eq!(out.pixels.len(), 320 * 240);
    }

    #[test]
    fn downscale_skips_when_already_small() {
        let src = egui::ColorImage {
            size: [100, 50],
            pixels: vec![egui::Color32::BLACK; 100 * 50],
        };
        assert_eq!(downscale_thumb(&src, 320).size, [100, 50]);
    }
}

fn is_video_file(path: &Path) -> bool {
    const EXTS: [&str; 7] = ["mp4", "webm", "mkv", "mov", "m4v", "avi", "ts"];
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .map_or(false, |e| EXTS.contains(&e.as_str()))
}

/// Nearest-neighbour downscale to `max_w` wide, preserving aspect ratio.
fn downscale_thumb(img: &egui::ColorImage, max_w: usize) -> egui::ColorImage {
    let [w, h] = img.size;
    if w == 0 || h == 0 || w <= max_w {
        return img.clone();
    }
    let new_w = max_w;
    let new_h = (h * max_w / w).max(1);
    let mut pixels = Vec::with_capacity(new_w * new_h);
    for y in 0..new_h {
        let sy = y * h / new_h;
        for x in 0..new_w {
            pixels.push(img.pixels[sy * w + x * w / new_w]);
        }
    }
    egui::ColorImage {
        size: [new_w, new_h],
        pixels,
    }
}

/// Decode one representative frame per file (10% in, to skip black intros) and
/// stream downscaled thumbnails back.
fn cache_thumbnails(paths: Vec<PathBuf>, tx: Sender<CacheThumb>) {
    for path in paths {
        let Ok(mut dec) = Decoder::open(&path.to_string_lossy()) else {
            continue;
        };
        let duration_secs = dec.duration_secs();
        let frame = dec.seek_secs(duration_secs * 0.1).or_else(|| dec.step_forward());
        if let Some(image) = frame {
            let thumb = CacheThumb {
                path,
                image: downscale_thumb(&image, THUMB_DECODE_WIDTH),
                duration_secs,
            };
            if tx.send(thumb).is_err() {
                return;
            }
        }
    }
}

struct App {
    url: String,
    info: Option<ytdlp::VideoInfo>,
    /// Download resolution cap; `None` means "Best" (the source's tallest).
    selected_height: Option<u32>,
    want_video: bool,
    want_audio: bool,
    status: String,
    /// Path of the most recently saved export; drives the "Open folder" button
    /// next to the status line. `None` until a save succeeds.
    saved_path: Option<PathBuf>,
    /// Full text of the last failed operation (yt-dlp fetch/download or export),
    /// shown in a dismissable error panel; `None` once cleared or after a success.
    last_error: Option<String>,
    /// True while a background `yt-dlp -U` is running.
    ytdlp_updating: bool,
    /// Cached `yt-dlp --version` for the Settings panel; fetched lazily, and
    /// cleared after an update so it refetches.
    ytdlp_version: Option<String>,
    rx: Option<Receiver<Msg>>,

    decoder: Option<DecoderHandle>,
    video_path: Option<PathBuf>,
    /// Suggested export name: yt-dlp title when downloaded, else the file stem.
    video_title: Option<String>,
    frame_tex: Option<egui::TextureHandle>,
    in_secs: f64,
    out_secs: f64,

    ui_scale: f32,
    /// Scale being edited in Settings; applied to `ui_scale` only on Apply.
    pending_scale: f32,
    /// Light/Dark/System appearance; `System` follows the desktop theme.
    theme: egui::ThemePreference,
    show_settings: bool,
    /// Configurable shortcuts for clip + playback actions.
    keybinds: Keybinds,
    /// In Settings, the action whose next keypress is being captured, if any.
    rebinding: Option<Bind>,
    /// Per nav action (skip back/fwd, step back/fwd) the input time at which a
    /// held key may next fire; `0.0` means "not held" so the next press fires now.
    nav_repeat_at: [f64; 4],
    /// Active download progress as `(downloaded, total)` bytes, if downloading.
    progress: Option<(u64, u64)>,
    /// True while an export (compile + save) runs on the worker thread; drives
    /// an indeterminate progress bar since the encode reports no fraction.
    exporting: bool,
    /// Destination of the in-progress export, polled for its growing size to show
    /// a "X so far" readout next to the bar; `None` when not exporting.
    export_path: Option<PathBuf>,
    /// Cancel flag for the in-flight export; set by the Cancel button, checked by
    /// the encode loop. A fresh flag is created per export.
    export_cancel: Arc<AtomicBool>,

    /// Where downloads are saved; `None` uses the managed cache directory.
    download_dir: Option<PathBuf>,
    /// Default folder the export save dialog opens in; `None` uses the system
    /// default. The dialog is always shown either way.
    output_dir: Option<PathBuf>,
    /// Clear the managed cache directory when the app closes.
    delete_cache_on_exit: bool,
    /// Reveal the saved file's folder in the system file manager after a save.
    open_dir_on_save: bool,

    show_cache: bool,
    cache_entries: Vec<CacheEntry>,
    cache_rx: Option<Receiver<CacheThumb>>,

    /// Target format for "Save audio only".
    audio_format: export::AudioFormat,
    /// Target container for "Save clip" / "Save full video".
    video_format: export::VideoFormat,
    /// Downscale height for saved video; `None` keeps the source resolution.
    export_height: Option<u32>,

    playing: bool,
    /// When set, playback stops once the master clock reaches this position
    /// (used by "Play Clip" to stop at the out point); `None` plays to the end.
    play_until: Option<f64>,
    /// Output volume in `0.0..=1.0`, applied to audio playback.
    volume: f32,
    /// Audio output during playback; `None` means play video without sound.
    audio: Option<AudioPlayer>,
    /// Master-clock origin for video-only playback (no audio device/track):
    /// the egui time and video position captured when playback started.
    play_start_wall: f64,
    play_start_pos: f64,
    /// After releasing a timeline drag: `(gen, position)` of the seek we're
    /// waiting to land on. The playhead stays here and earlier decodes are
    /// dropped until the frame with this gen arrives.
    awaiting_release: Option<(u64, f64)>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            url: String::new(),
            info: None,
            selected_height: None,
            want_video: true,
            want_audio: true,
            status: String::new(),
            saved_path: None,
            last_error: None,
            ytdlp_updating: false,
            ytdlp_version: None,
            rx: None,
            decoder: None,
            video_path: None,
            video_title: None,
            frame_tex: None,
            in_secs: 0.0,
            out_secs: 0.0,
            ui_scale: 1.0,
            pending_scale: 1.0,
            theme: egui::ThemePreference::System,
            show_settings: false,
            keybinds: Keybinds::default(),
            rebinding: None,
            nav_repeat_at: [0.0; 4],
            progress: None,
            exporting: false,
            export_path: None,
            export_cancel: Arc::new(AtomicBool::new(false)),
            download_dir: None,
            output_dir: None,
            delete_cache_on_exit: false,
            open_dir_on_save: false,
            show_cache: false,
            cache_entries: Vec::new(),
            cache_rx: None,
            audio_format: export::AudioFormat::Mp3,
            video_format: export::VideoFormat::Mp4,
            export_height: None,
            playing: false,
            play_until: None,
            volume: 0.5,
            audio: None,
            play_start_wall: 0.0,
            play_start_pos: 0.0,
            awaiting_release: None,
        }
    }
}

impl App {
    /// Run `work` on a background thread, handing it the channel so it can send
    /// progress updates plus a final terminal message.
    fn spawn<F>(&mut self, work: F)
    where
        F: FnOnce(Sender<Msg>) + Send + 'static,
    {
        let (tx, rx) = channel();
        self.rx = Some(rx);
        thread::spawn(move || work(tx));
    }

    fn poll(&mut self) {
        let Some(rx) = self.rx.take() else { return };
        loop {
            match rx.try_recv() {
                Ok(Msg::Progress { downloaded, total }) => {
                    self.progress = Some((downloaded, total))
                }
                Ok(Msg::Info(info)) => {
                    self.status = format!("{} — {} formats", info.title, info.formats.len());
                    self.last_error = None;
                    self.selected_height = None;
                    self.info = Some(info);
                    self.progress = None;
                    return;
                }
                Ok(Msg::Downloaded(path)) => {
                    self.status = format!("downloaded: {}", path.display());
                    self.last_error = None;
                    self.progress = None;
                    let title = self
                        .info
                        .as_ref()
                        .map(|i| i.title.clone())
                        .filter(|t| !t.is_empty());
                    self.load_video(path);
                    if let Some(title) = title {
                        self.video_title = Some(title);
                    }
                    return;
                }
                Ok(Msg::Exported(path)) => {
                    self.status = format!("saved: {}", path.display());
                    self.last_error = None;
                    self.progress = None;
                    self.exporting = false;
                    self.export_path = None;
                    if self.open_dir_on_save {
                        reveal_in_file_manager(&path);
                    }
                    self.saved_path = Some(path);
                    return;
                }
                Ok(Msg::ExportCanceled(path)) => {
                    // Drop the incomplete output so no truncated file is left behind.
                    let _ = std::fs::remove_file(&path);
                    self.status = "export canceled".into();
                    self.progress = None;
                    self.exporting = false;
                    self.export_path = None;
                    return;
                }
                Ok(Msg::Error(e)) => {
                    // Keep the full text for the error panel; the status line just
                    // flags that something failed.
                    self.status = "error".into();
                    self.last_error = Some(e);
                    self.progress = None;
                    self.exporting = false;
                    self.export_path = None;
                    return;
                }
                Ok(Msg::YtdlpUpdated(result)) => {
                    self.ytdlp_updating = false;
                    self.ytdlp_version = None;
                    match result {
                        Ok(report) => {
                            self.status = format!("yt-dlp: {report}");
                            self.last_error = None;
                        }
                        Err(e) => self.last_error = Some(e),
                    }
                    return;
                }
                // Worker still running: keep the receiver for the next frame.
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.rx = Some(rx);
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            }
        }
    }

    /// The directory downloads are written to (custom override or cache).
    fn effective_download_dir(&self) -> PathBuf {
        self.download_dir.clone().unwrap_or_else(managed_cache_dir)
    }

    fn load_video(&mut self, path: PathBuf) {
        self.video_title = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned());
        self.decoder = Some(DecoderHandle::spawn(path.to_string_lossy().into_owned()));
        self.video_path = Some(path);
        self.in_secs = 0.0;
        self.out_secs = 0.0;
        self.frame_tex = None;
        self.stop_play();
    }

    /// Stop playback and release the audio output.
    fn stop_play(&mut self) {
        self.playing = false;
        self.play_until = None;
        self.audio = None;
        self.awaiting_release = None;
    }

    /// Seek `delta` seconds from the current position (clamped to the video) and
    /// pin the playhead to the target until that frame lands. Builds on the last
    /// *requested* target (a still-in-flight seek), not the decoder's reported
    /// position, which lags on long videos — so a held key keeps advancing
    /// instead of stalling on a stale `current_secs`.
    fn skip_secs(&mut self, delta: f64) {
        let Some((base, dur)) = self.decoder.as_ref().map(|dec| {
            let base = self.awaiting_release.map_or(dec.current_secs, |(_, pos)| pos);
            (base, dec.duration_secs)
        }) else {
            return;
        };
        let target = (base + delta).clamp(0.0, dur);
        self.stop_play();
        if let Some(gen) = self.decoder.as_ref().map(|dec| dec.seek_secs(target)) {
            self.awaiting_release = Some((gen, target));
        }
    }

    /// Step exactly one frame forward or backward.
    fn step_frame(&mut self, forward: bool) {
        self.stop_play();
        if let Some(dec) = self.decoder.as_ref() {
            if forward {
                dec.step_forward();
            } else {
                dec.step_backward();
            }
        }
    }

    /// Start playback from the current position (with audio if available), or
    /// stop if already playing.
    fn toggle_play(&mut self, now: f64) {
        if self.playing {
            self.stop_play();
            return;
        }
        let Some(pos) = self.decoder.as_ref().filter(|d| d.ready).map(|d| d.current_secs) else {
            return;
        };
        self.playing = true;
        self.play_until = None;
        self.awaiting_release = None;
        self.play_start_wall = now;
        self.play_start_pos = pos;
        let volume = self.volume;
        self.audio = self
            .video_path
            .as_ref()
            .and_then(|p| AudioPlayer::start(&p.to_string_lossy(), pos, volume).ok());
    }

    /// Play/pause within the clip: pause if playing, else play to the out point,
    /// resuming from the current spot when it's inside the clip and otherwise
    /// starting from the in point.
    fn toggle_play_clip(&mut self, now: f64) {
        if self.playing {
            self.stop_play();
            return;
        }
        let Some(cur) = self.decoder.as_ref().filter(|d| d.ready).map(|d| d.current_secs) else {
            return;
        };
        let start = if (self.in_secs..self.out_secs).contains(&cur) {
            cur
        } else {
            self.in_secs
        };
        self.play_from(start, Some(self.out_secs), now);
    }

    /// Jump to `pos` and start playing from exactly there, stopping at `until`
    /// (the clip out point) if given, otherwise at the end of the video.
    fn play_from(&mut self, pos: f64, until: Option<f64>, now: f64) {
        if !self.decoder.as_ref().is_some_and(|d| d.ready) {
            return;
        }
        if let Some(dec) = &self.decoder {
            dec.seek_secs(pos);
        }
        self.awaiting_release = None;
        self.playing = true;
        self.play_until = until;
        self.play_start_wall = now;
        self.play_start_pos = pos;
        let volume = self.volume;
        self.audio = self
            .video_path
            .as_ref()
            .and_then(|p| AudioPlayer::start(&p.to_string_lossy(), pos, volume).ok());
    }

    /// Playback position of the master clock: audio if present, else wall time.
    fn master_clock(&self, now: f64) -> f64 {
        match &self.audio {
            Some(audio) => audio.clock_secs(),
            None => self.play_start_pos + (now - self.play_start_wall),
        }
    }

    /// Drain frames/metadata from the decoder thread, uploading the newest frame.
    fn poll_decoder(&mut self, ctx: &egui::Context) {
        let awaiting = self.awaiting_release;
        let mut latest_frame = None;
        let mut opened = None;
        let mut error = None;
        let mut landed = false;
        if let Some(dec) = &self.decoder {
            loop {
                match dec.event_rx.try_recv() {
                    Ok(DecodeEvent::Opened { width, height, duration_secs, fps }) => {
                        opened = Some((width, height, duration_secs, fps))
                    }
                    Ok(DecodeEvent::Frame { image, secs, gen }) => match awaiting {
                        // Waiting on a released seek: take only its frame, drop
                        // any superseded mid-drag decodes still arriving.
                        Some((await_gen, _)) => {
                            if gen == await_gen {
                                latest_frame = Some((image, secs));
                                landed = true;
                            }
                        }
                        None => latest_frame = Some((image, secs)),
                    },
                    Ok(DecodeEvent::Error(e)) => error = Some(e),
                    Err(_) => break,
                }
            }
        }
        if landed {
            self.awaiting_release = None;
        }

        if let Some((width, height, duration_secs, fps)) = opened {
            if let Some(dec) = &mut self.decoder {
                dec.width = width;
                dec.height = height;
                dec.duration_secs = duration_secs;
                dec.fps = fps;
                dec.ready = true;
            }
            self.in_secs = 0.0;
            self.out_secs = duration_secs;
        }
        if let Some((image, secs)) = latest_frame {
            if let Some(dec) = &mut self.decoder {
                dec.current_secs = secs;
            }
            self.set_frame(ctx, image);
        }
        if let Some(e) = error {
            self.status = format!("decode error: {e}");
            self.decoder = None;
        }
    }

    /// Upload a freshly decoded frame to the preview texture.
    fn set_frame(&mut self, ctx: &egui::Context, img: egui::ColorImage) {
        let tex = ctx.load_texture("frame", img, egui::TextureOptions::LINEAR);
        self.frame_tex = Some(tex);
    }

    /// List the cached videos and kick off background thumbnail decoding.
    fn open_cache_browser(&mut self) {
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
        self.cache_entries = paths
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
        self.cache_rx = Some(rx);
        thread::spawn(move || cache_thumbnails(paths, tx));
        self.show_cache = true;
    }

    /// Attach thumbnails that the worker has finished decoding.
    fn poll_cache(&mut self, ctx: &egui::Context) {
        let mut ready = Vec::new();
        let mut done = false;
        if let Some(rx) = &self.cache_rx {
            loop {
                match rx.try_recv() {
                    Ok(item) => ready.push(item),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
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
            if let Some(entry) = self.cache_entries.iter_mut().find(|e| e.path == thumb.path) {
                entry.tex = Some(tex);
                entry.duration = Some(thumb.duration_secs);
            }
        }
        if done {
            self.cache_rx = None;
        }
    }

    /// Grid of cached videos with thumbnails; clicking one loads it.
    fn cache_window(&mut self, ctx: &egui::Context) {
        if !self.show_cache {
            return;
        }
        let mut open = true;
        let mut selected = None;
        egui::Window::new("Cached videos")
            .open(&mut open)
            .default_size([680.0, 460.0])
            .show(ctx, |ui| {
                if self.cache_entries.is_empty() {
                    ui.label("No cached videos found.");
                    return;
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for entry in &self.cache_entries {
                            ui.allocate_ui(egui::vec2(CACHE_THUMB_W, CACHE_THUMB_H + 44.0), |ui| {
                                ui.vertical(|ui| {
                                    ui.set_max_width(CACHE_THUMB_W);
                                    let clicked = match &entry.tex {
                                        Some(tex) => ui
                                            .add(egui::ImageButton::new(
                                                egui::Image::new(tex).max_size(egui::vec2(
                                                    CACHE_THUMB_W,
                                                    CACHE_THUMB_H,
                                                )),
                                            ))
                                            .clicked(),
                                        None => ui
                                            .add_sized(
                                                [CACHE_THUMB_W, CACHE_THUMB_H],
                                                egui::Button::new("decoding…"),
                                            )
                                            .clicked(),
                                    };
                                    ui.label(&entry.name);
                                    let dur = entry
                                        .duration
                                        .map_or_else(|| "—".to_owned(), fmt_duration);
                                    ui.weak(format!("{}  ·  {}", fmt_size(entry.size), dur));
                                    if clicked {
                                        selected = Some(entry.path.clone());
                                    }
                                });
                            });
                        }
                    });
                });
            });

        self.show_cache = open;
        if let Some(path) = selected {
            self.load_video(path);
            self.show_cache = false;
            self.url.clear();
            self.status.clear();
        }
        if !self.show_cache {
            self.cache_entries.clear();
            self.cache_rx = None;
        }
    }

    /// Prompt for a destination and run the export on a background thread. A
    /// configured output folder just preselects where the save dialog opens.
    fn start_export(&mut self, mode: Mode, ext: &str) {
        let Some(input) = self.video_path.clone() else { return };
        let base = self.video_title.as_deref().unwrap_or("video");
        let stem = sanitize_filename(base);
        let mut dialog = rfd::FileDialog::new().set_file_name(format!("{stem}.{ext}"));
        if let Some(dir) = &self.output_dir {
            dialog = dialog.set_directory(dir);
        }
        let Some(out) = dialog.save_file() else {
            return;
        };

        // Resolution applies to saved video only; audio-only ignores it.
        let scale_height = if matches!(mode, Mode::AudioOnly(_)) {
            None
        } else {
            self.export_height
        };
        let spec = ExportSpec {
            input: input.to_string_lossy().into_owned(),
            output: out.to_string_lossy().into_owned(),
            start_secs: self.in_secs,
            end_secs: self.out_secs,
            mode,
            scale_height,
        };
        self.status = "exporting…".into();
        self.exporting = true;
        self.saved_path = None;
        self.export_path = Some(out);
        let cancel = Arc::new(AtomicBool::new(false));
        self.export_cancel = cancel.clone();
        self.spawn(move |tx| {
            let output = PathBuf::from(spec.output.as_str());
            let msg = match export::export_cancellable(&spec, &cancel) {
                Ok(()) => Msg::Exported(output),
                // A cancel makes the encode loop bail; tell them apart by the flag.
                Err(_) if cancel.load(Ordering::Relaxed) => Msg::ExportCanceled(output),
                Err(e) => Msg::Error(e.to_string()),
            };
            let _ = tx.send(msg);
        });
    }

    /// Run `yt-dlp -U` on a worker thread; the result arrives as `YtdlpUpdated`.
    fn start_ytdlp_update(&mut self) {
        if self.ytdlp_updating {
            return;
        }
        self.ytdlp_updating = true;
        self.status = "updating yt-dlp…".into();
        self.spawn(|tx| {
            let _ = tx.send(Msg::YtdlpUpdated(ytdlp::update().map_err(|e| e.to_string())));
        });
    }

    /// Show the last failure (full yt-dlp/export text) with Copy/Dismiss and,
    /// when the message looks like an outdated binary, an "Update yt-dlp" button.
    fn error_panel(&mut self, ui: &mut egui::Ui) {
        let Some(err) = self.last_error.clone() else { return };
        const ERROR_MAX_HEIGHT: f32 = 120.0;
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.colored_label(egui::Color32::LIGHT_RED, "Error");
            if ytdlp::suggests_update(&err) {
                let label = if self.ytdlp_updating { "Updating…" } else { "Update yt-dlp" };
                if ui.add_enabled(!self.ytdlp_updating, egui::Button::new(label)).clicked() {
                    self.start_ytdlp_update();
                }
            }
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(err.clone());
            }
            if ui.button("Dismiss").clicked() {
                self.last_error = None;
            }
        });
        egui::ScrollArea::vertical()
            .max_height(ERROR_MAX_HEIGHT)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(&err).monospace().color(egui::Color32::LIGHT_RED))
                        .wrap(),
                );
            });
    }

    /// Floating settings window. The scale slider edits a pending value and is
    /// only applied on Apply, so the UI doesn't reflow under the cursor mid-drag.
    fn settings_window(&mut self, ctx: &egui::Context) {
        /// Fixed content width so the panel reads roomy and the keyboard
        /// shortcuts fit comfortably in two columns.
        const SETTINGS_WIDTH: f32 = 620.0;
        /// Vertical breathing room placed around each section separator.
        const SECTION_GAP: f32 = 8.0;
        /// Width reserved for the scale slider's value field so the rail can fill
        /// the rest of its column without the field overflowing.
        const SCALE_VALUE_W: f32 = 64.0;

        let current = self.ui_scale;
        let mut pending = self.pending_scale;
        let mut apply_to = None;

        let cache_dir = managed_cache_dir();
        let cache_bytes = dir_size(&cache_dir);
        let effective_dir = self.effective_download_dir();
        let mut delete_on_exit = self.delete_cache_on_exit;
        let mut new_download_dir: Option<Option<PathBuf>> = None;
        let output_display = match &self.output_dir {
            Some(d) => d.display().to_string(),
            None => "Not set — dialog opens at the system default".to_owned(),
        };
        let mut new_output_dir: Option<Option<PathBuf>> = None;
        let mut open_dir_on_save = self.open_dir_on_save;
        let mut clear_cache = false;
        let keybinds = self.keybinds;
        let mut rebinding = self.rebinding;
        let mut theme = self.theme;

        // Lazily resolve the version once per open (cleared after an update so it
        // refetches); errors are cached too so it doesn't re-probe every frame.
        if self.ytdlp_version.is_none() {
            self.ytdlp_version = Some(match ytdlp::version() {
                Ok(v) => v,
                Err(e) => format!("unavailable ({e})"),
            });
        }
        let ytdlp_version = self.ytdlp_version.clone().unwrap_or_default();
        let ytdlp_updating = self.ytdlp_updating;
        let mut start_update = false;

        egui::Window::new("Settings")
            .open(&mut self.show_settings)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.set_width(SETTINGS_WIDTH);
                // Interface scale (left) and Theme (right) share one row, with a
                // vertical divider painted down the gutter between them.
                let row = ui
                    .scope(|ui| {
                        ui.columns(2, |cols| {
                            let ui = &mut cols[0];
                            ui.label("Interface scale");
                            let slider_w = (ui.available_width() - SCALE_VALUE_W).max(0.0);
                            ui.spacing_mut().slider_width = slider_w;
                            // Match the slider's editable value box to the button
                            // height so it lines up with Apply/Reset below.
                            ui.spacing_mut().interact_size.y = button_height(ui);
                            ui.add(
                                egui::Slider::new(&mut pending, 0.75..=2.5)
                                    .step_by(0.05)
                                    .suffix("×"),
                            );
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                let changed = (pending - current).abs() > f32::EPSILON;
                                ui.label(format!("current: {current:.2}×"));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.button("Reset").clicked() {
                                            apply_to = Some(1.0);
                                        }
                                        if ui
                                            .add_enabled(changed, egui::Button::new("Apply"))
                                            .clicked()
                                        {
                                            apply_to = Some(pending);
                                        }
                                    },
                                );
                            });

                            let ui = &mut cols[1];
                            ui.label("Theme");
                            egui::ComboBox::from_id_salt("theme_select")
                                .selected_text(theme_pref_label(theme))
                                .show_ui(ui, |ui| {
                                    for pref in [
                                        egui::ThemePreference::System,
                                        egui::ThemePreference::Light,
                                        egui::ThemePreference::Dark,
                                    ] {
                                        ui.selectable_value(&mut theme, pref, theme_pref_label(pref));
                                    }
                                });
                            ui.small("“Match desktop” follows your OS light/dark setting.");
                        });
                    })
                    .response
                    .rect;
                ui.painter().vline(
                    row.center().x,
                    egui::Rangef::new(row.top(), row.bottom()),
                    ui.visuals().widgets.noninteractive.bg_stroke,
                );

                ui.add_space(SECTION_GAP);
                ui.separator();
                ui.add_space(SECTION_GAP);
                ui.horizontal(|ui| {
                    ui.label("Downloads location:");
                    ui.label(effective_dir.display().to_string());
                });
                ui.horizontal(|ui| {
                    if ui.button("Choose folder…").clicked() {
                        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                            new_download_dir = Some(Some(folder));
                        }
                    }
                    if ui.button("Use default cache").clicked() {
                        new_download_dir = Some(None);
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(format!("Cache: {}", fmt_size(cache_bytes)));
                    if ui.button("Clear downloads").clicked() {
                        clear_cache = true;
                    }
                });
                ui.checkbox(&mut delete_on_exit, "Delete cache on exit");
                ui.small("Clearing affects only the cache, not a custom folder.");

                ui.add_space(SECTION_GAP);
                ui.separator();
                ui.add_space(SECTION_GAP);
                ui.horizontal(|ui| {
                    ui.label("Output location:");
                    ui.label(output_display.as_str());
                });
                ui.horizontal(|ui| {
                    if ui.button("Choose folder…").clicked() {
                        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                            new_output_dir = Some(Some(folder));
                        }
                    }
                    if ui.button("Clear").clicked() {
                        new_output_dir = Some(None);
                    }
                });
                ui.checkbox(&mut open_dir_on_save, "Open output folder after saving");

                ui.add_space(SECTION_GAP);
                ui.separator();
                ui.add_space(SECTION_GAP);
                ui.label("Keyboard shortcuts");
                ui.add_space(4.0);
                // Two columns: each grid row holds two actions, label left and
                // its key button right within each half.
                let mut bind_row = |ui: &mut egui::Ui, bind: Bind, label: &str| {
                    ui.label(label);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let text = if rebinding == Some(bind) {
                            "Press a key…".to_owned()
                        } else {
                            keybinds.shortcut(bind).label()
                        };
                        if ui.button(text).clicked() {
                            rebinding = Some(bind);
                        }
                    });
                };
                for pair in Bind::ALL.chunks(2) {
                    ui.columns(2, |cols| {
                        for (col, (bind, label)) in cols.iter_mut().zip(pair) {
                            col.horizontal(|ui| bind_row(ui, *bind, *label));
                        }
                    });
                    ui.add_space(4.0);
                }
                ui.small("Click a key, then press the new key. Esc cancels.");

                ui.add_space(SECTION_GAP);
                ui.separator();
                ui.add_space(SECTION_GAP);
                ui.label("yt-dlp");
                ui.horizontal(|ui| {
                    ui.label(format!("Version: {ytdlp_version}"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if ytdlp_updating { "Updating…" } else { "Update" };
                        if ui.add_enabled(!ytdlp_updating, egui::Button::new(label)).clicked() {
                            start_update = true;
                        }
                    });
                });
                ui.small("Update fixes most download failures when a site changes.");
            });

        if start_update {
            self.start_ytdlp_update();
        }
        self.pending_scale = pending;
        if let Some(scale) = apply_to {
            self.ui_scale = scale;
            self.pending_scale = scale;
            ctx.set_zoom_factor(scale);
        }
        if theme != self.theme {
            self.theme = theme;
            apply_theme(ctx, theme);
        }
        self.delete_cache_on_exit = delete_on_exit;
        self.open_dir_on_save = open_dir_on_save;
        if let Some(dir) = new_download_dir {
            self.download_dir = dir;
        }
        if let Some(dir) = new_output_dir {
            self.output_dir = dir;
        }
        if clear_cache {
            clear_dir(&cache_dir);
        }

        // While an action is capturing, the next key press rebinds it (Esc
        // cancels). The main shortcut handler is suppressed during capture.
        self.keybinds = keybinds;
        if let Some(bind) = rebinding {
            let captured = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => {
                        Some((*key, modifiers.shift))
                    }
                    _ => None,
                })
            });
            if let Some((key, shift)) = captured {
                if key != egui::Key::Escape {
                    self.keybinds.rebind(bind, Shortcut { key, shift });
                }
                rebinding = None;
            }
        }
        self.rebinding = rebinding;
    }

    /// Top toolbar: URL/download, format picker, settings.
    fn toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.add_space(4.0);
            // Fixed-height row so the label, field, and buttons all center
            // vertically (plain `horizontal` leaves the short label top-aligned
            // once the taller field grows the row).
            let row_h = button_height(ui);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), row_h),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.label("URL:");
                    let url_field = ui.add_sized(
                        [260.0, row_h],
                        egui::TextEdit::singleline(&mut self.url).margin(INPUT_MARGIN),
                    );
                    let submitted =
                        url_field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let fetch = icon_button(ui, download_icon(), "Fetch");
                    if (fetch.clicked() || submitted) && !self.url.is_empty() {
                        let url = self.url.clone();
                        self.status = "fetching…".into();
                        self.spawn(move |tx| {
                            let _ = tx.send(match ytdlp::fetch_info(&url) {
                                Ok(info) => Msg::Info(info),
                                Err(e) => Msg::Error(e.to_string()),
                            });
                        });
                    }
                    if ui.button("Open file…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_file() {
                            self.load_video(p);
                            self.url.clear();
                            self.status.clear();
                        }
                    }
                    if ui.button("Open from cache…").clicked() {
                        self.open_cache_browser();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_button(ui, settings_icon(), "Settings").clicked() {
                            self.show_settings = true;
                            self.pending_scale = self.ui_scale;
                        }
                    });
                },
            );

            // Snapshot the available heights so the row closure doesn't hold a
            // borrow of `self.info` while also mutating `self`. These come from
            // the source's formats, so the menu never exceeds the real maximum.
            let heights: Vec<u32> = self
                .info
                .as_ref()
                .map(ytdlp::available_heights)
                .unwrap_or_default();

            // Estimated size of the current selection, shown before downloading.
            let est_size = self.info.as_ref().and_then(|info| {
                ytdlp::estimated_size(info, self.selected_height, self.want_video, self.want_audio)
            });

            if self.info.is_some() {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.want_video, "Video");
                    ui.checkbox(&mut self.want_audio, "Audio");

                    // "Source" picks the tallest stream; label it with that height.
                    let source_label = match heights.first() {
                        Some(h) => format!("Source ({h}p)"),
                        None => "Source".to_string(),
                    };
                    let selected_text = match self.selected_height {
                        None => source_label.clone(),
                        Some(h) => format!("{h}p"),
                    };
                    egui::ComboBox::from_id_salt("download_resolution")
                        .selected_text(selected_text)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.selected_height, None, &source_label);
                            // Skip the tallest height; "Source" already covers it.
                            for h in heights.iter().skip(1) {
                                ui.selectable_value(
                                    &mut self.selected_height,
                                    Some(*h),
                                    format!("{h}p"),
                                );
                            }
                        });

                    if ui.button("Download").clicked() {
                        let selector = ytdlp::resolution_selector(
                            self.selected_height,
                            self.want_video,
                            self.want_audio,
                        );
                        let url = self.url.clone();
                        let dir = self.effective_download_dir();
                        let _ = std::fs::create_dir_all(&dir);
                        self.status = "downloading…".into();
                        self.progress = Some((0, 0));
                        self.spawn(move |tx| {
                            let progress_tx = tx.clone();
                            let result = ytdlp::download(
                                &url,
                                selector.as_deref(),
                                &dir,
                                |downloaded, total| {
                                    let _ = progress_tx.send(Msg::Progress { downloaded, total });
                                },
                            );
                            let _ = tx.send(match result {
                                Ok(p) => Msg::Downloaded(p),
                                Err(e) => Msg::Error(e.to_string()),
                            });
                        });
                    }

                    match est_size {
                        Some(bytes) => ui.label(format!("≈ {}", fmt_size(bytes))),
                        None => ui.weak("size unknown"),
                    };
                });
            }

            if !self.status.is_empty() {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    // Pin the row to the button's height so the labels and the
                    // taller button both center vertically against it.
                    ui.set_min_height(button_height(ui));
                    ui.label("Status:");
                    ui.monospace(&self.status);
                    if let Some(saved) = &self.saved_path {
                        if ui.button("Open folder").clicked() {
                            reveal_in_file_manager(saved);
                        }
                    }
                });
            }
            self.error_panel(ui);
            if let Some((downloaded, total)) = self.progress {
                let frac = if total > 0 {
                    (downloaded as f32 / total as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let text = if total > 0 {
                    format!("{} / {} ({:.0}%)", fmt_size(downloaded), fmt_size(total), frac * 100.0)
                } else {
                    "starting…".to_owned()
                };
                ui.add(egui::ProgressBar::new(frac).text(text));
            }
            if self.exporting {
                let written = self
                    .export_path
                    .as_ref()
                    .and_then(|p| std::fs::metadata(p).ok())
                    .map_or(0, |m| m.len());
                let text = if written > 0 {
                    format!("compiling & saving… {} so far", fmt_size(written))
                } else {
                    "compiling & saving…".to_owned()
                };
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new());
                    ui.label(text);
                    if ui.button("Cancel").clicked() {
                        self.export_cancel.store(true, Ordering::Relaxed);
                        self.status = "canceling…".into();
                    }
                });
                // Keep repainting so the size readout grows even without input.
                ui.ctx().request_repaint();
            }
            ui.add_space(4.0);
        });
    }

    /// Paint the timeline: dimmed outside the clip, highlighted in [in, out],
    /// with in/out markers and a playhead. Returns `(target, released)` while
    /// the timeline is clicked or dragged; `released` is true on click/release.
    fn draw_timeline(&self, ui: &mut egui::Ui, cur: f64, dur: f64) -> Option<(f64, bool)> {
        const HEIGHT: f32 = 28.0;
        let dur = dur.max(f64::EPSILON);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), HEIGHT),
            egui::Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals();
        let x_at = |t: f64| rect.left() + (t / dur).clamp(0.0, 1.0) as f32 * rect.width();

        let in_x = x_at(self.in_secs);
        let out_x = x_at(self.out_secs);
        // Normalize so the highlight always spans between the two markers: a
        // reversed rect (min.x > max.x) is silently dropped by egui, which would
        // leave the clip region uncolored if in/out ever cross.
        let lo = in_x.min(out_x);
        let hi = in_x.max(out_x);
        let dim = egui::Color32::from_black_alpha(150);

        painter.rect_filled(rect, egui::Rounding::same(4.0), visuals.extreme_bg_color);
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(lo, rect.top()), egui::pos2(hi, rect.bottom())),
            egui::Rounding::ZERO,
            visuals.selection.bg_fill,
        );
        // gray out before the in point and after the out point
        painter.rect_filled(
            egui::Rect::from_min_max(rect.left_top(), egui::pos2(lo, rect.bottom())),
            egui::Rounding::ZERO,
            dim,
        );
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(hi, rect.top()), rect.right_bottom()),
            egui::Rounding::ZERO,
            dim,
        );

        let marker = egui::Stroke::new(2.0, visuals.selection.stroke.color);
        painter.vline(in_x, rect.y_range(), marker);
        painter.vline(out_x, rect.y_range(), marker);

        let mut result = None;
        let mut live_pos = None;
        if response.dragged() || response.clicked() || response.drag_stopped() {
            if let Some(pos) = response.interact_pointer_pos() {
                let t = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64 * dur;
                live_pos = Some(t);
                result = Some((t, response.drag_stopped() || response.clicked()));
            }
        }

        // Playhead: follow the cursor while interacting, hold at a pending
        // release target until its frame lands, otherwise show the current frame.
        let playhead = live_pos
            .or_else(|| self.awaiting_release.map(|(_, pos)| pos))
            .unwrap_or(cur);
        painter.vline(
            x_at(playhead),
            rect.y_range(),
            egui::Stroke::new(2.0, visuals.strong_text_color()),
        );
        result
    }

    /// Clip controls shown under the preview: scrub, frame step, in/out, save.
    fn clip_controls(
        &mut self,
        ui: &mut egui::Ui,
        cur: f64,
        dur: f64,
        nav: &mut Option<Nav>,
        export_req: &mut Option<(Mode, &'static str)>,
    ) {
        ui.add_space(CONTROL_PAD);

        // One row with three containers: a left group and a right group that each
        // hug their content, and a center group placed at an exactly-centered
        // rect in the gap between them. Widths are measured up front so the
        // center lands precisely, independent of egui's layout-direction quirks.
        let btn_font = egui::TextStyle::Button.resolve(ui.style());
        let mono_font = egui::TextStyle::Monospace.resolve(ui.style());
        let pad = ui.spacing().button_padding.x;
        let gap = ui.spacing().item_spacing.x;
        let text_w = |ui: &egui::Ui, text: &str, font: &egui::FontId| -> f32 {
            ui.fonts(|f| f.layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE).size().x)
        };
        let btn_w = |ui: &egui::Ui, text: &str| text_w(ui, text, &btn_font) + 2.0 * pad;

        let in_time = fmt_time(self.in_secs);
        let out_time = fmt_time(self.out_secs);
        let start_label = format!("⟦ Set Start ({})", self.keybinds.set_start.label());
        let end_label = format!("Set End ({}) ⟧", self.keybinds.set_end.label());
        let left_w = btn_w(ui, &start_label) + gap + text_w(ui, &in_time, &mono_font);
        let right_w = text_w(ui, &out_time, &mono_font) + gap + btn_w(ui, &end_label);
        let center_w = btn_w(ui, "▶ Play Clip") + gap + btn_w(ui, "⏸ Pause");

        let row_h = button_height(ui);
        let (row, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), egui::Sense::hover());
        let sub = |min_x: f32, w: f32, layout: egui::Layout| {
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(
                    egui::pos2(min_x, row.min.y),
                    egui::vec2(w, row_h),
                ))
                .layout(layout)
        };
        // Center within the gap between the left and right groups.
        let center_x = (row.left() + left_w + row.right() - right_w) / 2.0 - center_w / 2.0;

        ui.allocate_new_ui(
            sub(row.left(), left_w, egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                let can_set_start = cur <= self.out_secs;
                if ui
                    .add_enabled(can_set_start, egui::Button::new(start_label.as_str()))
                    .clicked()
                {
                    self.in_secs = cur;
                }
                ui.monospace(&in_time);
            },
        );
        ui.allocate_new_ui(
            sub(center_x, center_w, egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                if ui.button("▶ Play Clip").clicked() {
                    let now = ui.input(|i| i.time);
                    self.play_from(self.in_secs, Some(self.out_secs), now);
                }
                if ui
                    .add_enabled(self.playing, egui::Button::new("⏸ Pause"))
                    .clicked()
                {
                    self.stop_play();
                }
            },
        );
        ui.allocate_new_ui(
            sub(row.right() - right_w, right_w, egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.monospace(&out_time);
                let can_set_end = cur >= self.in_secs;
                if ui
                    .add_enabled(can_set_end, egui::Button::new(end_label.as_str()))
                    .clicked()
                {
                    self.out_secs = cur;
                }
            },
        );

        ui.add_space(4.0);
        if let Some((t, released)) = self.draw_timeline(ui, cur, dur) {
            self.stop_play();
            *nav = Some(Nav::Seek { secs: t, released });
        }

        ui.add_space(4.0);
        ui.vertical_centered(|ui| {
            ui.horizontal(|ui| {
                if ui.button("⏮  Frame").clicked() {
                    self.stop_play();
                    *nav = Some(Nav::Back);
                }
                let play_label = if self.playing { "⏸  Pause" } else { "▶  Play" };
                if ui.button(play_label).clicked() {
                    self.toggle_play(ui.input(|i| i.time));
                }
                if ui.button("Frame  ⏭").clicked() {
                    self.stop_play();
                    *nav = Some(Nav::Forward);
                }
                // Follow a pending seek target (held skip / released drag) like the
                // playhead does, so the readout doesn't lag behind the position.
                let shown = self.awaiting_release.map_or(cur, |(_, pos)| pos);
                ui.monospace(format!("{}  /  {}", fmt_time(shown), fmt_time(dur)));

                ui.separator();
                ui.label("🔊");
                ui.spacing_mut().slider_width = 90.0;
                // Match the slider's value box to the button height so it lines
                // up with the transport buttons beside it.
                ui.spacing_mut().interact_size.y = button_height(ui);
                let vol = ui.add(
                    egui::Slider::new(&mut self.volume, 0.0..=1.0)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
                if vol.changed() {
                    if let Some(audio) = &self.audio {
                        audio.set_volume(self.volume);
                    }
                }
            });
        });

        ui.add_space(CONTROL_PAD);
        ui.separator();
        ui.add_space(CONTROL_PAD);

        // Audio export on the left; video export right-aligned. Comboboxes size
        // their content to `icon_width` while buttons don't, so they differ in
        // height. Give the row a fixed height (so a short leading label like
        // "Audio:" centers within it instead of pinning to the top) and one
        // shared widget height (so buttons match the taller comboboxes).
        let row_h = ui
            .text_style_height(&egui::TextStyle::Button)
            .max(ui.spacing().icon_width)
            + 2.0 * ui.spacing().button_padding.y;
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), row_h),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
            ui.spacing_mut().interact_size.y = row_h;

            let vid = self.video_format.extension();
            let src_height = self.decoder.as_ref().map_or(0, |d| d.height);

            // Left group: audio format + Save audio only.
            ui.label("Audio:");
            egui::ComboBox::from_id_salt("audio_format")
                .selected_text(audio_format_label(self.audio_format))
                .show_ui(ui, |ui| {
                    use export::AudioFormat::*;
                    ui.selectable_value(&mut self.audio_format, Mp3, audio_format_label(Mp3));
                    ui.selectable_value(&mut self.audio_format, Aac, audio_format_label(Aac));
                    ui.selectable_value(
                        &mut self.audio_format,
                        Original,
                        audio_format_label(Original),
                    );
                });
            if icon_button(ui, save_icon(), "Save audio only…").clicked() {
                let fmt = self.audio_format;
                let ext = self
                    .video_path
                    .as_ref()
                    .and_then(|p| export::audio_extension(&p.to_string_lossy(), fmt).ok())
                    .unwrap_or("mp3");
                *export_req = Some((Mode::AudioOnly(fmt), ext));
            }
            ui.separator();

            // Right group, added right-to-left so it reads: Video, Resolution,
            // Save full video, Save clip.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, save_icon(), "Save clip…").clicked() {
                    *export_req = Some((Mode::Clip, vid));
                }
                if icon_button(ui, save_icon(), "Save full video…").clicked() {
                    *export_req = Some((Mode::Full, vid));
                }

                // Downscale menu, capped to the source height so it never upscales.
                let res_text = match self.export_height {
                    None => "Original".to_string(),
                    Some(h) => format!("{h}p"),
                };
                egui::ComboBox::from_id_salt("export_height")
                    .selected_text(res_text)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.export_height, None, "Original");
                        for h in EXPORT_HEIGHT_LADDER.iter().filter(|h| **h < src_height) {
                            ui.selectable_value(&mut self.export_height, Some(*h), format!("{h}p"));
                        }
                    });
                ui.label("Resolution:");

                egui::ComboBox::from_id_salt("video_format")
                    .selected_text(vid.to_uppercase())
                    .show_ui(ui, |ui| {
                        use export::VideoFormat::*;
                        ui.selectable_value(&mut self.video_format, Mp4, "MP4");
                        ui.selectable_value(&mut self.video_format, Mkv, "MKV");
                        ui.selectable_value(&mut self.video_format, Mov, "MOV");
                        ui.selectable_value(&mut self.video_format, Webm, "WebM");
                    });
                ui.label("Video:");
            });
        });
        ui.add_space(CONTROL_PAD);
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if self.delete_cache_on_exit {
            clear_dir(&managed_cache_dir());
        }
    }
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, SCALE_STORAGE_KEY, &self.ui_scale);
        eframe::set_value(storage, DOWNLOAD_DIR_KEY, &self.download_dir);
        eframe::set_value(storage, OUTPUT_DIR_KEY, &self.output_dir);
        eframe::set_value(storage, DELETE_ON_EXIT_KEY, &self.delete_cache_on_exit);
        eframe::set_value(storage, OPEN_DIR_ON_SAVE_KEY, &self.open_dir_on_save);
        eframe::set_value(storage, VOLUME_KEY, &self.volume);
        eframe::set_value(storage, THEME_KEY, &theme_pref_name(self.theme).to_owned());
        // Persist each shortcut as (key name, shift) so we don't depend on egui's
        // serde feature; keyed by stable action id so the loader is order-proof.
        let keys: Vec<(String, String, bool)> = Bind::ALL
            .iter()
            .map(|(bind, _)| {
                let sc = self.keybinds.shortcut(*bind);
                (bind.id().to_owned(), sc.key.name().to_owned(), sc.shift)
            })
            .collect();
        eframe::set_value(storage, KEYBINDS_KEY, &keys);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        self.poll_decoder(ctx);
        self.poll_cache(ctx);
        ctx.request_repaint();

        let ready = self.decoder.as_ref().is_some_and(|d| d.ready);
        let now = ctx.input(|i| i.time);

        // Clip + playback shortcuts, unless a text field has focus or Settings is
        // capturing a key. Set start/end mirror the buttons' position guards.
        if ready && self.rebinding.is_none() && !ctx.wants_keyboard_input() {
            let kb = self.keybinds;
            let cur = self.decoder.as_ref().map(|d| d.current_secs);
            if ctx.input(|i| shortcut_pressed(i, kb.play_pause)) {
                self.toggle_play(now);
            }
            if ctx.input(|i| shortcut_pressed(i, kb.play_pause_clip)) {
                self.toggle_play_clip(now);
            }
            if let Some(cur) = cur {
                if ctx.input(|i| shortcut_pressed(i, kb.set_start)) && cur <= self.out_secs {
                    self.in_secs = cur;
                }
                if ctx.input(|i| shortcut_pressed(i, kb.set_end)) && cur >= self.in_secs {
                    self.out_secs = cur;
                }
            }

            // Nav keys repeat while held, on a timer so they don't fire too fast:
            // fire on press, pause `NAV_REPEAT_DELAY`, then every `..._INTERVAL`.
            let nav = [kb.skip_back, kb.skip_forward, kb.step_back, kb.step_forward];
            for (idx, sc) in nav.into_iter().enumerate() {
                if ctx.input(|i| shortcut_down(i, sc)) {
                    if now >= self.nav_repeat_at[idx] {
                        let first = self.nav_repeat_at[idx] == 0.0;
                        let wait = if first { NAV_REPEAT_DELAY } else { NAV_REPEAT_INTERVAL };
                        self.nav_repeat_at[idx] = now + wait;
                        match idx {
                            0 => self.skip_secs(-SKIP_SECS),
                            1 => self.skip_secs(SKIP_SECS),
                            2 => self.step_frame(false),
                            _ => self.step_frame(true),
                        }
                    }
                } else {
                    self.nav_repeat_at[idx] = 0.0;
                }
            }
        }

        // Advance video toward the master clock: step when close, seek to resync
        // when it has fallen far behind (e.g. a slow codec dropping frames).
        if self.playing {
            const RESYNC_SECS: f64 = 0.5;
            let clock = self.master_clock(now);
            if let Some(end) = self.play_until.filter(|&end| clock >= end) {
                // "Play Clip" reached the out point: stop and pin to the end.
                self.stop_play();
                if let Some(dec) = &self.decoder {
                    dec.seek_secs(end);
                }
            } else {
                let info = self
                    .decoder
                    .as_ref()
                    .filter(|d| d.ready)
                    .map(|d| (d.fps, d.current_secs, d.duration_secs));
                match info {
                    Some((fps, video_t, dur)) if clock + 1.0 / fps.max(1.0) < dur => {
                        if video_t + 1.0 / fps.max(1.0) <= clock {
                            if let Some(dec) = &self.decoder {
                                if clock - video_t > RESYNC_SECS {
                                    dec.seek_secs(clock);
                                } else {
                                    dec.step_forward();
                                }
                            }
                        }
                    }
                    // Reached the end (or no decoder): stop.
                    _ => self.stop_play(),
                }
            }
        }

        self.settings_window(ctx);
        self.cache_window(ctx);
        self.toolbar(ctx);

        // The decoder runs on its own thread; the UI only reads the last known
        // position and sends requests, never blocking on a decode.
        let pos = self
            .decoder
            .as_ref()
            .filter(|d| d.ready)
            .map(|d| (d.current_secs, d.duration_secs));
        let mut nav: Option<Nav> = None;
        let mut export_req: Option<(Mode, &'static str)> = None;

        if let Some((cur, dur)) = pos {
            egui::TopBottomPanel::bottom("controls")
                .resizable(false)
                .show(ctx, |ui| self.clip_controls(ui, cur, dur, &mut nav, &mut export_req));
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            // Editable title (seeds the export filename).
            if self.video_path.is_some() {
                let mut title = self.video_title.clone().unwrap_or_default();
                let mut pick_output = false;
                ui.horizontal(|ui| {
                    ui.label("Title:");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("Choose folder…")
                            .on_hover_text("Folder the save dialog opens in (set a default in Settings)")
                            .clicked()
                        {
                            pick_output = true;
                        }
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut title)
                                    .desired_width(f32::INFINITY)
                                    .margin(INPUT_MARGIN),
                            )
                            .changed()
                        {
                            self.video_title = Some(title);
                        }
                    });
                });
                if pick_output {
                    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                        self.output_dir = Some(folder);
                    }
                }
                ui.add_space(4.0);
            }

            match &self.frame_tex {
                Some(tex) => {
                    // Reserve the whole area once, then paint the frame into a
                    // centered sub-rect. Deriving widget size from available
                    // space directly would feed back and collapse to zero.
                    let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
                    let img = tex.size_vec2();
                    if img.x > 0.0 && img.y > 0.0 {
                        let scale = (rect.width() / img.x).min(rect.height() / img.y);
                        let drawn = egui::Rect::from_center_size(rect.center(), img * scale);
                        egui::Image::new(tex).paint_at(ui, drawn);
                    }
                }
                None => {
                    ui.centered_and_justified(|ui| {
                        ui.label("Open a file or download a video to begin.");
                    });
                }
            }
        });

        // Requests are non-blocking; decoded frames arrive via `poll_decoder`.
        // A released seek records its gen so the playhead pins there until that
        // exact frame lands; everything else clears any pending release.
        if let Some(nav) = nav {
            let new_awaiting = self.decoder.as_ref().and_then(|dec| match nav {
                Nav::Back => {
                    dec.step_backward();
                    None
                }
                Nav::Forward => {
                    dec.step_forward();
                    None
                }
                Nav::Seek { secs, released } => {
                    let gen = dec.seek_secs(secs);
                    released.then_some((gen, secs))
                }
            });
            self.awaiting_release = new_awaiting;
        }

        if let Some((mode, ext)) = export_req {
            self.start_export(mode, ext);
        }
    }
}
