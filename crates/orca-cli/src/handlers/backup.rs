use crate::commands::BackupAction;
use orca_core::backup::{BackupConfig, BackupManager, BackupTarget};

use super::backup_report::BackupReport;
use super::volume_backup;

/// Run a backup command. Returns `false` when a backup ran and anything in it
/// failed, so `main` can exit non-zero (#197). The scheduler and agents judge
/// a run by that exit code.
pub async fn handle_backup(action: BackupAction) -> bool {
    // The two volume operations are async, so we just `.await` them on the
    // ambient `#[tokio::main]` runtime — creating a nested `Runtime::new()`
    // here would panic with "Cannot start a runtime from within a runtime".
    match &action {
        BackupAction::All => {
            let mut report = BackupReport::default();
            volume_backup::backup_all_volumes(&mut report).await;
            let backup_cfg = load_backup_config();
            handle_basic(&BackupManager::new(backup_cfg), &mut report);
            return finish(&report);
        }
        BackupAction::RestoreVolume {
            volume_name,
            from_s3,
        } => {
            return volume_backup::restore_volume(volume_name, from_s3.as_deref()).await;
        }
        _ => {}
    }

    let backup_cfg = load_backup_config();
    let mgr = BackupManager::new(backup_cfg.clone());

    match action {
        BackupAction::Basic => {
            let mut report = BackupReport::default();
            handle_basic(&mgr, &mut report);
            finish(&report)
        }
        BackupAction::List => {
            handle_list(&mgr, &backup_cfg);
            true
        }
        BackupAction::Restore { id, identity } => {
            super::restore_cmd::restore_by_id(&backup_cfg, &id, identity.as_deref())
        }
        BackupAction::RestoreBasic { identity, force } => {
            super::restore_cmd::restore_basic(&backup_cfg, identity.as_deref(), force)
        }
        BackupAction::All | BackupAction::RestoreVolume { .. } => unreachable!(),
    }
}

/// Print the run's summary as the LAST stdout line (it becomes the recorded
/// message) and report success.
fn finish(report: &BackupReport) -> bool {
    println!("{}", report.summary());
    report.ok()
}

pub(crate) fn load_backup_config() -> BackupConfig {
    // 1. Master-dispatched agent run: full config passed as JSON env var.
    if let Ok(json) = std::env::var("ORCA_BACKUP_CONFIG_JSON")
        && let Ok(cfg) = serde_json::from_str::<BackupConfig>(&json)
    {
        return cfg;
    }
    // 2. Master node: search for cluster.toml in cwd, ~/orca, and ~/.orca.
    let mut candidates = vec![
        std::path::PathBuf::from("cluster.toml"),
        std::path::PathBuf::from("/etc/orca/cluster.toml"),
    ];
    if let Some(home) = dirs_next::home_dir() {
        candidates.push(home.join("orca/cluster.toml"));
        candidates.push(home.join(".orca/cluster.toml"));
    }
    for candidate in &candidates {
        if candidate.exists() {
            match orca_core::config::ClusterConfig::load(candidate) {
                Ok(cc) => return cc.backup.unwrap_or_else(default_backup_config),
                Err(e) => tracing::warn!("Failed to load {}: {e}", candidate.display()),
            }
            break;
        }
    }
    // 3. Agent node manual run: use the config cached by the agent daemon the
    //    last time master dispatched a BackupRequest.
    if let Some(cached) = load_cached_agent_config() {
        return cached;
    }
    default_backup_config()
}

fn load_cached_agent_config() -> Option<BackupConfig> {
    let path = dirs_next::home_dir()?.join(".orca/backup_config.json");
    let json = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<BackupConfig>(&json) {
        Ok(cfg) => Some(cfg),
        Err(e) => {
            tracing::warn!("Failed to parse cached backup config: {e}");
            None
        }
    }
}

