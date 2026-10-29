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
    if response.drag_started() && let Some(pos) = response.interact_pointer_pos() {
        let grabbed = grabbed_marker(pos.x);
        clip.timeline_drag = Some(grabbed.unwrap_or(TimelineDrag::Playhead));
        if grabbed.is_some() {
            clip.begin_gesture();
        }
    }
    if let Some(pos) = response.hover_pos() && grabbed_marker(pos.x).is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
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
    } else if (response.dragged() || response.clicked() || response.drag_stopped())
        && let Some(pos) = response.interact_pointer_pos() {
        let t = axis.secs_at(pos.x);
        live_pos = Some(t);
        result = Some((t, response.drag_stopped() || response.clicked()));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::clip::ClipRange;

    const DURATION_SECS: f64 = 100.0;
    const SCREEN_WIDTH: f32 = 1000.0;
    const SCREEN_HEIGHT: f32 = 200.0;
    const FRAME_SECS: f64 = 1.0 / 60.0;
    const DRAG_STEP_PIXELS: f32 = 8.0;
    const SCROLL_SETTLE_FRAMES: usize = 60;
    const SECS_PER_PIXEL: f64 = DURATION_SECS / SCREEN_WIDTH as f64;

    struct TimelineHarness {
        context: egui::Context,
        clip: Clip,
        playback: Playback,
        time_secs: f64,
        seeks: Vec<(f64, bool)>,
    }

    impl TimelineHarness {
        fn new(in_secs: f64, out_secs: f64, view_start_secs: f64, view_end_secs: f64) -> Self {
            let mut clip = Clip::default();
            clip.restore(ClipRange { in_secs, out_secs, view_start_secs, view_end_secs });
            let mut harness = Self {
                context: egui::Context::default(),
                clip,
                playback: Playback::default(),
                time_secs: 0.0,
                seeks: Vec::new(),
            };
            harness.run_frame(Vec::new());
            harness
        }

        fn run_frame(&mut self, events: Vec<egui::Event>) {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(SCREEN_WIDTH, SCREEN_HEIGHT),
                )),
                time: Some(self.time_secs),
                events,
                ..Default::default()
            };
            let Self { context, clip, playback, seeks, .. } = self;
            let _ = context.run(input, |context| {
                egui::CentralPanel::default().frame(egui::Frame::none()).show(context, |ui| {
                    if let Some(seek) = timeline(ui, clip, playback, 0.0, DURATION_SECS) {
                        seeks.push(seek);
                    }
                });
            });
            self.time_secs += FRAME_SECS;
        }

        fn x_at(&self, secs: f64) -> f32 {
            let rect = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(SCREEN_WIDTH, TIMELINE_HEIGHT),
            );
            TimeAxis::new(rect, self.clip.view_start_secs, self.clip.view_end_secs).x_at(secs)
        }

        fn drag_through(&mut self, waypoints: &[f32]) {
            let (&first_x, rest) = waypoints.split_first().expect("a drag needs a start point");
            self.run_frame(vec![egui::Event::PointerMoved(pointer_at(first_x))]);
            self.run_frame(vec![primary_button(first_x, true)]);
            let mut x = first_x;
            for &target_x in rest {
                while (target_x - x).abs() > DRAG_STEP_PIXELS {
                    x += (target_x - x).signum() * DRAG_STEP_PIXELS;
                    self.run_frame(vec![egui::Event::PointerMoved(pointer_at(x))]);
                }
                x = target_x;
                self.run_frame(vec![egui::Event::PointerMoved(pointer_at(x))]);
            }
            self.run_frame(vec![primary_button(x, false)]);
            self.run_frame(Vec::new());
        }

        fn click(&mut self, x: f32) {
            self.run_frame(vec![egui::Event::PointerMoved(pointer_at(x))]);
            self.run_frame(vec![primary_button(x, true)]);
            self.run_frame(vec![primary_button(x, false)]);
            self.run_frame(Vec::new());
        }

        fn scroll(&mut self, x: f32, delta_y: f32) {
            self.run_frame(vec![
                egui::Event::PointerMoved(pointer_at(x)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, delta_y),
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            for _ in 0..SCROLL_SETTLE_FRAMES {
                self.run_frame(Vec::new());
            }
        }
    }

    fn pointer_at(x: f32) -> egui::Pos2 {
        egui::pos2(x, TIMELINE_HEIGHT / 2.0)
    }

    fn primary_button(x: f32, is_pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: pointer_at(x),
            button: egui::PointerButton::Primary,
            pressed: is_pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn assert_near(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "{actual} is not within {tolerance} of {expected}"
        );
    }

    #[test]
    fn dragging_the_in_marker_moves_it_as_one_undo_step() {
        let mut harness = TimelineHarness::new(25.0, 75.0, 0.0, DURATION_SECS);
        harness.drag_through(&[harness.x_at(25.0), harness.x_at(50.0)]);
        assert_near(harness.clip.in_secs, 50.0, SECS_PER_PIXEL);
        assert_eq!(harness.clip.out_secs, 75.0);
        assert!(harness.seeks.is_empty(), "a marker drag must not seek: {:?}", harness.seeks);
        harness.clip.undo();
        assert_eq!(harness.clip.in_secs, 25.0, "one undo must restore the pre-drag point");
    }

    #[test]
    fn the_in_marker_stops_short_of_the_out_marker() {
        let mut harness = TimelineHarness::new(25.0, 75.0, 0.0, DURATION_SECS);
        harness.drag_through(&[harness.x_at(25.0), harness.x_at(95.0)]);
        assert_eq!(harness.clip.in_secs, 75.0 - MIN_RANGE_SECS);
    }

    #[test]
    fn the_out_marker_stops_short_of_the_in_marker() {
        let mut harness = TimelineHarness::new(25.0, 75.0, 0.0, DURATION_SECS);
        harness.drag_through(&[harness.x_at(75.0), harness.x_at(5.0)]);
        assert_eq!(harness.clip.out_secs, 25.0 + MIN_RANGE_SECS);
    }

    #[test]
    fn a_drag_back_to_its_start_records_nothing_and_keeps_redo() {
        let mut harness = TimelineHarness::new(25.0, 75.0, 0.0, DURATION_SECS);
        harness.clip.set_in_secs(30.0);
        harness.clip.undo();
        let start_x = harness.x_at(25.0);
        harness.drag_through(&[start_x, harness.x_at(50.0), start_x]);
        assert_eq!(harness.clip.in_secs, 25.0);
        harness.clip.redo();
        assert_eq!(harness.clip.in_secs, 30.0, "the round trip must not clear the redo step");
    }

    #[test]
    fn clicking_away_from_the_markers_seeks_there() {
        let mut harness = TimelineHarness::new(25.0, 75.0, 0.0, DURATION_SECS);
        harness.click(harness.x_at(50.0));
        assert_eq!(harness.seeks.len(), 1, "one click, one seek: {:?}", harness.seeks);
        let (target_secs, is_released) = harness.seeks[0];
        assert_near(target_secs, 50.0, SECS_PER_PIXEL);
        assert!(is_released);
        assert_eq!((harness.clip.in_secs, harness.clip.out_secs), (25.0, 75.0));
    }

    #[test]
    fn scrubbing_reports_live_positions_then_the_release() {
        let mut harness = TimelineHarness::new(25.0, 75.0, 0.0, DURATION_SECS);
        harness.drag_through(&[harness.x_at(40.0), harness.x_at(60.0)]);
        let (last, live) = harness.seeks.split_last().expect("a scrub must seek");
        assert!(!live.is_empty(), "a scrub must seek while dragging, not only on release");
        assert!(live.iter().all(|&(_, is_released)| !is_released), "{live:?}");
        assert_near(last.0, 60.0, SECS_PER_PIXEL);
        assert!(last.1, "the final seek must be marked as released");
        assert_eq!((harness.clip.in_secs, harness.clip.out_secs), (25.0, 75.0));
    }

    #[test]
    fn a_marker_outside_the_zoom_window_cannot_be_grabbed() {
        let mut harness = TimelineHarness::new(20.0, 80.0, 50.0, DURATION_SECS);
        let left_edge_x = harness.x_at(harness.clip.view_start_secs);
        harness.drag_through(&[left_edge_x + 1.0, harness.x_at(65.0)]);
        assert_eq!(harness.clip.in_secs, 20.0, "the hidden in marker moved");
        assert!(!harness.seeks.is_empty(), "the drag should have scrubbed the playhead instead");
    }

    #[test]
    fn scrolling_in_zooms_about_the_cursor() {
        let mut harness = TimelineHarness::new(25.0, 75.0, 0.0, DURATION_SECS);
        let cursor_x = harness.x_at(25.0);
        harness.scroll(cursor_x, 200.0);
        let span_secs = harness.clip.view_end_secs - harness.clip.view_start_secs;
        assert!(span_secs < DURATION_SECS, "scrolling in did not zoom: span {span_secs}");
        let rect = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(SCREEN_WIDTH, TIMELINE_HEIGHT),
        );
        let axis = TimeAxis::new(rect, harness.clip.view_start_secs, harness.clip.view_end_secs);
        assert_near(axis.secs_at(cursor_x), 25.0, SECS_PER_PIXEL);
        assert_eq!((harness.clip.in_secs, harness.clip.out_secs), (25.0, 75.0));
    }

    #[test]
    fn scrolling_out_stops_at_the_whole_video() {
        let mut harness = TimelineHarness::new(25.0, 75.0, 40.0, 60.0);
        harness.scroll(harness.x_at(50.0), -5000.0);
        assert_eq!(harness.clip.view_start_secs, 0.0);
        assert_eq!(harness.clip.view_end_secs, DURATION_SECS);
    }
}
