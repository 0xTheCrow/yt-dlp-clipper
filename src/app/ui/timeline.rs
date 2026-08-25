use crate::app::clip::{Clip, TimelineDrag, WindowDrag, MIN_RANGE_SECS};
use crate::app::playback::Playback;
use crate::app::ui::time_axis::{nearest_handle, TimeAxis};

/// Visible span multiplier per unit of scroll over the timeline.
const SCROLL_ZOOM_RATE: f32 = 0.0015;
/// Scroll-zoom has no release event, so its undo gesture closes after this idle.
const SCROLL_GESTURE_IDLE_SECS: f64 = 0.4;

const OVERVIEW_HEIGHT: f32 = 12.0;
const OVERVIEW_ROUNDING: f32 = 3.0;
const GRIP_WIDTH: f32 = 4.0;
const TIMELINE_HEIGHT: f32 = 28.0;
const TIMELINE_ROUNDING: f32 = 4.0;
const MARKER_THICKNESS: f32 = 2.0;

/// Paint the whole video as a strip with the zoom window outlined on it, and
/// handle dragging that window's edges (resize) or body (pan).
pub(crate) fn overview_bar(ui: &mut egui::Ui, clip: &mut Clip, dur: f64) {
    let dur = dur.max(f64::EPSILON);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), OVERVIEW_HEIGHT),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    let axis = TimeAxis::new(rect, 0.0, dur);

    let start_x = axis.x_at(clip.view_start_secs);
    let end_x = axis.x_at(clip.view_end_secs);
    let grabbed_edge = |x: f32| {
        nearest_handle(x, [(Some(start_x), WindowDrag::Start), (Some(end_x), WindowDrag::End)])
    };

    let is_spanning_whole_video = clip.view_end_secs - clip.view_start_secs >= dur;
    if response.drag_started() {
        // A drag is only recognised once the pointer leaves the press point, so
        // the gesture is chosen from where the press landed rather than from a
        // position that has already drifted off the handle.
        let press_pos = ui
            .ctx()
            .input(|i| i.pointer.press_origin())
            .or_else(|| response.interact_pointer_pos());
        if let Some(pos) = press_pos {
            let is_inside_window = pos.x > start_x && pos.x < end_x;
            clip.window_drag = Some(match grabbed_edge(pos.x) {
                Some(edge) => edge,
                None if is_inside_window && !is_spanning_whole_video => WindowDrag::Pan {
                    grab_offset_secs: axis.secs_at(pos.x) - clip.view_start_secs,
                },
                None => WindowDrag::Draw { anchor_secs: axis.secs_at(pos.x) },
            });
            clip.begin_gesture();
        }
    }
    if let (Some(drag), Some(pos)) = (clip.window_drag, response.interact_pointer_pos()) {
        let t = axis.secs_at(pos.x);
        match drag {
            WindowDrag::Start => {
                let end = clip.view_end_secs;
                clip.set_view(t.min(end - MIN_RANGE_SECS), end, dur)
            }
            WindowDrag::End => {
                let start = clip.view_start_secs;
                clip.set_view(start, t.max(start + MIN_RANGE_SECS), dur)
            }
            WindowDrag::Pan { grab_offset_secs } => {
                let span = clip.view_end_secs - clip.view_start_secs;
                clip.set_view(t - grab_offset_secs, t - grab_offset_secs + span, dur);
            }
            WindowDrag::Draw { anchor_secs } => {
                clip.set_view(anchor_secs.min(t), anchor_secs.max(t), dur)
            }
        }
    }
    if response.drag_stopped() {
        clip.commit_gesture();
        clip.window_drag = None;
    }
    if let Some(pos) = response.hover_pos() {
        let is_inside_window = pos.x > start_x && pos.x < end_x;
        ui.ctx().set_cursor_icon(match grabbed_edge(pos.x) {
            Some(_) => egui::CursorIcon::ResizeHorizontal,
            None if is_inside_window && !is_spanning_whole_video => egui::CursorIcon::Grab,
            None => egui::CursorIcon::Crosshair,
        });
    }

    let start_x = axis.x_at(clip.view_start_secs);
    let end_x = axis.x_at(clip.view_end_secs);
    painter.rect_filled(rect, egui::Rounding::same(OVERVIEW_ROUNDING), visuals.extreme_bg_color);
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(axis.x_at(clip.in_secs), rect.top()),
            egui::pos2(axis.x_at(clip.out_secs), rect.bottom()),
        ),
        egui::Rounding::ZERO,
        visuals.selection.bg_fill,
    );
    let dim = egui::Color32::from_black_alpha(120);
    painter.rect_filled(
        egui::Rect::from_min_max(rect.left_top(), egui::pos2(start_x, rect.bottom())),
        egui::Rounding::ZERO,
        dim,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(egui::pos2(end_x, rect.top()), rect.right_bottom()),
        egui::Rounding::ZERO,
        dim,
    );
    painter.rect_stroke(
        egui::Rect::from_min_max(
            egui::pos2(start_x, rect.top()),
            egui::pos2(end_x, rect.bottom()),
        ),
        egui::Rounding::same(OVERVIEW_ROUNDING),
        egui::Stroke::new(MARKER_THICKNESS, visuals.strong_text_color()),
    );
    for edge_x in [start_x, end_x] {
        let center_x =
            edge_x.clamp(rect.left() + GRIP_WIDTH / 2.0, rect.right() - GRIP_WIDTH / 2.0);
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(center_x - GRIP_WIDTH / 2.0, rect.top()),
                egui::pos2(center_x + GRIP_WIDTH / 2.0, rect.bottom()),
            ),
            egui::Rounding::same(GRIP_WIDTH / 2.0),
            visuals.strong_text_color(),
        );
    }
    response.on_hover_text("Drag to set the zoom range. Scroll over the timeline to zoom.");
}

