use crate::commands::BackupAction;
use orca_core::backup::{BackupConfig, BackupManager, BackupTarget};

use super::volume_backup;

pub fn handle_backup(action: BackupAction) {
    match &action {
        BackupAction::All => {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            rt.block_on(volume_backup::backup_all_volumes());
            return;
        }
        BackupAction::RestoreVolume { volume_name } => {
            let name = volume_name.clone();
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            rt.block_on(volume_backup::restore_volume(&name));
            return;
        }
        _ => {}
    }

    let backup_cfg = load_backup_config();
    let mgr = BackupManager::new(backup_cfg.clone());

    match action {
        BackupAction::Create => handle_create(&mgr),
        BackupAction::List => handle_list(&mgr, &backup_cfg),
        BackupAction::Restore { id } => restore_backup(&mgr, &backup_cfg, &id),
        BackupAction::All | BackupAction::RestoreVolume { .. } => unreachable!(),
    }
}

fn load_backup_config() -> BackupConfig {
    let config = std::path::Path::new("cluster.toml");
    if config.exists() {
        match orca_core::config::ClusterConfig::load(config) {
            Ok(cc) => cc.backup.unwrap_or_else(default_backup_config),
            Err(e) => {
                tracing::warn!("Failed to load cluster.toml: {e}");
                default_backup_config()
            }
        }
    } else {
        default_backup_config()
    }
}

fn handle_create(mgr: &BackupManager) {
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

/// Parse a backup filename like `secrets_20260329T120000Z.json` into (name, ext).
fn parse_backup_filename(filename: &str) -> Option<(&str, &str)> {
    // Format: {name}_{timestamp}.{ext}
    let underscore = filename.find('_')?;
    let name = &filename[..underscore];
    let rest = &filename[underscore + 1..];
    let dot = rest.find('.')?;
    let ext = &rest[dot + 1..];
    Some((name, ext))
}

/// Determine the restore target path from a backup filename.
fn restore_target(filename: &str) -> Option<String> {
    let (name, ext) = parse_backup_filename(filename)?;
    match (name, ext) {
        ("secrets", "json") => Some("secrets.json".to_string()),
        ("cluster", "toml") => Some("cluster.toml".to_string()),
        ("services", "toml") => Some("services.toml".to_string()),
        (_, "tar.gz" | "tgz") => None, // volume backups handled separately
        (n, e) => Some(format!("{n}.{e}")),
    }
}

fn restore_backup(mgr: &BackupManager, config: &BackupConfig, id: &str) {
    // Find the backup file in the first local target
    for target in &config.targets {
        match target {
            BackupTarget::Local { path } => {
                let dir = std::path::Path::new(path);
                let entries = match mgr.list_backups(target) {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::error!("Failed to list backups: {e}");
                        continue;
                    }
                };
                let matched: Vec<_> = entries.iter().filter(|e| e.contains(id)).collect();
                if matched.is_empty() {
                    println!("No backups matching '{id}' in {path}");
                    continue;
                }
                if matched.len() > 1 {
                    println!("Multiple matches for '{id}':");
                    for m in &matched {
                        println!("  {m}");
                    }
                    println!("Please specify a more precise id.");
                    return;
                }
                let filename = matched[0];
                let src = dir.join(filename);
                if filename.ends_with(".tar.gz") || filename.ends_with(".tgz") {
                    let tmp = std::env::temp_dir().join(format!("orca-restore-{id}"));
                    std::fs::create_dir_all(&tmp).ok();
                    let status = std::process::Command::new("tar")
                        .args(["-xzf", &src.display().to_string(), "-C"])
                        .arg(&tmp)
                        .status();
                    match status {
                        Ok(s) if s.success() => {
                            println!("Extracted volume backup to {}", tmp.display());
                        }
                        _ => {
                            tracing::error!("Failed to extract {filename}");
                        }
                    }
                } else if let Some(target_path) = restore_target(filename) {
                    match std::fs::copy(&src, &target_path) {
                        Ok(_) => println!("Restored {filename} -> {target_path}"),
                        Err(e) => tracing::error!("Failed to restore: {e}"),
                    }
                } else {
                    println!("Cannot determine restore target for {filename}");
                }
                return;
            }
            BackupTarget::S3 { .. } => {
                println!("S3 restore not yet supported.");
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_secrets_backup_filename() {
        let (name, ext) = parse_backup_filename("secrets_20260329T120000Z.json").unwrap();
        assert_eq!(name, "secrets");
        assert_eq!(ext, "json");
    }

    #[test]
    fn parse_cluster_backup_filename() {
        let (name, ext) = parse_backup_filename("cluster_20260329T120000Z.toml").unwrap();
        assert_eq!(name, "cluster");
        assert_eq!(ext, "toml");
    }

    #[test]
    fn restore_target_secrets() {
        assert_eq!(
            restore_target("secrets_20260329T120000Z.json"),
            Some("secrets.json".to_string())
        );
    }

    #[test]
    fn restore_target_cluster() {
        assert_eq!(
            restore_target("cluster_20260329T120000Z.toml"),
            Some("cluster.toml".to_string())
        );
    }

    #[test]
    fn parse_invalid_filename_returns_none() {
        assert!(parse_backup_filename("nounderscorehere").is_none());
    }
}
