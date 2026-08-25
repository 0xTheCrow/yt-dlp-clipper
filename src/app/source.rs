use crate::decoder_thread::DecoderHandle;
use std::path::PathBuf;

/// Metadata from the decoder thread for a newly opened source, flattening the
/// video and audio-only open events into the one shape the UI applies.
#[derive(Clone, Copy)]
pub(crate) struct OpenedSource {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) duration_secs: f64,
    pub(crate) fps: f64,
    pub(crate) has_video: bool,
}

/// The open video (or audio-only) file: its decoder thread, where it came from,
/// and the most recently decoded frame.
#[derive(Default)]
pub(crate) struct Source {
    pub(crate) decoder: Option<DecoderHandle>,
    pub(crate) video_path: Option<PathBuf>,
    /// Suggested export name: yt-dlp title when downloaded, else the file stem.
    pub(crate) video_title: Option<String>,
    pub(crate) frame_tex: Option<egui::TextureHandle>,
}

impl Source {
    /// Open `path` on a fresh decoder thread, naming the export after its stem.
    pub(crate) fn open(&mut self, path: PathBuf) {
        self.video_title = path.file_stem().map(|s| s.to_string_lossy().into_owned());
        self.decoder = Some(DecoderHandle::spawn(path.to_string_lossy().into_owned()));
        self.video_path = Some(path);
    }

    pub(crate) fn clear(&mut self) {
        self.decoder = None;
        self.video_path = None;
        self.video_title = None;
        self.frame_tex = None;
    }

    /// True once the decoder has reported its metadata and a first frame.
    pub(crate) fn is_ready(&self) -> bool {
        self.decoder.as_ref().is_some_and(|dec| dec.ready)
    }

    /// Current playhead position, once the decoder is ready to report one.
    pub(crate) fn ready_position_secs(&self) -> Option<f64> {
        self.decoder.as_ref().filter(|dec| dec.ready).map(|dec| dec.current_secs)
    }

    /// True while the open source has a video stream. False for an audio-only
    /// file, which still gets a timeline and an audio export but no frames.
    pub(crate) fn has_video(&self) -> bool {
        self.decoder.as_ref().is_some_and(|dec| dec.has_video)
    }

    /// Upload a freshly decoded frame to the preview texture.
    pub(crate) fn set_frame(&mut self, ctx: &egui::Context, img: egui::ColorImage) {
        self.frame_tex = Some(ctx.load_texture("frame", img, egui::TextureOptions::LINEAR));
    }
}
