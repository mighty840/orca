//! Self-update: download latest orca binary from GitHub releases.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::VERSION;

const RELEASES_URL: &str = "https://api.github.com/repos/mighty840/orca/releases/latest";
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

/// Find the newest release, including prereleases if the current version is a prerelease.
async fn find_newest_release(client: &reqwest::Client) -> Result<GithubRelease> {
    let current = current_version();
    let is_prerelease = current.contains("rc");

    // Always try stable latest first
    if let Ok(resp) = client.get(RELEASES_URL).send().await
        && let Ok(release) = resp.json::<GithubRelease>().await
    {
        let ver = release.tag_name.trim_start_matches('v');
        if is_newer(ver, current) {
            return Ok(release);
        }
    }

    // If running a prerelease, also scan recent releases for newer RCs
    if is_prerelease
        && let Ok(resp) = client.get(ALL_RELEASES_URL).send().await
        && let Ok(releases) = resp.json::<Vec<GithubRelease>>().await
    {
        let mut best: Option<GithubRelease> = None;
        for rel in releases {
            let ver = rel.tag_name.trim_start_matches('v');
            if !is_newer(ver, current) {
                continue;
            }
            if rel.assets.iter().any(|a| a.name == ASSET_NAME) {
                match &best {
                    None => best = Some(rel),
                    Some(prev) => {
                        let prev_ver = prev.tag_name.trim_start_matches('v');
                        if is_newer(ver, prev_ver) {
                            best = Some(rel);
                        }
                    }
                }
            }
        }
        if let Some(release) = best {
            return Ok(release);
        }
    }

    anyhow::bail!("no newer release found (current: {current})")
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
