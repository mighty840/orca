//! Self-update: download latest orca binary from GitHub releases.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::VERSION;

const RELEASES_URL: &str = "https://api.github.com/repos/mighty840/orca/releases/latest";
const ASSET_NAME: &str = "orca-linux-x86_64";

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

/// Handle the `orca update` command.
pub async fn handle_update() -> Result<()> {
    println!("Checking for updates...");

    let client = reqwest::Client::builder().user_agent("orca-cli").build()?;

    let release: GithubRelease = client
        .get(RELEASES_URL)
        .send()
        .await
        .context("failed to fetch latest release")?
        .json()
        .await
        .context("failed to parse release JSON")?;

    let latest = release.tag_name.trim_start_matches('v');
    let current = current_version();

    if !is_newer(latest, current) {
        println!("Already on latest version ({current}).");
        return Ok(());
    }

    let asset = release
        .assets
        .iter()
        .find(|a| a.name == ASSET_NAME)
        .context("release asset not found")?;

    println!("Downloading {ASSET_NAME} v{latest}...");
    let bytes = client
        .get(&asset.browser_download_url)
        .send()
        .await
        .context("download failed")?
        .bytes()
        .await?;

    let current_exe = std::env::current_exe().context("cannot determine current binary path")?;
    let tmp_path = current_exe.with_extension("tmp");

    std::fs::write(&tmp_path, &bytes).context("failed to write temp file")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&tmp_path, perms)?;
    }

    std::fs::rename(&tmp_path, &current_exe).context("failed to replace binary")?;

    println!("Updated to v{latest}. Restart orca to apply.");
    Ok(())
}

/// Extract the semver portion from VERSION (e.g. "0.1.0-rc.3-abc123" -> "0.1.0-rc.3").
fn current_version() -> &'static str {
    // VERSION format: "{cargo_version}-{commit_hash}"
    // We want everything before the last dash-hexstring
    match VERSION.rfind('-') {
        Some(pos) => &VERSION[..pos],
        None => VERSION,
    }
}

/// Simple semver comparison: returns true if `latest` is newer than `current`.
fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.split(|c: char| !c.is_ascii_digit())
            .filter(|p| !p.is_empty())
            .filter_map(|p| p.parse().ok())
            .collect()
    };
    let l = parse(latest);
    let c = parse(current);
    l > c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_version_detected() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
    }

    #[test]
    fn same_version_not_newer() {
        assert!(!is_newer("0.1.0", "0.1.0"));
    }

    #[test]
    fn older_version_not_newer() {
        assert!(!is_newer("0.1.0", "0.2.0"));
    }

    #[test]
    fn rc_versions_compared() {
        assert!(is_newer("0.1.0-rc.4", "0.1.0-rc.3"));
        assert!(!is_newer("0.1.0-rc.2", "0.1.0-rc.3"));
    }

    #[test]
    fn test_version_newer_detected() {
        // Full release is newer than release candidate
        assert!(is_newer("0.2.0", "0.1.0-rc.4"));
    }

    #[test]
    fn test_version_same_not_newer() {
        assert!(!is_newer("0.1.0-rc.4", "0.1.0-rc.4"));
    }

    #[test]
    fn test_version_older_not_newer() {
        assert!(!is_newer("0.1.0-rc.3", "0.1.0-rc.4"));
    }

    #[test]
    fn test_parse_github_release_tag() {
        // Simulates what handle_update does: trim leading 'v'
        let tag = "v0.1.0-rc.4";
        let version = tag.trim_start_matches('v');
        assert_eq!(version, "0.1.0-rc.4");
    }
}
