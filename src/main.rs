mod app;
mod binaries;
mod cache;
mod decoder_thread;
mod format;
mod keybinds;
mod theme;
mod widgets;

use ffmpeg_the_third as ffmpeg;
use std::path::PathBuf;
use yt_dlp_clipper::ytdlp;

use app::App;
use binaries::{resolve_ffmpeg, resolve_qjs, resolve_ytdlp};

/// On-disk identifier for the app-data dir (settings + download cache). Kept
/// generic and stable so renaming the app doesn't orphan persisted state.
pub(crate) const STORAGE_APP_ID: &str = "yt-dlp-clipper";

/// User-facing name, matching the window title, the desktop entry, and the
/// `.app` bundle.
const APP_DISPLAY_NAME: &str = "Cooper Clipper";

/// Runs the non-GUI startup path and exits, so a downloaded build can prove it
/// loads its libraries on this machine before it replaces the installed one.
const SELF_TEST_FLAG: &str = "--self-test";

/// Exercise the startup work that depends on the host: the tool binaries the
/// caller has already resolved, and linking against libav*. A non-zero exit
/// marks the build unusable here.
fn self_test() -> eframe::Result<()> {
    ffmpeg::init().map_err(|e| eframe::Error::AppCreation(Box::new(e)))?;
    println!("yt-dlp-clipper {} self-test ok", env!("CARGO_PKG_VERSION"));
    Ok(())
}

fn main() -> eframe::Result<()> {
    // Resolve the tool binaries to absolute paths up front (seeding yt-dlp into a
    // writable managed copy) so we never invoke a bare name the OS could resolve
    // to a planted binary, and yt-dlp merges with exactly the ffmpeg we shipped.
    if let Some(ytdlp_bin) = resolve_ytdlp() {
        ytdlp::set_binary(ytdlp_bin);
    }
    if let Some(ffmpeg_bin) = resolve_ffmpeg() {
        ytdlp::set_ffmpeg(ffmpeg_bin);
    }
    if let Some(qjs_bin) = resolve_qjs() {
        ytdlp::set_js_runtime(qjs_bin);
    }

    let mut cli_path = None;
    for arg in std::env::args().skip(1) {
        if arg == SELF_TEST_FLAG {
            return self_test();
        }
        cli_path.get_or_insert(arg);
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_DISPLAY_NAME)
            .with_inner_size([960.0, 720.0])
            .with_min_inner_size([800.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        STORAGE_APP_ID,
        options,
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(App::new(cc, cli_path.map(PathBuf::from))))
        }),
    )
}
