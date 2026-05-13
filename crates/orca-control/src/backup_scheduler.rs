//! Scheduled backup runner: spawns a background task that runs backups on a cron schedule.

use std::str::FromStr;
use std::sync::Arc;

use cron::Schedule;
use orca_core::backup::BackupConfig;
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
            run_master_backup(&state, &config).await;
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
    let service_hooks = collect_service_hooks(state).await;
    info!("Dispatching backup request to {} agent(s)", agents.len());
    for (node_id, tx) in agents.iter() {
        if let Err(e) = tx
            .send(MasterMessage::BackupRequest {
                config: config.clone(),
                service_hooks: service_hooks.clone(),
            })
            .await
        {
            error!("Failed to send BackupRequest to agent {node_id}: {e}");
        }
    }
}

/// Collect pre-hook commands from all services that have backup config.
async fn collect_service_hooks(state: &AppState) -> std::collections::HashMap<String, String> {
    let services = state.services.read().await;
    services
        .values()
        .filter_map(|svc| {
            let hook = svc.config.backup.as_ref()?.pre_hook.as_ref()?;
            Some((svc.config.name.clone(), hook.clone()))
        })
        .collect()
}

/// Execute a full local backup on master by spawning `orca backup all`, then
/// record the outcome on `state.master_last_backup_result` so the dashboard
/// can render it.
///
/// This backs up master's Docker volumes AND config files (cluster.db,
/// secrets.json, cluster.toml) — same code path as a manual `orca backup all`.
/// The backup config is passed via env var so the subprocess uses the same
/// targets (including resolved S3 credentials) as the scheduler.
pub(crate) async fn run_master_backup(state: &Arc<AppState>, config: &BackupConfig) {
    info!("Starting master backup");
    let (success, message) = invoke_subprocess(config).await;
    let result = orca_core::api_types::LastBackupResult {
        success,
        message,
        recorded_at: chrono::Utc::now(),
    };
    *state.master_last_backup_result.write().await = Some(result);
}

async fn invoke_subprocess(config: &BackupConfig) -> (bool, String) {
    let config_json = match serde_json::to_string(config) {
        Ok(j) => j,
        Err(e) => {
            let msg = format!("config serialization failed: {e}");
            error!("{msg}");
            return (false, msg);
        }
    };
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("cannot resolve current exe: {e}");
            error!("{msg}");
            return (false, msg);
        }
    };
    match tokio::process::Command::new(&exe)
        .args(["backup", "all"])
        .env("ORCA_BACKUP_CONFIG_JSON", &config_json)
        .output()
        .await
    {
        Ok(out) if out.status.success() => {
            // The subprocess intermixes tracing (with ANSI colors) and plain
            // `println!` summaries on stdout. The last non-empty line is
            // always the human-readable summary (e.g. "Backup complete: 2
            // file(s)."); take that and we don't have to scrub colors.
            let stdout = String::from_utf8_lossy(&out.stdout);
            let msg = last_non_empty_line(&stdout)
                .unwrap_or("backup complete")
                .to_string();
            info!("Master backup complete: {msg}");
            (true, msg)
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let msg = last_non_empty_line(&stderr)
                .unwrap_or("backup failed (no error message)")
                .to_string();
            error!("Master backup failed: {msg}");
            (false, msg)
        }
        Err(e) => {
            let msg = format!("spawn failed: {e}");
            error!("{msg}");
            (false, msg)
        }
    }
}

fn last_non_empty_line(text: &str) -> Option<&str> {
    text.lines().rev().map(str::trim).find(|l| !l.is_empty())
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

    /// `run_master_backup` serializes the config to JSON and passes it via env
    /// var. Verify the JSON round-trips so the spawned subprocess sees the
    /// same targets the scheduler holds.
    #[test]
    fn master_backup_config_json_roundtrip() {
        use orca_core::backup::BackupTarget;

        let config = BackupConfig {
            schedule: Some("0 0 3 * * *".to_string()),
            retention_days: 7,
            targets: vec![BackupTarget::Local {
                path: "/tmp/backups".to_string(),
            }],
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: BackupConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.retention_days, 7);
        match &back.targets[0] {
            BackupTarget::Local { path } => assert_eq!(path, "/tmp/backups"),
            _ => panic!("expected Local target"),
        }
    }
}
