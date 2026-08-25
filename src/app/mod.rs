pub(crate) mod action;
pub(crate) mod clip;
pub(crate) mod download;
pub(crate) mod export_options;
pub(crate) mod jobs;
pub(crate) mod playback;
pub(crate) mod settings;
pub(crate) mod source;
pub(crate) mod ui;
pub(crate) mod ui_state;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;
use std::sync::Arc;
use yt_dlp_clipper::export::{self, ExportSpec, Mode};
use yt_dlp_clipper::ytdlp;

use crate::binaries::{clear_dir, managed_cache_dir};
use crate::decoder_thread::DecodeEvent;
use crate::format::sanitize_filename;
use crate::keybinds::{shortcut_down, shortcut_pressed};
use crate::theme::apply_theme;
use crate::widgets::reveal_in_file_manager;
use action::{Action, Nav};
use clip::Clip;
use download::Download;
use export_options::ExportOptions;
use jobs::{Jobs, Msg};
use playback::Playback;
use settings::Settings;
use source::{OpenedSource, Source};
use ui::{cache_window, controls, preview, settings_window, toolbar};
use ui_state::UiState;

const SKIP_SECS: f64 = 5.0;
/// Frame rate reported for an audio-only source, which has no frames of its
/// own. It only sets the granularity of the seconds-to-timeline arithmetic.
const AUDIO_ONLY_FPS: f64 = 30.0;
/// Held nav key: how long before auto-repeat begins, then the interval between
/// repeats (seconds). Keeps a held key from firing too fast.
const NAV_REPEAT_DELAY: f64 = 0.3;
const NAV_REPEAT_INTERVAL: f64 = 0.1;

pub(crate) struct App {
    /// The download form: URL, fetched metadata, and format choices.
    download: Download,
    /// Background work in flight and what it last reported.
    jobs: Jobs,

    /// The open file: decoder thread, origin path, and latest decoded frame.
    source: Source,
    /// Clip in/out points, the timeline zoom window, and the undo history.
    clip: Clip,

    /// Everything that persists between sessions via eframe storage.
    settings: Settings,
    /// Transient interface state: panel visibility and in-progress interactions.
    ui: UiState,

    /// Target containers and optional downscale for the save buttons.
    export_options: ExportOptions,

    /// Playback clock, audio output, and the post-seek playhead pin.
    playback: Playback,
}

impl Default for App {
    fn default() -> Self {
        let settings = Settings::default();
        Self {
            download: Download::default(),
            jobs: Jobs::default(),
            source: Source::default(),
            clip: Clip::default(),
            ui: UiState { pending_scale: settings.ui_scale, ..UiState::default() },
            settings,
            export_options: ExportOptions::default(),
            playback: Playback::default(),
        }
    }
}

impl App {
    /// Build the app from persisted settings, applying the saved zoom and theme
    /// before the first frame, and open the video named on the command line.
    pub(crate) fn new(cc: &eframe::CreationContext<'_>, cli_path: Option<PathBuf>) -> Self {
        let mut app = App::default();
        if let Some(storage) = cc.storage {
            app.settings = Settings::load(storage);
            app.ui.pending_scale = app.settings.ui_scale;
            cc.egui_ctx.set_zoom_factor(app.settings.ui_scale);
        }
        apply_theme(&cc.egui_ctx, app.settings.theme);
        if let Some(path) = cli_path {
            app.load_video(path);
        }
        app
    }

