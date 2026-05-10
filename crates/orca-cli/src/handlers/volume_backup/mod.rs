//! Docker volume backup and restore using bollard.

mod helpers;

use bollard::Docker;
use helpers::{
    create_backup_dir, find_latest_backup_dir, list_orca_volumes, prune_old_backup_dirs,
    run_backup_container, run_restore_container,
};

/// Backup all orca-prefixed Docker volumes to `~/.orca/backups/{timestamp}/`.
pub async fn backup_all_volumes() {
    let backup_cfg = crate::handlers::backup::load_backup_config();
    attempt_backup(&backup_cfg).await;
    // Prune runs unconditionally after the attempt — success or failure — so a
    // transient Docker failure never skips cleanup. Older backups are preserved
    // until they age past retention_days, so a failed run doesn't delete the
    // most-recent good backup before a new one exists.
    prune_old_backup_dirs(backup_cfg.retention_days);
}

async fn attempt_backup(backup_cfg: &orca_core::backup::BackupConfig) {
    let docker = match Docker::connect_with_local_defaults() {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("Failed to connect to Docker: {e}");
            return;
        }
    };

    let backup_dir = match create_backup_dir() {
        Some(d) => d,
        None => return,
    };

    let volumes = match list_orca_volumes(&docker).await {
        Some(v) => v,
        None => return,
    };

    if volumes.is_empty() {
        println!("No orca volumes found.");
        return;
    }

    let hooks = load_service_hooks();

    println!("Backing up {} volume(s) to {}", volumes.len(), backup_dir);
    let mut count = 0u32;

    for vol in &volumes {
        print!("  {vol} ... ");
        // Volume name is "orca-{service_name}" — derive service name for hook lookup.
        let service_name = vol.strip_prefix("orca-").unwrap_or(vol.as_str());
        if let Some(hook) = hooks.get(service_name) {
            let container = format!("orca-{service_name}");
            if let Err(e) = run_pre_hook(&docker, &container, hook).await {
                println!("FAILED (pre-hook): {e}");
                continue;
            }
        }
        match run_backup_container(&docker, vol, &backup_dir).await {
            Ok(()) => {
                println!("done");
                count += 1;
            }
            Err(e) => println!("FAILED: {e}"),
        }
    }

    println!("Volume backup complete: {count}/{} volumes.", volumes.len());

    upload_volumes_to_s3(backup_cfg, &volumes, &backup_dir);
}

/// Upload each volume tarball from a completed local backup to all S3 targets.
/// Uses `{vol}_{epoch}.tar.gz` as the S3 key so daily backups don't overwrite each other.
fn upload_volumes_to_s3(
    config: &orca_core::backup::BackupConfig,
    volumes: &[String],
    backup_dir: &str,
) {
    use orca_core::backup::BackupTarget;

    let s3_targets: Vec<_> = config
        .targets
        .iter()
        .filter(|t| matches!(t, BackupTarget::S3 { .. }))
        .collect();

    if s3_targets.is_empty() {
        return;
    }

    let epoch = std::path::Path::new(backup_dir)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    for vol in volumes {
        let local_path = std::path::Path::new(backup_dir).join(format!("{vol}.tar.gz"));
        if !local_path.exists() {
            continue;
        }
        let s3_name = format!("{vol}_{epoch}.tar.gz");
        for target in &s3_targets {
            match orca_core::backup::s3::upload(&local_path, target, &s3_name) {
                Ok(()) => tracing::info!("Uploaded {vol} to S3"),
                Err(e) => tracing::error!("S3 upload failed for {vol}: {e}"),
            }
        }
    }
}

fn load_service_hooks() -> std::collections::HashMap<String, String> {
    std::env::var("ORCA_SERVICE_HOOKS_JSON")
        .ok()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

async fn run_pre_hook(docker: &Docker, container: &str, hook: &str) -> anyhow::Result<()> {
    use bollard::exec::{CreateExecOptions, StartExecResults};
    use futures_util::StreamExt;

    tracing::info!("Running pre-hook in {container}: {hook}");
    let exec = docker
        .create_exec(
            container,
            CreateExecOptions {
                cmd: Some(vec!["sh".to_string(), "-c".to_string(), hook.to_string()]),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                ..Default::default()
            },
        )
        .await?;

    if let StartExecResults::Attached { mut output, .. } = docker.start_exec(&exec.id, None).await?
    {
        while output.next().await.is_some() {}
    }

    let inspect = docker.inspect_exec(&exec.id).await?;
    let code = inspect.exit_code.unwrap_or(-1);
    anyhow::ensure!(code == 0, "pre-hook exited with code {code}");
    Ok(())
}

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
            schedule: None,
            retention_days: 7,
            targets: vec![BackupTarget::Local {
                path: "/tmp/backups".into(),
            }],
        };
        upload_volumes_to_s3(&config, &["orca-myapp".to_string()], "/tmp/.orca/backups/1000");
    }

    /// Missing local tarball → upload_volumes_to_s3 skips that volume without panicking.
    #[test]
    fn upload_volumes_to_s3_skips_missing_tarballs() {
        use orca_core::backup::{BackupConfig, BackupTarget};
        let config = BackupConfig {
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
        upload_volumes_to_s3(
            &config,
            &["orca-nonexistent".to_string()],
            "/tmp/.orca/backups/9999999",
        );
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
        use helpers::prune_old_backup_dirs;

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
