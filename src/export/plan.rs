//! Which source streams an export carries, and whether each is stream-copied or
//! re-encoded. The decision is made once, up front, so the choice between a pure
//! remux and a transcode rests on the same information the transcode acts on.

use anyhow::{anyhow, Result};
use ffmpeg_the_third as ffmpeg;
use ffmpeg::media::Type;

use super::ExportSpec;

/// Frame rate assumed when a stream reports an invalid average frame rate.
const DEFAULT_FPS: i32 = 25;

/// Container family of the output path, deciding which codecs can be copied and
/// which codecs a re-encode must target.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Container {
    /// Matroska (`.mkv`) — accepts essentially any codec.
    Mkv,
    /// MP4/MOV family — a fixed set of codecs only.
    Mp4,
    /// WebM — VP8/VP9/AV1 video, Opus/Vorbis audio only.
    Webm,
}

pub(super) fn container_kind(output: &str) -> Container {
    match output.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "mkv" => Container::Mkv,
        "webm" => Container::Webm,
        _ => Container::Mp4,
    }
}

/// Whether `container` can hold `codec` as a video stream via stream copy. In
/// `compatibility_mode` the MP4/MOV family is narrowed to H.264 — the one video
/// codec every target device decodes — so HEVC/AV1/MPEG-4 are re-encoded rather
/// than copied into a file phones can't play.
pub(super) fn video_fits(
    container: Container,
    codec: ffmpeg::codec::Id,
    compatibility_mode: bool,
) -> bool {
    use ffmpeg::codec::Id::*;
    match container {
        Container::Mkv => true,
        Container::Mp4 if compatibility_mode => matches!(codec, H264),
        Container::Mp4 => matches!(codec, H264 | HEVC | MPEG4 | AV1),
        Container::Webm => matches!(codec, VP8 | VP9 | AV1),
    }
}

/// Whether `container` can hold `codec` as an audio stream via stream copy. In
/// `compatibility_mode` the MP4/MOV family is narrowed to AAC/MP3 (AC-3 doesn't
/// decode on iOS, Apple Lossless doesn't on non-Apple devices), so those
/// re-encode.
pub(super) fn audio_fits(
    container: Container,
    codec: ffmpeg::codec::Id,
    compatibility_mode: bool,
) -> bool {
    use ffmpeg::codec::Id::*;
    match container {
        Container::Mkv => true,
        Container::Mp4 if compatibility_mode => matches!(codec, AAC | MP3),
        Container::Mp4 => matches!(codec, AAC | MP3 | AC3 | ALAC),
        Container::Webm => matches!(codec, OPUS | VORBIS),
    }
}

/// 8-bit 4:2:0 pixel formats a copied MP4 video stream can use and still decode
/// on phone/TV hardware. 10-bit (and 4:2:2/4:4:4) H.264 plays on computers but
/// not on most mobile decoders, so compatibility mode re-encodes it down to one
/// of these rather than stream-copying it.
pub(super) fn pix_copy_safe(params: &ffmpeg::codec::Parameters) -> bool {
    use ffmpeg::ffi::AVPixelFormat::*;
    let fmt = unsafe { (*params.as_ptr()).format };
    fmt == AV_PIX_FMT_YUV420P as i32
        || fmt == AV_PIX_FMT_YUVJ420P as i32
        || fmt == AV_PIX_FMT_NV12 as i32
}

/// Whether the video stream `(codec, params)` can be stream-copied into
/// `container` and still play on every target device. Layers the
/// `compatibility_mode` pixel-format guard (MP4 only) on top of the container's
/// codec list.
pub(super) fn video_copyable(
    container: Container,
    codec: ffmpeg::codec::Id,
    params: &ffmpeg::codec::Parameters,
    compatibility_mode: bool,
) -> bool {
    video_fits(container, codec, compatibility_mode)
        && (!(compatibility_mode && container == Container::Mp4) || pix_copy_safe(params))
}

/// Output `w`×`h` for a source sized `w`×`h`, downscaled so the height is at
/// most `max_height` (aspect preserved, dimensions rounded down to even for
/// YUV 4:2:0). Returns the source size unchanged when no downscale is needed.
pub(super) fn scaled_dims(w: u32, h: u32, max_height: Option<u32>) -> (u32, u32) {
    match max_height {
        Some(th) if h > th && h > 0 => {
            let tw = (w as u64 * th as u64 / h as u64) as u32;
            (tw & !1, th & !1)
        }
        _ => (w, h),
    }
}

