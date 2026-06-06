//! Export a trimmed clip or an audio-only file using ffmpeg-the-third.
//!
//! A clip's video is re-encoded so the cut begins exactly on the chosen `in`
//! frame, and its audio is re-encoded and trimmed to the exact sample (a stream
//! copy can't split a packet, so an exact `in`/`out` requires re-encoding).

use anyhow::{anyhow, bail, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use ffmpeg_the_third as ffmpeg;
use ffmpeg::channel_layout::ChannelLayout;
use ffmpeg::format::Pixel;
use ffmpeg::media::Type;
use ffmpeg::software::scaling::{context::Context as Scaler, flag::Flags};
use ffmpeg::util::frame::video::Video;

/// Frame rate assumed when a stream reports an invalid average frame rate.
const DEFAULT_FPS: i32 = 25;
/// libx264 speed/quality preset for the re-encoded clip.
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
/// Bitrate for re-encoded audio (MP3/AAC), in bits per second.
const AUDIO_BITRATE: usize = 192_000;

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
        Mode::AudioOnly(format) => export_audio_only(spec, format, cancel),
        Mode::Clip => export_clip(spec, cancel),
    }
}

/// Error out when the export has been cancelled, so the encode loop unwinds.
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

/// Container family of the output path, deciding which codecs can be copied and
/// which codecs a re-encode must target.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Container {
    /// Matroska (`.mkv`) — accepts essentially any codec.
    Mkv,
    /// MP4/MOV family — a fixed set of codecs only.
    Mp4,
    /// WebM — VP8/VP9/AV1 video, Opus/Vorbis audio only.
    Webm,
}

fn container_kind(output: &str) -> Container {
    match output.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "mkv" => Container::Mkv,
        "webm" => Container::Webm,
        _ => Container::Mp4,
    }
}

/// Whether `container` can hold `codec` as a video stream via stream copy.
fn video_fits(container: Container, codec: ffmpeg::codec::Id) -> bool {
    use ffmpeg::codec::Id::*;
    match container {
        Container::Mkv => true,
        Container::Mp4 => matches!(codec, H264 | HEVC | MPEG4 | AV1),
        Container::Webm => matches!(codec, VP8 | VP9 | AV1),
    }
}

/// Whether `container` can hold `codec` as an audio stream via stream copy.
fn audio_fits(container: Container, codec: ffmpeg::codec::Id) -> bool {
    use ffmpeg::codec::Id::*;
    match container {
        Container::Mkv => true,
        Container::Mp4 => matches!(codec, AAC | MP3 | AC3 | ALAC),
        Container::Webm => matches!(codec, OPUS | VORBIS),
    }
}

/// Output `w`×`h` for a source sized `w`×`h`, downscaled so the height is at
/// most `max_height` (aspect preserved, dimensions rounded down to even for
/// YUV 4:2:0). Returns the source size unchanged when no downscale is needed.
fn scaled_dims(w: u32, h: u32, max_height: Option<u32>) -> (u32, u32) {
    match max_height {
        Some(th) if h > th && h > 0 => {
            let tw = (w as u64 * th as u64 / h as u64) as u32;
            (tw & !1, th & !1)
        }
        _ => (w, h),
    }
}

/// Audio codec a re-encode targets for `container`.
fn audio_encode_codec(container: Container) -> ffmpeg::codec::Id {
    match container {
        Container::Webm => ffmpeg::codec::Id::OPUS,
        _ => ffmpeg::codec::Id::AAC,
    }
}

/// Save the whole file in the chosen container, stream-copying every stream that
/// the container can hold and re-encoding only those it cannot.
fn export_full(spec: &ExportSpec, cancel: &AtomicBool) -> Result<()> {
    let container = container_kind(&spec.output);
    let ictx = ffmpeg::format::input(&spec.input)?;
    // Downscaling can't be done by a stream copy, so it forces a re-encode.
    let (video_ok, downscale) = match ictx.streams().best(Type::Video) {
        Some(s) => {
            let fits = video_fits(container, s.parameters().id());
            let src_h = unsafe { (*s.parameters().as_ptr()).height } as u32;
            (fits, matches!(spec.scale_height, Some(th) if src_h > th))
        }
        None => (true, false),
    };
    let audio_ok = ictx
        .streams()
        .best(Type::Audio)
        .map_or(true, |s| audio_fits(container, s.parameters().id()));
    drop(ictx);

    if video_ok && audio_ok && !downscale {
        remux_copy(spec, cancel)
    } else {
        transcode(spec, container, false, cancel)
    }
}

