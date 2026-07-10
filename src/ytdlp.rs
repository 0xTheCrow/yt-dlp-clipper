//! Thin wrapper around the `yt-dlp` binary (spawned as a subprocess).

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Absolute path to the yt-dlp binary, set once at startup. Resolving it up front
/// (and always invoking the full path, never the bare name "yt-dlp") stops the
/// OS from re-searching PATH/CWD and running a planted binary.
static YTDLP_BIN: OnceLock<PathBuf> = OnceLock::new();
/// Absolute path to the ffmpeg binary yt-dlp merges with, passed via
/// `--ffmpeg-location` so yt-dlp doesn't search PATH/CWD for one of its own.
static FFMPEG_BIN: OnceLock<PathBuf> = OnceLock::new();

/// Point every yt-dlp invocation at this exact binary. Call once at startup.
pub fn set_binary(path: PathBuf) {
    let _ = YTDLP_BIN.set(path);
}

/// Tell yt-dlp which ffmpeg to merge with. Call once at startup.
pub fn set_ffmpeg(path: PathBuf) {
    let _ = FFMPEG_BIN.set(path);
}

/// The resolved yt-dlp path, falling back to the bare name when unset (dev/test
/// runs with yt-dlp on PATH).
fn binary() -> PathBuf {
    YTDLP_BIN.get().cloned().unwrap_or_else(|| PathBuf::from("yt-dlp"))
}

/// One downloadable format as reported by `yt-dlp -J`.
#[derive(Debug, Clone, Deserialize)]
pub struct Format {
    pub format_id: String,
    #[serde(default)]
    pub ext: String,
    #[serde(default)]
    pub resolution: Option<String>,
    #[serde(default)]
    pub vcodec: Option<String>,
    #[serde(default)]
    pub acodec: Option<String>,
    #[serde(default)]
    pub filesize: Option<u64>,
    #[serde(default)]
    pub filesize_approx: Option<u64>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub abr: Option<f64>,
}

impl Format {
    pub fn has_video(&self) -> bool {
        self.vcodec.as_deref().map_or(false, |c| c != "none")
    }

    pub fn has_audio(&self) -> bool {
        self.acodec.as_deref().map_or(false, |c| c != "none")
    }

    /// Exact size if known, otherwise yt-dlp's approximate estimate.
    pub fn size(&self) -> Option<u64> {
        self.filesize.or(self.filesize_approx)
    }