/// Audio codec a re-encode targets for `container`.
pub(super) fn audio_encode_codec(container: Container) -> ffmpeg::codec::Id {
    match container {
        Container::Webm => ffmpeg::codec::Id::OPUS,
        _ => ffmpeg::codec::Id::AAC,
    }
}

/// A source video stream and what the export does with it.
pub(super) struct VideoPlan {
    pub(super) index: usize,
    pub(super) in_tb: ffmpeg::Rational,
    pub(super) fps: ffmpeg::Rational,
    pub(super) params: ffmpeg::codec::Parameters,
    /// Re-encode rather than stream-copy: the cut needs an exact `in` frame, the
    /// output is downscaled, or the container can't hold the source codec.
    pub(super) reencode: bool,
}

/// A source audio stream and what the export does with it.
pub(super) struct AudioPlan {
    pub(super) index: usize,
    pub(super) in_tb: ffmpeg::Rational,
    pub(super) params: ffmpeg::codec::Parameters,
    /// Codec a re-encode targets; `None` stream-copies the source.
    pub(super) encode_to: Option<ffmpeg::codec::Id>,
}

/// Per-stream copy/re-encode decisions for one export, derived once from the
/// source so the choice between a pure remux and a transcode rests on the same
/// information the transcode then acts on.
pub(super) struct StreamPlan {
    pub(super) container: Container,
    pub(super) video: Option<VideoPlan>,
    pub(super) audio: Option<AudioPlan>,
}

impl StreamPlan {
    /// Plan a Full or Clip export. A clip re-encodes both streams: video for a
    /// frame-accurate `in` point, audio for a sample-accurate one.
    pub(super) fn video_export(
        ictx: &ffmpeg::format::context::Input,
        spec: &ExportSpec,
        container: Container,
        clip: bool,
    ) -> Result<Self> {
        let video = ictx.streams().best(Type::Video).map(|stream| {
            let params = stream.parameters();
            let source_fps = stream.avg_frame_rate();
            let fps = if f64::from(source_fps) > 0.0 {
                source_fps
            } else {
                ffmpeg::Rational(DEFAULT_FPS, 1)
            };
            let source_height = unsafe { (*params.as_ptr()).height } as u32;
            // A stream copy can't scale, so a downscale forces a re-encode.
            let is_downscaling = matches!(spec.scale_height, Some(th) if source_height > th);
            let reencode = clip
                || is_downscaling
                || !video_copyable(container, params.id(), &params, spec.compatibility_mode);
            VideoPlan { index: stream.index(), in_tb: stream.time_base(), fps, params, reencode }
        });
        if clip && video.is_none() {
            return Err(anyhow!("no video stream found"));
        }
        let audio = ictx.streams().best(Type::Audio).map(|stream| {
            let params = stream.parameters();
            let copyable = !clip && audio_fits(container, params.id(), spec.compatibility_mode);
            AudioPlan {
                index: stream.index(),
                in_tb: stream.time_base(),
                params,
                encode_to: (!copyable).then(|| audio_encode_codec(container)),
            }
        });
        Ok(StreamPlan { container, video, audio })
    }

    /// Plan an audio-only export re-encoding the audio stream to `codec_id`.
    pub(super) fn audio_export(
        ictx: &ffmpeg::format::context::Input,
        container: Container,
        codec_id: ffmpeg::codec::Id,
    ) -> Result<Self> {
        let stream = ictx
            .streams()
            .best(Type::Audio)
            .ok_or_else(|| anyhow!("no audio stream found"))?;
        Ok(StreamPlan {
            container,
            video: None,
            audio: Some(AudioPlan {
                index: stream.index(),
                in_tb: stream.time_base(),
                params: stream.parameters(),
                encode_to: Some(codec_id),
            }),
        })
    }

    /// Whether every stream can be stream-copied, making the export a pure remux.
    pub(super) fn is_copy_only(&self) -> bool {
        self.video.as_ref().map_or(true, |video| !video.reencode)
            && self.audio.as_ref().map_or(true, |audio| audio.encode_to.is_none())
    }
}
