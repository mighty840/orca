use crate::commands::BackupAction;
use orca_core::backup::{BackupConfig, BackupManager, BackupTarget};

pub fn handle_backup(action: BackupAction) {
    let config = std::path::Path::new("cluster.toml");
    let backup_cfg = if config.exists() {
        match orca_core::config::ClusterConfig::load(config) {
            Ok(cc) => cc.backup.unwrap_or_else(default_backup_config),
            Err(e) => {
                tracing::warn!("Failed to load cluster.toml: {e}");
                default_backup_config()
            }
        }
    } else {
        default_backup_config()
    };

    let mgr = BackupManager::new(backup_cfg.clone());

    match action {
        BackupAction::Create => {
            let files = ["secrets.json", "cluster.toml", "services.toml"];
            let mut count = 0u32;
            for file in &files {
                let path = std::path::Path::new(file);
                if path.exists() {
                    match mgr.backup_file(file, path) {
                        Ok(()) => {
                            println!("Backed up: {file}");
                            count += 1;
                        }
                        Err(e) => tracing::error!("Failed to backup {file}: {e}"),
                    }
                }
            }
            if count == 0 {
                println!("No files found to backup.");
            } else {
                println!("Backup complete: {count} file(s).");
            }
        }
        BackupAction::List => {
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
        BackupAction::Restore { id } => {
            println!("Restore from backup '{id}' not yet implemented.");
        }
    }
}

fn default_backup_config() -> BackupConfig {
    BackupConfig {
        schedule: None,
        retention_days: 30,
        targets: vec![BackupTarget::Local {
            path: "./backups".to_string(),
        }],
    }
}
