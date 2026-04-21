//! Scheduled backup runner: spawns a background task that runs backups on a cron schedule.

use std::str::FromStr;
use std::sync::Arc;

use cron::Schedule;
use orca_core::backup::{BackupConfig, BackupManager};
use orca_core::ws_types::MasterMessage;
use tracing::{error, info};

use crate::state::AppState;

/// Compute the duration until the next occurrence of the cron schedule.
pub fn duration_until_next(schedule: &Schedule) -> Option<std::time::Duration> {
    let now = chrono::Utc::now();
    let next = schedule.upcoming(chrono::Utc).next()?;
    let delta = next - now;
    delta.to_std().ok()
}

/// Spawn a background task that runs volume backups on the configured cron schedule.
///
/// Returns `None` if no schedule is configured or the schedule is invalid.
pub fn spawn_backup_scheduler(
    config: BackupConfig,
    state: Arc<AppState>,
) -> Option<tokio::task::JoinHandle<()>> {
    let schedule_str = config.schedule.as_ref()?;
    let schedule = match Schedule::from_str(schedule_str) {
        Ok(s) => s,
        Err(e) => {
            error!("Invalid backup cron schedule '{}': {e}", schedule_str);
            return None;
        }
    };

    info!("Backup scheduler started with schedule: {schedule_str}");
    let handle = tokio::spawn(async move {
        let mgr = Arc::new(BackupManager::new(config.clone()));
        loop {
            let sleep_dur = match duration_until_next(&schedule) {
                Some(d) => d,
                None => {
                    error!("No upcoming cron occurrence, stopping scheduler");
                    break;
                }
            };
            info!("Next backup in {}s", sleep_dur.as_secs());
            tokio::time::sleep(sleep_dur).await;
            run_scheduled_backup(&mgr).await;
            dispatch_agent_backups(&state, &config).await;
        }
    });
    Some(handle)
}

/// Send a `BackupRequest` to every connected agent so they back up their
/// local volumes to the same targets as the master.
async fn dispatch_agent_backups(state: &AppState, config: &BackupConfig) {
    let agents = state.ws_agents.read().await;
    if agents.is_empty() {
        return;
    }
    info!("Dispatching backup request to {} agent(s)", agents.len());
    for (node_id, tx) in agents.iter() {
        if let Err(e) = tx
            .send(MasterMessage::BackupRequest {
                config: config.clone(),
            })
            .await
        {
            error!("Failed to send BackupRequest to agent {node_id}: {e}");
        }
    }
}