    pub fn label(&self) -> String {
        const BYTES_PER_MB: f64 = 1_000_000.0;
        let res = self.resolution.clone().unwrap_or_else(|| "—".into());
        let v = self.vcodec.as_deref().unwrap_or("none");
        let a = self.acodec.as_deref().unwrap_or("none");
        let size = match self.size() {
            Some(bytes) => format!("{:.1}MB", bytes as f64 / BYTES_PER_MB),
            None => "?".into(),
        };
        format!(
            "{:>6}  {:<5} {:<10} v:{} a:{} {}",
            self.format_id, self.ext, res, v, a, size
        )
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct VideoInfo {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub formats: Vec<Format>,
}

/// Tallest video format no taller than `max_height` (any height when `None`).
fn best_video(formats: &[Format], max_height: Option<u32>) -> Option<&Format> {
    formats
        .iter()
        .filter(|f| f.has_video())
        .filter(|f| max_height.map_or(true, |th| f.height.map_or(false, |h| h <= th)))
        .max_by_key(|f| f.height.unwrap_or(0))
}

/// Distinct video heights the source offers, tallest first. The download/save
/// resolution menus are built from this, so they never exceed what exists.
pub fn available_heights(info: &VideoInfo) -> Vec<u32> {
    let mut heights: Vec<u32> = info
        .formats
        .iter()
        .filter(|f| f.has_video())
        .filter_map(|f| f.height)
        .filter(|h| *h > 0)
        .collect();
    heights.sort_unstable_by(|a, b| b.cmp(a));
    heights.dedup();
    heights
}

/// yt-dlp `-f` selector for a download capped at `height` (best when `None`),
/// merging best audio into a video-only pick. `None` means "let yt-dlp decide".
pub fn resolution_selector(
    height: Option<u32>,
    want_video: bool,
    want_audio: bool,
) -> Option<String> {
    match (want_video, want_audio) {
        (true, true) => Some(match height {
            Some(h) => format!("bestvideo[height<={h}]+bestaudio/best[height<={h}]"),
            None => "bestvideo+bestaudio/best".into(),
        }),
        (true, false) => Some(match height {
            Some(h) => format!("bestvideo[height<={h}]"),
            None => "bestvideo".into(),
        }),
        (false, true) => Some("bestaudio".into()),
        (false, false) => None,
    }
}

fn best_audio(formats: &[Format]) -> Option<&Format> {
    formats
        .iter()
        .filter(|f| f.has_audio() && !f.has_video())
        .max_by(|a, b| {
            a.abr
                .unwrap_or(0.0)
                .total_cmp(&b.abr.unwrap_or(0.0))
        })
}

/// Estimate the download size for a resolution selection, mirroring what
/// `resolution_selector` would fetch: the tallest video within `height` plus
/// best audio, per the Video/Audio toggles. `None` if no size is known.
pub fn estimated_size(
    info: &VideoInfo,
    height: Option<u32>,
    want_video: bool,
    want_audio: bool,
) -> Option<u64> {
    let mut total = 0;
    let mut known = false;
    if want_video {
        if let Some(bytes) = best_video(&info.formats, height).and_then(Format::size) {
            total += bytes;
            known = true;
        }
    }
    if want_audio {
        if let Some(bytes) = best_audio(&info.formats).and_then(Format::size) {
            total += bytes;
            known = true;
        }
    }
    known.then_some(total)
}

/// True when yt-dlp's output looks like the failure is (likely) an outdated
/// binary — a site changed and the installed yt-dlp can't keep up. yt-dlp tells
/// users to run `yt-dlp -U` in these cases, so the GUI offers that action.
pub fn suggests_update(text: &str) -> bool {
    const MARKERS: [&str; 4] = ["yt-dlp -u", "please update", "outdated version", "latest version"];
    let lower = text.to_lowercase();
    MARKERS.iter().any(|m| lower.contains(m))
}

/// The installed yt-dlp's version string (`yt-dlp --version`), trimmed.
pub fn version() -> Result<String> {
    let output = Command::new(binary())
        .arg("--version")
        .output()
        .context("failed to run yt-dlp — is it installed and on PATH?")?;
    if !output.status.success() {
        bail!("yt-dlp --version failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Self-update via `yt-dlp -U`, returning yt-dlp's report. `-U` is a no-op when
/// already current, so the click itself doubles as the up-to-date check.
pub fn update() -> Result<String> {
    let output = Command::new(binary())
        .arg("-U")
        .output()
        .context("failed to run yt-dlp — is it installed and on PATH?")?;
    let report = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() {
        bail!("yt-dlp -U failed: {}{}", report, String::from_utf8_lossy(&output.stderr));
    }
    Ok(report)
}

/// Fetch metadata (`yt-dlp -J <url>`) without downloading anything.
pub fn fetch_info(url: &str) -> Result<VideoInfo> {
    let output = Command::new(binary())
        // `--` ends option parsing so a URL starting with `-` can't be taken as
        // a yt-dlp flag (e.g. `--exec`, `--config-location`).
        .args(["-J", "--no-playlist", "--", url])
        .output()
        .context("failed to run yt-dlp — is it installed and on PATH?")?;

    if !output.status.success() {
        bail!(
            "yt-dlp exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let info: VideoInfo =
        serde_json::from_slice(&output.stdout).context("could not parse yt-dlp JSON output")?;
    Ok(info)
}

/// Download into `dir`, returning the produced file path. A `selector` of
/// `None` lets yt-dlp use its default (best video + audio merged); otherwise
/// it is passed verbatim to `-f`. `on_progress` receives `(downloaded, total)`
/// byte counts for the file currently being fetched.
pub fn download(
    url: &str,
    selector: Option<&str>,
    dir: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<PathBuf> {
    // Name files by title (yt-dlp sanitizes it); the id keeps them unique. The
    // real path is read back from `--print` below, so the name need not be
    // predictable.
    let template = dir.join("%(title)s [%(id)s].%(ext)s");
    let template = template.to_string_lossy().into_owned();

    // Prefixed progress + output lines keep parsing unambiguous on stdout.
    let progress_template =
        "PROGRESS %(progress.downloaded_bytes)s %(progress.total_bytes)s \
         %(progress.total_bytes_estimate)s";

    // `--print` silences progress unless `--progress` forces it (onto stdout).
    let mut args = vec!["--no-playlist", "--newline", "--progress"];
    if let Some(sel) = selector {
        args.extend(["-f", sel]);
    }
    // Pin the ffmpeg used for the video+audio merge to our resolved binary, so
    // yt-dlp can't pick up a planted `ffmpeg` from PATH/CWD.
    let ffmpeg_loc = FFMPEG_BIN.get().map(|p| p.to_string_lossy().into_owned());
    if let Some(loc) = &ffmpeg_loc {
        args.extend(["--ffmpeg-location", loc.as_str()]);
    }
    args.extend([
        "-o",
        &template,
        "--progress-template",
        progress_template,
        "--print",
        "after_move:OUTPUT %(filepath)s",
        // `--` ends option parsing so a URL starting with `-` can't be taken as a
        // yt-dlp flag (e.g. `--exec`, `--config-location`, `-o`).
        "--",
        url,
    ]);

    let mut child = Command::new(binary())
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to run yt-dlp — is it installed and on PATH?")?;

    let stdout = child.stdout.take().expect("piped stdout");
    let mut final_path = None;
    for line in BufReader::new(stdout).lines() {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("download canceled");
        }
        let line = line.unwrap_or_default();
        if let Some(rest) = line.strip_prefix("PROGRESS ") {
            let fields: Vec<&str> = rest.split_whitespace().collect();
            if let [downloaded, total, estimate] = fields[..] {
                if let Ok(done) = downloaded.parse::<u64>() {
                    let total = total.parse::<u64>().or_else(|_| estimate.parse::<u64>());
                    if let Ok(total) = total {
                        if total > 0 {
                            on_progress(done, total);
                        }
                    }
                }
            }
        } else if let Some(path) = line.strip_prefix("OUTPUT ") {
            final_path = Some(PathBuf::from(path.trim()));
        }
    }

    let status = child.wait().context("yt-dlp did not finish cleanly")?;
    if !status.success() {
        let mut err = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut err);
        }
        bail!("yt-dlp download failed: {err}");
    }

    final_path.context("yt-dlp did not report an output path")
}

#[cfg(test)]
mod tests {
    use super::suggests_update;

    #[test]
    fn flags_update_hints() {
        // Extractor breakage and the explicit `-U` nudge yt-dlp prints.
        assert!(suggests_update(
            "ERROR: [youtube] dQw4: Unable to extract. Confirm you are on the \
             latest version using yt-dlp -U"
        ));
        assert!(suggests_update("WARNING: You are using an outdated version; please update"));
    }

    #[test]
    fn ignores_unrelated_errors() {
        assert!(!suggests_update("ERROR: unable to open file: permission denied"));
        assert!(!suggests_update("ERROR: [generic] None: Requested format is not available"));
    }
}
