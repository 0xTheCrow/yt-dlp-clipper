use yt_dlp_clipper::export;

/// Format seconds as `M:SS.mmm`.
pub(crate) fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0);
    let minutes = (s / 60.0) as u64;
    let rem = s - (minutes as f64) * 60.0;
    format!("{minutes}:{rem:06.3}")
}

/// Dropdown label for an audio export format.
pub(crate) fn audio_format_label(format: export::AudioFormat) -> &'static str {
    match format {
        export::AudioFormat::Mp3 => "MP3",
        export::AudioFormat::Aac => "AAC (.m4a)",
        export::AudioFormat::Original => "Original (lossless)",
    }
}

/// Compact duration as `M:SS`.
pub(crate) fn fmt_duration(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Make a string safe to use as a filename across platforms.
pub(crate) fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .take(100)
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    if trimmed.is_empty() {
        "video".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Format a byte count as MB/GB (decimal units).
pub(crate) fn fmt_size(bytes: u64) -> String {
    const MB: f64 = 1_000_000.0;
    const GB: f64 = 1_000_000_000.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else {
        format!("{:.1} MB", b / MB)
    }
}
