use crate::app::action::Action;
use crate::app::ui::controls;
use crate::app::export_options::ExportOptions;
use crate::app::jobs::Jobs;
use crate::app::settings::Settings;
use crate::app::source::Source;
use crate::widgets::{attach_text_menu, button_height, text_edit_selection};
use crate::app::ui::{CONTROL_PAD, INPUT_MARGIN};

/// Gap between the output filename's extension and the "Save to:" folder control.
const SAVE_TARGET_GAP: f32 = 24.0;

/// The central panel: the save-as title row and format controls above an
/// aspect-fit preview of the current frame.
pub(crate) fn show(
    ctx: &egui::Context,
    source: &mut Source,
    export_options: &mut ExportOptions,
    settings: &mut Settings,
    jobs: &Jobs,
) -> Option<Action> {
    egui::CentralPanel::default()
        .show(ctx, |ui| {
            let mut export_req = None;
            if let Some(input) = source.video_path.clone() {
                let mut title = source.video_title.clone().unwrap_or_default();
                let ext = export_options.video_format.extension();
                let folder_label = match &settings.output_dir {
                    Some(d) => d
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| d.display().to_string()),
                    None => "Choose folder…".to_owned(),
                };
                let folder_hover = match &settings.output_dir {
                    Some(d) => format!("Saving into {}\n(set a default in Settings)", d.display()),
                    None => "Pick the folder to save into (set a default in Settings)".to_owned(),
                };
                let row_h = button_height(ui);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), row_h),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.label("Save as:");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(folder_label).on_hover_text(folder_hover).clicked() {
                                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                    settings.output_dir = Some(folder);
                                }
                            }
                            ui.label("Save to:");
                            ui.add_space(SAVE_TARGET_GAP);
                            ui.label(format!(".{ext}"));
                            let title_id = egui::Id::new("title_input_field");
                            let prev_selection = text_edit_selection(ui.ctx(), title_id);
                            let mut title_field = ui.add(
                                egui::TextEdit::singleline(&mut title)
                                    .id(title_id)
                                    .desired_width(f32::INFINITY)
                                    .margin(INPUT_MARGIN),
                            );
                            attach_text_menu(
                                ui,
                                title_id,
                                &mut title,
                                &mut title_field,
                                prev_selection,
                            );
                            if title_field.changed() {
                                source.video_title = Some(title);
                            }
                        });
                    },
                );
                ui.add_space(CONTROL_PAD);
                export_req =
                    controls::output_controls(ui, export_options, source, settings, jobs);
                ui.add_space(CONTROL_PAD);
                ui.separator();
                ui.add_space(CONTROL_PAD);
                if let Some(name) = input.file_name() {
                    ui.weak(format!("From: {}", name.to_string_lossy()));
                }
                ui.add_space(4.0);
            }

            match &source.frame_tex {
                Some(tex) => {
                    // Reserve the whole area once, then paint the frame into a
                    // centered sub-rect. Deriving widget size from available
                    // space directly would feed back and collapse to zero.
                    let (rect, _) =
                        ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
                    let img = tex.size_vec2();
                    if img.x > 0.0 && img.y > 0.0 {
                        let scale = (rect.width() / img.x).min(rect.height() / img.y);
                        let drawn = egui::Rect::from_center_size(rect.center(), img * scale);
                        egui::Image::new(tex).paint_at(ui, drawn);
                    }
                }
                None => {
                    let is_audio_only = source.is_ready() && !source.has_video();
                    ui.centered_and_justified(|ui| {
                        if is_audio_only {
                            ui.label("Audio only — no video stream to preview.");
                        } else {
                            ui.label("Open a file or download a video to begin.");
                        }
                    });
                }
            }
            export_req
        })
        .inner
}
