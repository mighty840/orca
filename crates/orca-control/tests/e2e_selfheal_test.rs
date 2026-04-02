//! E2E tests for self-healing: watchdog restart, rolling update.
//!
//! Run with: `cargo test -p orca-control --test e2e_selfheal_test -- --ignored`

mod e2e_selfheal_helpers;

use e2e_selfheal_helpers::{Client, cleanup, start_server_with_watchdog};
use std::time::Duration;

/// Kill a container externally (simulating crash), verify watchdog restarts it.
#[tokio::test]
#[ignore]
async fn e2e_watchdog_restarts_killed_container() {
    let (_port, state) = start_server_with_watchdog().await;
    let client = Client::new(_port);

    let deploy = serde_json::json!({
        "services": [{"name": "e2e-wd", "image": "nginx:alpine", "replicas": 1, "port": 80}]
    });
    client.deploy(&deploy).await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify running
    let services = state.services.read().await;
    assert_eq!(services["e2e-wd"].instances.len(), 1);
    let container_id = services["e2e-wd"].instances[0].handle.runtime_id.clone();
    drop(services);

    // Kill the container externally
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    docker
        .kill_container::<&str>(&container_id, None)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Mark instance as failed so watchdog picks it up
    {
        let mut services = state.services.write().await;
        let svc = services.get_mut("e2e-wd").unwrap();
        svc.instances[0].status = orca_core::types::WorkloadStatus::Failed;
    }

    // Wait for watchdog cycle (30s default, but we trigger manually)
    orca_control::watchdog::run_watchdog_cycle(&state).await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify new container was created
    let services = state.services.read().await;
    let svc = &services["e2e-wd"];
    assert_eq!(
        svc.instances.len(),
        1,
        "watchdog should have recreated instance"
    );
    assert_ne!(
        svc.instances[0].handle.runtime_id, container_id,
        "new container should have different ID"
    );
    drop(services);

    cleanup("orca-e2e-wd").await;
}

/// Deploy, then redeploy with a different image — verify rolling update.
#[tokio::test]
#[ignore]
async fn e2e_rolling_update_new_image() {
    let (_port, state) = start_server_with_watchdog().await;
    let client = Client::new(_port);

    // Deploy nginx:alpine
    let deploy_v1 = serde_json::json!({
        "services": [{"name": "e2e-roll", "image": "nginx:alpine", "replicas": 1, "port": 80}]
    });
    client.deploy(&deploy_v1).await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let old_id = {
        let services = state.services.read().await;
        services["e2e-roll"].instances[0].handle.runtime_id.clone()
    };

    // Redeploy with httpd (different image)
    let deploy_v2 = serde_json::json!({
        "services": [{"name": "e2e-roll", "image": "httpd:alpine", "replicas": 1, "port": 80}]
    });
    client.deploy(&deploy_v2).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Verify new container with new image
    let services = state.services.read().await;
    let svc = &services["e2e-roll"];
    assert_eq!(svc.instances.len(), 1);
    assert_ne!(
        svc.instances[0].handle.runtime_id, old_id,
        "should be new container"
    );
    drop(services);

    // Verify old container is gone
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let result = docker.inspect_container(&old_id, None).await;
    assert!(result.is_err() || !result.unwrap().state.unwrap().running.unwrap_or(false));

    cleanup("orca-e2e-roll").await;
}
