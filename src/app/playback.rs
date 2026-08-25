use std::path::Path;
use yt_dlp_clipper::audio::AudioPlayer;

/// Playback clock and audio output. Video chases the master clock, which is the
/// audio device when one is open and wall time otherwise.
#[derive(Default)]
pub(crate) struct Playback {
    pub(crate) playing: bool,
    /// When set, playback stops once the master clock reaches this position;
    /// `None` plays to the end.
    pub(crate) play_until: Option<f64>,
    /// Audio output during playback; `None` means play video without sound.
    pub(crate) audio: Option<AudioPlayer>,
    /// Master-clock origin for video-only playback (no audio device/track):
    /// the egui time and video position captured when playback started.
    pub(crate) play_start_wall: f64,
    pub(crate) play_start_pos: f64,
    /// After releasing a timeline drag: `(gen, position)` of the seek we're
    /// waiting to land on. The playhead stays here and earlier decodes are
    /// dropped until the frame with this gen arrives.
    pub(crate) awaiting_release: Option<(u64, f64)>,
}

impl Playback {
    /// Stop playback and release the audio output.
    pub(crate) fn stop(&mut self) {
        self.playing = false;
        self.play_until = None;
        self.audio = None;
        self.awaiting_release = None;
    }

    /// Begin playing from `pos`, stopping at `until` if given. `source_path` is
    /// the file the audio track is read from; without one, video plays silently.
    pub(crate) fn start(
        &mut self,
        pos: f64,
        until: Option<f64>,
        now: f64,
        source_path: Option<&Path>,
        volume: f32,
    ) {
        self.awaiting_release = None;
        self.playing = true;
        self.play_until = until;
        self.play_start_wall = now;
        self.play_start_pos = pos;
        self.audio = source_path
            .and_then(|path| AudioPlayer::start(&path.to_string_lossy(), pos, volume).ok());
    }

    /// Playback position of the master clock: audio if present, else wall time.
    pub(crate) fn master_clock(&self, now: f64) -> f64 {
        match &self.audio {
            Some(audio) => audio.clock_secs(),
            None => self.play_start_pos + (now - self.play_start_wall),
        }
    }
}
