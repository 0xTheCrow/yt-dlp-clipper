use crate::app::action::{Action, Nav};
use crate::app::clip::Clip;
use crate::app::export_options::ExportOptions;
use crate::format::{audio_format_label, fmt_time};
use crate::app::jobs::Jobs;
use crate::app::playback::Playback;
use crate::app::settings::Settings;
use crate::app::source::Source;
use crate::app::ui::timeline;
use crate::widgets::{
    arrow_image, bracket_left_icon, bracket_right_icon, bracketed_button, button_height,
    icon_button, play_selection_icon, save_icon, toggle_switch, BRACKET_ASPECT,
};
use crate::app::ui::CONTROL_PAD;
use yt_dlp_clipper::export::{self, Mode};

/// Width of the volume slider track.
const VOLUME_SLIDER_WIDTH: f32 = 90.0;

/// Standard heights offered when downscaling a saved video, tallest first. Only
/// those shorter than the source are shown, so the menu never upscales.
const EXPORT_HEIGHT_LADDER: [u32; 8] = [2160, 1440, 1080, 720, 480, 360, 240, 144];

/// Clip controls shown under the preview: scrub, frame step, in/out.
pub(crate) fn clip_controls(
    ui: &mut egui::Ui,
    clip: &mut Clip,
    playback: &mut Playback,
    settings: &mut Settings,
    cur: f64,
    dur: f64,
    actions: &mut Vec<Action>,
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

    let row_h = button_height(ui);
    let icon_w = ui.text_style_height(&egui::TextStyle::Button);
    let icon_gap = ui.spacing().icon_spacing;
    let bracket_icon_w = icon_w * BRACKET_ASPECT;
    let icon_btn_w = |ui: &egui::Ui, icon_w: f32, text: &str| {
        icon_w + icon_gap + text_w(ui, text, &btn_font) + 2.0 * pad
    };
    let play_selection_w = icon_btn_w(ui, icon_w, "Play Selection");

    let set_pos = playback.awaiting_release.map_or(cur, |(_, pos)| pos);
    let in_time = fmt_time(clip.in_secs);
    let out_time = fmt_time(clip.out_secs);
    let start_label = format!("Set Start ({})", settings.keybinds.set_start.label());
    let end_label = format!("Set End ({})", settings.keybinds.set_end.label());
    let left_w =
        icon_btn_w(ui, bracket_icon_w, &start_label) + gap + text_w(ui, &in_time, &mono_font);
    let right_w =
        text_w(ui, &out_time, &mono_font) + gap + icon_btn_w(ui, bracket_icon_w, &end_label);
    let center_w = play_selection_w + gap + btn_w(ui, "⏸ Pause");
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
    let center_x = (row.left() + left_w + row.right() - right_w) / 2.0 - center_w / 2.0;

    ui.allocate_new_ui(
        sub(row.left(), left_w, egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            let can_set_start = set_pos < clip.out_secs;
            let icon = bracket_left_icon();
            let start = bracketed_button(ui, &start_label, icon, true, can_set_start);
            if start.clicked() {
                clip.set_in_secs(set_pos);
            }
            ui.monospace(&in_time);
        },
    );
    ui.allocate_new_ui(
        sub(center_x, center_w, egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            if icon_button(ui, play_selection_icon(), "Play Selection")
                .on_hover_ui(|ui| {
                    ui.horizontal(|ui| {
                        ui.label("Plays the trimmed selection (Start");
                        arrow_image(ui);
                        ui.label("End)");
                    });
                })
                .clicked()
            {
                actions.push(Action::PlaySelection);
            }
            if ui
                .add_enabled(playback.playing, egui::Button::new("⏸ Pause"))
                .clicked()
            {
                playback.stop();
            }
        },
    );
    ui.allocate_new_ui(
        sub(row.right() - right_w, right_w, egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.monospace(&out_time);
            let can_set_end = set_pos > clip.in_secs;
            let end = bracketed_button(ui, &end_label, bracket_right_icon(), false, can_set_end);
            if end.clicked() {
                clip.set_out_secs(set_pos);
            }
        },
    );

    ui.add_space(4.0);
    timeline::overview_bar(ui, clip, dur);
    ui.add_space(2.0);
    let seek = timeline::timeline(ui, clip, playback, cur, dur);
    if let Some((t, released)) = seek {
        playback.stop();
        actions.push(Action::Nav(Nav::Seek { secs: t, released }));
    }

    ui.add_space(4.0);
    let transport_h = button_height(ui);
    let play_label = if playback.playing { "⏸  Pause" } else { "▶  Play from Seeker" };
    let play_w = text_w(ui, "▶  Play from Seeker", &btn_font) + 2.0 * pad;
    let left_w = btn_w(ui, "⏮  Frame") + gap + play_w + gap + btn_w(ui, "Frame  ⏭");
    let pct_w = text_w(ui, "100%", &btn_font) + 2.0 * pad;
    let volume_w = text_w(ui, "🔊", &btn_font) + gap + VOLUME_SLIDER_WIDTH + gap + pct_w;

    // Follow a pending seek target (held skip / released drag) like the
    // playhead does, so the readout doesn't lag behind the position.
    let shown = playback.awaiting_release.map_or(cur, |(_, pos)| pos);
    let time_readout = format!("{}  /  {}", fmt_time(shown), fmt_time(dur));
    let time_w = text_w(ui, &time_readout, &mono_font);

    let (transport_row, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), transport_h), egui::Sense::hover());
    let transport_sub = |min_x: f32, w: f32, layout: egui::Layout| {
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_size(
                egui::pos2(min_x, transport_row.min.y),
                egui::vec2(w, transport_h),
            ))
            .layout(layout)
    };

    ui.allocate_new_ui(
        transport_sub(
            transport_row.left(),
            left_w,
            egui::Layout::left_to_right(egui::Align::Center),
        ),
        |ui| {
            if ui.button("⏮  Frame").clicked() {
                playback.stop();
                actions.push(Action::Nav(Nav::Back));
            }
            if ui
                .add(egui::Button::new(play_label).min_size(egui::vec2(play_w, 0.0)))
                .on_hover_text("Plays from the current seeker position to the end")
                .clicked()
            {
                actions.push(Action::TogglePlay);
            }
            if ui.button("Frame  ⏭").clicked() {
                playback.stop();
                actions.push(Action::Nav(Nav::Forward));
            }
        },
    );

    let time_x = transport_row.center().x - time_w / 2.0;
    ui.allocate_new_ui(
        transport_sub(time_x, time_w, egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.monospace(time_readout);
        },
    );

    // Added right-to-left so it reads 🔊, slider, then the fixed-width percent.
    ui.allocate_new_ui(
        transport_sub(
            transport_row.right() - volume_w,
            volume_w,
            egui::Layout::right_to_left(egui::Align::Center),
        ),
        |ui| {
            ui.add_sized(
                egui::vec2(pct_w, transport_h),
                egui::Label::new(format!("{:.0}%", settings.volume * 100.0)),
            );
            ui.spacing_mut().slider_width = VOLUME_SLIDER_WIDTH;
            let vol = ui.add(
                egui::Slider::new(&mut settings.volume, 0.0..=1.0).show_value(false),
            );
            if vol.changed() {
                if let Some(audio) = &playback.audio {
                    audio.set_volume(settings.volume);
                }
            }
            ui.label("🔊");
        },
    );

    ui.add_space(CONTROL_PAD);
}

