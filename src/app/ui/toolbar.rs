use crate::app::action::Action;
use crate::app::download::Download;
use crate::app::ui::error_panel;
use crate::format::fmt_size;
use crate::app::jobs::{Jobs, Msg};
use crate::app::settings::Settings;
use crate::app::ui_state::UiState;
use crate::widgets::{
    attach_text_menu, button_height, download_icon, icon_button, reveal_in_file_manager,
    settings_icon, text_edit_selection,
};
use crate::app::ui::INPUT_MARGIN;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use yt_dlp_clipper::ytdlp;

const URL_FIELD_WIDTH: f32 = 260.0;

/// Top toolbar: URL/download, format picker, settings.
pub(crate) fn show(
    ctx: &egui::Context,
    download: &mut Download,
    jobs: &mut Jobs,
    settings: &mut Settings,
    ui_state: &mut UiState,
) -> Option<Action> {
    let is_worker_busy = jobs.is_worker_busy();
    let mut action = None;

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
                let url_id = egui::Id::new("url_input_field");
                // Read the selection before drawing the field; a right-press
                // collapses it, and `attach_text_menu` restores this.
                let prev_selection = text_edit_selection(ui.ctx(), url_id);
                let mut url_field = ui.add_sized(
                    [URL_FIELD_WIDTH, row_h],
                    egui::TextEdit::singleline(&mut download.url)
                        .id(url_id)
                        .margin(INPUT_MARGIN),
                );
                attach_text_menu(ui, url_id, &mut download.url, &mut url_field, prev_selection);
                let submitted =
                    url_field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let fetch = ui
                    .add_enabled_ui(!is_worker_busy, |ui| icon_button(ui, download_icon(), "Fetch"))
                    .inner;
                let is_url_present = !download.url.is_empty();
                if (fetch.clicked() || submitted) && !is_worker_busy && is_url_present {
                    action = Some(Action::Fetch);
                }
                if ui.button("Open file…").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_file() {
                        action = Some(Action::OpenFile(path));
                    }
                }
                if ui.button("Open from cache…").clicked() {
                    ui_state.cache.open();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, settings_icon(), "Settings").clicked() {
                        ui_state.is_settings_open = true;
                        ui_state.pending_scale = settings.ui_scale;
                    }
                });
            },
        );

        let heights: Vec<u32> =
            download.info.as_ref().map(ytdlp::available_heights).unwrap_or_default();
        let est_size = download.info.as_ref().and_then(|info| {
            ytdlp::estimated_size(
                info,
                download.selected_height,
                download.want_video,
                download.want_audio,
            )
        });

        if download.info.is_some() {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut download.want_video, "Video");
                ui.checkbox(&mut download.want_audio, "Audio");

                let source_label = match heights.first() {
                    Some(h) => format!("Source ({h}p)"),
                    None => "Source".to_string(),
                };
                let selected_text = match download.selected_height {
                    None => source_label.clone(),
                    Some(h) => format!("{h}p"),
                };
                egui::ComboBox::from_id_salt("download_resolution")
                    .selected_text(selected_text)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut download.selected_height, None, &source_label);
                        // Skip the tallest height; "Source" already covers it.
                        for h in heights.iter().skip(1) {
                            let height = &mut download.selected_height;
                            ui.selectable_value(height, Some(*h), format!("{h}p"));
                        }
                    });

                let start = egui::Button::new("Download");
                if ui.add_enabled(!is_worker_busy, start).clicked() {
                    let selector = ytdlp::resolution_selector(
                        download.selected_height,
                        download.want_video,
                        download.want_audio,
                    );
                    let url = download.url.clone();
                    let cookies = settings.cookies.clone();
                    let dir = settings.effective_download_dir();
                    let _ = std::fs::create_dir_all(&dir);
                    jobs.status = "downloading…".into();
                    jobs.progress = Some((0, 0));
                    let cancel = Arc::new(AtomicBool::new(false));
                    jobs.download_cancel = cancel.clone();
                    jobs.spawn(move |tx| {
                        let progress_tx = tx.clone();
                        let result = ytdlp::download(
                            &url,
                            selector.as_deref(),
                            cookies.as_ref(),
                            &dir,
                            &cancel,
                            |downloaded, total| {
                                let _ = progress_tx.send(Msg::Progress { downloaded, total });
                            },
                        );
                        let _ = tx.send(match result {
                            Ok(path) => Msg::Downloaded(path),
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

        if !jobs.status.is_empty() {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                // Pin the row to the button's height so the labels and the
                // taller button both center vertically against it.
                ui.set_min_height(button_height(ui));
                ui.label("Status:");
                ui.monospace(&jobs.status);
                if let Some(saved) = &jobs.saved_path {
                    if ui.button("Open folder").clicked() {
                        reveal_in_file_manager(saved);
                    }
                }
            });
        }
        error_panel::show(ui, jobs, settings);
        if let Some((downloaded, total)) = jobs.progress {
            let frac =
                if total > 0 { (downloaded as f32 / total as f32).clamp(0.0, 1.0) } else { 0.0 };
            let text = if total > 0 {
                format!("{} / {} ({:.0}%)", fmt_size(downloaded), fmt_size(total), frac * 100.0)
            } else {
                "starting…".to_owned()
            };
            ui.add(egui::ProgressBar::new(frac).text(text));
        }
        if jobs.exporting {
            let written = jobs
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
                    jobs.export_cancel.store(true, Ordering::Relaxed);
                    jobs.status = "canceling…".into();
                }
            });
            // Keep repainting so the size readout grows even without input.
            ui.ctx().request_repaint();
        }
        ui.add_space(4.0);
    });

    action
}
