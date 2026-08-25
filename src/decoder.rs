//! Frame-accurate video decoding via ffmpeg-the-third.
//!
//! Stepping and scrubbing are built on one primitive: seek to the keyframe at
//! or before a target timestamp, then decode forward to the exact frame.
//! Timestamps are kept in the stream's integer time base to avoid rounding
//! drift.

use anyhow::{anyhow, Result};
use ffmpeg_the_third as ffmpeg;
use ffmpeg::format::Pixel;
use ffmpeg::media::Type;
use ffmpeg::software::scaling::{context::Context as Scaler, flag::Flags};
use ffmpeg::util::frame::video::Video;

/// Frame rate assumed when a stream reports an invalid average frame rate.
const DEFAULT_FPS: f64 = 25.0;
/// Bytes per pixel in the RGBA buffers handed to egui.
const RGBA_BYTES: usize = 4;

pub struct Decoder {
    ictx: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Video,
    scaler: Scaler,
    stream_index: usize,
    codec_id: ffmpeg::codec::Id,
    pub width: u32,
    pub height: u32,

    /// seconds per time-base unit (for converting pts <-> seconds for display)
    time_base: f64,
    /// one frame's duration, in time-base units
    frame_dur_ts: i64,
    /// container duration, in time-base units
    duration_ts: i64,
    /// pts of the most recently produced frame, in time-base units
    current_pts: i64,
}

/// Playable length of a file that carries audio but no video, so an audio-only
/// source still gets a timeline to trim against. Errors when the file has no
/// audio stream either.
pub fn audio_duration_secs(path: &str) -> Result<f64> {
    ffmpeg::init()?;
    let ictx = ffmpeg::format::input(&path)?;
    let stream = ictx
        .streams()
        .best(Type::Audio)
        .ok_or_else(|| anyhow!("no audio stream found"))?;

    let time_base = f64::from(stream.time_base());
    // WebM/Matroska leaves the per-stream duration unset; fall back to the
    // container duration (in AV_TIME_BASE units) so the timeline isn't zero.
    let secs = if stream.duration() > 0 {
        stream.duration() as f64 * time_base
    } else {
        ictx.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)
    };
    Ok(secs.max(0.0))
}

impl Decoder {
    pub fn open(path: &str) -> Result<Self> {
        ffmpeg::init()?;
        let ictx = ffmpeg::format::input(&path)?;

        let stream = ictx
            .streams()
            .best(Type::Video)
            .ok_or_else(|| anyhow!("no video stream found"))?;
        let stream_index = stream.index();

        let time_base = f64::from(stream.time_base());
        // WebM/Matroska leaves the per-stream duration unset; fall back to the
        // container duration (in AV_TIME_BASE units) so the timeline isn't zero.
        let duration_ts = if stream.duration() > 0 {
            stream.duration()
        } else {
            let container_secs = ictx.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE);
            ((container_secs / time_base) as i64).max(0)
        };

        let fps = f64::from(stream.avg_frame_rate());
        let fps = if fps > 0.0 { fps } else { DEFAULT_FPS };
        // one frame in time-base units, at least 1 so stepping always moves
        let frame_dur_ts = ((1.0 / fps) / time_base).round().max(1.0) as i64;

        let params = stream.parameters();
        let codec_id = params.id();
        let ctx = ffmpeg::codec::context::Context::from_parameters(params)?;
        let decoder = ctx.decoder().video()?;
        let (width, height) = (decoder.width(), decoder.height());

        let scaler = Scaler::get(
            decoder.format(),
            width,
            height,
            Pixel::RGBA,
            width,
            height,
            Flags::BILINEAR,
        )?;