    fn poll(&mut self) {
        let Some(rx) = self.jobs.take_receiver() else { return };
        loop {
            match rx.try_recv() {
                Ok(Msg::Progress { downloaded, total }) => {
                    self.jobs.progress = Some((downloaded, total))
                }
                Ok(Msg::Info(info)) => {
                    let status = format!("{} — {} formats", info.title, info.formats.len());
                    self.jobs.succeed(status);
                    self.download.selected_height = None;
                    self.download.info = Some(info);
                    return;
                }
                Ok(Msg::Downloaded(path)) => {
                    self.jobs.succeed(format!("downloaded: {}", path.display()));
                    let title = self
                        .download.info
                        .as_ref()
                        .map(|i| i.title.clone())
                        .filter(|t| !t.is_empty());
                    self.load_video(path);
                    if let Some(title) = title {
                        self.source.video_title = Some(title);
                    }
                    return;
                }
                Ok(Msg::Exported(path)) => {
                    self.jobs.succeed(format!("saved: {}", path.display()));
                    self.jobs.finish_export();
                    if self.settings.open_dir_on_save {
                        reveal_in_file_manager(&path);
                    }
                    self.jobs.saved_path = Some(path);
                    return;
                }
                Ok(Msg::ExportCanceled(path)) => {
                    // Drop the incomplete output so no truncated file is left behind.
                    let _ = std::fs::remove_file(&path);
                    self.jobs.status = "export canceled".into();
                    self.jobs.progress = None;
                    self.jobs.finish_export();
                    return;
                }
                Ok(Msg::Error(e)) => {
                    // Keep the full text for the error panel; the status line just
                    // flags that something failed.
                    self.jobs.status = "error".into();
                    self.jobs.last_error = Some(e);
                    self.jobs.progress = None;
                    self.jobs.finish_export();
                    return;
                }
                Err(TryRecvError::Empty) => {
                    self.jobs.restore_receiver(rx);
                    return;
                }
                Err(TryRecvError::Disconnected) => return,
            }
        }
    }

    fn reset_for_new_video(&mut self) {
        self.playback.stop();
        self.jobs.clear_receiver();
        self.source.clear();
        self.clip.in_secs = 0.0;
        self.clip.out_secs = 0.0;
        self.clip.view_start_secs = 0.0;
        self.clip.view_end_secs = 0.0;
        self.clip.window_drag = None;
        self.clip.timeline_drag = None;
        self.clip.clear_history();
        self.ui.nav_repeat_at = [0.0; 4];
        self.playback.play_start_wall = 0.0;
        self.playback.play_start_pos = 0.0;
        self.jobs.saved_path = None;
        self.jobs.last_error = None;
        self.jobs.progress = None;
        self.jobs.exporting = false;
        self.jobs.export_path = None;
        self.jobs.export_cancel.store(true, Ordering::Relaxed);
        self.jobs.export_cancel = Arc::new(AtomicBool::new(false));
        self.jobs.download_cancel.store(true, Ordering::Relaxed);
        self.jobs.download_cancel = Arc::new(AtomicBool::new(false));
        self.export_options.scale_height = None;
        self.download.selected_height = None;
    }

    fn load_video(&mut self, path: PathBuf) {
        self.reset_for_new_video();
        self.source.open(path);
    }

    /// Seek `delta` seconds from the current position (clamped to the video) and
    /// pin the playhead to the target until that frame lands. Builds on the last
    /// *requested* target (a still-in-flight seek), not the decoder's reported
    /// position, which lags on long videos — so a held key keeps advancing
    /// instead of stalling on a stale `current_secs`.
    fn skip_secs(&mut self, delta: f64) {
        let Some((base, dur)) = self.source.decoder.as_ref().map(|dec| {
            let base = self.playback.awaiting_release.map_or(dec.current_secs, |(_, pos)| pos);
            (base, dec.duration_secs)
        }) else {
            return;
        };
        let target = (base + delta).clamp(0.0, dur);
        if !self.source.has_video() {
            self.move_audio_only_playhead(target);
            return;
        }
        self.playback.stop();
        if let Some(gen) = self.source.decoder.as_ref().map(|dec| dec.seek_secs(target)) {
            self.playback.awaiting_release = Some((gen, target));
        }
    }

    /// Step exactly one frame forward or backward. An audio-only source has no
    /// frames to land on, so stepping does nothing there.
    fn step_frame(&mut self, forward: bool) {
        if !self.source.has_video() {
            return;
        }
        self.playback.stop();
        if let Some(dec) = self.source.decoder.as_ref() {
            if forward {
                dec.step_forward();
            } else {
                dec.step_backward();
            }
        }
    }

    /// Place the playhead of an audio-only source, which has no decoded frame
    /// arriving to carry the position. Playback stops so the audio clock can't
    /// immediately overwrite the new spot.
    fn move_audio_only_playhead(&mut self, secs: f64) {
        self.playback.stop();
        self.playback.awaiting_release = None;
        if let Some(dec) = &mut self.source.decoder {
            dec.current_secs = secs.clamp(0.0, dec.duration_secs);
        }
    }

