//! NetBird lifecycle management: install, connect, status, IP resolution.

use std::process::Command;

use anyhow::{Context, Result};
use tracing::{info, warn};

/// Manages the NetBird WireGuard mesh on this node.
///
/// Designed for self-hosted NetBird. Pass your management URL
/// (e.g., `https://netbird.example.com`). If not provided, uses
/// whatever NetBird has configured locally.
pub struct NetbirdManager {
    management_url: Option<String>,
}

impl NetbirdManager {
    /// Create a new manager. Pass the management URL for self-hosted NetBird,
    /// or `None` to use NetBird's local configuration.
    pub fn new(management_url: Option<String>) -> Self {
        Self { management_url }
    }

    /// Check if NetBird is installed.
    pub fn is_installed(&self) -> bool {
        Command::new("netbird").arg("version").output().is_ok()
    }

    /// Install NetBird if not present.
    pub fn install(&self) -> Result<()> {
        if self.is_installed() {
            info!("NetBird already installed");
            return Ok(());
        }

        info!("Installing NetBird...");
        let status = Command::new("sh")
            .arg("-c")
            .arg("curl -fsSL https://pkgs.netbird.io/install.sh | sh")
            .status()
            .context("failed to run NetBird installer")?;

        if !status.success() {
            anyhow::bail!("NetBird installation failed");
        }
        info!("NetBird installed");
        Ok(())
    }

    /// Connect to the mesh using a setup key.
    pub fn connect(&self, setup_key: &str) -> Result<()> {
        info!("Connecting to NetBird mesh...");

        let mut cmd = Command::new("netbird");
        cmd.args(["up", "--setup-key", setup_key]);

        if let Some(url) = &self.management_url {
            cmd.args(["--management-url", url]);
        }

        let output = cmd.output().context("failed to run netbird up")?;

        if output.status.success() {
            info!("Connected to NetBird mesh");
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("NetBird connect failed: {stderr}")
        }
    }

    /// Disconnect from the mesh.
    pub fn disconnect(&self) -> Result<()> {
        let output = Command::new("netbird")
            .arg("down")
            .output()
            .context("failed to run netbird down")?;

        if output.status.success() {
            info!("Disconnected from NetBird mesh");
        } else {
            warn!("NetBird disconnect may have failed");
        }
        Ok(())
    }

    /// Get this node's NetBird IP address.
    pub fn get_ip(&self) -> Result<Option<String>> {
        let output = Command::new("netbird")
            .args(["status", "--json"])
            .output()
            .context("failed to get netbird status")?;

        if !output.status.success() {
            return Ok(None);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        // Parse JSON output for the IP
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
            let ip = json
                .get("localPeerState")
                .and_then(|s| s.get("ip"))
                .and_then(|ip| ip.as_str())
                .map(|ip| ip.trim_end_matches("/32").to_string());
            Ok(ip)
        } else {
            // Fallback: parse text output
            for line in stdout.lines() {
                if (line.contains("NetBird IP:") || line.contains("IP:"))
                    && let Some(ip) = line.split_whitespace().last()
                {
                    return Ok(Some(ip.trim_end_matches("/32").to_string()));
                }
            }
            Ok(None)
        }
    }

    /// Check if NetBird is connected.
    pub fn is_connected(&self) -> bool {
        Command::new("netbird")
            .args(["status"])
            .output()
            .map(|o| {
                let stdout = String::from_utf8_lossy(&o.stdout);
                stdout.contains("Connected") || stdout.contains("connected")
            })
            .unwrap_or(false)
    }
}
