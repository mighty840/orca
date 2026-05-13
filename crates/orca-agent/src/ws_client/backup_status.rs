//! Backup status reporter: respond to `MasterMessage::BackupStatusRequest` by
//! enumerating this node's local `~/.orca/backups/` and sending the result
//! back as `AgentMessage::BackupStatusReport`.

use tokio::sync::mpsc;
use tracing::warn;

use orca_core::backup::enumerate_local_backups;
use orca_core::ws_types::{AgentMessage, BackupStatusReportData};

/// Enumerate local snapshots and send the report. Spawned as a task by the
/// dispatch loop so it doesn't block heartbeat or other traffic.
pub(super) async fn send_backup_status(
    request_id: String,
    node_id: u64,
    out_tx: mpsc::Sender<AgentMessage>,
) {
    let snapshots = match dirs_next::home_dir() {
        Some(home) => enumerate_local_backups(&home),
        None => {
            warn!("WS: cannot enumerate backups — no home directory");
            Vec::new()
        }
    };
    let _ = out_tx
        .send(AgentMessage::BackupStatusReport {
            request_id,
            data: BackupStatusReportData {
                node_id,
                hostname: node_hostname(),
                snapshots,
            },
        })
        .await;
}

/// Resolve a human-readable host name. Falls back to environment then to a
/// placeholder so the dashboard never shows an empty cell.
fn node_hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    /// Even when the local backup dir is missing, the agent must reply with a
    /// well-formed empty report so the master's collector loop doesn't time
    /// out waiting on a node that simply hasn't run any backups yet.
    #[tokio::test]
    async fn send_backup_status_replies_even_with_no_backups() {
        let (tx, mut rx) = mpsc::channel::<AgentMessage>(4);
        send_backup_status("req-1".into(), 42, tx).await;
        let got = rx.try_recv().expect("expected a report");
        match got {
            AgentMessage::BackupStatusReport { request_id, data } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(data.node_id, 42);
                // We can't assert snapshots is empty (CI runner could have
                // ~/.orca/backups), but the message must exist.
                let _ = data.snapshots;
            }
            _ => panic!("expected BackupStatusReport"),
        }
    }
}
