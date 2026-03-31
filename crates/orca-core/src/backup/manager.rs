use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use tracing::{info, warn};

use super::config::{BackupConfig, BackupTarget};

/// Result of a single backup operation.
#[derive(Debug, Clone)]
pub struct BackupResult {
    pub service_name: String,
    pub timestamp: String,
    pub size_bytes: u64,
    pub target: String,
}

/// Manages backup operations for volumes, configs, and secrets.
pub struct BackupManager {
    config: BackupConfig,
}

impl BackupManager {
    pub fn new(config: BackupConfig) -> Self {
        Self { config }
    }

    /// Backup a service volume directory. Runs an optional pre-hook command first,
    /// then creates a tar.gz archive and stores it to each configured target.
    pub fn backup_volume(
        &self,
        service_name: &str,
        volume_path: &str,
        pre_hook: Option<&str>,
    ) -> Result<BackupResult> {
        let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();

        // Run pre-hook if configured (e.g. pg_dump).
        if let Some(hook) = pre_hook {
            info!(service = service_name, hook, "Running pre-backup hook");
            let status = std::process::Command::new("sh")
                .arg("-c")
                .arg(hook)
                .status()
                .context("Failed to execute pre-backup hook")?;
            if !status.success() {
                anyhow::bail!("Pre-backup hook failed with exit code: {:?}", status.code());
            }
        }

        // Create tar.gz archive in a temp location.
        let archive_name = format!("{service_name}_{timestamp}.tar.gz");
        let tmp_dir = std::env::temp_dir();
        let archive_path = tmp_dir.join(&archive_name);

        info!(
            service = service_name,
            src = volume_path,
            archive = %archive_path.display(),
            "Creating volume archive"
        );

        let status = std::process::Command::new("tar")
            .args([
                "-czf",
                archive_path.to_str().unwrap_or_default(),
                "-C",
                volume_path,
                ".",
            ])
            .status()
            .context("Failed to create tar archive")?;

        if !status.success() {
            anyhow::bail!("tar failed with exit code: {:?}", status.code());
        }

        let size_bytes = std::fs::metadata(&archive_path)
            .map(|m| m.len())
            .unwrap_or(0);

        // Store to each configured target.
        let mut target_desc = String::new();
        for target in &self.config.targets {
            match target {
                BackupTarget::Local { path } => {
                    self.store_local(&archive_path, path, &archive_name)?;
                    target_desc = format!("local:{path}");
                }
                BackupTarget::S3 {
                    bucket,
                    region,
                    prefix,
                } => {
                    let pfx = prefix.as_deref().unwrap_or("");
                    self.store_s3(&archive_path, bucket, region, pfx, &archive_name)?;
                    target_desc = format!("s3://{bucket}");
                }
            }
        }

        // Clean up temp archive.
        let _ = std::fs::remove_file(&archive_path);

        Ok(BackupResult {
            service_name: service_name.to_string(),
            timestamp,
            size_bytes,
            target: target_desc,
        })
    }

    /// Backup a single file (config, secrets.json, redb) to all targets.
    pub fn backup_file(&self, name: &str, path: &Path) -> Result<()> {
        let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("bak");
        let backup_name = format!("{name}_{timestamp}.{ext}");

        for target in &self.config.targets {
            match target {
                BackupTarget::Local { path: dest } => {
                    self.store_local(path, dest, &backup_name)?;
                }
                BackupTarget::S3 {
                    bucket,
                    region,
                    prefix,
                } => {
                    let pfx = prefix.as_deref().unwrap_or("");
                    self.store_s3(path, bucket, region, pfx, &backup_name)?;
                }
            }
        }
        Ok(())
    }

    /// Copy a file to a local backup directory with a timestamped name.
    fn store_local(&self, data_path: &Path, target_path: &str, backup_name: &str) -> Result<()> {
        let dest_dir = Path::new(target_path);
        std::fs::create_dir_all(dest_dir)
            .with_context(|| format!("Failed to create backup dir: {target_path}"))?;

        let dest = dest_dir.join(backup_name);
        std::fs::copy(data_path, &dest)
            .with_context(|| format!("Failed to copy to {}", dest.display()))?;

        info!(dest = %dest.display(), "Stored backup locally");
        Ok(())
    }

    /// Placeholder for S3 upload — logs intent, actual SDK integration deferred to M5.
    fn store_s3(
        &self,
        _data_path: &Path,
        bucket: &str,
        region: &str,
        prefix: &str,
        name: &str,
    ) -> Result<()> {
        let key = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}{name}")
        };
        warn!(
            bucket,
            region, key, "S3 upload not yet implemented — would upload here (M5)"
        );
        Ok(())
    }

    /// List backup files in a local target directory.
    pub fn list_backups(&self, target: &BackupTarget) -> Result<Vec<String>> {
        match target {
            BackupTarget::Local { path } => {
                let dir = Path::new(path);
                if !dir.exists() {
                    return Ok(vec![]);
                }
                let mut entries = Vec::new();
                for entry in std::fs::read_dir(dir)
                    .with_context(|| format!("Failed to read backup dir: {path}"))?
                {
                    let entry = entry?;
                    if let Some(name) = entry.file_name().to_str() {
                        entries.push(name.to_string());
                    }
                }
                entries.sort();
                Ok(entries)
            }
            BackupTarget::S3 { bucket, .. } => {
                warn!(bucket, "S3 listing not yet implemented (M5)");
                Ok(vec![])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::config::BackupConfig;

    #[test]
    fn backup_file_to_local() {
        let tmp = tempfile::tempdir().unwrap();
        let target_dir = tmp.path().join("backups");

        let config = BackupConfig {
            schedule: None,
            retention_days: 7,
            targets: vec![BackupTarget::Local {
                path: target_dir.to_str().unwrap().to_string(),
            }],
        };

        let mgr = BackupManager::new(config);

        // Create a test file to back up.
        let src = tmp.path().join("test.json");
        std::fs::write(&src, r#"{"key":"value"}"#).unwrap();

        mgr.backup_file("secrets", &src).unwrap();

        let backups = std::fs::read_dir(&target_dir).unwrap().count();
        assert_eq!(backups, 1);
    }
}
