//! Video re-encoding: opening the container's video encoder and driving the
//! decode -> scale -> encode pipeline for one stream.

use anyhow::{anyhow, Result};
use ffmpeg_the_third as ffmpeg;
use ffmpeg::format::Pixel;
use ffmpeg::software::scaling::context::Context as Scaler;
use ffmpeg::util::frame::video::Video;

use super::copy::write_packet;
use super::plan::Container;

const X264_PRESET: &str = "medium";
/// Constant Rate Factor for libx264 (lower is higher quality; 18-28 is typical).
const X264_CRF: &str = "23";
/// Constant-quality CRF for libvpx-vp9 (0-63; ~31 is a sane default).
const VP9_CRF: &str = "31";
/// libvpx-vp9 speed/quality knob (0-8, higher is faster); VP9 encoding is slow.
const VP9_CPU_USED: &str = "5";
/// Sentinel telling FFmpeg a rate-control field is unset, so the encoder's own
/// (preset-derived) value is used instead.
const RC_UNSET: i32 = -1;

/// Add a video output stream sized to `w`×`h` at `fps`, encoded to the codec
/// `container` requires (H.264 for MP4/MOV/MKV, VP9 for WebM).
pub(super) fn open_video_encoder(
    octx: &mut ffmpeg::format::context::Output,
    container: Container,
    w: u32,
    h: u32,
    fps: ffmpeg::Rational,
    global_header: bool,
) -> Result<(usize, ffmpeg::codec::encoder::Video)> {
    let vp9 = container == Container::Webm;
    let codec = if vp9 {
        ffmpeg::encoder::find_by_name("libvpx-vp9")
            .ok_or_else(|| anyhow!("VP9 encoder (libvpx-vp9) unavailable"))?
    } else {
        ffmpeg::encoder::find(ffmpeg::codec::Id::H264)
            .ok_or_else(|| anyhow!("H.264 encoder unavailable"))?
    };

    let mut v_out = octx.add_stream(codec)?;
    let mut enc = ffmpeg::codec::context::Context::from_parameters(v_out.parameters())?
        .encoder()
        .video()?;
    enc.set_width(w);
    enc.set_height(h);
    enc.set_format(Pixel::YUV420P);
    enc.set_time_base(fps.invert());
    enc.set_frame_rate(Some(fps));
    if global_header {
        enc.set_flags(ffmpeg::codec::Flags::GLOBAL_HEADER);
    }

    let mut opts = ffmpeg::Dictionary::new();
    if vp9 {
        // Constant-quality mode: bitrate 0 + CRF. cpu-used trades speed for size.
        enc.set_bit_rate(0);
        opts.set("crf", VP9_CRF);
        opts.set("cpu-used", VP9_CPU_USED);
        opts.set("deadline", "good");
        // Row-level MT lets libvpx-vp9 encode rows in parallel; tile-columns
        // (log2) divides the frame into independently-encodable vertical slices.
        opts.set("row-mt", "1");
        opts.set("tile-columns", "2");
    } else {
        // Leave rate-control fields unset so FFmpeg keeps the x264 preset's
        // values instead of forcing defaults libx264 rejects as "broken".
        unsafe {
            let ctx = enc.as_mut_ptr();
            (*ctx).qmin = RC_UNSET;
            (*ctx).qmax = RC_UNSET;
            (*ctx).me_range = RC_UNSET;
            (*ctx).gop_size = RC_UNSET;
        }
        opts.set("preset", X264_PRESET);
        opts.set("crf", X264_CRF);
        // 0 = auto-detect; x264 spawns one encode thread per logical CPU.
        opts.set("threads", "0");
    }
    let encoder = enc.open_as_with(codec, opts)?;
    v_out.set_parameters(&encoder);
    Ok((v_out.index(), encoder))
}

/// A video stream being re-encoded to H.264, optionally trimmed to a window.
pub(super) struct VideoReenc {
    pub(super) decoder: ffmpeg::codec::decoder::Video,
    pub(super) encoder: ffmpeg::codec::encoder::Video,
    pub(super) scaler: Scaler,
    pub(super) out_index: usize,
    pub(super) enc_tb: ffmpeg::Rational,
    pub(super) windowed: bool,
    pub(super) v_start: i64,
    pub(super) v_end: i64,
    pub(super) max_out_frames: i64,
    pub(super) out_pts: i64,
}

impl VideoReenc {
    /// Pull decoded frames, scale + encode the ones inside the window. Returns
    /// `true` once the window end is reached and the caller should stop reading.
    fn drain_decoder(&mut self, octx: &mut ffmpeg::format::context::Output) -> Result<bool> {
        let mut frame = Video::empty();
        while self.decoder.receive_frame(&mut frame).is_ok() {
            let pts = frame.pts().unwrap_or(0);
            if self.windowed {
                if pts < self.v_start {
                    continue;
                }
                if pts > self.v_end || self.out_pts >= self.max_out_frames {
                    return Ok(true);
                }
            }
            let mut yuv = Video::empty();
            self.scaler.run(&frame, &mut yuv)?;
            yuv.set_pts(Some(self.out_pts));
            self.out_pts += 1;
            self.encoder.send_frame(&yuv)?;
            write_encoded(&mut self.encoder, octx, self.out_index, self.enc_tb)?;
        }
        Ok(false)
    }

    /// Decode `packet`, encode the frames inside the window. Returns `true` once
    /// the window end is reached and the caller should stop reading.
    pub(super) fn process(
        &mut self,
        packet: &ffmpeg::Packet,
        octx: &mut ffmpeg::format::context::Output,
    ) -> Result<bool> {
        self.decoder.send_packet(packet)?;
        self.drain_decoder(octx)
    }

    pub(super) fn flush(&mut self, octx: &mut ffmpeg::format::context::Output) -> Result<()> {
        // Drain the decoder first: its reorder delay (B-frames) holds the last
        // frames, which a Full re-encode would otherwise truncate at EOF.
        self.decoder.send_eof()?;
        self.drain_decoder(octx)?;
        self.encoder.send_eof()?;
        write_encoded(&mut self.encoder, octx, self.out_index, self.enc_tb)?;
        Ok(())
    }
}

/// Drain ready packets from a video encoder and write them to the output.
pub(super) fn write_encoded(
    encoder: &mut ffmpeg::encoder::Video,
    octx: &mut ffmpeg::format::context::Output,
    out_index: usize,
    enc_tb: ffmpeg::Rational,
) -> Result<()> {
    let mut packet = ffmpeg::Packet::empty();
    while encoder.receive_packet(&mut packet).is_ok() {
        write_packet(&mut packet, enc_tb, out_index, octx)?;
    }
    Ok(())
}