/// Remux every stream of the whole file into the output container (copy only).
fn remux_copy(spec: &ExportSpec, cancel: &AtomicBool) -> Result<()> {
    let mut ictx = ffmpeg::format::input(&spec.input)?;
    let mut octx = ffmpeg::format::output(&spec.output)?;

    // input stream index -> (output index, input time base)
    let mut mapping: Vec<Option<(usize, ffmpeg::Rational)>> =
        vec![None; ictx.nb_streams() as usize];

    for in_stream in ictx.streams() {
        let in_index = in_stream.index();
        let in_tb = in_stream.time_base();
        let params = in_stream.parameters();

        let mut out_stream = octx.add_stream(ffmpeg::encoder::find(ffmpeg::codec::Id::None))?;
        out_stream.set_parameters(params);
        // Clear the source codec tag so the output container assigns its own.
        unsafe {
            (*out_stream.parameters().as_mut_ptr()).codec_tag = 0;
        }
        mapping[in_index] = Some((out_stream.index(), in_tb));
    }

    octx.write_header()?;

    let mut packet = ffmpeg::Packet::empty();
    while packet.read(&mut ictx).is_ok() {
        check_cancel(cancel)?;
        if let Some((out_index, in_tb)) = mapping[packet.stream()] {
            let out_tb = octx.stream(out_index).unwrap().time_base();
            packet.rescale_ts(in_tb, out_tb);
            packet.set_stream(out_index);
            packet.write_interleaved(&mut octx)?;
        }
    }

    octx.write_trailer()?;
    Ok(())
}

/// Copy the audio packets that fall within the window into a new container.
fn export_audio_only(spec: &ExportSpec, format: AudioFormat, cancel: &AtomicBool) -> Result<()> {
    match format {
        AudioFormat::Original => export_audio_copy(spec, cancel),
        AudioFormat::Mp3 => export_audio_reencode(spec, ffmpeg::codec::Id::MP3, cancel),
        AudioFormat::Aac => export_audio_reencode(spec, ffmpeg::codec::Id::AAC, cancel),
    }
}

/// Stream-copy the windowed audio into the output container, losslessly.
fn export_audio_copy(spec: &ExportSpec, cancel: &AtomicBool) -> Result<()> {
    let mut ictx = ffmpeg::format::input(&spec.input)?;
    let mut octx = ffmpeg::format::output(&spec.output)?;

    let in_stream = ictx
        .streams()
        .best(Type::Audio)
        .ok_or_else(|| anyhow!("no audio stream found"))?;
    let in_index = in_stream.index();
    let in_tb = in_stream.time_base();

    let out_index = {
        let mut out_stream = octx.add_stream(ffmpeg::encoder::find(ffmpeg::codec::Id::None))?;
        out_stream.set_parameters(in_stream.parameters());
        unsafe {
            (*out_stream.parameters().as_mut_ptr()).codec_tag = 0;
        }
        out_stream.index()
    };

    octx.write_header()?;

    let start_ts = (spec.start_secs / f64::from(in_tb)).round() as i64;
    let end_ts = (spec.end_secs / f64::from(in_tb)).round() as i64;

    seek_to(&mut ictx, spec.start_secs)?;

    let mut packet = ffmpeg::Packet::empty();
    while packet.read(&mut ictx).is_ok() {
        check_cancel(cancel)?;
        if packet.stream() != in_index {
            continue;
        }
        let pts = packet.pts().unwrap_or(0);
        if pts < start_ts {
            continue;
        }
        if pts > end_ts {
            break;
        }
        let out_tb = octx.stream(out_index).unwrap().time_base();
        rebase(&mut packet, start_ts);
        packet.rescale_ts(in_tb, out_tb);
        packet.set_stream(out_index);
        packet.write_interleaved(&mut octx)?;
    }

    octx.write_trailer()?;
    Ok(())
}

