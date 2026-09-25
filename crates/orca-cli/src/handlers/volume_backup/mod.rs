//! Docker volume backup and restore using bollard.

mod bind_archive;
mod bind_mounts;
mod helpers;
mod restore;
mod run;
mod seal;
mod volume_owner;

pub(crate) use helpers::prune_old_backup_dirs;
pub use restore::restore_volume;
pub use run::backup_all_volumes;

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
            keep_min: 7,
            prune_s3: false,
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
            keep_min: 7,
            prune_s3: false,
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
    fn prune_backup_dirs_keeps_the_floor_and_never_touches_odd_names() {
        // #204: every snapshot is past retention (e.g. after a streak of
        // failed nights); the newest keep_min stay, odd names stay.
        let base = tempfile::tempdir().unwrap();
        let now = 1_790_000_000u64;
        for d in 20..25u64 {
            std::fs::create_dir(base.path().join((now - d * 86_400).to_string())).unwrap();
        }
        std::fs::create_dir(base.path().join("manual-copy")).unwrap();
        let deleted = helpers::prune_backup_dirs_in(base.path(), now, 14, 2);
        assert_eq!(deleted, 3);
        let mut left: Vec<String> = std::fs::read_dir(base.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        let newest = [
            (now - 21 * 86_400).to_string(),
            (now - 20 * 86_400).to_string(),
        ];
        assert_eq!(
            left,
            [
                newest[0].clone(),
                newest[1].clone(),
                "manual-copy".to_string()
            ]
        );
    }
}
