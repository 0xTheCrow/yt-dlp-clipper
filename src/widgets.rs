use std::path::Path;

/// The floppy-disk "save" icon (embedded at compile time).
pub(crate) fn save_icon() -> egui::ImageSource<'static> {
    egui::include_image!("../assets/save.svg")
}

/// The "download" icon used by the Fetch button (embedded at compile time).
pub(crate) fn download_icon() -> egui::ImageSource<'static> {
    egui::include_image!("../assets/download.svg")
}

/// The gear "settings" icon (embedded at compile time).
pub(crate) fn settings_icon() -> egui::ImageSource<'static> {
    egui::include_image!("../assets/settings.svg")
}

pub(crate) fn play_selection_icon() -> egui::ImageSource<'static> {
    egui::include_image!("../assets/play-selection.svg")
}

pub(crate) fn bracket_left_icon() -> egui::ImageSource<'static> {
    egui::include_image!("../assets/bracket-left.svg")
}

pub(crate) fn bracket_right_icon() -> egui::ImageSource<'static> {
    egui::include_image!("../assets/bracket-right.svg")
}

pub(crate) fn arrow_right_icon() -> egui::ImageSource<'static> {
    egui::include_image!("../assets/arrow-right.svg")
}

pub(crate) const BRACKET_ASPECT: f32 = 1.0 / 3.0;
pub(crate) const ARROW_ASPECT: f32 = 1.4;
pub(crate) const ARROW_HEIGHT_RATIO: f32 = 0.62;

/// A button styled like a plain egui button but with a framing bracket `icon`
/// inside it, on the trailing edge when `icon_leading` is false (which egui's
/// own `Button` can't place). The icon tints and dims with the button state.
pub(crate) fn bracketed_button(
    ui: &mut egui::Ui,
    text: &str,
    icon: egui::ImageSource<'static>,
    icon_leading: bool,
    enabled: bool,
) -> egui::Response {
    ui.add_enabled_ui(enabled, |ui| {
        let padding = ui.spacing().button_padding;
        let icon_gap = ui.spacing().icon_spacing;
        let font = egui::TextStyle::Button.resolve(ui.style());
        let galley = ui.fonts(|f| f.layout_no_wrap(text.to_owned(), font, egui::Color32::PLACEHOLDER));
        let icon_side = ui.text_style_height(&egui::TextStyle::Button);
        let icon_size = egui::vec2(icon_side * BRACKET_ASPECT, icon_side);

        let content = egui::vec2(
            icon_size.x + icon_gap + galley.size().x,
            icon_size.y.max(galley.size().y),
        );
        let mut desired = content + 2.0 * padding;
        desired.y = desired.y.max(ui.spacing().interact_size.y);

        let (rect, response) = ui.allocate_at_least(desired, egui::Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), text));

        if ui.is_rect_visible(rect) {
            let visuals = ui.style().interact(&response);
            ui.painter().rect(
                rect.expand2(egui::Vec2::splat(visuals.expansion)),
                visuals.rounding,
                visuals.weak_bg_fill,
                visuals.bg_stroke,
            );
            let inner = rect.shrink2(padding);
            let icon_x = if icon_leading { inner.min.x } else { inner.max.x - icon_size.x };
            let text_x = if icon_leading { inner.min.x + icon_size.x + icon_gap } else { inner.min.x };
            let icon_rect = egui::Rect::from_min_size(
                egui::pos2(icon_x, inner.center().y - icon_size.y / 2.0),
                icon_size,
            );
            egui::Image::new(icon).tint(visuals.text_color()).paint_at(ui, icon_rect);
            ui.painter().galley(
                egui::pos2(text_x, inner.center().y - galley.size().y / 2.0),
                galley,
                visuals.text_color(),
            );
        }
        response
    })
    .inner
}

pub(crate) fn arrow_image(ui: &mut egui::Ui) -> egui::Response {
    let height = ui.text_style_height(&egui::TextStyle::Body) * ARROW_HEIGHT_RATIO;
    ui.add(
        egui::Image::new(arrow_right_icon())
            .fit_to_exact_size(egui::vec2(height * ARROW_ASPECT, height))
            .tint(ui.visuals().text_color()),
    )
}

