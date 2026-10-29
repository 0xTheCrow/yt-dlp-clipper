use yt_dlp_clipper::ytdlp;

/// The download form: the URL being fetched, what yt-dlp reported about it, and
/// the choices applied to the next download.
pub(crate) struct Download {
    pub(crate) url: String,
    pub(crate) info: Option<ytdlp::VideoInfo>,
    /// The cookies the fetch of `info` needed, which the download must reuse.
    pub(crate) sign_in_cookies: Option<ytdlp::CookieSource>,
    /// Download resolution cap; `None` means "Best" (the source's tallest).
    pub(crate) selected_height: Option<u32>,
    pub(crate) want_video: bool,
    pub(crate) want_audio: bool,
}

impl Default for Download {
    fn default() -> Self {
        Self {
            url: String::new(),
            info: None,
            sign_in_cookies: None,
            selected_height: None,
            want_video: true,
            want_audio: true,
        }
    }
}
