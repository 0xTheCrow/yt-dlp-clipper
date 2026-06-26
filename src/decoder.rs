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
            (container_secs / time_base) as i64
        };

        // frames-per-second, from the stream's average frame rate
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

    // --- public stepping API -------------------------------------------------

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
            Ordering::Less => {
                let target = (self.current_pts - (-n) * self.frame_dur_ts).max(0);
                self.seek_exact(target)
            }
            Ordering::Equal => None,
        }
    }

    /// Frame-accurate scrub to `secs`.
    pub fn seek_secs(&mut self, secs: f64) -> Option<egui::ColorImage> {
        let target = (secs / self.time_base).round() as i64;
        self.seek_exact(target.clamp(0, self.duration_ts.max(0)))
    }

    // --- internals -----------------------------------------------------------

    /// Seek to the keyframe at/before `target_pts`, then decode forward until
    /// the first frame whose pts >= target_pts.
    fn seek_exact(&mut self, target_pts: i64) -> Option<egui::ColorImage> {
        // The container seek works in AV_TIME_BASE units; `..seek_ts` keeps the
        // landing keyframe at or before the target.
        let seek_ts =
            (target_pts as f64 * self.time_base * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
        let _ = self.ictx.seek(seek_ts, ..seek_ts);
        self.decoder.flush();

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

    /// Pull packets and feed the decoder until one frame is produced.
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
