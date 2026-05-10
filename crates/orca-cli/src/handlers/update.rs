//! Self-update: download latest orca binary from GitHub releases.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::VERSION;

const ALL_RELEASES_URL: &str = "https://api.github.com/repos/mighty840/orca/releases?per_page=10";
const ASSET_NAME: &str = "orca-linux-x86_64";

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
    #[serde(default)]
    #[allow(dead_code)]
    prerelease: bool,
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

    // Try /releases/latest first (stable). If current version is a prerelease,
    // also check recent releases to find newer RCs.
    let release = find_newest_release(&client).await?;

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
        .context(format!(
            "binary asset '{ASSET_NAME}' not found in release {latest}"
        ))?;

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

    // Restore cap_net_bind_service so orca can still bind 80/443.
    restore_setcap(&current_exe);

    println!("Updated to v{latest}. Restart orca to apply.");
    Ok(())
}

/// Restore `cap_net_bind_service` on the binary after an update.
///
/// `mv`/`rename` across filesystems creates a new inode, which clears
/// the capability. This runs `sudo -n setcap` to restore it silently.
fn restore_setcap(exe: &std::path::Path) {
    let status = std::process::Command::new("sudo")
        .arg("-n")
        .arg("setcap")
        .arg("cap_net_bind_service=+ep")
        .arg(exe)
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("Restored cap_net_bind_service on {}", exe.display());
        }
        _ => {
            println!(
                "Could not restore cap_net_bind_service. Run manually:\n  \
                 sudo setcap 'cap_net_bind_service=+ep' {}",
                exe.display()
            );
        }
    }
}

/// Find the newest release. Always scans all releases (stable + prerelease)
/// so that RC builds are found even when the running binary reports a stable
/// version string (Cargo.toml version never carries an rc suffix).
async fn find_newest_release(client: &reqwest::Client) -> Result<GithubRelease> {
    let current = current_version();

    let resp = client
        .get(ALL_RELEASES_URL)
        .send()
        .await
        .context("failed to fetch releases")?;
    let releases: Vec<GithubRelease> = resp.json().await.context("failed to parse releases")?;

    let mut best: Option<GithubRelease> = None;
    for rel in releases {
        let ver = rel.tag_name.trim_start_matches('v');
        if !is_newer(ver, current) {
            continue;
        }
        if !rel.assets.iter().any(|a| a.name == ASSET_NAME) {
            continue;
        }
        let is_best = best
            .as_ref()
            .map(|b| is_newer(ver, b.tag_name.trim_start_matches('v')))
            .unwrap_or(true);
        if is_best {
            best = Some(rel);
        }
    }

    best.ok_or_else(|| anyhow::anyhow!("no newer release found (current: {current})"))
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

/// Split `"0.2.5-rc.7"` into `("0.2.5", Some("rc.7"))`.
fn split_prerelease(v: &str) -> (&str, Option<&str>) {
    match v.find('-') {
        Some(pos) => (&v[..pos], Some(&v[pos + 1..])),
        None => (v, None),
    }
}

/// Simple semver comparison: returns true if `latest` is newer than `current`.
///
/// Handles pre-release suffixes correctly: `0.2.5` > `0.2.5-rc.7` because a
/// stable release always supersedes any RC of the same base version.
fn is_newer(latest: &str, current: &str) -> bool {
    let parse_base =
        |s: &str| -> Vec<u64> { s.split('.').filter_map(|p| p.parse().ok()).collect() };

    let (l_base, l_pre) = split_prerelease(latest);
    let (c_base, c_pre) = split_prerelease(current);
    let l_nums = parse_base(l_base);
    let c_nums = parse_base(c_base);

    if l_nums != c_nums {
        return l_nums > c_nums;
    }
    // Same base version: stable beats any RC; higher RC beats lower RC.
    match (l_pre, c_pre) {
        (None, None) => false,
        (None, Some(_)) => true,
        (Some(_), None) => false,
        (Some(l), Some(c)) => {
            let rc_num = |s: &str| -> u64 {
                s.split('.')
                    .next_back()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(0)
            };
            rc_num(l) > rc_num(c)
        }
    }
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
    fn stable_beats_rc_of_same_version() {
        // The bug: 0.2.5 must be considered newer than 0.2.5-rc.7
        assert!(is_newer("0.2.5", "0.2.5-rc.7"));
        assert!(!is_newer("0.2.5-rc.7", "0.2.5"));
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