/// Paint the zoom window's span of the timeline: dimmed outside the clip,
/// highlighted in [in, out], with draggable in/out markers and a playhead.
/// Returns `(target, released)` while the timeline is clicked or dragged for a
/// seek; `released` is true on click/release.
pub(crate) fn timeline(
    ui: &mut egui::Ui,
    clip: &mut Clip,
    playback: &Playback,
    cur: f64,
    dur: f64,
) -> Option<(f64, bool)> {
    let dur = dur.max(f64::EPSILON);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), TIMELINE_HEIGHT),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();

    let scroll_y = if response.hovered() { ui.input(|i| i.smooth_scroll_delta.y) } else { 0.0 };
    if let (true, Some(pos)) = (scroll_y != 0.0, response.hover_pos()) {
        let span = (clip.view_end_secs - clip.view_start_secs).max(MIN_RANGE_SECS);
        let axis = TimeAxis::new(rect, clip.view_start_secs, clip.view_start_secs + span);
        let fraction = axis.fraction_at(pos.x);
        let focus_secs = clip.view_start_secs + fraction * span;
        let zoomed_span = span * (-scroll_y * SCROLL_ZOOM_RATE).exp() as f64;
        clip.begin_gesture();
        clip.set_view(
            focus_secs - fraction * zoomed_span,
            focus_secs + (1.0 - fraction) * zoomed_span,
            dur,
        );
        clip.scroll_gesture_commit_at = Some(ui.input(|i| i.time) + SCROLL_GESTURE_IDLE_SECS);
    }

    let view_start = clip.view_start_secs;
    let span = (clip.view_end_secs - clip.view_start_secs).max(MIN_RANGE_SECS);
    let axis = TimeAxis::new(rect, view_start, view_start + span);

    // Zoom is view-only, so an in/out point may sit outside the window; one that
    // does is neither painted nor grabbable.
    let is_in_visible = axis.contains(clip.in_secs);
    let is_out_visible = axis.contains(clip.out_secs);
    let in_x = axis.x_at(clip.in_secs);
    let out_x = axis.x_at(clip.out_secs);
    let grabbed_marker = |x: f32| {
        nearest_handle(
            x,
            [
                (is_in_visible.then_some(in_x), TimelineDrag::In),
                (is_out_visible.then_some(out_x), TimelineDrag::Out),
            ],
        )
    };
    if response.drag_started() {
        if let Some(pos) = response.interact_pointer_pos() {
            let grabbed = grabbed_marker(pos.x);
            clip.timeline_drag = Some(grabbed.unwrap_or(TimelineDrag::Playhead));
            if grabbed.is_some() {
                clip.begin_gesture();
            }
        }
    }
    if let Some(pos) = response.hover_pos() {
        if grabbed_marker(pos.x).is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
    }

    let mut result = None;
    let mut live_pos = None;
    let is_dragging_marker =
        matches!(clip.timeline_drag, Some(TimelineDrag::In) | Some(TimelineDrag::Out));
    if is_dragging_marker {
        if let (Some(drag), Some(pos)) = (clip.timeline_drag, response.interact_pointer_pos()) {
            let t = axis.secs_at(pos.x);
            match drag {
                TimelineDrag::In => clip.in_secs = clip.clamped_in_secs(t),
                TimelineDrag::Out => clip.out_secs = clip.clamped_out_secs(t),
                TimelineDrag::Playhead => {}
            }
        }
    } else if response.dragged() || response.clicked() || response.drag_stopped() {
        if let Some(pos) = response.interact_pointer_pos() {
            let t = axis.secs_at(pos.x);
            live_pos = Some(t);
            result = Some((t, response.drag_stopped() || response.clicked()));
        }
    }
    if response.drag_stopped() {
        clip.commit_gesture();
        clip.timeline_drag = None;
    }

    let in_x = axis.x_at(clip.in_secs);
    let out_x = axis.x_at(clip.out_secs);
    // Normalize so the highlight always spans between the two markers: a
    // reversed rect (min.x > max.x) is silently dropped by egui, which would
    // leave the clip region uncolored if in/out ever cross.
    let lo = in_x.min(out_x);
    let hi = in_x.max(out_x);
    let dim = egui::Color32::from_black_alpha(150);

    painter.rect_filled(rect, egui::Rounding::same(TIMELINE_ROUNDING), visuals.extreme_bg_color);
    painter.rect_filled(
        egui::Rect::from_min_max(egui::pos2(lo, rect.top()), egui::pos2(hi, rect.bottom())),
        egui::Rounding::ZERO,
        visuals.selection.bg_fill,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(rect.left_top(), egui::pos2(lo, rect.bottom())),
        egui::Rounding::ZERO,
        dim,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(egui::pos2(hi, rect.top()), rect.right_bottom()),
        egui::Rounding::ZERO,
        dim,
    );

    let marker = egui::Stroke::new(MARKER_THICKNESS, visuals.selection.stroke.color);
    if is_in_visible {
        painter.vline(in_x, rect.y_range(), marker);
    }
    if is_out_visible {
        painter.vline(out_x, rect.y_range(), marker);
    }

    // Playhead: follow the cursor while interacting, hold at a pending release
    // target until its frame lands, otherwise show the current frame.
    let playhead =
        live_pos.or_else(|| playback.awaiting_release.map(|(_, pos)| pos)).unwrap_or(cur);
    if axis.contains(playhead) {
        painter.vline(
            axis.x_at(playhead),
            rect.y_range(),
            egui::Stroke::new(MARKER_THICKNESS, visuals.strong_text_color()),
        );
    }
    result
}