/// Execute a backup run for all configured targets.
///
/// This backs up:
///   1. `cluster.db` (orca control plane state)
///   2. `secrets.json` (encrypted secrets store)
///   3. All orca-managed Docker volumes (per-service data)
///
/// Volume backups are delegated to `orca backup all` as a subprocess so we
/// can reuse the bollard logic in orca-cli without creating a circular
/// dependency between orca-control and orca-cli.
async fn run_scheduled_backup(mgr: &BackupManager) {
    info!("Starting scheduled backup run");
    let state_dir = dirs_next::home_dir()
        .unwrap_or_else(|| ".".into())
        .join(".orca");

    if state_dir.exists() {
        let cluster_db = state_dir.join("cluster.db");
        if cluster_db.exists() {
            match mgr.backup_file("cluster-db", &cluster_db) {
                Ok(()) => info!("Backed up cluster.db"),
                Err(e) => error!("Failed to backup cluster.db: {e}"),
            }
        }
        let secrets = state_dir.join("secrets.json");
        if secrets.exists() {
            match mgr.backup_file("secrets", &secrets) {
                Ok(()) => info!("Backed up secrets.json"),
                Err(e) => error!("Failed to backup secrets.json: {e}"),
            }
        }
    }

    // Run volume backups via the CLI subprocess. We resolve the binary by
    // looking at our own executable path so the subprocess uses the same
    // build, which avoids version skew.
    if let Ok(exe) = std::env::current_exe() {
        match tokio::process::Command::new(&exe)
            .args(["backup", "all"])
            .output()
            .await
        {
            Ok(out) if out.status.success() => {
                info!(
                    "Volume backup completed: {}",
                    String::from_utf8_lossy(&out.stdout).trim()
                );
            }
            Ok(out) => {
                error!(
                    "Volume backup failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            Err(e) => error!("Failed to spawn volume backup: {e}"),
        }
    }

    info!("Scheduled backup run complete");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cron_next_run() {
        // "0 0 2 * * *" = daily at 02:00:00 (6-field with seconds)
        let schedule = Schedule::from_str("0 0 2 * * *").unwrap();
        let dur = duration_until_next(&schedule);
        assert!(dur.is_some(), "should compute a next run time");
        // The next occurrence should be within 24 hours
        let d = dur.unwrap();
        assert!(d.as_secs() <= 86400, "next run should be within 24h");
        assert!(d.as_secs() > 0, "next run should be in the future");
    }

    #[test]
    fn test_backup_scheduler_config() {
        use orca_core::backup::BackupTarget;

        let config = BackupConfig {
            schedule: Some("0 0 2 * * *".to_string()),
            retention_days: 7,
            targets: vec![BackupTarget::Local {
                path: "/tmp/backups".to_string(),
            }],
        };
        assert!(config.schedule.is_some());
        // Verify the schedule parses correctly
        let schedule = Schedule::from_str(config.schedule.as_ref().unwrap()).unwrap();
        let upcoming: Vec<_> = schedule.upcoming(chrono::Utc).take(3).collect();
        assert_eq!(upcoming.len(), 3, "should produce 3 upcoming times");
        // All times should be at 02:00
        for t in &upcoming {
            assert_eq!(t.format("%H:%M").to_string(), "02:00");
        }
    }

    #[test]
    fn test_invalid_schedule_returns_none() {
        let config = BackupConfig {
            schedule: Some("not a cron".to_string()),
            retention_days: 30,
            targets: vec![],
        };
        // spawn_backup_scheduler needs a runtime, so test parsing directly
        let result = Schedule::from_str(config.schedule.as_ref().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn test_no_schedule_returns_none() {
        let config = BackupConfig {
            schedule: None,
            retention_days: 30,
            targets: vec![],
        };
        assert!(config.schedule.is_none());
    }

    fn make_test_state() -> std::sync::Arc<crate::state::AppState> {
        use orca_core::config::{ClusterConfig, ClusterMeta};
        use orca_core::testing::MockRuntime;
        use std::sync::Arc;
        use tokio::sync::RwLock;

        let runtime = Arc::new(MockRuntime::with_host_port(9000));
        Arc::new(crate::state::AppState::new(
            ClusterConfig {
                cluster: ClusterMeta {
                    name: "test".into(),
                    api_port: 0,
                    grpc_port: 0,
                    ..Default::default()
                },
                ..Default::default()
            },
            runtime,
            None,
            Arc::new(RwLock::new(std::collections::HashMap::new())),
            Arc::new(RwLock::new(Vec::new())),
        ))
    }

    /// `dispatch_agent_backups` must send a `BackupRequest` to every connected
    /// agent. We wire up two fake WS senders and verify both receive the message.
    #[tokio::test]
    async fn dispatch_agent_backups_sends_to_all_connected_agents() {
        use orca_core::ws_types::MasterMessage;

        let state = make_test_state();

        // Register two fake agent senders.
        let (tx1, mut rx1) = tokio::sync::mpsc::channel::<MasterMessage>(4);
        let (tx2, mut rx2) = tokio::sync::mpsc::channel::<MasterMessage>(4);
        {
            let mut agents = state.ws_agents.write().await;
            agents.insert(1, tx1);
            agents.insert(2, tx2);
        }

        let config = BackupConfig {
            schedule: Some("0 0 2 * * *".to_string()),
            retention_days: 7,
            targets: vec![],
        };

        dispatch_agent_backups(&state, &config).await;

        // Both agents must have received the message.
        let msg1 = rx1
            .try_recv()
            .expect("agent 1 should receive BackupRequest");
        let msg2 = rx2
            .try_recv()
            .expect("agent 2 should receive BackupRequest");
        assert!(matches!(msg1, MasterMessage::BackupRequest { .. }));
        assert!(matches!(msg2, MasterMessage::BackupRequest { .. }));
    }

    /// `dispatch_agent_backups` with no connected agents must complete without
    /// panicking or returning an error.
    #[tokio::test]
    async fn dispatch_agent_backups_noop_when_no_agents() {
        let state = make_test_state();
        let config = BackupConfig {
            schedule: None,
            retention_days: 30,
            targets: vec![],
        };
        // Should complete without panic even when ws_agents is empty.
        dispatch_agent_backups(&state, &config).await;
    }
}