    /// Start playback from the current position (with audio if available), or
    /// stop if already playing.
    fn toggle_play(&mut self, now: f64) {
        if self.playback.playing {
            self.playback.stop();
            return;
        }
        let Some(pos) = self.source.ready_position_secs() else {
            return;
        };
        self.start_playback(pos, None, now);
    }

    /// Play/pause within the clip: pause if playing, else play to the out point,
    /// resuming from the current spot when it's inside the clip and otherwise
    /// starting from the in point.
    fn toggle_play_clip(&mut self, now: f64) {
        if self.playback.playing {
            self.playback.stop();
            return;
        }
        let Some(cur) = self.source.ready_position_secs() else {
            return;
        };
        let start = if (self.clip.in_secs..self.clip.out_secs).contains(&cur) {
            cur
        } else {
            self.clip.in_secs
        };
        self.play_from(start, Some(self.clip.out_secs), now);
    }

    /// Jump to `pos` and start playing from exactly there, stopping at `until`
    /// (the clip out point) if given, otherwise at the end of the video.
    fn play_from(&mut self, pos: f64, until: Option<f64>, now: f64) {
        if !self.source.is_ready() {
            return;
        }
        if let Some(dec) = &self.source.decoder {
            dec.seek_secs(pos);
        }
        self.start_playback(pos, until, now);
    }

    /// Begin playing from `pos`, reading audio from the open source file.
    fn start_playback(&mut self, pos: f64, until: Option<f64>, now: f64) {
        let source_path = self.source.video_path.clone();
        self.playback.start(pos, until, now, source_path.as_deref(), self.settings.volume);
    }

