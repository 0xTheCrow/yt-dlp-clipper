pub(crate) mod cache_window;
pub(crate) mod controls;
pub(crate) mod error_panel;
pub(crate) mod preview;
pub(crate) mod settings_window;
pub(crate) mod time_axis;
pub(crate) mod timeline;
pub(crate) mod toolbar;

/// Vertical breathing room between the stacked rows of a control panel.
pub(crate) const CONTROL_PAD: f32 = 6.0;

/// Inner margin for text input fields. Vertical padding is kept small so inputs
/// never stand taller than the buttons beside them.
pub(crate) const INPUT_MARGIN: egui::Margin = egui::Margin {
    left: 8.0,
    right: 8.0,
    top: 4.0,
    bottom: 4.0,
};
