/// Cap on retained clip-range edits per undo/redo stack. Every zoom-window
/// placement is an undoable step, so browsing draws on it as well as editing.
const CLIP_HISTORY_LIMIT: usize = 1024;

/// Shortest span the zoom window or the clip may be narrowed to.
pub(crate) const MIN_RANGE_SECS: f64 = 0.05;

/// Narrowing the zoom window trims the clip, so both restore together.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct ClipRange {
    pub(crate) in_secs: f64,
    pub(crate) out_secs: f64,
    pub(crate) view_start_secs: f64,
    pub(crate) view_end_secs: f64,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum WindowDrag {
    Start,
    End,
    Pan { grab_offset_secs: f64 },
    Draw { anchor_secs: f64 },
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum TimelineDrag {
    In,
    Out,
    Playhead,
}

/// The clip in/out points, the timeline's zoom window, and the undo history over
/// both. Zoom is view-only: it never moves the in/out points, so they may sit
/// outside the window.
#[derive(Default)]
pub(crate) struct Clip {
    pub(crate) in_secs: f64,
    pub(crate) out_secs: f64,
    pub(crate) view_start_secs: f64,
    pub(crate) view_end_secs: f64,
    /// Pre-gesture range stashed by `begin_gesture`, so a continuous drag
    /// records one undo step instead of one per frame.
    pub(crate) pending_undo_range: Option<ClipRange>,
    /// Scroll-zoom has no release event, so its gesture closes on this deadline.
    pub(crate) scroll_gesture_commit_at: Option<f64>,
    pub(crate) window_drag: Option<WindowDrag>,
    pub(crate) timeline_drag: Option<TimelineDrag>,
    undo_stack: Vec<ClipRange>,
    redo_stack: Vec<ClipRange>,
}

impl Clip {
    pub(crate) fn range(&self) -> ClipRange {
        ClipRange {
            in_secs: self.in_secs,
            out_secs: self.out_secs,
            view_start_secs: self.view_start_secs,
            view_end_secs: self.view_end_secs,
        }
    }

    pub(crate) fn restore(&mut self, range: ClipRange) {
        self.in_secs = range.in_secs;
        self.out_secs = range.out_secs;
        self.view_start_secs = range.view_start_secs;
        self.view_end_secs = range.view_end_secs;
    }

    fn push_undo(&mut self, range: ClipRange) {
        self.undo_stack.push(range);
        if self.undo_stack.len() > CLIP_HISTORY_LIMIT {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub(crate) fn record_edit(&mut self) {
        let range = self.range();
        self.push_undo(range);
    }

    pub(crate) fn begin_gesture(&mut self) {
        if self.pending_undo_range.is_none() {
            self.pending_undo_range = Some(self.range());
        }
    }

    pub(crate) fn commit_gesture(&mut self) {
        if let Some(range) = self.pending_undo_range.take() {
            if range != self.range() {
                self.push_undo(range);
            }
        }
    }

    /// Recover the state a missing `drag_stopped` would strand. Runs at the end
    /// of a frame, after the bars have applied the drag's final position.
    pub(crate) fn commit_abandoned_gesture(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        self.window_drag = None;
        self.timeline_drag = None;
        if self.scroll_gesture_commit_at.is_none() {
            self.commit_gesture();
        }
    }

    pub(crate) fn clamped_in_secs(&self, secs: f64) -> f64 {
        secs.min(self.out_secs - MIN_RANGE_SECS).max(0.0)
    }

    pub(crate) fn clamped_out_secs(&self, secs: f64) -> f64 {
        secs.max(self.in_secs + MIN_RANGE_SECS)
    }

    pub(crate) fn set_view(&mut self, start_secs: f64, end_secs: f64, dur: f64) {
        let span = (end_secs - start_secs).clamp(MIN_RANGE_SECS.min(dur), dur);
        self.view_start_secs = start_secs.clamp(0.0, (dur - span).max(0.0));
        self.view_end_secs = self.view_start_secs + span;
    }

    pub(crate) fn set_in_secs(&mut self, secs: f64) {
        let secs = self.clamped_in_secs(secs);
        if self.in_secs != secs {
            self.record_edit();
            self.in_secs = secs;
        }
    }

    pub(crate) fn set_out_secs(&mut self, secs: f64) {
        let secs = self.clamped_out_secs(secs);
        if self.out_secs != secs {
            self.record_edit();
            self.out_secs = secs;
        }
    }

    pub(crate) fn undo(&mut self) {
        if let Some(range) = self.undo_stack.pop() {
            let current = self.range();
            self.redo_stack.push(current);
            self.restore(range);
        }
    }

    pub(crate) fn redo(&mut self) {
        if let Some(range) = self.redo_stack.pop() {
            let current = self.range();
            self.undo_stack.push(current);
            self.restore(range);
        }
    }

    pub(crate) fn clear_history(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.pending_undo_range = None;
        self.scroll_gesture_commit_at = None;
    }
}
