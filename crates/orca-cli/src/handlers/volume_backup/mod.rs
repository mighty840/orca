//! Docker volume backup and restore using bollard.

mod bind_archive;
mod bind_mounts;
mod helpers;
mod run;
mod volume_owner;

use bollard::Docker;
use helpers::{find_latest_backup_dir, run_restore_container};

pub(crate) use helpers::prune_old_backup_dirs;
pub use run::backup_all_volumes;

/// Restore a Docker volume from the latest local backup directory, or from
/// the S3 object `from_s3` (#200: a fresh host has no local backups, so the
/// tarballs in S3 were unreachable through the CLI). Returns success.
pub async fn restore_volume(volume_name: &str, from_s3: Option<&str>) -> bool {
    let docker = match Docker::connect_with_local_defaults() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Failed to connect to Docker: {e}");
            return false;
        }
    };

    let backup_dir = match from_s3 {
        Some(key) => match stage_from_s3(volume_name, key) {
            Ok(dir) => dir,
            Err(e) => {
                eprintln!("Cannot fetch {key} from S3: {e:#}");
                return false;
            }
        },
        None => match find_latest_backup_dir() {
            Some(d) => d,
            None => {
                eprintln!(
                    "No backup directories found in ~/.orca/backups/. To restore from \
                     S3, pass --from-s3 <key> (see `orca backup list`)."
                );
                return false;
            }
        },
    };

    let archive = format!("{backup_dir}/{volume_name}.tar.gz");
    if !std::path::Path::new(&archive).exists() {
        eprintln!("No backup found for volume '{volume_name}' in {backup_dir}");
        return false;
    }

    println!("Restoring {volume_name} from {backup_dir} ...");
    match run_restore_container(&docker, volume_name, &backup_dir).await {
        Ok(()) => {
            println!("Restored volume '{volume_name}' successfully.");
            true
        }
        Err(e) => {
            eprintln!("Restore failed: {e}");
            false
        }
    }
}

/// Download `key` from the first S3 target into a fresh staging directory as
/// `<volume>.tar.gz`, the layout `run_restore_container` expects.
fn stage_from_s3(volume_name: &str, key: &str) -> anyhow::Result<String> {
    let cfg = crate::handlers::backup::load_backup_config();
    let target = cfg
        .targets
        .iter()
        .find(|t| matches!(t, orca_core::backup::BackupTarget::S3 { .. }))
        .ok_or_else(|| anyhow::anyhow!("no S3 backup target configured"))?;
    anyhow::ensure!(
        !key.ends_with(orca_core::backup::encrypt::AGE_SUFFIX),
        "{key} is age-encrypted; volume tarballs are not, so this is not a volume backup"
    );
    let home = dirs_next::home_dir().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
    let stamp = chrono::Utc::now().timestamp();
    let dir = home.join(format!(".orca/restore/{stamp}-{volume_name}"));
    orca_core::fsutil::create_private_dir(&dir)?;
    orca_core::backup::s3::download(target, key, &dir.join(format!("{volume_name}.tar.gz")))?;
    Ok(dir.display().to_string())
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