/// A button with a square SVG `icon` to the left of `text`. The icon is sized to
/// the button font's height so it lines up with the caption.
pub(crate) fn icon_button(ui: &mut egui::Ui, icon: egui::ImageSource<'_>, text: &str) -> egui::Response {
    let size = ui.text_style_height(&egui::TextStyle::Button);
    let image = egui::Image::new(icon)
        .fit_to_exact_size(egui::Vec2::splat(size))
        .tint(ui.visuals().text_color());
    ui.add(egui::Button::image_and_text(image, text))
}

/// Width of the toggle switch as a multiple of its height (the track is a
/// rounded pill, so this is how far the knob travels plus its diameter).
const SWITCH_WIDTH_RATIO: f32 = 1.6;
/// Knob radius as a fraction of the track's half-height; big enough to hold the
/// "on"/"off" label, slightly inset from the track edge.
const SWITCH_KNOB_RATIO: f32 = 0.8;
/// "on"/"off" label font size as a fraction of the switch height.
const SWITCH_LABEL_RATIO: f32 = 0.42;

/// A sliding on/off switch: a pill track with a knob that animates between
/// sides. egui has no built-in switch, so this is the canonical hand-drawn one,
/// styled from the active theme's selectable colors.
pub(crate) fn toggle_switch(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let desired_size = ui.spacing().interact_size.y * egui::vec2(SWITCH_WIDTH_RATIO, 1.0);
    let (rect, mut response) = ui.allocate_exact_size(desired_size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, "")
    });

    if ui.is_rect_visible(rect) {
        let how_on = ui.ctx().animate_bool(response.id, *on);
        let visuals = ui.style().interact_selectable(&response, *on);
        let rect = rect.expand(visuals.expansion);
        let radius = 0.5 * rect.height();
        // Track fills with the accent color when on (via interact_selectable), so
        // the state reads as both knob-on-the-right and a color change.
        ui.painter().rect(rect, radius, visuals.bg_fill, visuals.bg_stroke);
        let knob_x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), how_on);
        let center = egui::pos2(knob_x, rect.center().y);
        let knob_radius = SWITCH_KNOB_RATIO * radius;
        // Contrasting knob fill (panel "extreme" bg) so the knob stands out from
        // the track in either theme; the label rides inside it.
        ui.painter()
            .circle(center, knob_radius, ui.visuals().extreme_bg_color, visuals.fg_stroke);
        let label = if *on { "on" } else { "off" };
        let font = egui::FontId::proportional(rect.height() * SWITCH_LABEL_RATIO);
        ui.painter()
            .text(center, egui::Align2::CENTER_CENTER, label, font, ui.visuals().text_color());
    }
    response
}

/// Read UTF-8 text from the system clipboard, or `None` when it's empty or
/// unavailable. Writing goes through egui (`Context::copy_text`); only reading
/// (for Paste) needs direct clipboard access.
fn clipboard_text() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

/// Byte offset of the `char_index`-th character in `s` (its end if past the end),
/// for turning egui's char-based cursor indices into `&str` slice bounds.
fn char_to_byte(s: &str, char_index: usize) -> usize {
    s.char_indices().nth(char_index).map_or(s.len(), |(b, _)| b)
}

/// Collapse a `TextEdit`'s caret to `char_index` and persist it, so an edit made
/// outside the widget (a context-menu Cut/Paste) leaves the cursor where the
/// user expects on the next frame.
fn set_text_cursor(ctx: &egui::Context, id: egui::Id, char_index: usize) {
    if let Some(mut state) = egui::text_edit::TextEditState::load(ctx, id) {
        let cursor = egui::text::CCursor::new(char_index);
        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(cursor)));
        state.store(ctx, id);
    }
}

/// The selection (or bare caret) of the `TextEdit` with this `id`, as char
/// indices.
pub(crate) fn text_edit_selection(ctx: &egui::Context, id: egui::Id) -> Option<egui::text::CCursorRange> {
    egui::text_edit::TextEditState::load(ctx, id).and_then(|s| s.cursor.char_range())
}

