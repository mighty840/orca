//! E2E tests: volume backup and restore with real Docker.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use orca_control::state::AppState;
use orca_core::config::{ClusterConfig, ClusterMeta};

fn test_state() -> Arc<AppState> {
    let runtime = Arc::new(orca_agent::docker::ContainerRuntime::new().expect("Docker required"));
    Arc::new(AppState::new(
        ClusterConfig {
            cluster: ClusterMeta {
                name: "e2e-backup".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        runtime,
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ))
}

async fn cleanup(prefix: &str) {
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let opts = bollard::container::ListContainersOptions::<String> {
        all: true,
        filters: HashMap::from([("name".to_string(), vec![prefix.to_string()])]),
        ..Default::default()
    };
    if let Ok(containers) = docker.list_containers(Some(opts)).await {
        for c in containers {
            if let Some(id) = c.id {
                let _ = docker
                    .remove_container(
                        &id,
                        Some(bollard::container::RemoveContainerOptions {
                            force: true,
                            ..Default::default()
                        }),
                    )
                    .await;
            }
        }
    }
}

/// Deploy a service with a volume, write data, backup, clear, restore, verify.
#[tokio::test]
#[ignore]
async fn e2e_backup_and_restore_volume() {
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let vol_name = "orca-e2e-backup-data";

    // Clean up any leftover volume
    let _ = docker.remove_volume(vol_name, None).await;

    // Create volume and write test data
    docker
        .create_volume(bollard::volume::CreateVolumeOptions {
            name: vol_name,
            ..Default::default()
        })
        .await
        .unwrap();

    // Write data into volume using busybox
    let config = bollard::container::Config {
        image: Some("busybox:latest"),
        cmd: Some(vec![
            "sh",
            "-c",
            "echo 'backup-test-data-12345' > /data/test.txt",
        ]),
        host_config: Some(bollard::models::HostConfig {
            binds: Some(vec![format!("{vol_name}:/data")]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let c = docker
        .create_container::<&str, &str>(None, config)
        .await
        .unwrap();
    docker.start_container::<&str>(&c.id, None).await.unwrap();
    docker.wait_container::<&str>(&c.id, None).next().await;
    let _ = docker
        .remove_container(
            &c.id,
            Some(bollard::container::RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;

    // Backup the volume
    let backup_dir = tempfile::tempdir().unwrap();
    let backup_path = backup_dir.path().join("backup");
    std::fs::create_dir_all(&backup_path).unwrap();

    let tar_name = format!("/backup/{vol_name}.tar.gz");
    let bind_data = format!("{vol_name}:/data:ro");
    let bind_backup = format!("{}:/backup", backup_path.display());
    let backup_config = bollard::container::Config {
        image: Some("busybox:latest"),
        cmd: Some(vec!["tar", "czf", &tar_name, "-C", "/data", "."]),
        host_config: Some(bollard::models::HostConfig {
            binds: Some(vec![bind_data, bind_backup]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let c = docker
        .create_container::<&str, &str>(None, backup_config)
        .await
        .unwrap();
    docker.start_container::<&str>(&c.id, None).await.unwrap();
    docker.wait_container::<&str>(&c.id, None).next().await;
    let _ = docker
        .remove_container(
            &c.id,
            Some(bollard::container::RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;

    // Verify backup file exists
    let tar_path = backup_path.join(format!("{vol_name}.tar.gz"));
    assert!(tar_path.exists(), "backup archive should exist");
    assert!(
        tar_path.metadata().unwrap().len() > 0,
        "archive should not be empty"
    );

    // Clear the volume
    let _ = docker.remove_volume(vol_name, None).await;
    docker
        .create_volume(bollard::volume::CreateVolumeOptions {
            name: vol_name,
            ..Default::default()
        })
        .await
        .unwrap();

    // Restore from backup
    let restore_tar = format!("/backup/{vol_name}.tar.gz");
    let restore_bind_data = format!("{vol_name}:/data");
    let restore_bind_backup = format!("{}:/backup:ro", backup_path.display());
    let restore_config = bollard::container::Config {
        image: Some("busybox:latest"),
        cmd: Some(vec!["tar", "xzf", &restore_tar, "-C", "/data"]),
        host_config: Some(bollard::models::HostConfig {
            binds: Some(vec![restore_bind_data, restore_bind_backup]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let c = docker
        .create_container::<&str, &str>(None, restore_config)
        .await
        .unwrap();
    docker.start_container::<&str>(&c.id, None).await.unwrap();
    docker.wait_container::<&str>(&c.id, None).next().await;
    let _ = docker
        .remove_container(
            &c.id,
            Some(bollard::container::RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;

    // Verify data was restored
    let verify_config = bollard::container::Config {
        image: Some("busybox:latest"),
        cmd: Some(vec!["cat", "/data/test.txt"]),
        host_config: Some(bollard::models::HostConfig {
            binds: Some(vec![format!("{vol_name}:/data:ro")]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let c = docker
        .create_container::<&str, &str>(None, verify_config)
        .await
        .unwrap();
    docker.start_container::<&str>(&c.id, None).await.unwrap();
    docker.wait_container::<&str>(&c.id, None).next().await;

    use bollard::container::LogsOptions;
    use futures_util::StreamExt;
    let mut logs = docker.logs::<&str>(
        &c.id,
        Some(LogsOptions {
            stdout: true,
            ..Default::default()
        }),
    );
    let mut output = String::new();
    while let Some(Ok(log)) = logs.next().await {
        output.push_str(&log.to_string());
    }
    assert!(
        output.contains("backup-test-data-12345"),
        "restored data should match: got '{output}'"
    );

    // Cleanup
    let _ = docker
        .remove_container(
            &c.id,
            Some(bollard::container::RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;
    let _ = docker.remove_volume(vol_name, None).await;
}
