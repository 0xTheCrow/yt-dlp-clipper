use std::path::PathBuf;
use yt_dlp_clipper::export::Mode;

/// A preview navigation: what the panels ask the decoder to do.
pub(crate) enum Nav {
    Back,
    Forward,
    Seek { secs: f64, released: bool },
}

/// Work a panel asks for but can't do itself, because it spans state the panel
/// doesn't borrow. Collected while the panels draw and applied at the end of
/// the frame, so nothing mutates state a widget still holds.
pub(crate) enum Action {
    /// Clear the open video, then fetch metadata for the URL in the field.
    Fetch,
    OpenFile(PathBuf),
    Nav(Nav),
    TogglePlay,
    PlaySelection,
    Export { mode: Mode, extension: &'static str },
}