/// Re-encode the windowed audio to `codec_id` (MP3 or AAC). The window is cut to
/// the exact sample, not the nearest packet, by `AudioReenc`'s `atrim` filter.
fn export_audio_reencode(
    spec: &ExportSpec,
    codec_id: ffmpeg::codec::Id,
    cancel: &AtomicBool,
) -> Result<()> {
    let mut ictx = ffmpeg::format::input(&spec.input)?;
    let mut octx = ffmpeg::format::output(&spec.output)?;

    let (in_index, in_tb, decoder) = {
        let stream = ictx
            .streams()
            .best(Type::Audio)
            .ok_or_else(|| anyhow!("no audio stream found"))?;
        let decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?
            .decoder()
            .audio()?;
        (stream.index(), stream.time_base(), decoder)
    };

    let global_header = octx
        .format()
        .flags()
        .contains(ffmpeg::format::Flags::GLOBAL_HEADER);
    let mut reenc = AudioReenc::new(
        &mut octx, decoder, codec_id, global_header, in_tb, spec.start_secs, spec.end_secs,
    )?;

    octx.write_header()?;
    reenc.out_tb = octx.stream(reenc.out_index).unwrap().time_base();

    // Feed from the seek point (a keyframe at/before the start); `atrim` drops the
    // leading samples, so no packets are pre-filtered out except past the end.
    seek_to(&mut ictx, spec.start_secs)?;
    let end_ts = (spec.end_secs / f64::from(in_tb)).round() as i64;
    let mut packet = ffmpeg::Packet::empty();
    while packet.read(&mut ictx).is_ok() {
        check_cancel(cancel)?;
        if packet.stream() != in_index {
            continue;
        }
        if packet.pts().unwrap_or(0) > end_ts {
            break;
        }
        reenc.process(&packet, &mut octx)?;
    }
    reenc.flush(&mut octx)?;
    octx.write_trailer()?;
    Ok(())
}

/// Add an audio output stream encoding to `codec_id`, picking a sample rate,
/// channel layout, and sample format the encoder supports (the filter graph
/// resamples to whatever rate is chosen).
fn open_audio_encoder(
    octx: &mut ffmpeg::format::context::Output,
    decoder: &ffmpeg::codec::decoder::Audio,
    codec_id: ffmpeg::codec::Id,
    global_header: bool,
) -> Result<(usize, ffmpeg::codec::encoder::Audio)> {
    // The native Opus encoder is experimental; prefer libopus when targeting Opus.
    let codec = match codec_id {
        ffmpeg::codec::Id::OPUS => ffmpeg::encoder::find_by_name("libopus"),
        _ => ffmpeg::encoder::find(codec_id),
    }
    .ok_or_else(|| anyhow!("audio encoder unavailable"))?
    .audio()?;

    // Keep the source rate if the encoder allows it (Opus rejects e.g. 44100),
    // otherwise pick the highest supported rate.
    let src_rate = decoder.rate() as i32;
    let rate = match codec.rates() {
        Some(rates) => {
            let supported: Vec<i32> = rates.collect();
            if supported.contains(&src_rate) {
                src_rate
            } else {
                supported.into_iter().max().unwrap_or(src_rate)
            }
        }
        None => src_rate,
    };
    let enc_tb = ffmpeg::Rational(1, rate);
    let in_layout = if decoder.channel_layout().is_empty() {
        ChannelLayout::default(decoder.channels() as i32)
    } else {
        decoder.channel_layout()
    };
    let enc_layout = codec
        .channel_layouts()
        .map(|layouts| layouts.best(in_layout.channels()))
        .unwrap_or(ChannelLayout::STEREO);
    let enc_format = codec
        .formats()
        .and_then(|mut f| f.next())
        .ok_or_else(|| anyhow!("audio encoder has no sample format"))?;

    let mut out = octx.add_stream(codec)?;
    let mut enc = ffmpeg::codec::context::Context::from_parameters(out.parameters())?
        .encoder()
        .audio()?;
    enc.set_rate(rate);
    enc.set_channel_layout(enc_layout);
    enc.set_channels(enc_layout.channels());
    enc.set_format(enc_format);
    enc.set_bit_rate(AUDIO_BITRATE);
    enc.set_time_base(enc_tb);
    if global_header {
        enc.set_flags(ffmpeg::codec::Flags::GLOBAL_HEADER);
    }
    let encoder = enc.open_as(codec)?;
    out.set_parameters(&encoder);
    out.set_time_base(enc_tb);
    Ok((out.index(), encoder))
}

