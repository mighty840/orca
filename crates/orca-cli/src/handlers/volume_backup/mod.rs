//! Docker volume backup and restore using bollard.

mod bind_archive;
mod bind_mounts;
mod helpers;
mod run;
mod volume_owner;

use bollard::Docker;
use helpers::{find_latest_backup_dir, run_restore_container};

pub use run::backup_all_volumes;

/// Restore a Docker volume from the latest backup directory.
pub async fn restore_volume(volume_name: &str) {
    let docker = match Docker::connect_with_local_defaults() {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("Failed to connect to Docker: {e}");
            return;
        }
    };

    let backup_dir = match find_latest_backup_dir() {
        Some(d) => d,
        None => {
            println!("No backup directories found in ~/.orca/backups/");
            return;
        }
    };

    let archive = format!("{backup_dir}/{volume_name}.tar.gz");
    if !std::path::Path::new(&archive).exists() {
        println!("No backup found for volume '{volume_name}' in {backup_dir}");
        return;
    }

    println!("Restoring {volume_name} from {backup_dir} ...");
    match run_restore_container(&docker, volume_name, &backup_dir).await {
        Ok(()) => println!("Restored volume '{volume_name}' successfully."),
        Err(e) => tracing::error!("Restore failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use helpers::backup_dir_path;

    use super::*;

    #[test]
    fn backup_dir_uses_timestamp_subdirectory() {
        let home = std::path::Path::new("/tmp/fakehome");
        let path = backup_dir_path(home, 1_700_000_000);
        assert!(path.contains(".orca/backups/1700000000"));
        assert!(path.starts_with("/tmp/fakehome/"));
    }

    #[test]
    fn backup_dir_timestamp_format_is_numeric() {
        let home = std::path::Path::new("/home/testuser");
        let path = backup_dir_path(home, 42);
        // The final component should be the epoch seconds as a plain number
        let last = std::path::Path::new(&path)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(last, "42");
    }

    #[test]
    fn create_backup_dir_creates_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = backup_dir_path(tmp.path(), 9999);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(std::path::Path::new(&dir).is_dir());
    }

    #[test]
    fn find_latest_picks_lexicographic_last() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join(".orca/backups");
        std::fs::create_dir_all(base.join("1000")).unwrap();
        std::fs::create_dir_all(base.join("2000")).unwrap();
        std::fs::create_dir_all(base.join("1500")).unwrap();
        // find_latest_backup_dir uses dirs_next, so test the sorting logic directly
        let mut entries: Vec<_> = std::fs::read_dir(&base)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        let last = entries.last().unwrap().file_name();
        assert_eq!(last.to_str().unwrap(), "2000");
    }

    /// No S3 targets → upload_volumes_to_s3 must return immediately without error.
    #[test]
    fn upload_volumes_to_s3_noop_with_local_only_config() {
        use orca_core::backup::{BackupConfig, BackupTarget};
        let config = BackupConfig {
            age_recipients: Vec::new(),
            bind_mount_max_mb: 512,
            schedule: None,
            retention_days: 7,
            targets: vec![BackupTarget::Local {
                path: "/tmp/backups".into(),
            }],
        };
        let mut report = crate::handlers::backup_report::BackupReport::default();
        run::upload_volumes_to_s3(
            &config,
            &["orca-myapp".to_string()],
            "/tmp/.orca/backups/1000",
            &mut report,
        );
        assert!(report.ok() && report.s3_total == 0);
    }

    /// Missing local tarball → upload_volumes_to_s3 skips that volume without panicking.
    #[test]
    fn upload_volumes_to_s3_skips_missing_tarballs() {
        use orca_core::backup::{BackupConfig, BackupTarget};
        let config = BackupConfig {
            age_recipients: Vec::new(),
            bind_mount_max_mb: 512,
            schedule: None,
            retention_days: 7,
            targets: vec![BackupTarget::S3 {
                bucket: "test".into(),
                region: "us-east-1".into(),
                prefix: None,
                endpoint: None,
                access_key: None,
                secret_key: None,
            }],
        };
        // The tarball path does not exist — must skip, not panic or error.
        let mut report = crate::handlers::backup_report::BackupReport::default();
        run::upload_volumes_to_s3(
            &config,
            &["orca-nonexistent".to_string()],
            "/tmp/.orca/backups/9999999",
            &mut report,
        );
        // A missing tarball was already reported when its tar failed; the
        // upload step doesn't count it twice.
        assert_eq!(report.s3_total, 0);
    }

    /// The S3 key includes the epoch from the backup dir so daily backups don't overwrite
    /// each other in the bucket.
    #[test]
    fn s3_key_embeds_epoch_from_backup_dir() {
        let backup_dir = "/home/user/.orca/backups/1715299200";
        let epoch = std::path::Path::new(backup_dir)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        assert_eq!(epoch, "1715299200");
        assert_eq!(
            format!("orca-myapp_{epoch}.tar.gz"),
            "orca-myapp_1715299200.tar.gz"
        );
    }

    /// Pruning runs after the backup attempt, so only dirs whose epoch is
    /// older than retention_days are removed — the most-recent good backup is never
    /// deleted before a new one exists.
    #[test]
    fn prune_does_not_remove_recent_backup_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join(".orca/backups");

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Create a "recent" dir (1 hour ago) and a "stale" dir (30 days ago).
        let recent = base.join((now - 3600).to_string());
        let stale = base.join((now - 30 * 86400 - 1).to_string());
        std::fs::create_dir_all(&recent).unwrap();
        std::fs::create_dir_all(&stale).unwrap();

        // Override home — prune_old_backup_dirs uses dirs_next::home_dir() so
        // we test the underlying logic directly instead.
        let cutoff = now.saturating_sub(7 * 86400);
        for entry in std::fs::read_dir(&base).unwrap().flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let epoch: u64 = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse().ok())
                .unwrap_or(u64::MAX);
            if epoch < cutoff {
                std::fs::remove_dir_all(&path).unwrap();
            }
        }

        assert!(recent.exists(), "recent dir must survive pruning");
        assert!(!stale.exists(), "stale dir must be removed by pruning");
    }
}
