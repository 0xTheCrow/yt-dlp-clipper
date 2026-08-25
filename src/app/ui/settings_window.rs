use crate::binaries::{clear_dir, dir_size, managed_cache_dir, reset_bundled_tools};
use crate::format::fmt_size;
use crate::app::jobs::{Jobs, UpdateState};
use crate::keybinds::{Bind, Shortcut};
use crate::app::settings::Settings;
use crate::theme::{apply_theme, theme_pref_label};
use crate::app::ui_state::UiState;
use crate::widgets::button_height;
use crate::APP_DISPLAY_NAME;
use yt_dlp_clipper::ytdlp::{self, Browser, CookieSource};

/// Fixed content width so the panel reads roomy and the keyboard shortcuts fit
/// comfortably in two columns.
const SETTINGS_WIDTH: f32 = 620.0;
/// Vertical breathing room placed around each section separator.
const SECTION_GAP: f32 = 8.0;
/// Width reserved for the scale slider's value field so the rail can fill the
/// rest of its column without the field overflowing.
const SCALE_VALUE_W: f32 = 64.0;
const MIN_UI_SCALE: f32 = 0.75;
const MAX_UI_SCALE: f32 = 2.5;
const UI_SCALE_STEP: f64 = 0.05;

pub(crate) fn show(
    ctx: &egui::Context,
    settings: &mut Settings,
    ui_state: &mut UiState,
    jobs: &mut Jobs,
) {
    let cache_dir = managed_cache_dir();
    let cache_bytes = dir_size(&cache_dir);
    let current_scale = settings.ui_scale;
    let previous_theme = settings.theme;
    // Applying a scale retunes the whole context, so it is deferred until the
    // window has finished drawing at the scale this frame started with.
    let mut apply_scale = None;

    // Lazily resolve the version once per open (cleared after an update so it
    // refetches); errors are cached too so it doesn't re-probe every frame.
    if jobs.ytdlp_version.is_none() {
        jobs.ytdlp_version = Some(match ytdlp::version() {
            Ok(version) => version,
            Err(e) => format!("unavailable ({e})"),
        });
    }

    egui::Window::new("Settings")
        .open(&mut ui_state.is_settings_open)
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
                            egui::Slider::new(
                                &mut ui_state.pending_scale,
                                MIN_UI_SCALE..=MAX_UI_SCALE,
                            )
                            .step_by(UI_SCALE_STEP)
                            .suffix("×"),
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let pending = ui_state.pending_scale;
                            let is_changed = (pending - current_scale).abs() > f32::EPSILON;
                            ui.label(format!("current: {current_scale:.2}×"));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("Reset").clicked() {
                                        apply_scale = Some(1.0);
                                    }
                                    let apply = egui::Button::new("Apply");
                                    if ui.add_enabled(is_changed, apply).clicked() {
                                        apply_scale = Some(pending);
                                    }
                                },
                            );
                        });

                        let ui = &mut cols[1];
                        ui.label("Theme");
                        egui::ComboBox::from_id_salt("theme_select")
                            .selected_text(theme_pref_label(settings.theme))
                            .show_ui(ui, |ui| {
                                for pref in [
                                    egui::ThemePreference::System,
                                    egui::ThemePreference::Light,
                                    egui::ThemePreference::Dark,
                                ] {
                                    let label = theme_pref_label(pref);
                                    ui.selectable_value(&mut settings.theme, pref, label);
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
                ui.label(settings.effective_download_dir().display().to_string());
            });
            ui.horizontal(|ui| {
                if ui.button("Choose folder…").clicked() {
                    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                        settings.download_dir = Some(folder);
                    }
                }
                if ui.button("Use default cache").clicked() {
                    settings.download_dir = None;
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(format!("Cache: {}", fmt_size(cache_bytes)));
                if ui.button("Clear downloads").clicked() {
                    clear_dir(&cache_dir);
                }
            });
            ui.checkbox(&mut settings.delete_cache_on_exit, "Delete cache on exit");
            ui.small("Clearing affects only the cache, not a custom folder.");

            ui.add_space(SECTION_GAP);
            ui.separator();
            ui.add_space(SECTION_GAP);
            ui.horizontal(|ui| {
                ui.label("Output location:");
                match &settings.output_dir {
                    Some(dir) => ui.label(dir.display().to_string()),
                    None => ui.label("Not set — dialog opens at the system default"),
                };
            });
            ui.horizontal(|ui| {
                if ui.button("Choose folder…").clicked() {
                    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                        settings.output_dir = Some(folder);
                    }
                }
                if ui.button("Clear").clicked() {
                    settings.output_dir = None;
                }
            });
            ui.checkbox(&mut settings.open_dir_on_save, "Open output folder after saving");

            ui.add_space(SECTION_GAP);
            ui.separator();
            ui.add_space(SECTION_GAP);
            ui.label("Keyboard shortcuts");
            ui.add_space(4.0);
            for pair in Bind::ALL.chunks(2) {
                ui.columns(2, |cols| {
                    for (col, (bind, label)) in cols.iter_mut().zip(pair) {
                        col.horizontal(|ui| {
                            ui.label(*label);
                            let layout = egui::Layout::right_to_left(egui::Align::Center);
                            ui.with_layout(layout, |ui| {
                                let text = if ui_state.rebinding == Some(*bind) {
                                    "Press a key…".to_owned()
                                } else {
                                    settings.keybinds.shortcut(*bind).label()
                                };
                                if ui.button(text).clicked() {
                                    ui_state.rebinding = Some(*bind);
                                }
                            });
                        });
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
                let version = jobs.ytdlp_version.clone().unwrap_or_default();
                ui.label(format!("Version: {version}"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let is_updating = jobs.ytdlp_updating;
                    let label = if is_updating { "Updating…" } else { "Update" };
                    if ui.add_enabled(!is_updating, egui::Button::new(label)).clicked() {
                        jobs.start_ytdlp_update();
                    }
                });
            });
            ui.small("Update fixes most download failures when a site changes.");

            ui.add_space(SECTION_GAP);
            ui.horizontal(|ui| {
                ui.label("Bundled tools");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Reset").clicked() {
                        reset_bundled_tools();
                        jobs.status = "bundled tools reset — restart to reinstall them".into();
                    }
                });
            });
            ui.small(
                "Reinstall the yt-dlp and ffmpeg copies shipped with the app. \
                 Takes effect on restart.",
            );

            ui.add_space(SECTION_GAP);
            ui.horizontal(|ui| {
                ui.spacing_mut().interact_size.y = button_height(ui);
                ui.label("Cookies:");
                // yt-dlp accepts only one cookie source per run, so the browser
                // and file pickers are laid out as alternatives — picking one
                // clears the other, rather than nesting a source-select dropdown
                // a reader has to open first.
                let selected_text = match &settings.cookies {
                    Some(CookieSource::Browser(browser)) => Browser::ALL
                        .iter()
                        .find(|(candidate, _)| candidate == browser)
                        .map_or("None", |(_, label)| label),
                    Some(CookieSource::File(_)) => "File selected",
                    None => "None",
                };
                egui::ComboBox::from_id_salt("cookies_from_browser_select")
                    .selected_text(selected_text)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut settings.cookies, None, "None");
                        for (browser, label) in Browser::ALL {
                            ui.selectable_value(
                                &mut settings.cookies,
                                Some(CookieSource::Browser(browser)),
                                label,
                            );
                        }
                    });
                ui.label("OR");
                if ui.button("Choose cookies file…").clicked() {
                    if let Some(picked) = rfd::FileDialog::new().pick_file() {
                        settings.cookies = Some(CookieSource::File(picked));
                    }
                }
            });
            if let Some(CookieSource::File(path)) = &settings.cookies {
                ui.weak(path.display().to_string());
            }
            ui.small(
                "Needed for age-restricted or members-only videos. \"From browser\" needs \
                 the browser closed if it reports the cookie database is locked; a cookies \
                 file (exported via a browser extension, e.g. \"Get cookies.txt\") works \
                 with the browser left open.",
            );

            ui.add_space(SECTION_GAP);
            ui.separator();
            ui.add_space(SECTION_GAP);
            ui.label(APP_DISPLAY_NAME);
            ui.horizontal(|ui| {
                ui.label(format!("Version: {}", env!("CARGO_PKG_VERSION")));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let is_checking = matches!(&jobs.update_state, UpdateState::Checking);
                    let label = if is_checking { "Checking…" } else { "Check for updates" };
                    if ui.add_enabled(!is_checking, egui::Button::new(label)).clicked() {
                        jobs.start_update_check();
                    }
                });
            });
            match &jobs.update_state {
                UpdateState::Idle | UpdateState::Checking => {}
                UpdateState::UpToDate => {
                    ui.small("Up to date.");
                }
                UpdateState::Available(release) => {
                    ui.horizontal(|ui| {
                        ui.small(format!("Version {} is available.", release.version));
                        ui.hyperlink_to("Open release page", &release.page_url);
                    });
                }
                UpdateState::Failed(error) => {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("Check failed: {error}"));
                }
            }
        });

    if let Some(scale) = apply_scale {
        settings.ui_scale = scale;
        ui_state.pending_scale = scale;
        ctx.set_zoom_factor(scale);
    }
    if settings.theme != previous_theme {
        apply_theme(ctx, settings.theme);
    }

    // While an action is capturing, the next key press rebinds it (Esc
    // cancels). The main shortcut handler is suppressed during capture.
    if let Some(bind) = ui_state.rebinding {
        let captured = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key { key, pressed: true, modifiers, .. } => {
                    Some((*key, modifiers.ctrl, modifiers.shift))
                }
                _ => None,
            })
        });
        if let Some((key, ctrl, shift)) = captured {
            if key != egui::Key::Escape {
                settings.keybinds.rebind(bind, Shortcut { key, ctrl, shift });
            }
            ui_state.rebinding = None;
        }
    }
}
