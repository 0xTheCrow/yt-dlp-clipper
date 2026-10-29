//! Checks GitHub for a release newer than the running build, and (Phase 3) the
//! primitives to download, verify, and swap it into place: `stage` owns the
//! on-disk commit-protocol state, `probe` runs the pre-swap self-test, and
//! `swap` holds the per-platform in-place replacement. Orchestrating those
//! (where the state file lives, when to call each) is the bin crate's job —
//! this module only knows the app's storage layout through paths callers pass
//! in, the same way `ytdlp::download` takes its destination `dir` rather than
//! computing one.

pub mod probe;
pub mod stage;
pub mod swap;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::time::Duration;
use ureq::Agent;

/// The app's own release feed. `/latest` never returns drafts or prereleases, so
/// a release whose assets are still uploading is never offered.
const RELEASES_API_URL: &str =
    "https://api.github.com/repos/0xTheCrow/yt-dlp-clipper/releases/latest";
/// GitHub answers API requests that omit a User-Agent with 403.
const UPDATE_USER_AGENT: &str = concat!("yt-dlp-clipper/", env!("CARGO_PKG_VERSION"));
const UPDATE_REQUEST_TIMEOUT_SECS: u64 = 15;

/// How often an enabled auto-check is allowed to run, so a persistent failure
/// doesn't retry every launch and a healthy check doesn't spam GitHub.
pub const UPDATE_CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;

/// A published release newer than the running build, with the asset matching
/// this platform already resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableRelease {
    pub version: String,
    pub notes: String,
    pub page_url: String,
    /// Direct download URL for the asset matching `platform_asset_suffix()`.
    pub asset_url: String,
}

#[derive(Deserialize)]
struct ReleaseResponse {
    tag_name: String,
    #[serde(default)]
    body: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// The release-asset filename suffix this build's platform publishes, matching
/// the packaging scripts: `make-appimage.sh`'s stable `yt-dlp-clipper-x86_64.AppImage`,
/// `release.yml`'s `yt-dlp-clipper-<ver>-win64.exe`, and `make-macos.sh`'s
/// `yt-dlp-clipper-<ver>-macos-{arm64,x86_64}.zip` (a `.zip` of the bare `.app`,
/// not the `.dmg` — a `.dmg` is a distribution container, not a swap target).
#[cfg(target_os = "linux")]
fn platform_asset_suffix() -> &'static str {
    "x86_64.AppImage"
}
#[cfg(target_os = "windows")]
fn platform_asset_suffix() -> &'static str {
    "win64.exe"
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn platform_asset_suffix() -> &'static str {
    "macos-arm64.zip"
}
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
fn platform_asset_suffix() -> &'static str {
    "macos-x86_64.zip"
}

/// The download URL of the first asset whose name ends with `suffix`.
fn select_asset(assets: &[Asset], suffix: &str) -> Option<String> {
    assets.iter().find(|a| a.name.ends_with(suffix)).map(|a| a.browser_download_url.clone())
}

/// Ask GitHub for the newest published release; `Ok(None)` means the running
/// build is already current. Errors (rather than silently offering nothing)
/// when a newer release exists but publishes no asset for this platform, since
/// that's a release-process mistake worth surfacing, not a normal "no update".
pub fn check() -> Result<Option<AvailableRelease>> {
    let agent: Agent = Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(UPDATE_REQUEST_TIMEOUT_SECS)))
        .build()
        .into();
    let body = agent
        .get(RELEASES_API_URL)
        .header("User-Agent", UPDATE_USER_AGENT)
        .call()
        .context("asking GitHub for the latest release")?
        .body_mut()
        .read_to_string()
        .context("reading GitHub's release response")?;
    let release: ReleaseResponse =
        serde_json::from_str(&body).context("parsing GitHub's release response")?;
    let Some(mut available) = newer_release(&release, env!("CARGO_PKG_VERSION")) else {
        return Ok(None);
    };
    let suffix = platform_asset_suffix();
    let Some(asset_url) = select_asset(&release.assets, suffix) else {
        bail!("release {} has no {suffix} asset for this platform", available.version);
    };
    available.asset_url = asset_url;
    Ok(Some(available))
}

/// The release, when its tag names a version above `running`. `asset_url` is
/// left empty here — `check()` fills it in, since asset selection needs the
/// full asset list this pure function doesn't take.
fn newer_release(release: &ReleaseResponse, running: &str) -> Option<AvailableRelease> {
    let latest = parse_version(&release.tag_name)?;
    let current = parse_version(running)?;
    (latest > current).then(|| AvailableRelease {
        version: release.tag_name.trim().trim_start_matches('v').to_string(),
        notes: release.body.clone(),
        page_url: release.html_url.clone(),
        asset_url: String::new(),
    })
}

