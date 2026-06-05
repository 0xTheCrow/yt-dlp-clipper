//! Audio playback via cpal. The playback position (`clock_secs`) is used as the
//! master clock that video frames sync to.
//!
//! A worker thread decodes the audio stream, resamples it to the device's
//! format, and pushes interleaved f32 samples into a ring buffer. The cpal
//! output callback drains that buffer and counts how many samples have actually
//! reached the device, which is what makes the clock accurate.

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ffmpeg_the_third as ffmpeg;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Roughly how much audio to keep buffered ahead, in seconds.
const BUFFER_SECS: usize = 1;

pub struct AudioPlayer {
    _stream: cpal::Stream,
    stop: Arc<AtomicBool>,
    /// Interleaved f32 samples that have been sent to the device.
    consumed: Arc<AtomicU64>,
    /// Output gain in `0.0..=1.0`, stored as f32 bits for lock-free updates.
    volume: Arc<AtomicU32>,
    start_secs: f64,
    sample_rate: u32,
    channels: usize,
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl AudioPlayer {
    /// Begin playing the file's audio from `start_secs` at `volume` (0.0..=1.0).
    /// Errors if there's no output device, no audio stream, or the device isn't
    /// f32 — callers should treat that as "play video without sound".
    pub fn start(path: &str, start_secs: f64, volume: f32) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("no audio output device"))?;
        let supported = device.default_output_config()?;
        if supported.sample_format() != cpal::SampleFormat::F32 {
            return Err(anyhow!("audio device is not f32"));
        }
        let sample_rate = supported.sample_rate().0;
        let channels = supported.channels() as usize;
        let config = supported.config();

        let ring: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
        let consumed = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let volume = Arc::new(AtomicU32::new(volume.clamp(0.0, 1.0).to_bits()));

        let cb_ring = ring.clone();
        let cb_consumed = consumed.clone();
        let cb_volume = volume.clone();
        let stream = device.build_output_stream(
            &config,
            move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let gain = f32::from_bits(cb_volume.load(Ordering::Relaxed));
                let mut buf = cb_ring.lock().unwrap();
                let mut filled = 0u64;
                for slot in out.iter_mut() {
                    match buf.pop_front() {
                        Some(sample) => {
                            *slot = sample * gain;
                            filled += 1;
                        }
                        None => *slot = 0.0, // underrun: silence
                    }
                }
                cb_consumed.fetch_add(filled, Ordering::Relaxed);
            },
            move |err| eprintln!("audio stream error: {err}"),
            None,
        )?;
        stream.play()?;

        let path = path.to_owned();
        let decode_ring = ring.clone();
        let decode_stop = stop.clone();
        std::thread::spawn(move || {
            if let Err(e) = decode_audio(
                &path,
                start_secs,
                sample_rate,
                channels,
                decode_ring,
                decode_stop,
            ) {
                eprintln!("audio decode stopped: {e}");
            }
        });

        Ok(Self {
            _stream: stream,
            stop,
            consumed,
            volume,
            start_secs,
            sample_rate,
            channels,
        })
    }

    /// Current playback position in seconds.
    pub fn clock_secs(&self) -> f64 {
        let consumed = self.consumed.load(Ordering::Relaxed) as f64;
        let frames = consumed / self.channels as f64;
        self.start_secs + frames / self.sample_rate as f64
    }

    /// Update output gain live (0.0..=1.0).
    pub fn set_volume(&self, volume: f32) {
        self.volume
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
}

fn decode_audio(
    path: &str,
    start_secs: f64,
    out_rate: u32,
    out_channels: usize,
    ring: Arc<Mutex<VecDeque<f32>>>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    ffmpeg::init()?;
    let mut ictx = ffmpeg::format::input(&path)?;
    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .ok_or_else(|| anyhow!("no audio stream"))?;
    let index = stream.index();
    let time_base = f64::from(stream.time_base());

    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?
        .decoder()
        .audio()?;

    let out_layout = ffmpeg::ChannelLayout::default(out_channels as i32);
    let mut resampler = ffmpeg::software::resampling::context::Context::get(
        decoder.format(),
        decoder.channel_layout(),
        decoder.rate(),
        ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
        out_layout,
        out_rate,
    )?;

    let seek_ts = (start_secs * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
    let _ = ictx.seek(seek_ts, ..seek_ts);
    decoder.flush();

    let max_buffered = out_rate as usize * out_channels * BUFFER_SECS;
    // A seek lands on a packet boundary at or before `start_secs`, so the first
    // decoded samples precede the in-point. Drop them (in output frames) so the
    // preview starts on the same sample the export does, keeping audio/video and
    // the clock honest. Set on the first decoded frame, whose PTS fixes the lead.
    let mut skip_frames: Option<i64> = None;
    let mut packet = ffmpeg::Packet::empty();
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if ring.lock().unwrap().len() > max_buffered {
            std::thread::sleep(std::time::Duration::from_millis(5));
            continue;
        }
        match packet.read(&mut ictx) {
            Ok(()) => {
                if packet.stream() != index {
                    continue;
                }
                if decoder.send_packet(&packet).is_err() {
                    continue;
                }
                let mut frame = ffmpeg::frame::Audio::empty();
                while decoder.receive_frame(&mut frame).is_ok() {
                    let skip = skip_frames.get_or_insert_with(|| {
                        let frame_secs = frame.pts().unwrap_or(0) as f64 * time_base;
                        ((start_secs - frame_secs).max(0.0) * out_rate as f64).round() as i64
                    });
                    let mut out = ffmpeg::frame::Audio::empty();
                    if resampler.run(&frame, &mut out).is_err() {
                        continue;
                    }
                    let count = out.samples() * out_channels;
                    let bytes = out.data(0);
                    if count == 0 || count * std::mem::size_of::<f32>() > bytes.len() {
                        continue;
                    }
                    // Resampled to packed f32, so plane 0 is the interleaved data.
                    let samples =
                        unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f32, count) };
                    let offset = if *skip > 0 {
                        let drop_frames = (*skip).min(out.samples() as i64);
                        *skip -= drop_frames;
                        drop_frames as usize * out_channels
                    } else {
                        0
                    };
                    ring.lock().unwrap().extend(samples[offset..].iter().copied());
                }
            }
            Err(_) => break, // end of stream
        }
    }
    Ok(())
}