/// Build `abuffer → atrim → abuffersink` converting the decoder's samples to the
/// encoder's format/layout/rate, with the sink chunked to the encoder's frame
/// size when the encoder needs fixed-size frames.
///
/// `trim` is `(start_sample, end_sample)` measured from the first sample fed to
/// the graph; `atrim` keeps `[start_sample, end_sample)` at sample precision so
/// the cut is exact rather than rounded to a packet boundary.
fn audio_filter(
    decoder: &ffmpeg::codec::decoder::Audio,
    encoder: &ffmpeg::codec::encoder::Audio,
    trim: (i64, i64),
) -> Result<ffmpeg::filter::Graph> {
    let mut graph = ffmpeg::filter::Graph::new();
    let layout = if decoder.channel_layout().is_empty() {
        ChannelLayout::default(decoder.channels() as i32)
    } else {
        decoder.channel_layout()
    };
    // Frames are fed with PTS in sample units (see `AudioReenc::drain_decoder`),
    // so the buffer time base is 1/sample_rate to match.
    let args = format!(
        "time_base=1/{}:sample_rate={}:sample_fmt={}:channel_layout=0x{:x}",
        decoder.rate(),
        decoder.rate(),
        decoder.format().name(),
        layout.bits()
    );
    graph.add(&ffmpeg::filter::find("abuffer").unwrap(), "in", &args)?;
    graph.add(&ffmpeg::filter::find("abuffersink").unwrap(), "out", "")?;
    {
        let mut out = graph.get("out").unwrap();
        out.set_sample_format(encoder.format());
        out.set_channel_layout(encoder.channel_layout());
        out.set_sample_rate(encoder.rate());
    }
    let (start_sample, end_sample) = trim;
    let chain = format!("atrim=start_sample={start_sample}:end_sample={end_sample}");
    graph.output("in", 0)?.input("out", 0)?.parse(&chain)?;
    graph.validate()?;

    if let Some(codec) = encoder.codec() {
        let variable = codec
            .capabilities()
            .contains(ffmpeg::codec::capabilities::Capabilities::VARIABLE_FRAME_SIZE);
        if !variable {
            graph.get("out").unwrap().sink().set_frame_size(encoder.frame_size());
        }
    }
    Ok(graph)
}

/// Pull filtered (encoder-ready) frames, stamp monotonic output PTS, encode, and
/// write the resulting packets.
fn drain_filter(
    filter: &mut ffmpeg::filter::Graph,
    encoder: &mut ffmpeg::codec::encoder::Audio,
    octx: &mut ffmpeg::format::context::Output,
    out_index: usize,
    out_tb: ffmpeg::Rational,
    out_samples: &mut i64,
) -> Result<()> {
    let mut frame = ffmpeg::frame::Audio::empty();
    while filter.get("out").unwrap().sink().frame(&mut frame).is_ok() {
        frame.set_pts(Some(*out_samples));
        *out_samples += frame.samples() as i64;
        encoder.send_frame(&frame)?;
        write_audio_packets(encoder, octx, out_index, out_tb)?;
    }
    Ok(())
}

/// Drain ready packets from an audio encoder and write them to the output.
fn write_audio_packets(
    encoder: &mut ffmpeg::codec::encoder::Audio,
    octx: &mut ffmpeg::format::context::Output,
    out_index: usize,
    out_tb: ffmpeg::Rational,
) -> Result<()> {
    let enc_tb = ffmpeg::Rational(1, encoder.rate() as i32);
    let mut packet = ffmpeg::Packet::empty();
    while encoder.receive_packet(&mut packet).is_ok() {
        packet.set_stream(out_index);
        packet.rescale_ts(enc_tb, out_tb);
        packet.write_interleaved(octx)?;
    }
    Ok(())
}

/// Save a frame-accurate clip in the chosen container. Video is always
/// re-encoded for an exact `in` point; audio is copied or re-encoded to fit.
fn export_clip(spec: &ExportSpec, cancel: &AtomicBool) -> Result<()> {
    transcode(spec, container_kind(&spec.output), true, cancel)
}

/// Add a video output stream sized to `w`×`h` at `fps`, encoded to the codec
/// `container` requires (H.264 for MP4/MOV/MKV, VP9 for WebM).
fn open_video_encoder(
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
    }
    let encoder = enc.open_as_with(codec, opts)?;
    v_out.set_parameters(&encoder);
    Ok((v_out.index(), encoder))
}