/// Format pickers and Save buttons, shown above the preview under the title
/// row. A fixed row height keeps a short leading label like "Audio:" centered,
/// and one shared widget height makes buttons match the taller comboboxes.
pub(crate) fn output_controls(
    ui: &mut egui::Ui,
    export_options: &mut ExportOptions,
    source: &Source,
    settings: &mut Settings,
    jobs: &Jobs,
) -> Option<Action> {
    let mut request = None;
    let row_h = ui
        .text_style_height(&egui::TextStyle::Button)
        .max(ui.spacing().icon_width)
        + 2.0 * ui.spacing().button_padding.y;
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), row_h),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
        ui.spacing_mut().interact_size.y = row_h;

        let vid = export_options.video_format.extension();
        let src_height = source.decoder.as_ref().map_or(0, |d| d.height);
        // A decode failure clears the decoder but leaves `video_path` set.
        let is_decoder_ready = source.is_ready();
        let can_export = is_decoder_ready && !jobs.is_worker_busy();
        let can_export_video = can_export && source.has_video();

        ui.label("Audio:");
        egui::ComboBox::from_id_salt("audio_format")
            .selected_text(audio_format_label(export_options.audio_format))
            .show_ui(ui, |ui| {
                use export::AudioFormat::*;
                let format = &mut export_options.audio_format;
                ui.selectable_value(format, Mp3, audio_format_label(Mp3));
                ui.selectable_value(format, Aac, audio_format_label(Aac));
                ui.selectable_value(format, Original, audio_format_label(Original));
            });
        let save_audio = ui
            .add_enabled_ui(can_export, |ui| {
                icon_button(ui, save_icon(), "Save audio only…")
            })
            .inner;
        if save_audio.clicked() {
            let fmt = export_options.audio_format;
            let extension = source
                .video_path
                .as_ref()
                .and_then(|p| export::audio_extension(&p.to_string_lossy(), fmt).ok())
                .unwrap_or("mp3");
            request = Some(Action::Export { mode: Mode::AudioOnly(fmt), extension });
        }
        ui.separator();

        // Added right-to-left so it reads: Video, Resolution, Save full video, Save clip.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let save_clip = ui
                .add_enabled_ui(can_export_video, |ui| {
                    icon_button(ui, save_icon(), "Save clip…")
                })
                .inner;
            if save_clip.clicked() {
                request = Some(Action::Export { mode: Mode::Clip, extension: vid });
            }
            let save_full = ui
                .add_enabled_ui(can_export_video, |ui| {
                    icon_button(ui, save_icon(), "Save full video…")
                })
                .inner;
            if save_full.clicked() {
                request = Some(Action::Export { mode: Mode::Full, extension: vid });
            }

            // Downscale menu, capped to the source height so it never upscales.
            let res_text = match export_options.scale_height {
                None => "Original".to_string(),
                Some(h) => format!("{h}p"),
            };
            egui::ComboBox::from_id_salt("export_height")
                .selected_text(res_text)
                .show_ui(ui, |ui| {
                    let height = &mut export_options.scale_height;
                    ui.selectable_value(height, None, "Original");
                    for h in EXPORT_HEIGHT_LADDER.iter().filter(|h| **h < src_height) {
                        let height = &mut export_options.scale_height;
                        ui.selectable_value(height, Some(*h), format!("{h}p"));
                    }
                });
            ui.label("Resolution:");

            // Compatibility only changes the MP4/MOV path; for MKV/WebM (not
            // iOS-playable regardless of codec) the control is greyed out.
            let compat_applies = matches!(
                export_options.video_format,
                export::VideoFormat::Mp4 | export::VideoFormat::Mov
            );
            let compat_hover = "Re-encode a saved MP4/MOV to H.264 8-bit + AAC so it \
                plays on phones (iOS/Android) and TVs, not just computers. Turn off to \
                keep the source codec and quality (HEVC/AV1, 10-bit, 4K HDR).";
            ui.add_enabled_ui(compat_applies, |ui| {
                // Right-to-left layout: add the switch first so the label lands
                // to its left, reading "Compatible ▢".
                toggle_switch(ui, &mut settings.compatibility_mode).on_hover_text(compat_hover);
                ui.label("Compatible").on_hover_text(compat_hover);
            });

            egui::ComboBox::from_id_salt("video_format")
                .selected_text(vid.to_uppercase())
                .show_ui(ui, |ui| {
                    use export::VideoFormat::*;
                    let format = &mut export_options.video_format;
                    ui.selectable_value(format, Mp4, "MP4");
                    ui.selectable_value(format, Mkv, "MKV");
                    ui.selectable_value(format, Mov, "MOV");
                    ui.selectable_value(format, Webm, "WebM");
                });
            ui.label("Video:");
        });
    });

    request
}
