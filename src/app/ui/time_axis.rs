/// How near the pointer must come to a marker or window edge to grab it.
pub(crate) const HANDLE_GRAB_RADIUS: f32 = 10.0;

/// Maps a span of seconds onto a rect's horizontal extent. Both timeline bars
/// derive their pixel↔seconds arithmetic from one, so the guard against a
/// zero-width rect lives in a single place: a collapsed panel makes `0.0 / 0.0`
/// a NaN, and a NaN reaching the view bounds is not cleared by any later clamp.
#[derive(Clone, Copy)]
pub(crate) struct TimeAxis {
    rect: egui::Rect,
    start_secs: f64,
    span_secs: f64,
}

impl TimeAxis {
    pub(crate) fn new(rect: egui::Rect, start_secs: f64, end_secs: f64) -> Self {
        Self { rect, start_secs, span_secs: (end_secs - start_secs).max(f64::EPSILON) }
    }

    fn width(&self) -> f32 {
        self.rect.width().max(f32::EPSILON)
    }

    pub(crate) fn x_at(&self, secs: f64) -> f32 {
        let fraction = ((secs - self.start_secs) / self.span_secs).clamp(0.0, 1.0) as f32;
        self.rect.left() + fraction * self.width()
    }

    /// Where `x` falls across the rect, as `0.0..=1.0`.
    pub(crate) fn fraction_at(&self, x: f32) -> f64 {
        ((x - self.rect.left()) / self.width()).clamp(0.0, 1.0) as f64
    }

    pub(crate) fn secs_at(&self, x: f32) -> f64 {
        self.start_secs + self.fraction_at(x) * self.span_secs
    }

    pub(crate) fn contains(&self, secs: f64) -> bool {
        (self.start_secs..=self.start_secs + self.span_secs).contains(&secs)
    }
}

/// The nearer of two handles within `HANDLE_GRAB_RADIUS` of `x`, or `None` when
/// neither is in reach. A handle given as `None` is absent — scrolled outside the
/// visible span — and can never be grabbed. Ties go to the first.
pub(crate) fn nearest_handle<T: Copy>(x: f32, handles: [(Option<f32>, T); 2]) -> Option<T> {
    let [(first_x, first), (second_x, second)] = handles;
    let distance = |handle: Option<f32>| handle.map_or(f32::INFINITY, |hx| (x - hx).abs());
    let first_distance = distance(first_x);
    let second_distance = distance(second_x);
    if first_distance > HANDLE_GRAB_RADIUS && second_distance > HANDLE_GRAB_RADIUS {
        None
    } else if first_distance <= second_distance {
        Some(first)
    } else {
        Some(second)
    }
}
