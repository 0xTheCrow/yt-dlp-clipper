use yt_dlp_clipper::export::{AudioFormat, VideoFormat};

/// What the save buttons produce: the target containers and an optional
/// downscale. `Settings::compatibility_mode` is the persisted counterpart.
pub(crate) struct ExportOptions {
    /// Target format for "Save audio only".
    pub(crate) audio_format: AudioFormat,
    /// Target container for "Save clip" / "Save full video".
    pub(crate) video_format: VideoFormat,
    /// Downscale height for saved video; `None` keeps the source resolution.
    pub(crate) scale_height: Option<u32>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            audio_format: AudioFormat::Mp3,
            video_format: VideoFormat::Mp4,
            scale_height: None,
        }
    }
}
