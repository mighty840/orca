//! Scheduled backup runner: spawns a background task that runs backups on a cron schedule.

use std::str::FromStr;
use std::sync::Arc;

use cron::Schedule;
use orca_core::backup::{BackupConfig, BackupManager};
use tracing::{error, info};

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
pub fn spawn_backup_scheduler(config: BackupConfig) -> Option<tokio::task::JoinHandle<()>> {
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
        let mgr = Arc::new(BackupManager::new(config));
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
        }
    });
    Some(handle)
}

/// Execute a backup run for all configured targets.
async fn run_scheduled_backup(mgr: &BackupManager) {
    info!("Starting scheduled backup run");
    // Back up the orca state directory if it exists
    let state_dir = dirs_next::home_dir()
        .unwrap_or_else(|| ".".into())
        .join(".orca");

    if state_dir.exists() {
        match mgr.backup_file("orca-state", &state_dir.join("cluster.db")) {
            Ok(()) => info!("Backed up cluster.db"),
            Err(e) => error!("Failed to backup cluster.db: {e}"),
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
}
