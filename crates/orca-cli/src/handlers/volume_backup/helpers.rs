//! Helper functions for Docker volume backup/restore operations.

use std::collections::HashMap;

use bollard::Docker;
use bollard::container::{
    Config, CreateContainerOptions, RemoveContainerOptions, StartContainerOptions,
    WaitContainerOptions,
};
use bollard::image::CreateImageOptions;
use bollard::models::HostConfig;
use bollard::volume::ListVolumesOptions;
use futures_util::StreamExt;

pub(crate) fn create_backup_dir() -> Option<String> {
    let home = dirs_next::home_dir()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let dir = backup_dir_path(&home, now);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::error!("Failed to create backup dir {dir}: {e}");
        return None;
    }
    Some(dir)
}

pub(crate) fn find_latest_backup_dir() -> Option<String> {
    let home = dirs_next::home_dir()?;
    let base = home.join(".orca/backups");
    let mut entries: Vec<_> = std::fs::read_dir(&base)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    entries.last().map(|e| e.path().display().to_string())
}

pub(crate) async fn list_orca_volumes(docker: &Docker) -> Option<Vec<String>> {
    let mut filters = HashMap::new();
    filters.insert("name".to_string(), vec!["orca-".to_string()]);
    let opts = ListVolumesOptions { filters };
    match docker.list_volumes(Some(opts)).await {
        Ok(resp) => {
            let names: Vec<String> = resp
                .volumes
                .unwrap_or_default()
                .iter()
                .map(|v| v.name.clone())
                .collect();
            Some(names)
        }
        Err(e) => {
            tracing::error!("Failed to list volumes: {e}");
            None
        }
    }
}

pub(crate) async fn run_backup_container(
    docker: &Docker,
    volume: &str,
    backup_dir: &str,
) -> anyhow::Result<()> {
    run_busybox_tar(
        docker,
        volume,
        backup_dir,
        vec![
            "tar".into(),
            "czf".into(),
            format!("/backup/{volume}.tar.gz"),
            "-C".into(),
            "/data".into(),
            ".".into(),
        ],
    )
    .await
}

pub(crate) async fn run_restore_container(
    docker: &Docker,
    volume: &str,
    backup_dir: &str,
) -> anyhow::Result<()> {
    run_busybox_tar(
        docker,
        volume,
        backup_dir,
        vec![
            "tar".into(),
            "xzf".into(),
            format!("/backup/{volume}.tar.gz"),
            "-C".into(),
            "/data".into(),
        ],
    )
    .await
}

/// Remove timestamped backup subdirs older than `retention_days`.
/// Apply retention to the snapshot directories in `~/.orca/backups` (#204).
/// Returns how many were deleted.
pub(crate) fn prune_old_backup_dirs(retention_days: u32, keep_min: u32, now: u64) -> usize {
    match dirs_next::home_dir() {
        Some(home) => {
            prune_backup_dirs_in(&home.join(".orca/backups"), now, retention_days, keep_min)
        }
        None => 0,
    }
}

/// Snapshot directories are named by their creation epoch. The newest
/// `keep_min` are always kept; of the rest, those older than
/// `retention_days` go. A directory whose name isn't an epoch is never
/// deleted.
pub(crate) fn prune_backup_dirs_in(
    base: &std::path::Path,
    now: u64,
    retention_days: u32,
    keep_min: u32,
) -> usize {
    let Ok(entries) = std::fs::read_dir(base) else {
        return 0;
    };
    let dirs: Vec<(std::path::PathBuf, u64)> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| Some((e.path(), e.file_name().to_str()?.parse().ok()?)))
        .collect();
    let mut deleted = 0;
    for path in
        orca_core::backup::retention::to_prune(&dirs, now, retention_days, keep_min as usize)
    {
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                tracing::info!(path = %path.display(), "Pruned old backup dir");
                deleted += 1;
            }
            Err(e) => tracing::warn!(path = %path.display(), "Failed to prune backup dir: {e}"),
        }
    }
    deleted
}

/// Build the backup directory path from a home dir and timestamp.
/// Extracted for testability (the public `create_backup_dir` uses real home dir).
pub(crate) fn backup_dir_path(home: &std::path::Path, epoch_secs: u64) -> String {
    home.join(".orca/backups")
        .join(epoch_secs.to_string())
        .display()
        .to_string()
}

async fn ensure_busybox(docker: &Docker) -> anyhow::Result<()> {
    if docker.inspect_image("busybox:latest").await.is_ok() {
        return Ok(());
    }
    tracing::info!("Pulling busybox:latest for backup");
    let opts = CreateImageOptions {
        from_image: "busybox:latest",
        ..Default::default()
    };
    let mut stream = docker.create_image(Some(opts), None, None);
    while let Some(result) = stream.next().await {
        result.map_err(|e| anyhow::anyhow!("failed to pull busybox: {e}"))?;
    }
    Ok(())
}

async fn run_busybox_tar(
    docker: &Docker,
    volume: &str,
    backup_dir: &str,
    cmd: Vec<String>,
) -> anyhow::Result<()> {
    ensure_busybox(docker).await?;
    let container_name = format!("orca-backup-{}", rand::random::<u32>());
    let binds = vec![format!("{volume}:/data"), format!("{backup_dir}:/backup")];
    let config = Config {
        image: Some("busybox:latest".to_string()),
        cmd: Some(cmd),
        host_config: Some(HostConfig {
            binds: Some(binds),
            ..Default::default()
        }),
        ..Default::default()
    };
    let opts = CreateContainerOptions {
        name: container_name.as_str(),
        platform: None,
    };
    docker.create_container(Some(opts), config).await?;
    docker
        .start_container(&container_name, None::<StartContainerOptions<String>>)
        .await?;
    let mut stream = docker.wait_container(&container_name, None::<WaitContainerOptions<String>>);
    while let Some(result) = stream.next().await {
        if let Ok(exit) = result
            && exit.status_code != 0
        {
            let _ = docker
                .remove_container(
                    &container_name,
                    Some(RemoveContainerOptions {
                        force: true,
                        ..Default::default()
                    }),
                )
                .await;
            anyhow::bail!("container exited with code {}", exit.status_code);
        }
    }
    docker
        .remove_container(
            &container_name,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await?;
    Ok(())
}
