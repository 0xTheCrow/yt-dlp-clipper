use crate::format::{fmt_duration, fmt_size};
use crate::app::ui_state::CacheBrowser;
use std::path::PathBuf;

/// Cache-browser thumbnail cell size.
const CACHE_THUMB_W: f32 = 200.0;
const CACHE_THUMB_H: f32 = 120.0;
/// Room under a thumbnail for the filename and the size/duration line.
const CACHE_CAPTION_H: f32 = 44.0;

/// Grid of cached videos with thumbnails; returns the one that was clicked.
pub(crate) fn show(ctx: &egui::Context, cache: &mut CacheBrowser) -> Option<PathBuf> {
    if !cache.is_open {
        return None;
    }
    let mut open = true;
    let mut selected = None;
    egui::Window::new("Cached videos")
        .open(&mut open)
        .default_size([680.0, 460.0])
        .show(ctx, |ui| {
            if cache.entries.is_empty() {
                ui.label("No cached videos found.");
                return;
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for entry in &cache.entries {
                        let cell = egui::vec2(CACHE_THUMB_W, CACHE_THUMB_H + CACHE_CAPTION_H);
                        ui.allocate_ui(cell, |ui| {
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

    cache.is_open = open;
    if selected.is_some() || !cache.is_open {
        cache.close();
    }
    selected
}
