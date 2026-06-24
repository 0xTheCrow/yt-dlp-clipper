use std::{env, fs, path::PathBuf};

fn main() {
    if env::var("CARGO_FEATURE_BUNDLE_TOOLS").is_ok() {
        let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
        embed("BUNDLE_YTDLP", "BUNDLED_YTDLP_PATH", "bundled-ytdlp", &out);
        embed("BUNDLE_FFMPEG_CLI", "BUNDLED_FFMPEG_CLI_PATH", "bundled-ffmpeg-cli", &out);

        // Emit link flags for the codec libraries statically compiled into FFmpeg
        // and the Windows system libs they depend on. The ffmpeg-sys-the-third build
        // script (FFMPEG_DIR path) only emits the libav* link flags; their transitive
        // codec deps and Windows system libs must be listed here.
        //
        // Check CARGO_CFG_TARGET_OS (the compile *target*), not cfg!(windows) (the
        // build *host*), so this also fires when cross-compiling from Linux.
        let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
        if target_os == "windows" {
            println!("cargo:rerun-if-env-changed=CODEC_LIB_DIR");
            let codec_lib_dir = env::var("CODEC_LIB_DIR").unwrap_or_else(|_| {
                panic!("CODEC_LIB_DIR must be set when building bundle-tools for Windows")
            });
            println!("cargo:rustc-link-search=native={codec_lib_dir}");
            for lib in ["x264", "vpx", "opus", "mp3lame"] {
                println!("cargo:rustc-link-lib=static={lib}");
            }
            // Windows system libs that FFmpeg depends on.
            for lib in ["bcrypt", "ws2_32", "secur32", "ole32", "user32"] {
                println!("cargo:rustc-link-lib={lib}");
            }
            // Ubuntu's mingw-w64 gcc defaults _FORTIFY_SOURCE on, so the codec and
            // libav* objects reference fortify/stack-protector symbols (__memcpy_chk,
            // __stack_chk_fail) from libssp, which the gcc driver does not auto-link
            // on MinGW. libssp.a lives in the gcc sysroot, which rustc's static-lib
            // search doesn't know; ask the cross-compiler for its directory. Emit it
            // last so it resolves those references from every preceding archive.
            if let Some(dir) = mingw_libssp_dir() {
                println!("cargo:rustc-link-search=native={dir}");
            }
            println!("cargo:rustc-link-lib=static=ssp");
        }
    }
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
