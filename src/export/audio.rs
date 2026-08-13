//! Audio re-encoding: opening a target encoder, the abuffer -> atrim ->
//! abuffersink graph that cuts the window to the exact sample, and the
//! decode -> filter -> encode pipeline for one stream.

use anyhow::{anyhow, Result};
use ffmpeg_the_third as ffmpeg;
use ffmpeg::channel_layout::ChannelLayout;

use super::copy::write_packet;

/// Bitrate for re-encoded audio (MP3/AAC), in bits per second.
const AUDIO_BITRATE: usize = 192_000;

/// Add an audio output stream encoding to `codec_id`, picking a sample rate,
/// channel layout, and sample format the encoder supports (the filter graph
/// resamples to whatever rate is chosen).
pub(super) fn open_audio_encoder(
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
pub(super) fn audio_filter(
    decoder: &ffmpeg::codec::decoder::Audio,
    encoder: &ffmpeg::codec::encoder::Audio,
    trim: Option<(i64, i64)>,
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
    let chain = match trim {
        Some((start_sample, end_sample)) => {
            format!("atrim=start_sample={start_sample}:end_sample={end_sample}")
        }
        None => "anull".to_string(),
    };
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
pub(super) fn drain_filter(
    filter: &mut ffmpeg::filter::Graph,
    encoder: &mut ffmpeg::codec::encoder::Audio,
    octx: &mut ffmpeg::format::context::Output,
    out_index: usize,
    out_samples: &mut i64,
) -> Result<()> {
    let mut frame = ffmpeg::frame::Audio::empty();
    while filter.get("out").unwrap().sink().frame(&mut frame).is_ok() {
        frame.set_pts(Some(*out_samples));
        *out_samples += frame.samples() as i64;
        encoder.send_frame(&frame)?;
        write_audio_packets(encoder, octx, out_index)?;
    }
    Ok(())
}

/// Drain ready packets from an audio encoder and write them to the output.
pub(super) fn write_audio_packets(
    encoder: &mut ffmpeg::codec::encoder::Audio,
    octx: &mut ffmpeg::format::context::Output,
    out_index: usize,
) -> Result<()> {
    let enc_tb = ffmpeg::Rational(1, encoder.rate() as i32);
    let mut packet = ffmpeg::Packet::empty();
    while encoder.receive_packet(&mut packet).is_ok() {
        write_packet(&mut packet, enc_tb, out_index, octx)?;
    }
    Ok(())
}

/// An audio stream being re-encoded (to AAC/MP3/Opus), trimmed to the exact
/// `[start_secs, end_secs)` sample window. The filter is built lazily on the
/// first decoded frame, once its PTS reveals where the post-seek run begins.
pub(super) struct AudioReenc {
    pub(super) decoder: ffmpeg::codec::decoder::Audio,
    pub(super) encoder: ffmpeg::codec::encoder::Audio,
    pub(super) filter: Option<ffmpeg::filter::Graph>,
    pub(super) out_index: usize,
    /// Source stream time base, for mapping a frame's PTS to a sample index.
    pub(super) in_tb: ffmpeg::Rational,
    /// Decoder sample rate, for converting the window seconds to samples.
    pub(super) rate: f64,
    /// Window bounds as absolute sample indices in the source. `None` re-encodes
    /// the stream end to end, which is what saving the full file wants.
    pub(super) window_samples: Option<(i64, i64)>,
    pub(super) in_samples: i64,
    pub(super) out_samples: i64,
}

impl AudioReenc {
    pub(super) fn new(
        octx: &mut ffmpeg::format::context::Output,
        decoder: ffmpeg::codec::decoder::Audio,
        codec_id: ffmpeg::codec::Id,
        global_header: bool,
        in_tb: ffmpeg::Rational,
        window_secs: Option<(f64, f64)>,
    ) -> Result<Self> {
        let rate = decoder.rate() as f64;
        let (out_index, encoder) = open_audio_encoder(octx, &decoder, codec_id, global_header)?;
        Ok(AudioReenc {
            decoder,
            encoder,
            filter: None,
            out_index,
            in_tb,
            rate,
            window_samples: window_secs.map(|(start_secs, end_secs)| {
                ((start_secs * rate).round() as i64, (end_secs * rate).round() as i64)
            }),
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
                let trim = self
                    .window_samples
                    .map(|(start, end)| ((start - base).max(0), end - base));
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
                &mut self.out_samples,
            )?;
        }
        Ok(())
    }

    pub(super) fn process(
        &mut self,
        packet: &ffmpeg::Packet,
        octx: &mut ffmpeg::format::context::Output,
    ) -> Result<()> {
        self.decoder.send_packet(packet)?;
        self.drain_decoder(octx)
    }

    pub(super) fn flush(&mut self, octx: &mut ffmpeg::format::context::Output) -> Result<()> {
        self.decoder.send_eof()?;
        self.drain_decoder(octx)?;
        if let Some(filter) = self.filter.as_mut() {
            filter.get("in").unwrap().source().flush()?;
            drain_filter(
                filter,
                &mut self.encoder,
                octx,
                self.out_index,
                &mut self.out_samples,
            )?;
        }
        self.encoder.send_eof()?;
        write_audio_packets(&mut self.encoder, octx, self.out_index)?;
        Ok(())
    }
}
