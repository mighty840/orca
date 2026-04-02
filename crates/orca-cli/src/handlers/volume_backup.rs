//! Docker volume backup and restore using bollard.

use std::collections::HashMap;

use bollard::Docker;
use bollard::container::{
    Config, CreateContainerOptions, RemoveContainerOptions, StartContainerOptions,
    WaitContainerOptions,
};
use bollard::models::HostConfig;
use bollard::volume::ListVolumesOptions;
use futures_util::StreamExt;

/// Backup all orca-prefixed Docker volumes to `~/.orca/backups/{timestamp}/`.
pub async fn backup_all_volumes() {
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

    println!("Backing up {} volume(s) to {}", volumes.len(), backup_dir);
    let mut count = 0u32;

    for vol in &volumes {
        print!("  {vol} ... ");
        match run_backup_container(&docker, vol, &backup_dir).await {
            Ok(()) => {
                println!("done");
                count += 1;
            }
            Err(e) => println!("FAILED: {e}"),
        }
    }

    println!("Volume backup complete: {count}/{} volumes.", volumes.len());
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

fn create_backup_dir() -> Option<String> {
    let home = dirs_next::home_dir()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let dir = home
        .join(".orca/backups")
        .join(now.to_string())
        .display()
        .to_string();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::error!("Failed to create backup dir {dir}: {e}");
        return None;
    }
    Some(dir)
}

fn find_latest_backup_dir() -> Option<String> {
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

async fn list_orca_volumes(docker: &Docker) -> Option<Vec<String>> {
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

async fn run_backup_container(
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

async fn run_restore_container(
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

async fn run_busybox_tar(
    docker: &Docker,
    volume: &str,
    backup_dir: &str,
    cmd: Vec<String>,
) -> anyhow::Result<()> {
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
