/// Inner padding applied to every button, enlarging the click target.
const BUTTON_PAD: egui::Vec2 = egui::Vec2::new(10.0, 6.0);

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
pub(crate) fn theme_pref_name(pref: egui::ThemePreference) -> &'static str {
    match pref {
        egui::ThemePreference::Dark => "dark",
        egui::ThemePreference::Light => "light",
        egui::ThemePreference::System => "system",
    }
}

pub(crate) fn theme_pref_from_name(name: &str) -> egui::ThemePreference {
    match name {
        "dark" => egui::ThemePreference::Dark,
        "light" => egui::ThemePreference::Light,
        _ => egui::ThemePreference::System,
    }
}

/// Label shown in the Settings theme dropdown.
pub(crate) fn theme_pref_label(pref: egui::ThemePreference) -> &'static str {
    match pref {
        egui::ThemePreference::Dark => "Dark",
        egui::ThemePreference::Light => "Light",
        egui::ThemePreference::System => "Match desktop",
    }
}

/// Register both theme palettes + shared spacing on the context, then activate
/// the chosen preference (egui resolves `System` against the desktop theme).
pub(crate) fn apply_theme(ctx: &egui::Context, pref: egui::ThemePreference) {
    ctx.set_visuals_of(egui::Theme::Dark, themed_visuals(egui::Theme::Dark));
    ctx.set_visuals_of(egui::Theme::Light, themed_visuals(egui::Theme::Light));
    ctx.all_styles_mut(|style| style.spacing.button_padding = BUTTON_PAD);
    ctx.set_theme(pref);
}
