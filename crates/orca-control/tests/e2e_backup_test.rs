//! E2E tests: volume backup and restore with real Docker.

use futures_util::StreamExt;

/// Pull `image` if it isn't already present locally. The test creates
/// short-lived busybox containers via `create_container`, which doesn't pull
/// implicitly — without this the test 404s on a fresh machine.
async fn ensure_image(docker: &bollard::Docker, image: &str) {
    if docker.inspect_image(image).await.is_ok() {
        return;
    }
    let opts = bollard::image::CreateImageOptions {
        from_image: image,
        ..Default::default()
    };
    let mut stream = docker.create_image(Some(opts), None, None);
    while let Some(item) = stream.next().await {
        item.unwrap_or_else(|e| panic!("failed to pull {image}: {e}"));
    }
}

/// Deploy a service with a volume, write data, backup, clear, restore, verify.
#[tokio::test]
#[ignore]
async fn e2e_backup_and_restore_volume() {
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    ensure_image(&docker, "busybox:latest").await;
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