        Ok(Self {
            ictx,
            decoder,
            scaler,
            stream_index,
            codec_id,
            width,
            height,
            time_base,
            frame_dur_ts,
            duration_ts,
            current_pts: 0,
        })
    }

    pub fn duration_secs(&self) -> f64 {
        self.duration_ts as f64 * self.time_base
    }

    pub fn current_secs(&self) -> f64 {
        self.current_pts as f64 * self.time_base
    }

    /// The video stream's codec name (e.g. `AV1`, `H264`), for error messages
    /// when no decoder in this build can produce frames from it.
    pub fn codec_name(&self) -> String {
        format!("{:?}", self.codec_id)
    }

    pub fn fps(&self) -> f64 {
        let interval = self.frame_dur_ts as f64 * self.time_base;
        if interval > 0.0 {
            1.0 / interval
        } else {
            DEFAULT_FPS
        }
    }

    /// Decode the next frame in presentation order. None at end of stream.
    pub fn step_forward(&mut self) -> Option<egui::ColorImage> {
        let mut frame = Video::empty();
        if !self.receive_next(&mut frame) {
            return None;
        }
        self.current_pts = frame.pts().unwrap_or(self.current_pts);
        self.to_image(&mut frame)
    }

    /// Step `n` frames from the current position: forward (`n > 0`) decodes
    /// ahead frame by frame; backward (`n < 0`) jumps back `|n|` frames in a
    /// single seek, so rapid or held backward stepping stays responsive instead
    /// of doing one seek per frame.
    pub fn step_by(&mut self, n: i64) -> Option<egui::ColorImage> {
        use std::cmp::Ordering;
        match n.cmp(&0) {
            Ordering::Greater => {
                let mut image = None;
                for _ in 0..n {
                    match self.step_forward() {
                        Some(img) => image = Some(img),
                        None => break,
                    }
                }
                image
            }
            Ordering::Less => self.step_backward_by((-n) as usize),
            Ordering::Equal => None,
        }
    }

    /// Land exactly `steps_back` frames before the current position. Actual
    /// per-frame pts deltas round to ±1 time-base unit around the nominal
    /// `frame_dur_ts` (VFR sources drift further), so subtracting
    /// `steps_back * frame_dur_ts` from `current_pts` can undershoot the true
    /// target and land back on the current frame — which then never advances,
    /// since every further step recomputes the same target from the same
    /// unchanged `current_pts`. Counting real decoded frames instead of pts
    /// arithmetic is exact regardless of drift.
    fn step_backward_by(&mut self, steps_back: usize) -> Option<egui::ColorImage> {
        let target_pts = self.pts_n_frames_before_current(steps_back);
        self.seek_exact(target_pts)
    }

    /// Find the pts of the frame `steps_back` presentation-order positions
    /// before `current_pts`. A single keyframe interval may hold fewer than
    /// `steps_back` frames — held or rapid-repeat backward stepping (see
    /// `decoder_thread`'s request coalescing) can ask for far more frames back
    /// than the current GOP contains — so this retries from the keyframe
    /// before that one, and the one before that, until it has collected
    /// enough or reached the start of the file.
    fn pts_n_frames_before_current(&mut self, steps_back: usize) -> i64 {
        let mut keyframe_search_pts = self.current_pts;
        // Detects "no progress" across retries, e.g. current_pts already IS a
        // keyframe: the first pass then finds it with zero frames before it,
        // and comparing against the search target (which equals current_pts
        // on that pass) would wrongly look like "no earlier keyframe exists".
        let mut prev_landed_keyframe_pts = i64::MAX;
        loop {
            self.seek_container(keyframe_search_pts);

            let mut history: std::collections::VecDeque<i64> =
                std::collections::VecDeque::with_capacity(steps_back + 1);
            let mut landed_keyframe_pts = None;
            let mut frame = Video::empty();
            let mut last_pts = 0;
            while self.receive_next(&mut frame) {
                let pts = frame.pts().unwrap_or(last_pts);
                last_pts = pts;
                landed_keyframe_pts.get_or_insert(pts);
                if pts >= self.current_pts {
                    break;
                }
                if history.len() == steps_back {
                    history.pop_front();
                }
                history.push_back(pts);
            }

            if history.len() >= steps_back {
                return history[0];
            }
            let landed = landed_keyframe_pts.unwrap_or(0);
            if landed <= 0 || landed >= prev_landed_keyframe_pts {
                // Already at the earliest keyframe: this is as far back as it goes.
                return history.front().copied().unwrap_or(0);
            }
            prev_landed_keyframe_pts = landed;
            keyframe_search_pts = landed - 1;
        }
    }

    /// Frame-accurate scrub to `secs`.
    pub fn seek_secs(&mut self, secs: f64) -> Option<egui::ColorImage> {
        let target = (secs / self.time_base).round() as i64;
        self.seek_exact(target.clamp(0, self.duration_ts.max(0)))
    }

    /// Seek the container to the keyframe at or before `target_pts` and flush
    /// the decoder, without decoding anything yet.
    fn seek_container(&mut self, target_pts: i64) {
        // The container seek works in AV_TIME_BASE units; `..seek_ts` keeps the
        // landing keyframe at or before the target.
        let seek_ts =
            (target_pts as f64 * self.time_base * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
        let _ = self.ictx.seek(seek_ts, ..seek_ts);
        self.decoder.flush();
    }

    fn seek_exact(&mut self, target_pts: i64) -> Option<egui::ColorImage> {
        self.seek_container(target_pts);

        let mut frame = Video::empty();
        let mut last_pts = 0;
        while self.receive_next(&mut frame) {
            let pts = frame.pts().unwrap_or(last_pts);
            last_pts = pts;
            if pts >= target_pts {
                self.current_pts = pts;
                return self.to_image(&mut frame);
            }
        }
        None
    }

    /// Returns false at end of stream.
    fn receive_next(&mut self, frame: &mut Video) -> bool {
        loop {
            match self.decoder.receive_frame(frame) {
                Ok(()) => return true,
                Err(ffmpeg::Error::Eof) => return false,
                Err(_) => {} // EAGAIN: needs more input, fall through
            }

            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut self.ictx) {
                Ok(()) => {
                    if packet.stream() == self.stream_index {
                        let _ = self.decoder.send_packet(&packet);
                    }
                }
                Err(_) => {
                    // No more packets: signal EOF and drain remaining frames.
                    let _ = self.decoder.send_eof();
                }
            }
        }
    }

    fn to_image(&mut self, frame: &mut Video) -> Option<egui::ColorImage> {
        let mut rgba = Video::empty();
        if let Err(e) = self.scaler.run(frame, &mut rgba) {
            // Don't panic the worker thread: a dead worker silently stalls the UI
            // (no frames, no error). Drop this frame instead.
            eprintln!("scaler failed: {e}");
            return None;
        }

        let w = self.width as usize;
        let h = self.height as usize;
        let stride = rgba.stride(0);
        let data = rgba.data(0);

        let mut pixels = Vec::with_capacity(w * h);
        for y in 0..h {
            let row = &data[y * stride..y * stride + w * RGBA_BYTES];
            for x in 0..w {
                let i = x * RGBA_BYTES;
                pixels.push(egui::Color32::from_rgba_unmultiplied(
                    row[i],
                    row[i + 1],
                    row[i + 2],
                    row[i + 3],
                ));
            }
        }
        Some(egui::ColorImage {
            size: [w, h],
            pixels,
        })
    }
}
