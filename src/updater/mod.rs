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
use std::collections::HashMap;
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

/// The manifest each release publishes beside its builds, written by
/// `scripts/make-release-manifest.sh`.
const MANIFEST_ASSET_NAME: &str = "latest.json";

/// A published release newer than the running build, with this platform's
/// update payload already resolved from the release manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableRelease {
    pub version: String,
    pub notes: String,
    pub page_url: String,
    pub asset_url: String,
    /// Lowercase hex SHA-256 the downloaded payload must match.
    pub sha256: String,
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

#[derive(Deserialize)]
struct ReleaseManifest {
    platforms: HashMap<String, ManifestEntry>,
}

#[derive(Deserialize)]
struct ManifestEntry {
    file: String,
    sha256: String,
}

/// This build's key in the release manifest: `linux-x86_64`, `windows-x86_64`,
/// `macos-aarch64`, and so on.
fn platform_key() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

fn asset_url(assets: &[Asset], name: &str) -> Option<String> {
    assets.iter().find(|asset| asset.name == name).map(|asset| asset.browser_download_url.clone())
}

/// The download URL and SHA-256 of `platform`'s update payload.
fn resolve_payload(
    manifest: &ReleaseManifest,
    assets: &[Asset],
    platform: &str,
) -> Result<(String, String)> {
    let entry = manifest
        .platforms
        .get(platform)
        .with_context(|| format!("the release publishes no build for {platform}"))?;
    let url = asset_url(assets, &entry.file).with_context(|| {
        format!("the release manifest names {}, which the release doesn't include", entry.file)
    })?;
    Ok((url, entry.sha256.clone()))
}

fn fetch_text(agent: &Agent, url: &str) -> Result<String> {
    agent
        .get(url)
        .header("User-Agent", UPDATE_USER_AGENT)
        .call()
        .with_context(|| format!("requesting {url}"))?
        .body_mut()
        .read_to_string()
        .with_context(|| format!("reading {url}"))
}

/// Ask GitHub for the newest published release; `Ok(None)` means the running
/// build is already current. Errors (rather than silently offering nothing)
/// when a newer release has no manifest or no build for this platform, since
/// that's a release-process mistake worth surfacing, not a normal "no update".
pub fn check() -> Result<Option<AvailableRelease>> {
    check_from(RELEASES_API_URL, env!("CARGO_PKG_VERSION"))
}

/// `check()` against any GitHub-shaped release feed at `releases_url`, as if the
/// running build were `running_version`.
pub fn check_from(releases_url: &str, running_version: &str) -> Result<Option<AvailableRelease>> {
    let agent: Agent = Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(UPDATE_REQUEST_TIMEOUT_SECS)))
        .build()
        .into();
    let release: ReleaseResponse = serde_json::from_str(&fetch_text(&agent, releases_url)?)
        .context("parsing GitHub's release response")?;
    let Some(mut available) = newer_release(&release, running_version) else {
        return Ok(None);
    };
    let Some(manifest_url) = asset_url(&release.assets, MANIFEST_ASSET_NAME) else {
        bail!("release {} publishes no {MANIFEST_ASSET_NAME}", available.version);
    };
    let manifest: ReleaseManifest = serde_json::from_str(&fetch_text(&agent, &manifest_url)?)
        .context("parsing the release manifest")?;
    let (asset_url, sha256) = resolve_payload(&manifest, &release.assets, &platform_key())?;
    available.asset_url = asset_url;
    available.sha256 = sha256;
    Ok(Some(available))
}

/// The release, when its tag names a version above `running`. `asset_url` and
/// `sha256` are left empty here; `check()` fills them from the manifest.
fn newer_release(release: &ReleaseResponse, running: &str) -> Option<AvailableRelease> {
    let latest = parse_version(&release.tag_name)?;
    let current = parse_version(running)?;
    (latest > current).then(|| AvailableRelease {
        version: release.tag_name.trim().trim_start_matches('v').to_string(),
        notes: release.body.clone(),
        page_url: release.html_url.clone(),
        asset_url: String::new(),
        sha256: String::new(),
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
        check, newer_release, parse_version, resolve_payload, Asset, ManifestEntry,
        ReleaseManifest, ReleaseResponse,
    };
    use std::collections::HashMap;

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

    fn manifest_naming(platform: &str, file: &str) -> ReleaseManifest {
        let entry = ManifestEntry { file: file.into(), sha256: "0".repeat(64) };
        ReleaseManifest { platforms: HashMap::from([(platform.to_string(), entry)]) }
    }

    fn asset(name: &str) -> Asset {
        Asset { name: name.into(), browser_download_url: format!("https://example.com/{name}") }
    }

    #[test]
    fn resolving_errors_when_the_manifest_lacks_this_platform() {
        let manifest = manifest_naming("windows-x86_64", "cooper-clipper-win64.exe");
        let assets = [asset("cooper-clipper-win64.exe")];
        assert!(resolve_payload(&manifest, &assets, "linux-x86_64").is_err());
    }

    #[test]
    fn resolving_errors_when_the_named_file_is_not_in_the_release() {
        let manifest = manifest_naming("linux-x86_64", "cooper-clipper-x86_64.AppImage");
        let assets = [asset("cooper-clipper-win64.exe")];
        assert!(resolve_payload(&manifest, &assets, "linux-x86_64").is_err());
    }
}
