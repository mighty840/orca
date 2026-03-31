use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use tracing::info;

use super::config::{BackupConfig, BackupTarget};
use super::s3 as s3_backend;

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

    /// Backup a service volume directory.
    pub fn backup_volume(
        &self,
        service_name: &str,
        volume_path: &str,
        pre_hook: Option<&str>,
    ) -> Result<BackupResult> {
        let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();

        if let Some(hook) = pre_hook {
            info!(service = service_name, hook, "Running pre-backup hook");
            let status = std::process::Command::new("sh")
                .arg("-c")
                .arg(hook)
                .status()
                .context("Failed to execute pre-backup hook")?;
            if !status.success() {
                anyhow::bail!("Pre-backup hook failed: {:?}", status.code());
            }
        }

        let archive_name = format!("{service_name}_{timestamp}.tar.gz");
        let archive_path = std::env::temp_dir().join(&archive_name);

        info!(
            service = service_name,
            src = volume_path,
            "Creating archive"
        );
        let status = std::process::Command::new("tar")
            .args([
                "-czf",
                archive_path.to_str().unwrap_or(""),
                "-C",
                volume_path,
                ".",
            ])
            .status()
            .context("Failed to create tar archive")?;
        if !status.success() {
            anyhow::bail!("tar failed: {:?}", status.code());
        }

        let size_bytes = std::fs::metadata(&archive_path)
            .map(|m| m.len())
            .unwrap_or(0);
        let mut target_desc = String::new();
        for t in &self.config.targets {
            target_desc = self.store(&archive_path, t, &archive_name)?;
        }
        let _ = std::fs::remove_file(&archive_path);

        Ok(BackupResult {
            service_name: service_name.to_string(),
            timestamp,
            size_bytes,
            target: target_desc,
        })
    }

    /// Backup a single file to all targets.
    pub fn backup_file(&self, name: &str, path: &Path) -> Result<()> {
        let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("bak");
        let backup_name = format!("{name}_{timestamp}.{ext}");
        for t in &self.config.targets {
            self.store(path, t, &backup_name)?;
        }
        Ok(())
    }

    fn store(&self, data_path: &Path, target: &BackupTarget, name: &str) -> Result<String> {
        match target {
            BackupTarget::Local { path } => {
                let dest_dir = Path::new(path);
                std::fs::create_dir_all(dest_dir)
                    .with_context(|| format!("create backup dir: {path}"))?;
                let dest = dest_dir.join(name);
                std::fs::copy(data_path, &dest)
                    .with_context(|| format!("copy to {}", dest.display()))?;
                info!(dest = %dest.display(), "Stored backup locally");
                Ok(format!("local:{path}"))
            }
            t @ BackupTarget::S3 { bucket, .. } => {
                s3_backend::upload(data_path, t, name)?;
                Ok(format!("s3://{bucket}"))
            }
        }
    }

    /// List backups in a target.
    pub fn list_backups(&self, target: &BackupTarget) -> Result<Vec<String>> {
        match target {
            BackupTarget::Local { path } => {
                let dir = Path::new(path);
                if !dir.exists() {
                    return Ok(vec![]);
                }
                let mut entries = Vec::new();
                for entry in std::fs::read_dir(dir)? {
                    if let Some(name) = entry?.file_name().to_str() {
                        entries.push(name.to_string());
                    }
                }
                entries.sort();
                Ok(entries)
            }
            t @ BackupTarget::S3 { .. } => s3_backend::list_objects(t),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let src = tmp.path().join("test.json");
        std::fs::write(&src, r#"{"key":"value"}"#).unwrap();
        mgr.backup_file("secrets", &src).unwrap();
        let backups = std::fs::read_dir(&target_dir).unwrap().count();
        assert_eq!(backups, 1);
    }
}
