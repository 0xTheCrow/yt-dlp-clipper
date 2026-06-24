use std::{env, fs, path::PathBuf};

fn main() {
    if env::var("CARGO_FEATURE_BUNDLE_TOOLS").is_ok() {
        let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
        embed("BUNDLE_YTDLP", "BUNDLED_YTDLP_PATH", "bundled-ytdlp", &out);
        embed("BUNDLE_FFMPEG_CLI", "BUNDLED_FFMPEG_CLI_PATH", "bundled-ffmpeg-cli", &out);

        // Emit link flags for the codec and system libraries the statically linked
        // FFmpeg depends on. The ffmpeg-sys-the-third build script (FFMPEG_DIR path)
        // emits only the libav* link flags; their transitive deps must be added here.
        // Read the exact, correctly-ordered set from FFmpeg's own pkg-config files
        // rather than hand-maintaining it.
        //
        // Check CARGO_CFG_TARGET_OS (the compile *target*), not cfg!(windows) (the
        // build *host*), so this also fires when cross-compiling from Linux.
        let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
        if target_os == "windows" {
            println!("cargo:rerun-if-env-changed=CODEC_LIB_DIR");
            let codec_lib_dir = env::var("CODEC_LIB_DIR").unwrap_or_else(|_| {
                panic!("CODEC_LIB_DIR must be set when building bundle-tools for Windows")
            });
            let deps = ffmpeg_private_libs(&format!("{codec_lib_dir}/pkgconfig"));
            println!("cargo:rustc-link-search=native={codec_lib_dir}");
            // libssp.a lives in the gcc sysroot, not on rustc's search path; ask the
            // cross-compiler for its directory so -l:libssp.a below resolves.
            if let Some(dir) = mingw_libssp_dir() {
                println!("cargo:rustc-link-search=native={dir}");
            }
            // These define symbols referenced by the upstream libav* objects (codec
            // functions, Windows system APIs), plus libssp for the fortify/stack-
            // protector symbols (__memcpy_chk, __stack_chk_*) that Ubuntu's mingw gcc
            // emits by default and does not auto-link. rustc places a crate's own
            // `rustc-link-lib` entries before the upstream rlibs, where single-pass
            // MinGW ld can't satisfy these backward references; emit them as link args
            // (which land after the upstream objects), wrapped in a group so order
            // among them is irrelevant.
            println!("cargo:rustc-link-arg=-Wl,--start-group");
            for lib in &deps {
                println!("cargo:rustc-link-arg=-l{lib}");
            }
            println!("cargo:rustc-link-arg=-l:libssp.a");
            println!("cargo:rustc-link-arg=-Wl,--end-group");
        }
    }
}

/// Libraries FFmpeg links against beyond the libav* themselves (codecs such as
/// x264/vpx/opus/mp3lame and Windows system libs), read from the installed
/// pkg-config files via `--static`. The libav* libraries are emitted by
/// ffmpeg-sys (FFMPEG_DIR), so they're filtered out to avoid relisting them.
/// Order is preserved as pkg-config reports it; duplicates are dropped.
fn ffmpeg_private_libs(pkgconfig_dir: &str) -> Vec<String> {
    const FFMPEG_LIBS: [&str; 7] = [
        "avcodec", "avformat", "avfilter", "avdevice", "avutil", "swscale", "swresample",
    ];
    let out = std::process::Command::new("pkg-config")
        .env("PKG_CONFIG_LIBDIR", pkgconfig_dir)
        .env("PKG_CONFIG_PATH", "")
        .args(["--libs", "--static"])
        .args([
            "libavcodec",
            "libavformat",
            "libavfilter",
            "libswscale",
            "libswresample",
            "libavutil",
        ])
        .output()
        .expect("running pkg-config for FFmpeg libs");
    assert!(
        out.status.success(),
        "pkg-config failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).expect("pkg-config output is utf8");
    let mut libs = Vec::new();
    for name in text.split_whitespace().filter_map(|t| t.strip_prefix("-l")) {
        if !FFMPEG_LIBS.contains(&name) && !libs.iter().any(|l| l == name) {
            libs.push(name.to_string());
        }
    }
    libs
}

/// Directory holding `libssp.a` in the mingw-w64 cross toolchain, queried from
/// the C compiler (`-print-file-name` returns the bare name if it can't locate
/// the lib, so only accept an absolute path).
fn mingw_libssp_dir() -> Option<String> {
    let cc = env::var("CC_x86_64_pc_windows_gnu")
        .unwrap_or_else(|_| "x86_64-w64-mingw32-gcc".into());
    let out = std::process::Command::new(cc)
        .arg("-print-file-name=libssp.a")
        .output()
        .ok()?;
    let path = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
    path.is_absolute()
        .then(|| path.parent().map(|p| p.display().to_string()))
        .flatten()
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
