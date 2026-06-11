use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use yt_dlp_clipper::decoder::Decoder;

/// Pixel width thumbnails decode to.
const THUMB_DECODE_WIDTH: usize = 320;

/// One cached video shown in the cache browser.
pub(crate) struct CacheEntry {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) duration: Option<f64>,
    pub(crate) tex: Option<egui::TextureHandle>,
}

/// What the thumbnail worker sends back per file.
pub(crate) struct CacheThumb {
    pub(crate) path: PathBuf,
    pub(crate) image: egui::ColorImage,
    pub(crate) duration_secs: f64,
}

pub(crate) fn is_video_file(path: &Path) -> bool {
    const EXTS: [&str; 7] = ["mp4", "webm", "mkv", "mov", "m4v", "avi", "ts"];
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .map_or(false, |e| EXTS.contains(&e.as_str()))
}

/// Nearest-neighbour downscale to `max_w` wide, preserving aspect ratio.
fn downscale_thumb(img: &egui::ColorImage, max_w: usize) -> egui::ColorImage {
    let [w, h] = img.size;
    if w == 0 || h == 0 || w <= max_w {
        return img.clone();
    }
    let new_w = max_w;
    let new_h = (h * max_w / w).max(1);
    let mut pixels = Vec::with_capacity(new_w * new_h);
    for y in 0..new_h {
        let sy = y * h / new_h;
        for x in 0..new_w {
            pixels.push(img.pixels[sy * w + x * w / new_w]);
        }
    }
    egui::ColorImage {
        size: [new_w, new_h],
        pixels,
    }
}

/// Decode one representative frame per file (10% in, to skip black intros) and
/// stream downscaled thumbnails back.
pub(crate) fn cache_thumbnails(paths: Vec<PathBuf>, tx: Sender<CacheThumb>) {
    for path in paths {
        let Ok(mut dec) = Decoder::open(&path.to_string_lossy()) else {
            continue;
        };
        let duration_secs = dec.duration_secs();
        let frame = dec.seek_secs(duration_secs * 0.1).or_else(|| dec.step_forward());
        if let Some(image) = frame {
            let thumb = CacheThumb {
                path,
                image: downscale_thumb(&image, THUMB_DECODE_WIDTH),
                duration_secs,
            };
            if tx.send(thumb).is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downscale_preserves_aspect_and_is_in_bounds() {
        let src = egui::ColorImage {
            size: [640, 480],
            pixels: vec![egui::Color32::WHITE; 640 * 480],
        };
        let out = downscale_thumb(&src, 320);
        assert_eq!(out.size, [320, 240]);
        assert_eq!(out.pixels.len(), 320 * 240);
    }

    #[test]
    fn downscale_skips_when_already_small() {
        let src = egui::ColorImage {
            size: [100, 50],
            pixels: vec![egui::Color32::BLACK; 100 * 50],
        };
        assert_eq!(downscale_thumb(&src, 320).size, [100, 50]);
    }
}
