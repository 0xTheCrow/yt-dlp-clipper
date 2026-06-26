use std::{env, fs, path::PathBuf};

fn main() {
    if env::var("CARGO_FEATURE_BUNDLE_TOOLS").is_ok() {
        let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
        embed("BUNDLE_YTDLP", "BUNDLED_YTDLP_PATH", "bundled-ytdlp", &out);
        embed("BUNDLE_FFMPEG_CLI", "BUNDLED_FFMPEG_CLI_PATH", "bundled-ffmpeg-cli", &out);
        link_media_foundation();
    }
}

/// vcpkg's static FFmpeg includes the MediaFoundation encoder (`mfenc.o`), whose COM
/// interface GUIDs (`IID_IMFTransform`, `IID_ICodecAPI`, ...) and platform functions
/// live in these Windows SDK import libs. ffmpeg-sys-the-third's vcpkg link step emits
/// the other system libs (ole32, secur32, ws2_32, bcrypt, user32) but not these.
fn link_media_foundation() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    for lib in ["mfplat", "mfuuid", "strmiids"] {
        println!("cargo:rustc-link-lib={lib}");
    }
}

fn embed(src_env: &str, rustc_env: &str, dest_name: &str, out: &std::path::Path) {
    println!("cargo:rerun-if-env-changed={src_env}");
    let src = env::var(src_env).unwrap_or_else(|_| {
        panic!("{src_env} must be set when building with the bundle-tools feature")
    });
    println!("cargo:rerun-if-changed={src}");
    let dst = out.join(dest_name);
    fs::copy(&src, &dst).unwrap_or_else(|e| panic!("copying {src}: {e}"));
    println!("cargo:rustc-env={rustc_env}={}", dst.display());
}