    /// Drain frames/metadata from the decoder thread, uploading the newest frame.
    fn poll_decoder(&mut self, ctx: &egui::Context) {
        let awaiting = self.playback.awaiting_release;
        let mut latest_frame = None;
        let mut opened = None;
        let mut error = None;
        let mut landed = false;
        if let Some(dec) = &self.source.decoder {
            loop {
                match dec.event_rx.try_recv() {
                    Ok(DecodeEvent::Opened { width, height, duration_secs, fps }) => {
                        opened = Some(OpenedSource {
                            width,
                            height,
                            duration_secs,
                            fps,
                            has_video: true,
                        })
                    }
                    Ok(DecodeEvent::OpenedAudioOnly { duration_secs }) => {
                        opened = Some(OpenedSource {
                            width: 0,
                            height: 0,
                            duration_secs,
                            fps: AUDIO_ONLY_FPS,
                            has_video: false,
                        })
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
            self.playback.awaiting_release = None;
        }

        if let Some(source) = opened {
            if let Some(dec) = &mut self.source.decoder {
                dec.width = source.width;
                dec.height = source.height;
                dec.duration_secs = source.duration_secs;
                dec.fps = source.fps;
                dec.has_video = source.has_video;
                dec.ready = true;
            }
            self.clip.in_secs = 0.0;
            self.clip.out_secs = source.duration_secs;
            self.clip.view_start_secs = 0.0;
            self.clip.view_end_secs = source.duration_secs;
            self.clip.clear_history();
        }
        if let Some((image, secs)) = latest_frame {
            if let Some(dec) = &mut self.source.decoder {
                dec.current_secs = secs;
            }
            self.source.set_frame(ctx, image);
        }
        if let Some(e) = error {
            self.jobs.status = "decode error".into();
            self.jobs.last_error = Some(e);
            self.source.decoder = None;
        }
    }

    /// Prompt for a destination and run the export on a background thread. A
    /// configured output folder just preselects where the save dialog opens.
    fn start_export(&mut self, mode: Mode, ext: &str) {
        let Some(input) = self.source.video_path.clone() else { return };
        let base = self.source.video_title.as_deref().unwrap_or("video");
        let stem = sanitize_filename(base);
        let mut dialog = rfd::FileDialog::new().set_file_name(format!("{stem}.{ext}"));
        if let Some(dir) = &self.settings.output_dir {
            dialog = dialog.set_directory(dir);
        }
        let Some(out) = dialog.save_file() else {
            return;
        };

        // Resolution applies to saved video only; audio-only ignores it.
        let scale_height = if matches!(mode, Mode::AudioOnly(_)) {
            None
        } else {
            self.export_options.scale_height
        };
        let spec = ExportSpec {
            input: input.to_string_lossy().into_owned(),
            output: out.to_string_lossy().into_owned(),
            start_secs: self.clip.in_secs,
            end_secs: self.clip.out_secs,
            mode,
            scale_height,
            compatibility_mode: self.settings.compatibility_mode,
        };
        self.jobs.status = "exporting…".into();
        self.jobs.exporting = true;
        self.jobs.saved_path = None;
        self.jobs.export_path = Some(out);
        let cancel = Arc::new(AtomicBool::new(false));
        self.jobs.export_cancel = cancel.clone();
        self.jobs.spawn(move |tx| {
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

    /// Carry out one action a panel asked for, after every panel has drawn.
    fn apply(&mut self, action: Action, now: f64) {
        match action {
            Action::Fetch => {
                let url = self.download.url.clone();
                let cookies = self.settings.cookies.clone();
                self.reset_for_new_video();
                self.download.info = None;
                self.jobs.status = "fetching…".into();
                self.jobs.spawn(move |tx| {
                    let _ = tx.send(match ytdlp::fetch_info(&url, cookies.as_ref()) {
                        Ok(info) => Msg::Info(info),
                        Err(e) => Msg::Error(e.to_string()),
                    });
                });
            }
            Action::OpenFile(path) => {
                self.load_video(path);
                self.download.info = None;
                self.download.url.clear();
                self.jobs.status.clear();
            }
            Action::Nav(nav) => self.apply_nav(nav),
            Action::TogglePlay => self.toggle_play(now),
            Action::PlaySelection => {
                self.play_from(self.clip.in_secs, Some(self.clip.out_secs), now);
            }
            Action::Export { mode, extension } => self.start_export(mode, extension),
        }
    }

    /// Requests are non-blocking; decoded frames arrive via `poll_decoder`. A
    /// released seek records its gen so the playhead pins there until that exact
    /// frame lands; everything else clears any pending release.
    fn apply_nav(&mut self, nav: Nav) {
        // An audio-only source decodes no frames, so the playhead moves here
        // rather than landing with one; stepping has nothing to land on.
        if !self.source.has_video() {
            if let Nav::Seek { secs, .. } = nav {
                self.move_audio_only_playhead(secs);
            }
            return;
        }
        let new_awaiting = self.source.decoder.as_ref().and_then(|dec| match nav {
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
        self.playback.awaiting_release = new_awaiting;
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if self.settings.delete_cache_on_exit {
            clear_dir(&managed_cache_dir());
        }
    }
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.settings.save(storage);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        self.jobs.poll_ytdlp_update();
        self.jobs.poll_update_check();
        self.poll_decoder(ctx);
        self.ui.cache.poll(ctx);
        ctx.request_repaint();

        let ready = self.source.is_ready();
        let now = ctx.input(|i| i.time);

        if self.clip.scroll_gesture_commit_at.is_some_and(|deadline| now >= deadline) {
            self.clip.scroll_gesture_commit_at = None;
            self.clip.commit_gesture();
        }

        // Clip + playback shortcuts, unless a text field has focus or Settings is
        // capturing a key. Set start/end mirror the buttons' position guards.
        if ready && self.ui.rebinding.is_none() && !ctx.wants_keyboard_input() {
            let kb = self.settings.keybinds;
            let cur = self.source.decoder.as_ref().map(|d| d.current_secs);
            if ctx.input(|i| shortcut_pressed(i, kb.play_pause)) {
                self.toggle_play(now);
            }
            if ctx.input(|i| shortcut_pressed(i, kb.play_pause_clip)) {
                self.toggle_play_clip(now);
            }
            if let Some(cur) = cur {
                let set_pos = self.playback.awaiting_release.map_or(cur, |(_, pos)| pos);
                if ctx.input(|i| shortcut_pressed(i, kb.set_start))
                    && set_pos < self.clip.out_secs
                {
                    self.clip.set_in_secs(set_pos);
                }
                if ctx.input(|i| shortcut_pressed(i, kb.set_end)) && set_pos > self.clip.in_secs {
                    self.clip.set_out_secs(set_pos);
                }
            }
            if ctx.input(|i| shortcut_pressed(i, kb.undo)) {
                self.clip.undo();
            }
            if ctx.input(|i| shortcut_pressed(i, kb.redo)) {
                self.clip.redo();
            }

            // Nav keys repeat while held, on a timer so they don't fire too fast:
            // fire on press, pause `NAV_REPEAT_DELAY`, then every `..._INTERVAL`.
            let nav = [kb.skip_back, kb.skip_forward, kb.step_back, kb.step_forward];
            for (idx, sc) in nav.into_iter().enumerate() {
                if ctx.input(|i| shortcut_down(i, sc)) {
                    if now >= self.ui.nav_repeat_at[idx] {
                        let first = self.ui.nav_repeat_at[idx] == 0.0;
                        let wait = if first { NAV_REPEAT_DELAY } else { NAV_REPEAT_INTERVAL };
                        self.ui.nav_repeat_at[idx] = now + wait;
                        match idx {
                            0 => self.skip_secs(-SKIP_SECS),
                            1 => self.skip_secs(SKIP_SECS),
                            2 => self.step_frame(false),
                            _ => self.step_frame(true),
                        }
                    }
                } else {
                    self.ui.nav_repeat_at[idx] = 0.0;
                }
            }
        }

        // Advance video toward the master clock: step when close, seek to resync
        // when it has fallen far behind (e.g. a slow codec dropping frames).
        if self.playback.playing {
            const RESYNC_SECS: f64 = 0.5;
            let clock = self.playback.master_clock(now);
            if let Some(end) = self.playback.play_until.filter(|&end| clock >= end) {
                if self.source.has_video() {
                    self.playback.stop();
                    if let Some(dec) = &self.source.decoder {
                        dec.seek_secs(end);
                    }
                } else {
                    self.move_audio_only_playhead(end);
                }
            } else if !self.source.has_video() {
                // No frames arrive to carry the position, so the playhead
                // follows the master clock directly.
                let duration_secs = self.source.decoder.as_ref().map_or(0.0, |d| d.duration_secs);
                if clock >= duration_secs {
                    self.playback.stop();
                } else if let Some(dec) = &mut self.source.decoder {
                    dec.current_secs = clock;
                }
            } else {
                let info = self
                    .source.decoder
                    .as_ref()
                    .filter(|d| d.ready)
                    .map(|d| (d.fps, d.current_secs, d.duration_secs));
                match info {
                    Some((fps, video_t, dur)) if clock + 1.0 / fps.max(1.0) < dur => {
                        if video_t + 1.0 / fps.max(1.0) <= clock {
                            if let Some(dec) = &self.source.decoder {
                                if clock - video_t > RESYNC_SECS {
                                    dec.seek_secs(clock);
                                } else {
                                    dec.step_forward();
                                }
                            }
                        }
                    }
                    _ => self.playback.stop(),
                }
            }
        }

        // Panels only report what they want done; every mutation that spans
        // more than the state they borrow happens in the drain below.
        let mut actions: Vec<Action> = Vec::new();

        settings_window::show(ctx, &mut self.settings, &mut self.ui, &mut self.jobs);
        if let Some(path) = cache_window::show(ctx, &mut self.ui.cache) {
            actions.push(Action::OpenFile(path));
        }
        actions.extend(toolbar::show(
            ctx,
            &mut self.download,
            &mut self.jobs,
            &mut self.settings,
            &mut self.ui,
        ));

        // The decoder runs on its own thread; the UI only reads the last known
        // position and sends requests, never blocking on a decode.
        let pos = self
            .source.decoder
            .as_ref()
            .filter(|d| d.ready)
            .map(|d| (d.current_secs, d.duration_secs));
        if let Some((cur, dur)) = pos {
            let clip = &mut self.clip;
            let playback = &mut self.playback;
            let settings = &mut self.settings;
            egui::TopBottomPanel::bottom("controls")
                .resizable(false)
                .show(ctx, |ui| {
                    controls::clip_controls(ui, clip, playback, settings, cur, dur, &mut actions)
                });
        }

        actions.extend(preview::show(
            ctx,
            &mut self.source,
            &mut self.export_options,
            &mut self.settings,
            &self.jobs,
        ));

        for action in actions {
            self.apply(action, now);
        }

        self.clip.commit_abandoned_gesture(ctx);
    }
}
