//! Export a trimmed clip or an audio-only file using ffmpeg-the-third.
//!
//! A clip's video is re-encoded so the cut begins exactly on the chosen `in`
//! frame, and its audio is re-encoded and trimmed to the exact sample (a stream
//! copy can't split a packet, so an exact `in`/`out` requires re-encoding).
//!
//! [`plan`] decides per stream whether to copy or re-encode; [`transcode`] is
//! the writer that carries that out, and [`copy`] holds the paths that move
//! packets without decoding them.

mod audio;
mod copy;
mod plan;
mod transcode;
mod video;

use anyhow::{anyhow, bail, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use ffmpeg_the_third as ffmpeg;
use ffmpeg::media::Type;

use copy::{export_audio_copy, remux_copy};
use plan::{container_kind, StreamPlan};
use transcode::transcode;

/// Write the output header, applying `+faststart` for MP4/MOV so the `moov`
/// atom lands at the front of the file and browsers/chat clients can stream
/// without downloading the whole file first.
fn write_header(
    octx: &mut ffmpeg::format::context::Output,
    path: &std::path::Path,
) -> Result<()> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if matches!(ext.as_str(), "mp4" | "mov" | "m4a") {
        let mut opts = ffmpeg::Dictionary::new();
        opts.set("movflags", "+faststart");
        octx.write_header_with(opts)?;
    } else {
        octx.write_header()?;
    }
    Ok(())
}

/// Output container for a video export (Full or Clip). Re-encodes target each
/// container's native codecs: H.264/AAC for MP4·MOV·MKV, VP9/Opus for WebM.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VideoFormat {
    Mp4,
    Mkv,
    Mov,
    Webm,
}

impl VideoFormat {
    pub fn extension(self) -> &'static str {
        match self {
            VideoFormat::Mp4 => "mp4",
            VideoFormat::Mkv => "mkv",
            VideoFormat::Mov => "mov",
            VideoFormat::Webm => "webm",
        }
    }
}

/// Target codec/container for an audio-only export.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    /// Re-encode to MP3 — the most universally playable audio format.
    Mp3,
    /// Re-encode to AAC in an `.m4a` container.
    Aac,
    /// Stream-copy the source audio losslessly into a fitting container.
    Original,
}

pub enum Mode {
    /// Every stream copied for the whole file, no re-encode.
    Full,
    /// Re-encoded video plus copied audio.
    Clip,
    /// Audio stream only, in the chosen format.
    AudioOnly(AudioFormat),
}

pub struct ExportSpec {
    pub input: String,
    pub output: String,
    pub start_secs: f64,
    pub end_secs: f64,
    pub mode: Mode,
    /// Downscale video to at most this many lines tall (preserving aspect), when
    /// `Some` and the source is taller. `None` keeps the source resolution. Never
    /// upscales. Ignored for audio-only exports.
    pub scale_height: Option<u32>,
    /// Restrict an MP4/MOV export to codecs and pixel formats that play on every
    /// target device (phones and TVs, not just computers): only H.264 8-bit
    /// 4:2:0 video and AAC/MP3 audio are stream-copied, anything else is
    /// re-encoded. When `false`, any codec the container can hold is copied as-is
    /// (e.g. HEVC/AV1, 10-bit, HDR), trading reach for fidelity. Ignored for the
    /// MKV/WebM containers and for audio-only exports.
    pub compatibility_mode: bool,
}

pub fn export(spec: &ExportSpec) -> Result<()> {
    export_cancellable(spec, &AtomicBool::new(false))
}

/// Like `export`, but aborts (returning an error) once `cancel` is set. The flag
/// is checked between packets, so a cancel takes effect within one packet.
pub fn export_cancellable(spec: &ExportSpec, cancel: &AtomicBool) -> Result<()> {
    ffmpeg::init()?;
    match spec.mode {
        Mode::Full => export_full(spec, cancel),
        Mode::AudioOnly(format) => {
            check_window(spec)?;
            export_audio_only(spec, format, cancel)
        }
        Mode::Clip => {
            check_window(spec)?;
            export_clip(spec, cancel)
        }
    }
}

/// A zero-length range writes a container holding no samples, which players
/// reject as corrupt.
fn check_window(spec: &ExportSpec) -> Result<()> {
    if spec.end_secs <= spec.start_secs {
        bail!("clip range is empty — set an end point after the start point");
    }
    Ok(())
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("export cancelled");
    }
    Ok(())
}

/// File extension for an audio-only export of `input` in `format`. For a
/// lossless copy this is the container that fits the source codec.
pub fn audio_extension(input: &str, format: AudioFormat) -> Result<&'static str> {
    match format {
        AudioFormat::Mp3 => Ok("mp3"),
        AudioFormat::Aac => Ok("m4a"),
        AudioFormat::Original => {
            ffmpeg::init()?;
            let ictx = ffmpeg::format::input(&input)?;
            let stream = ictx
                .streams()
                .best(Type::Audio)
                .ok_or_else(|| anyhow!("no audio stream found"))?;
            Ok(copy_container_ext(stream.parameters().id()))
        }
    }
}

/// Container extension that can losslessly hold `codec` via stream copy.
/// Matroska audio (`.mka`) is the catch-all for anything without a snug fit.
fn copy_container_ext(codec: ffmpeg::codec::Id) -> &'static str {
    use ffmpeg::codec::Id;
    match codec {
        Id::AAC => "m4a",
        Id::OPUS => "opus",
        Id::VORBIS => "ogg",
        Id::MP3 => "mp3",
        Id::FLAC => "flac",
        Id::AC3 => "ac3",
        _ => "mka",
    }
}

/// Save the whole file in the chosen container, stream-copying every stream that
/// the container can hold and re-encoding only those it cannot.
fn export_full(spec: &ExportSpec, cancel: &AtomicBool) -> Result<()> {
    let mut ictx = ffmpeg::format::input(&spec.input)?;
    let plan = StreamPlan::video_export(&ictx, spec, container_kind(&spec.output), false)?;
    if plan.is_copy_only() {
        remux_copy(&mut ictx, spec, cancel)
    } else {
        transcode(&mut ictx, spec, plan, false, cancel)
    }
}

/// Save a frame-accurate clip in the chosen container. Video is always
/// re-encoded for an exact `in` point; audio is re-encoded for an exact cut.
fn export_clip(spec: &ExportSpec, cancel: &AtomicBool) -> Result<()> {
    let mut ictx = ffmpeg::format::input(&spec.input)?;
    let plan = StreamPlan::video_export(&ictx, spec, container_kind(&spec.output), true)?;
    transcode(&mut ictx, spec, plan, true, cancel)
}

fn export_audio_only(spec: &ExportSpec, format: AudioFormat, cancel: &AtomicBool) -> Result<()> {
    let codec_id = match format {
        AudioFormat::Original => return export_audio_copy(spec, cancel),
        AudioFormat::Mp3 => ffmpeg::codec::Id::MP3,
        AudioFormat::Aac => ffmpeg::codec::Id::AAC,
    };
    let mut ictx = ffmpeg::format::input(&spec.input)?;
    let plan = StreamPlan::audio_export(&ictx, container_kind(&spec.output), codec_id)?;
    transcode(&mut ictx, spec, plan, true, cancel)
}

fn seek_to(ictx: &mut ffmpeg::format::context::Input, secs: f64) -> Result<()> {
    let ts = (secs * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
    ictx.seek(ts, ..ts)?;
    Ok(())
}
