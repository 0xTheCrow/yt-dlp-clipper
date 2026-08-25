use crate::app::jobs::Jobs;
use crate::app::settings::Settings;
use crate::widgets::button_height;
use yt_dlp_clipper::ytdlp::{self, Browser, CookieSource};

const ERROR_MAX_HEIGHT: f32 = 120.0;

/// Show the last failure (full yt-dlp/export text) with Copy/Dismiss and, when
/// the message looks like an outdated binary, an "Update yt-dlp" button, or like
/// an age/bot-check wall, a browser-cookies picker.
pub(crate) fn show(ui: &mut egui::Ui, jobs: &mut Jobs, settings: &mut Settings) {
    let Some(err) = jobs.last_error.clone() else { return };
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        // Match the combo box to the buttons' height so it lines up with them
        // instead of sitting shorter in the row.
        ui.spacing_mut().interact_size.y = button_height(ui);
        ui.colored_label(egui::Color32::LIGHT_RED, "Error");
        if ytdlp::suggests_update(&err) {
            let is_updating = jobs.ytdlp_updating;
            let label = if is_updating { "Updating…" } else { "Update yt-dlp" };
            if ui.add_enabled(!is_updating, egui::Button::new(label)).clicked() {
                jobs.start_ytdlp_update();
            }
        }
        if ytdlp::suggests_cookies(&err) {
            let selected_text = match &settings.cookies {
                Some(CookieSource::Browser(browser)) => Browser::ALL
                    .iter()
                    .find(|(candidate, _)| candidate == browser)
                    .map_or("Use cookies from browser", |(_, label)| label),
                _ => "Use cookies from browser",
            };
            egui::ComboBox::from_id_salt("error_cookies_browser_select")
                .selected_text(selected_text)
                .show_ui(ui, |ui| {
                    for (browser, label) in Browser::ALL {
                        let is_selected = matches!(
                            &settings.cookies,
                            Some(CookieSource::Browser(chosen)) if *chosen == browser
                        );
                        if ui.selectable_label(is_selected, label).clicked() {
                            settings.cookies = Some(CookieSource::Browser(browser));
                        }
                    }
                });
        }
        if ui.button("Copy").clicked() {
            ui.ctx().copy_text(err.clone());
        }
        if ui.button("Dismiss").clicked() {
            jobs.last_error = None;
        }
    });
    egui::ScrollArea::vertical()
        .max_height(ERROR_MAX_HEIGHT)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            let text = egui::RichText::new(&err).monospace().color(egui::Color32::LIGHT_RED);
            ui.add(egui::Label::new(text).wrap());
        });
}