/// A video stream being re-encoded to H.264, optionally trimmed to a window.
struct VideoReenc {
    decoder: ffmpeg::codec::decoder::Video,
    encoder: ffmpeg::codec::encoder::Video,
    scaler: Scaler,
    out_index: usize,
    enc_tb: ffmpeg::Rational,
    windowed: bool,
    v_start: i64,
    v_end: i64,
    max_out_frames: i64,
    out_pts: i64,
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
    fn process(&mut self, packet: &ffmpeg::Packet, octx: &mut ffmpeg::format::context::Output) -> Result<bool> {
        self.decoder.send_packet(packet)?;
        self.drain_decoder(octx)
    }

    fn flush(&mut self, octx: &mut ffmpeg::format::context::Output) -> Result<()> {
        // Drain the decoder first: its reorder delay (B-frames) holds the last
        // frames, which a Full re-encode would otherwise truncate at EOF.
        self.decoder.send_eof()?;
        self.drain_decoder(octx)?;
        self.encoder.send_eof()?;
        write_encoded(&mut self.encoder, octx, self.out_index, self.enc_tb)?;
        Ok(())
    }
}

/// An audio stream being re-encoded (to AAC/MP3/Opus), trimmed to the exact
/// `[start_secs, end_secs)` sample window. The filter is built lazily on the
/// first decoded frame, once its PTS reveals where the post-seek run begins.
struct AudioReenc {
    decoder: ffmpeg::codec::decoder::Audio,
    encoder: ffmpeg::codec::encoder::Audio,
    filter: Option<ffmpeg::filter::Graph>,
    out_index: usize,
    out_tb: ffmpeg::Rational,
    /// Source stream time base, for mapping a frame's PTS to a sample index.
    in_tb: ffmpeg::Rational,
    /// Decoder sample rate, for converting the window seconds to samples.
    rate: f64,
    /// Window bounds as absolute sample indices in the source.
    start_sample: i64,
    end_sample: i64,
    in_samples: i64,
    out_samples: i64,
}

impl AudioReenc {
    fn new(
        octx: &mut ffmpeg::format::context::Output,
        decoder: ffmpeg::codec::decoder::Audio,
        codec_id: ffmpeg::codec::Id,
        global_header: bool,
        in_tb: ffmpeg::Rational,
        start_secs: f64,
        end_secs: f64,
    ) -> Result<Self> {
        let rate = decoder.rate() as f64;
        let (out_index, encoder) = open_audio_encoder(octx, &decoder, codec_id, global_header)?;
        Ok(AudioReenc {
            decoder,
            encoder,
            filter: None,
            out_index,
            out_tb: ffmpeg::Rational(1, 1),
            in_tb,
            rate,
            start_sample: (start_secs * rate).round() as i64,
            end_sample: (end_secs * rate).round() as i64,
            in_samples: 0,
            out_samples: 0,
        })
    }

    /// Pull decoded frames, building the trim filter the first time (the leading
    /// frame's PTS fixes where sample 0 of the graph sits in the source), then
    /// feed each frame in and drain the encoder.
    fn drain_decoder(&mut self, octx: &mut ffmpeg::format::context::Output) -> Result<()> {
        let mut frame = ffmpeg::frame::Audio::empty();
        while self.decoder.receive_frame(&mut frame).is_ok() {
            if self.filter.is_none() {
                // The first fed sample lands at this frame's source position, so
                // shift the window bounds to be relative to it for `atrim`.
                let base =
                    (frame.pts().unwrap_or(0) as f64 * f64::from(self.in_tb) * self.rate).round() as i64;
                let trim = ((self.start_sample - base).max(0), self.end_sample - base);
                self.filter = Some(audio_filter(&self.decoder, &self.encoder, trim)?);
            }
            let filter = self.filter.as_mut().unwrap();
            frame.set_pts(Some(self.in_samples));
            self.in_samples += frame.samples() as i64;
            filter.get("in").unwrap().source().add(&frame)?;
            drain_filter(
                filter,
                &mut self.encoder,
                octx,
                self.out_index,
                self.out_tb,
                &mut self.out_samples,
            )?;
        }
        Ok(())
    }

    fn process(&mut self, packet: &ffmpeg::Packet, octx: &mut ffmpeg::format::context::Output) -> Result<()> {
        self.decoder.send_packet(packet)?;
        self.drain_decoder(octx)
    }

