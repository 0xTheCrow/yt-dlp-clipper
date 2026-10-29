use crate::app::jobs::{apply_update, Jobs, UpdateApplyState, UpdateState};
use crate::app::ui_state::UiState;
use crate::format::fmt_size;

/// Dismissible "a new version is available" banner, shown when an automatic
/// check finds one — placed near `error_panel` but kept separate, since this
/// isn't an error. Dismissing ("Later") is session-only:
/// `ui_state.update_banner_dismissed` is never persisted, so the next launch
/// checks again and shows it once more if the release is still available.
/// Settings keeps its own inline notice too (the Phase 1 manual-check UI),
/// since that's opt-in and distinct from this automatic banner.
pub(crate) fn show(ui: &mut egui::Ui, jobs: &mut Jobs, ui_state: &mut UiState) {
    if ui_state.update_banner_dismissed {
        return;
    }
    let UpdateState::Available(release) = jobs.update_state.clone() else { return };

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(format!("Version {} is available.", release.version));
        match jobs.update_apply.clone() {
            UpdateApplyState::Idle | UpdateApplyState::Failed(_) => {
                if ui.button("Update").clicked() {
                    jobs.start_update_download(release.clone());
                }
            }
            UpdateApplyState::Downloading(downloaded, total) => {
                let frac = if total > 0 {
                    (downloaded as f32 / total as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let text = if total > 0 {
                    format!("downloading {} / {}", fmt_size(downloaded), fmt_size(total))
                } else {
                    "downloading…".to_owned()
                };
                ui.add(egui::ProgressBar::new(frac).text(text));
            }
            UpdateApplyState::Probing => {
                ui.label("checking the download…");
            }
            UpdateApplyState::ReadyToApply(pending) => {
                // Restart gate: an export or download in flight is never
                // interrupted by a self-update swap.
                let busy = jobs.is_worker_busy();
                if ui
                    .add_enabled(!busy, egui::Button::new("Restart now"))
                    .on_disabled_hover_text("Finish the current export/download first")
                    .clicked()
                    && let Err(error) = apply_update(&pending) {
                    jobs.update_apply = UpdateApplyState::Failed(error.to_string());
                }
            }
        }
        if ui.button("Later").clicked() {
            ui_state.update_banner_dismissed = true;
        }
    });
    if let UpdateApplyState::Failed(error) = &jobs.update_apply {
        ui.colored_label(egui::Color32::LIGHT_RED, format!("Update failed: {error}"));
    }
}
