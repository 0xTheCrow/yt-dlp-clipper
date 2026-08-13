//! The single copy-or-re-encode writer every video export and every re-encoded
//! audio-only export routes through.

use anyhow::Result;
use std::sync::atomic::AtomicBool;
use ffmpeg_the_third as ffmpeg;
use ffmpeg::format::Pixel;
use ffmpeg::software::scaling::{context::Context as Scaler, flag::Flags};

use super::audio::AudioReenc;
use super::copy::{add_copy_stream, write_packet};
use super::plan::{scaled_dims, StreamPlan};
use super::video::{open_video_encoder, VideoReenc};
use super::{check_cancel, seek_to, write_header, ExportSpec};

/// Write the file (whole, or the `windowed` clip range) following `plan`'s
/// per-stream decisions: stream-copy what the container can hold, re-encode the
/// rest. Only the planned video and audio streams are carried over.
pub(super) fn transcode(
    ictx: &mut ffmpeg::format::context::Input,
    spec: &ExportSpec,
    plan: StreamPlan,
    windowed: bool,
    cancel: &AtomicBool,
) -> Result<()> {
    let mut octx = ffmpeg::format::output(&spec.output)?;
    let global_header = octx
        .format()
        .flags()
        .contains(ffmpeg::format::Flags::GLOBAL_HEADER);

    let video_index = plan.video.as_ref().map(|video| video.index);
    let video_in_tb = plan.video.as_ref().map_or(ffmpeg::Rational(1, 1), |video| video.in_tb);
    let audio_index = plan.audio.as_ref().map(|audio| audio.index);
    let audio_in_tb = plan.audio.as_ref().map_or(ffmpeg::Rational(1, 1), |audio| audio.in_tb);

    let mut video_copy: Option<(ffmpeg::Rational, usize)> = None;
    let mut video_reenc: Option<VideoReenc> = None;
    if let Some(video) = plan.video {
        if video.reencode {
            let decoder = ffmpeg::codec::context::Context::from_parameters(video.params)?
                .decoder()
                .video()?;
            let (width, height) = (decoder.width(), decoder.height());
            let (out_width, out_height) = scaled_dims(width, height, spec.scale_height);
            let scaler = Scaler::get(
                decoder.format(),
                width,
                height,
                Pixel::YUV420P,
                out_width,
                out_height,
                Flags::BILINEAR,
            )?;
            let (out_index, encoder) = open_video_encoder(
                &mut octx,
                plan.container,
                out_width,
                out_height,
                video.fps,
                global_header,
            )?;
            let window_secs = (spec.end_secs - spec.start_secs).max(0.0);
            video_reenc = Some(VideoReenc {
                decoder,
                encoder,
                scaler,
                out_index,
                enc_tb: video.fps.invert(),
                windowed,
                v_start: (spec.start_secs / f64::from(video.in_tb)).round() as i64,
                v_end: (spec.end_secs / f64::from(video.in_tb)).round() as i64,
                max_out_frames: (window_secs * f64::from(video.fps)).round() as i64 + 1,
                out_pts: 0,
            });
        } else {
            video_copy = Some((video.in_tb, add_copy_stream(&mut octx, video.params)?));
        }
    }

    let mut audio_copy: Option<usize> = None;
    let mut audio_reenc: Option<AudioReenc> = None;
    if let Some(audio) = plan.audio {
        match audio.encode_to {
            // A windowed export trims to the exact sample via `atrim`. Saving the
            // whole file re-encodes only because the container can't hold the
            // source codec, so the stream goes through untrimmed.
            Some(codec_id) => {
                let decoder = ffmpeg::codec::context::Context::from_parameters(audio.params)?
                    .decoder()
                    .audio()?;
                audio_reenc = Some(AudioReenc::new(
                    &mut octx,
                    decoder,
                    codec_id,
                    global_header,
                    audio.in_tb,
                    windowed.then_some((spec.start_secs, spec.end_secs)),
                )?);
            }
            None => audio_copy = Some(add_copy_stream(&mut octx, audio.params)?),
        }
    }

    write_header(&mut octx, spec.output.as_ref())?;

    // Feed from the seek point (a keyframe at or before the start); `atrim` and
    // `VideoReenc`'s window drop the leading samples and frames.
    if windowed {
        seek_to(ictx, spec.start_secs)?;
    }
    // Audio packets can lag the matching video in the interleave, so a windowed
    // export keeps reading audio past the video's end (until this PTS) instead of
    // stopping the moment video finishes, which would cut the audio tail short.
    let audio_end_ts = (spec.end_secs / f64::from(audio_in_tb)).round() as i64;
    // Same backstop for video: if frame PTS are missing so `process` never
    // reports the window end, the packet timeline still bounds the read instead
    // of decoding every remaining packet to EOF.
    let video_end_ts = (spec.end_secs / f64::from(video_in_tb)).round() as i64;
    let mut is_video_done = video_reenc.is_none();
    let mut is_audio_done = audio_reenc.is_none();

    let mut packet = ffmpeg::Packet::empty();
    while packet.read(ictx).is_ok() {
        check_cancel(cancel)?;
        let index = Some(packet.stream());
        if index == video_index {
            if let Some(reenc) = video_reenc.as_mut() {
                if !is_video_done && reenc.process(&packet, &mut octx)? {
                    is_video_done = true;
                }
                if windowed && packet.pts().is_some_and(|pts| pts > video_end_ts) {
                    is_video_done = true;
                }
            } else if let Some((in_tb, out_index)) = video_copy {
                write_packet(&mut packet, in_tb, out_index, &mut octx)?;
            }
        } else if index == audio_index {
            if let Some(reenc) = audio_reenc.as_mut() {
                if windowed && packet.pts().unwrap_or(0) > audio_end_ts {
                    is_audio_done = true;
                } else if !is_audio_done {
                    reenc.process(&packet, &mut octx)?;
                }
            } else if let Some(out_index) = audio_copy {
                write_packet(&mut packet, audio_in_tb, out_index, &mut octx)?;
            }
        }
        if windowed && is_video_done && is_audio_done {
            break;
        }
    }

    if let Some(reenc) = video_reenc.as_mut() {
        reenc.flush(&mut octx)?;
    }
    if let Some(reenc) = audio_reenc.as_mut() {
        reenc.flush(&mut octx)?;
    }
    octx.write_trailer()?;
    Ok(())
}
