//! Backup execution for the WS client: spawn the agent backup subprocess.

use tokio::sync::mpsc;
use tracing::{error, info};

use orca_core::ws_types::AgentMessage;

/// Run a backup on this agent node using the config sent by master.
/// Passes backup targets as JSON via env var so the CLI subprocess can use them.
pub(super) async fn run_agent_backup(
    node_id: u64,
    config: orca_core::backup::BackupConfig,
    service_hooks: std::collections::HashMap<String, String>,
    out_tx: mpsc::Sender<AgentMessage>,
) {
    let config_json = match serde_json::to_string(&config) {
        Ok(j) => j,
        Err(e) => {
            error!("WS: failed to serialize backup config: {e}");
            let _ = out_tx
                .send(AgentMessage::BackupResult {
                    node_id,
                    success: false,
                    message: format!("config serialization failed: {e}"),
                })
                .await;
            return;
        }
    };

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            let _ = out_tx
                .send(AgentMessage::BackupResult {
                    node_id,
                    success: false,
                    message: format!("cannot resolve binary: {e}"),
                })
                .await;
            return;
        }
    };

    // Cache the config so `orca backup all` run manually on this node picks
    // up S3 targets without needing the env var.
    cache_backup_config(&config_json);

    let hooks_json = serde_json::to_string(&service_hooks).unwrap_or_default();
    match tokio::process::Command::new(&exe)
        .args(["backup", "all"])
        .env("ORCA_BACKUP_CONFIG_JSON", &config_json)
        .env("ORCA_SERVICE_HOOKS_JSON", &hooks_json)
        .output()
        .await
    {
        Ok(out) if out.status.success() => {
            let msg = String::from_utf8_lossy(&out.stdout).trim().to_string();
            info!("WS: agent backup complete: {msg}");
            let _ = out_tx
                .send(AgentMessage::BackupResult {
                    node_id,
                    success: true,
                    message: msg,
                })
                .await;
        }
        Ok(out) => {
            let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
            error!("WS: agent backup failed: {msg}");
            let _ = out_tx
                .send(AgentMessage::BackupResult {
                    node_id,
                    success: false,
                    message: msg,
                })
                .await;
        }
        Err(e) => {
            let _ = out_tx
                .send(AgentMessage::BackupResult {
                    node_id,
                    success: false,
                    message: format!("spawn failed: {e}"),
                })
                .await;
        }
    }
}

fn cache_backup_config(json: &str) {
    let Some(home) = dirs_next::home_dir() else {
        return;
    };
    if let Err(e) = cache_backup_config_at(&home.join(".orca/backup_config.json"), json) {
        tracing::warn!("Failed to cache backup config: {e}");
    }
}

/// Write the cached backup config owner-only (#205): it carries the
/// object-storage access key and secret key for every S3 target.
fn cache_backup_config_at(path: &std::path::Path, json: &str) -> std::io::Result<()> {
    orca_core::fsutil::write_private(path, json.as_bytes())
}

#[cfg(test)]
mod tests {
    /// `run_agent_backup` config JSON must round-trip so the subprocess sees
    /// the correct targets.
    #[test]
    fn backup_config_json_roundtrip() {
        use orca_core::backup::{BackupConfig, BackupTarget};
        let cfg = BackupConfig {
            age_recipients: Vec::new(),
            bind_mount_max_mb: 512,
            schedule: Some("0 0 2 * * *".into()),
            retention_days: 14,
            targets: vec![BackupTarget::Local {
                path: "/data/backups".into(),
            }],
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: BackupConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.retention_days, 14);
        match &back.targets[0] {
            BackupTarget::Local { path } => assert_eq!(path, "/data/backups"),
            _ => panic!("expected Local target"),
        }
    }

    /// BackupResult with success=false and an error message serializes and
    /// deserializes correctly so master can log the failure.
    #[test]
    fn backup_result_failure_roundtrip() {
        use orca_core::ws_types::AgentMessage;
        let msg = AgentMessage::BackupResult {
            node_id: 5,
            success: false,
            message: "spawn failed: No such file".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: AgentMessage = serde_json::from_str(&json).unwrap();
        match back {
            AgentMessage::BackupResult {
                success,
                message,
                node_id,
            } => {
                assert_eq!(node_id, 5);
                assert!(!success);
                assert!(message.contains("spawn failed"));
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn the_cached_config_is_owner_only() {
        // It holds S3 credentials; it used to be written with default mode.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".orca/backup_config.json");
        super::cache_backup_config_at(&path, r#"{"secret_key":"s"}"#).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"secret_key":"s"}"#
        );
    }
}