/// Starting width of the text-field context menu, so its left-aligned options
/// have room and don't hug the cursor.
const MENU_MIN_WIDTH: f32 = 180.0;
/// Padding around each context-menu option (the menu style otherwise zeroes the
/// vertical), enlarging the click target.
const MENU_BUTTON_PADDING: egui::Vec2 = egui::vec2(10.0, 8.0);

/// Attach a Cut/Copy/Paste context menu to a single-line `TextEdit` editing
/// `text` and identified by `id`. `prev_selection` is the field's selection read
/// before it was drawn this frame: a right-press collapses the selection, so it
/// is restored here for the menu to act on. Cut/Paste edit `text` directly and
/// mark `response` changed, so a caller mirroring `text` elsewhere still sees it.
pub(crate) fn attach_text_menu(
    ui: &mut egui::Ui,
    id: egui::Id,
    text: &mut String,
    response: &mut egui::Response,
    prev_selection: Option<egui::text::CCursorRange>,
) {
    // A right-press collapses the selection, so restore it (and focus the field)
    // for Cut/Copy/Paste to operate on.
    if response.contains_pointer() && ui.input(|i| i.pointer.secondary_pressed()) {
        if let (Some(range), Some(mut state)) =
            (prev_selection, egui::text_edit::TextEditState::load(ui.ctx(), id))
        {
            state.cursor.set_char_range(Some(range));
            state.store(ui.ctx(), id);
        }
        ui.memory_mut(|m| m.request_focus(id));
    }

    let mut edited = false;
    response.context_menu(|ui| {
        ui.set_min_width(MENU_MIN_WIDTH);
        ui.spacing_mut().button_padding = MENU_BUTTON_PADDING;
        // Sorted (start, end) char indices of the selection; equal for a caret.
        let (start, end) = text_edit_selection(ui.ctx(), id).map_or((0, 0), |r| {
            let (p, s) = (r.primary.index, r.secondary.index);
            (p.min(s), p.max(s))
        });
        let has_selection = start != end;
        let (byte_start, byte_end) = (char_to_byte(text, start), char_to_byte(text, end));

        // Paste leads: pasting a link is the most likely reason to open the menu.
        if ui.button("Paste").clicked() {
            if let Some(clip) = clipboard_text() {
                // Trim surrounding whitespace/newlines a single-line field can't hold.
                let pasted = clip.trim();
                text.replace_range(byte_start..byte_end, pasted);
                set_text_cursor(ui.ctx(), id, start + pasted.chars().count());
                edited = true;
            }
            ui.close_menu();
        }
        if ui.add_enabled(has_selection, egui::Button::new("Cut")).clicked() {
            ui.ctx().copy_text(text[byte_start..byte_end].to_owned());
            text.replace_range(byte_start..byte_end, "");
            set_text_cursor(ui.ctx(), id, start);
            edited = true;
            ui.close_menu();
        }
        if ui.add_enabled(has_selection, egui::Button::new("Copy")).clicked() {
            ui.ctx().copy_text(text[byte_start..byte_end].to_owned());
            ui.close_menu();
        }
    });
    if edited {
        response.mark_changed();
    }
}

/// Reveal a saved file in the system file manager, selecting it when the
/// platform supports it and otherwise opening its containing folder.
pub(crate) fn reveal_in_file_manager(path: &Path) {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(&path)
        .spawn();
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // Explorer parses its own command line: it needs `/select,` and the
        // path as one unquoted token with the path itself quoted, and only
        // accepts backslash separators. Rust's argument escaping would instead
        // quote the whole token whenever the path contains a space, which
        // Explorer ignores in favor of its default folder.
        let native = path.to_string_lossy().replace('/', "\\");
        let _ = std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{native}\""))
            .spawn();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let dir = path.parent().unwrap_or(path.as_path());
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

/// Height of a standard button (font height + vertical padding). Used to size
/// manually-laid-out rows so their contents stay vertically centered.
pub(crate) fn button_height(ui: &egui::Ui) -> f32 {
    let text = ui.text_style_height(&egui::TextStyle::Button);
    (text + 2.0 * ui.spacing().button_padding.y).max(ui.spacing().interact_size.y)
}
