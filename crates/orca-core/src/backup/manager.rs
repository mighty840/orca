use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use tracing::{info, warn};

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
    ///
    /// `s3_prefix` is prepended to the S3 object key so backups land in a
    /// structured path (e.g. `"master/2026-05-12"`). Local targets always
    /// use a flat filename regardless of the prefix.
    ///
    /// Failures on individual targets are logged and skipped so a broken S3
    /// config never prevents the local backup from being written.
    pub fn backup_file(&self, name: &str, path: &Path, s3_prefix: &str) -> Result<()> {
        let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("bak");
        // With recipients configured, store an age-encrypted copy instead of
        // the file itself (#117, #199). A failure here fails the artifact:
        // never fall back to storing it in the clear.
        let encrypted = if self.encrypts() {
            Some(
                super::encrypt::encrypt_to_temp(path, &self.config.age_recipients)
                    .with_context(|| format!("encrypt {name} for backup"))?,
            )
        } else {
            None
        };
        let (path, suffix) = match &encrypted {
            Some(tmp) => (tmp.path(), super::encrypt::AGE_SUFFIX),
            None => (path, ""),
        };
        let local_name = format!("{name}_{timestamp}.{ext}{suffix}");
        let s3_name = if s3_prefix.is_empty() {
            local_name.clone()
        } else {
            format!("{s3_prefix}/{local_name}")
        };
        // Every target must store the artifact. "Stored somewhere" used to
        // count as success, so a broken target (rotated S3 credentials, a
        // full disk) went unreported as long as another one worked (#197,
        // found by #204's tests).
        let mut stored = 0usize;
        let mut errors = Vec::new();
        for t in &self.config.targets {
            let key = match t {
                BackupTarget::S3 { .. } => s3_name.as_str(),
                BackupTarget::Local { .. } => &local_name,
            };
            match self.store(path, t, key) {
                Ok(_) => stored += 1,
                Err(e) => {
                    warn!("backup target failed for {name}: {e}");
                    errors.push(format!("{e:#}"));
                }
            }
        }
        if !errors.is_empty() {
            anyhow::bail!(
                "{name} stored on {stored}/{} target(s): {}",
                self.config.targets.len(),
                errors.join("; ")
            );
        }
        Ok(())
    }

    /// Whether artifacts are age-encrypted before they are stored.
    pub fn encrypts(&self) -> bool {
        !self.config.age_recipients.is_empty()
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

    /// Delete local backup files older than `retention_days`.
    /// Apply retention to the config artifacts in every local target
    /// (#204): per artifact name, keep the newest `keep_min`, delete the rest
    /// once older than `retention_days`. Only files named like artifacts
    /// (`<name>_<timestamp>.<ext>`) are considered; anything else in the
    /// directory is never touched. Returns how many files were deleted.
    ///
    /// Called by the CLI after a successful run only, never mid-run.
    pub fn prune_local_files(&self, now: u64) -> usize {
        let mut deleted = 0;
        for t in &self.config.targets {
            let BackupTarget::Local { path } = t else {
                continue;
            };
            let Ok(entries) = std::fs::read_dir(path) else {
                continue;
            };
            let mut groups: std::collections::BTreeMap<String, Vec<(std::path::PathBuf, u64)>> =
                Default::default();
            for e in entries.flatten() {
                let p = e.path();
                if !p.is_file() {
                    continue;
                }
                if let Some((name, time)) = e
                    .file_name()
                    .to_str()
                    .and_then(super::retention::artifact_time)
                {
                    groups.entry(name).or_default().push((p, time));
                }
            }
            for items in groups.values() {
                for p in super::retention::to_prune(
                    items,
                    now,
                    self.config.retention_days,
                    self.config.keep_min as usize,
                ) {
                    match std::fs::remove_file(&p) {
                        Ok(()) => {
                            info!(path = %p.display(), "Pruned old backup");
                            deleted += 1;
                        }
                        Err(e) => warn!(path = %p.display(), "Failed to prune old backup: {e}"),
                    }
                }
            }
        }
        deleted
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
            age_recipients: Vec::new(),
            bind_mount_max_mb: 512,
            keep_min: 7,
            prune_s3: false,
            schedule: None,
            retention_days: 7,
            targets: vec![BackupTarget::Local {
                path: target_dir.to_str().unwrap().to_string(),
            }],
        };
        let mgr = BackupManager::new(config);
        let src = tmp.path().join("test.json");
        std::fs::write(&src, r#"{"key":"value"}"#).unwrap();
        mgr.backup_file("secrets", &src, "").unwrap();
        let backups = std::fs::read_dir(&target_dir).unwrap().count();
        assert_eq!(backups, 1);
    }

    #[test]
    fn prune_local_files_keeps_a_floor_per_artifact_and_ignores_foreign_files() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = BackupManager::new(BackupConfig {
            age_recipients: Vec::new(),
            bind_mount_max_mb: 512,
            keep_min: 2,
            prune_s3: false,
            schedule: None,
            retention_days: 7,
            targets: vec![BackupTarget::Local {
                path: dir.path().display().to_string(),
            }],
        });
        // 5 nights of two artifacts, all older than retention, plus a file
        // that isn't an orca artifact.
        for d in 1..=5 {
            for name in ["secrets", "cluster"] {
                let f = dir.path().join(format!("{name}_2026080{d}T030000Z.json"));
                std::fs::write(f, "x").unwrap();
            }
        }
        std::fs::write(dir.path().join("operator-notes.txt"), "keep me").unwrap();

        let now = super::super::retention::artifact_time("x_20260930T000000Z.json")
            .unwrap()
            .1;
        assert_eq!(mgr.prune_local_files(now), 6);
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "cluster_20260804T030000Z.json",
                "cluster_20260805T030000Z.json",
                "operator-notes.txt",
                "secrets_20260804T030000Z.json",
                "secrets_20260805T030000Z.json",
            ]
        );
    }
}