fn handle_basic(mgr: &BackupManager, report: &mut BackupReport) {
    let date = chrono::Utc::now().format("%Y-%m-%d");
    let prefix = format!("master/{date}");
    let home = dirs_next::home_dir();
    let outcome = super::backup_files::run(mgr, home.as_deref(), &prefix);
    super::backup_files::report(&outcome);
    report.config_stored += outcome.stored.len() as u32;
    report.config_skipped += outcome.skipped_unencrypted.len() as u32;
    for name in &outcome.failed {
        report.fail(format!("backup of config file {name} failed"));
    }
}

fn handle_list(mgr: &BackupManager, backup_cfg: &BackupConfig) {
    for target in &backup_cfg.targets {
        match &target {
            BackupTarget::Local { path } => println!("Local backups in {path}:"),
            BackupTarget::S3 { bucket, .. } => println!("S3 backups in {bucket}:"),
        }
        match mgr.list_backups(target) {
            Ok(entries) if entries.is_empty() => println!("  (none)"),
            Ok(entries) => {
                for e in entries {
                    println!("  {e}");
                }
            }
            Err(e) => tracing::error!("Failed to list backups: {e}"),
        }
    }
}

fn default_backup_config() -> BackupConfig {
    BackupConfig {
        age_recipients: Vec::new(),
        bind_mount_max_mb: 512,
        schedule: None,
        retention_days: 30,
        targets: vec![BackupTarget::Local {
            path: "./backups".to_string(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Serialize tests that mutate ORCA_BACKUP_CONFIG_JSON to prevent races.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn load_backup_config_reads_from_env_var() {
        use orca_core::backup::BackupTarget;
        let _guard = ENV_MUTEX.lock().unwrap();
        let cfg = BackupConfig {
            age_recipients: Vec::new(),
            bind_mount_max_mb: 512,
            schedule: Some("0 0 2 * * *".to_string()),
            retention_days: 14,
            targets: vec![BackupTarget::S3 {
                bucket: "my-bucket".to_string(),
                region: "us-east-1".to_string(),
                prefix: Some("backups/".to_string()),
                endpoint: None,
                access_key: Some("AKID".to_string()),
                secret_key: Some("SECRET".to_string()),
            }],
        };
        let json = serde_json::to_string(&cfg).unwrap();
        unsafe { std::env::set_var("ORCA_BACKUP_CONFIG_JSON", &json) };
        let loaded = load_backup_config();
        unsafe { std::env::remove_var("ORCA_BACKUP_CONFIG_JSON") };
        assert_eq!(loaded.retention_days, 14);
        assert_eq!(loaded.schedule.as_deref(), Some("0 0 2 * * *"));
        match &loaded.targets[0] {
            BackupTarget::S3 { bucket, .. } => assert_eq!(bucket, "my-bucket"),
            _ => panic!("expected S3 target"),
        }
    }

    #[test]
    fn load_backup_config_malformed_env_var_falls_through_to_default() {
        let _guard = ENV_MUTEX.lock().unwrap();
        unsafe { std::env::set_var("ORCA_BACKUP_CONFIG_JSON", "not-valid-json") };
        let loaded = load_backup_config();
        unsafe { std::env::remove_var("ORCA_BACKUP_CONFIG_JSON") };
        assert_eq!(loaded.retention_days, 30);
    }

    /// The default config (used when neither env var nor cluster.toml is
    /// present) must have sensible conservative values.
    #[test]
    fn default_backup_config_has_sensible_values() {
        use orca_core::backup::BackupTarget;
        let cfg = default_backup_config();
        assert_eq!(
            cfg.retention_days, 30,
            "default retention should be 30 days"
        );
        assert!(cfg.schedule.is_none(), "default should have no schedule");
        assert_eq!(cfg.targets.len(), 1, "default should have one local target");
        match &cfg.targets[0] {
            BackupTarget::Local { path } => assert!(!path.is_empty()),
            _ => panic!("default target should be Local"),
        }
    }
}