    fn flush(&mut self, octx: &mut ffmpeg::format::context::Output) -> Result<()> {
        self.decoder.send_eof()?;
        self.drain_decoder(octx)?;
        if let Some(filter) = self.filter.as_mut() {
            filter.get("in").unwrap().source().flush()?;
            drain_filter(
                filter,
                &mut self.encoder,
                octx,
                self.out_index,
                self.out_tb,
                &mut self.out_samples,
            )?;
        }
        self.encoder.send_eof()?;
        write_audio_packets(&mut self.encoder, octx, self.out_index, self.out_tb)?;
        Ok(())
    }
}

/// Write the file (whole, or `clip`'s window) into `container`, stream-copying
/// each stream the container can hold and re-encoding the rest. Clips always
/// re-encode video for a frame-accurate `in` point.
fn transcode(spec: &ExportSpec, container: Container, clip: bool, cancel: &AtomicBool) -> Result<()> {
    let mut ictx = ffmpeg::format::input(&spec.input)?;
    let mut octx = ffmpeg::format::output(&spec.output)?;
    let global_header = octx
        .format()
        .flags()
        .contains(ffmpeg::format::Flags::GLOBAL_HEADER);

    let video_meta = ictx.streams().best(Type::Video).map(|s| {
        let fps = s.avg_frame_rate();
        let fps = if f64::from(fps) > 0.0 {
            fps
        } else {
            ffmpeg::Rational(DEFAULT_FPS, 1)
        };
        (s.index(), s.time_base(), fps, s.parameters().id(), s.parameters())
    });
    if clip && video_meta.is_none() {
        return Err(anyhow!("no video stream found"));
    }
    let audio_meta = ictx
        .streams()
        .best(Type::Audio)
        .map(|s| (s.index(), s.time_base(), s.parameters().id(), s.parameters()));

    // ---- video pipe: re-encode (clip, or codec the container can't hold) or copy ----
    let mut v_in_index = usize::MAX;
    let mut v_in_tb = ffmpeg::Rational(1, 1);
    let mut v_copy: Option<(ffmpeg::Rational, usize)> = None;
    let mut v_reenc: Option<VideoReenc> = None;
    if let Some((index, in_tb, fps, codec, params)) = video_meta {
        v_in_index = index;
        v_in_tb = in_tb;
        let src_h = unsafe { (*params.as_ptr()).height } as u32;
        let downscale = matches!(spec.scale_height, Some(th) if src_h > th);
        if clip || downscale || !video_fits(container, codec) {
            let decoder = ffmpeg::codec::context::Context::from_parameters(params)?
                .decoder()
                .video()?;
            let (w, h) = (decoder.width(), decoder.height());
            let (ow, oh) = scaled_dims(w, h, spec.scale_height);
            let scaler = Scaler::get(decoder.format(), w, h, Pixel::YUV420P, ow, oh, Flags::BILINEAR)?;
            let (out_index, encoder) =
                open_video_encoder(&mut octx, container, ow, oh, fps, global_header)?;
            let window_secs = (spec.end_secs - spec.start_secs).max(0.0);
            v_reenc = Some(VideoReenc {
                decoder,
                encoder,
                scaler,
                out_index,
                enc_tb: fps.invert(),
                windowed: clip,
                v_start: (spec.start_secs / f64::from(in_tb)).round() as i64,
                v_end: (spec.end_secs / f64::from(in_tb)).round() as i64,
                max_out_frames: (window_secs * f64::from(fps)).round() as i64 + 1,
                out_pts: 0,
            });
        } else {
            let mut out = octx.add_stream(ffmpeg::encoder::find(ffmpeg::codec::Id::None))?;
            out.set_parameters(params);
            unsafe {
                (*out.parameters().as_mut_ptr()).codec_tag = 0;
            }
            v_copy = Some((in_tb, out.index()));
        }
    }

    // ---- audio pipe: a clip always re-encodes for a sample-exact window; the
    // whole file copies when the container can hold the codec, else re-encodes ----
    let mut a_in_index = usize::MAX;
    let mut a_in_tb = ffmpeg::Rational(1, 1);
    let mut a_copy: Option<usize> = None;
    let mut a_reenc: Option<AudioReenc> = None;
    if let Some((index, in_tb, codec, params)) = audio_meta {
        a_in_index = index;
        a_in_tb = in_tb;
        if !clip && audio_fits(container, codec) {
            let mut out = octx.add_stream(ffmpeg::encoder::find(ffmpeg::codec::Id::None))?;
            out.set_parameters(params);
            unsafe {
                (*out.parameters().as_mut_ptr()).codec_tag = 0;
            }
            a_copy = Some(out.index());
        } else {
            let decoder = ffmpeg::codec::context::Context::from_parameters(params)?
                .decoder()
                .audio()?;
            a_reenc = Some(AudioReenc::new(
                &mut octx,
                decoder,
                audio_encode_codec(container),
                global_header,
                in_tb,
                spec.start_secs,
                spec.end_secs,
            )?);
        }
    }

    octx.write_header()?;
    if let Some(ar) = a_reenc.as_mut() {
        ar.out_tb = octx.stream(ar.out_index).unwrap().time_base();
    }

    if clip {
        seek_to(&mut ictx, spec.start_secs)?;
    }
    // Audio packets can lag the matching video in the interleave, so a clip keeps
    // reading audio past the video's end (until this PTS) instead of stopping the
    // moment video finishes, which would clip the audio tail short.
    let a_end_ts = (spec.end_secs / f64::from(a_in_tb)).round() as i64;
    // Same backstop for video: if frame PTS are missing so `process` never
    // reports the window end, the packet timeline still bounds the read instead
    // of decoding every remaining packet to EOF.
    let v_end_ts = (spec.end_secs / f64::from(v_in_tb)).round() as i64;
    let mut v_done = v_reenc.is_none();
    let mut a_done = a_reenc.is_none();

    let mut packet = ffmpeg::Packet::empty();
    while packet.read(&mut ictx).is_ok() {
        check_cancel(cancel)?;
        let idx = packet.stream();
        if idx == v_in_index {
            if let Some(vr) = v_reenc.as_mut() {
                if !v_done && vr.process(&packet, &mut octx)? {
                    v_done = true;
                }
                if clip && packet.pts().is_some_and(|p| p > v_end_ts) {
                    v_done = true;
                }
            } else if let Some((in_tb, out_index)) = v_copy {
                let out_tb = octx.stream(out_index).unwrap().time_base();
                packet.rescale_ts(in_tb, out_tb);
                packet.set_stream(out_index);
                packet.write_interleaved(&mut octx)?;
            }
        } else if idx == a_in_index {
            // Re-encode feeds every post-seek packet (`atrim` trims the window to
            // the sample); the copy path is whole-file only, so no trimming.
            if let Some(ar) = a_reenc.as_mut() {
                if clip && packet.pts().unwrap_or(0) > a_end_ts {
                    a_done = true;
                } else if !a_done {
                    ar.process(&packet, &mut octx)?;
                }
            } else if let Some(out_index) = a_copy {
                let out_tb = octx.stream(out_index).unwrap().time_base();
                packet.rescale_ts(a_in_tb, out_tb);
                packet.set_stream(out_index);
                packet.write_interleaved(&mut octx)?;
            }
        }
        if clip && v_done && a_done {
            break;
        }
    }

    if let Some(vr) = v_reenc.as_mut() {
        vr.flush(&mut octx)?;
    }
    if let Some(ar) = a_reenc.as_mut() {
        ar.flush(&mut octx)?;
    }
    octx.write_trailer()?;
    Ok(())
}

/// Drain ready packets from a video encoder and write them to the output.
fn write_encoded(
    encoder: &mut ffmpeg::encoder::Video,
    octx: &mut ffmpeg::format::context::Output,
    out_index: usize,
    enc_tb: ffmpeg::Rational,
) -> Result<()> {
    let out_tb = octx.stream(out_index).unwrap().time_base();
    let mut packet = ffmpeg::Packet::empty();
    while encoder.receive_packet(&mut packet).is_ok() {
        packet.set_stream(out_index);
        packet.rescale_ts(enc_tb, out_tb);
        packet.write_interleaved(octx)?;
    }
    Ok(())
}

/// Shift a copied packet's timestamps so the clip starts at zero.
fn rebase(packet: &mut ffmpeg::Packet, offset: i64) {
    packet.set_pts(packet.pts().map(|v| v - offset));
    packet.set_dts(packet.dts().map(|v| v - offset));
}

fn seek_to(ictx: &mut ffmpeg::format::context::Input, secs: f64) -> Result<()> {
    let ts = (secs * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
    ictx.seek(ts, ..ts)?;
    Ok(())
}