/// `major.minor.patch` from a release tag, ignoring a leading `v` and any
/// prerelease or build suffix. Absent components read as zero.
fn parse_version(tag: &str) -> Option<(u32, u32, u32)> {
    let core = tag.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::{
        check, newer_release, parse_version, platform_asset_suffix, select_asset, Asset,
        ReleaseResponse,
    };

    /// Reaches the real GitHub endpoint; run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn queries_the_live_release_feed() {
        check().expect("the live release feed answers");
    }

    fn release(tag: &str) -> ReleaseResponse {
        ReleaseResponse {
            tag_name: tag.into(),
            body: String::new(),
            html_url: String::new(),
            assets: Vec::new(),
        }
    }

    fn asset(name: &str) -> Asset {
        Asset { name: name.into(), browser_download_url: format!("https://example.com/{name}") }
    }

    #[test]
    fn parses_tags_with_and_without_prefix() {
        assert_eq!(parse_version("v0.1.4"), Some((0, 1, 4)));
        assert_eq!(parse_version("0.1.4"), Some((0, 1, 4)));
        assert_eq!(parse_version(" v0.1.4 "), Some((0, 1, 4)));
    }

    #[test]
    fn absent_components_read_as_zero() {
        assert_eq!(parse_version("v1"), Some((1, 0, 0)));
        assert_eq!(parse_version("v1.2"), Some((1, 2, 0)));
    }

    #[test]
    fn ignores_prerelease_and_build_suffixes() {
        assert_eq!(parse_version("v1.2.3-rc1"), Some((1, 2, 3)));
        assert_eq!(parse_version("v1.2.3+build7"), Some((1, 2, 3)));
    }

    #[test]
    fn rejects_unparseable_tags() {
        assert_eq!(parse_version("nightly"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn orders_components_numerically_not_lexically() {
        assert!(parse_version("v0.1.10") > parse_version("v0.1.9"));
        assert!(parse_version("v0.10.0") > parse_version("v0.9.9"));
    }

    #[test]
    fn offers_only_releases_above_the_running_build() {
        assert!(newer_release(&release("v0.1.5"), "0.1.4").is_some());
        assert!(newer_release(&release("v0.1.4"), "0.1.4").is_none());
        assert!(newer_release(&release("v0.1.3"), "0.1.4").is_none());
    }

    #[test]
    fn unparseable_tag_offers_nothing() {
        assert!(newer_release(&release("nightly"), "0.1.4").is_none());
    }

    #[test]
    fn reported_version_drops_the_tag_prefix() {
        let offered = newer_release(&release("v0.2.0"), "0.1.4").expect("newer");
        assert_eq!(offered.version, "0.2.0");
    }

    #[test]
    fn parses_a_github_release_payload() {
        let payload = r#"{
            "tag_name": "v0.2.0",
            "name": "v0.2.0",
            "body": "Adds self-update.",
            "html_url": "https://github.com/0xTheCrow/yt-dlp-clipper/releases/tag/v0.2.0",
            "draft": false,
            "prerelease": false,
            "assets": []
        }"#;
        let release: ReleaseResponse = serde_json::from_str(payload).expect("parses");
        let offered = newer_release(&release, "0.1.4").expect("newer");
        assert_eq!(offered.version, "0.2.0");
        assert_eq!(offered.notes, "Adds self-update.");
        assert!(offered.page_url.ends_with("/tag/v0.2.0"));
    }

    #[test]
    fn select_asset_matches_by_suffix() {
        let assets =
            [asset("yt-dlp-clipper-x86_64.AppImage"), asset("yt-dlp-clipper-0.2.0-win64.exe")];
        assert_eq!(
            select_asset(&assets, "x86_64.AppImage"),
            Some("https://example.com/yt-dlp-clipper-x86_64.AppImage".into())
        );
        assert_eq!(
            select_asset(&assets, "win64.exe"),
            Some("https://example.com/yt-dlp-clipper-0.2.0-win64.exe".into())
        );
    }

    #[test]
    fn select_asset_finds_nothing_for_an_unmatched_platform() {
        let assets = [asset("yt-dlp-clipper-0.2.0-win64.exe")];
        assert_eq!(select_asset(&assets, "x86_64.AppImage"), None);
    }

    #[test]
    fn check_errors_rather_than_panics_without_a_matching_asset() {
        // `check()` itself needs a live HTTP call, so this exercises the same
        // decision inline: a newer release with no matching asset must not
        // silently offer `None` (that reads as "you're current") nor panic.
        let release = release("v99.0.0");
        let available = newer_release(&release, env!("CARGO_PKG_VERSION")).expect("newer");
        assert!(select_asset(&release.assets, platform_asset_suffix()).is_none());
        assert_eq!(available.asset_url, "");
    }
}
