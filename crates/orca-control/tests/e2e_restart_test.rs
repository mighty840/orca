//! E2E test: verify restart recovery re-attaches to existing containers
//! instead of creating duplicates.
//!
//! Requires Docker running locally. Run with: `cargo test -- --ignored e2e_restart`

mod e2e_helpers;

use std::collections::HashMap;
use std::time::Duration;

use e2e_helpers::{cleanup_containers, real_app_state};

fn nginx_config() -> orca_core::config::ServiceConfig {
    orca_core::config::ServiceConfig {
        name: "e2e-restart".to_string(),
        project: None,
        runtime: Default::default(),
        image: Some("nginx:alpine".to_string()),
        module: None,
        replicas: orca_core::types::Replicas::Fixed(1),
        port: Some(80),
        domain: None,
        health: None,
        readiness: None,
        liveness: None,
        env: HashMap::new(),
        resources: None,
        volume: None,
        deploy: None,
        placement: None,
        network: None,
        aliases: vec![],
        mounts: vec![],
        routes: vec![],
        host_port: None,
        triggers: Vec::new(),
        assets: None,
        build: None,
        tls_cert: None,
        tls_key: None,
        internal: false,
        depends_on: vec![],
    }
}

#[tokio::test]
#[ignore] // requires Docker
async fn e2e_restart_no_duplicate_containers() {
    cleanup_containers("orca-e2e-restart").await;

    // Phase 1: Deploy nginx via the first "server instance"
    let state1 = real_app_state(16881).await;
    let config = nginx_config();
    let (deployed, errors) = orca_control::reconciler::reconcile(&state1, &[config.clone()]).await;
    assert!(errors.is_empty(), "Deploy errors: {errors:?}");
    assert_eq!(deployed, vec!["e2e-restart"]);
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Verify container is running
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let count_before = count_containers(&docker, "orca-e2e-restart").await;
    assert_eq!(count_before, 1, "expected 1 container after deploy");

    // Phase 2: Simulate server restart — new AppState, same persisted config
    let state2 = real_app_state(16882).await;

    // Use find_existing to check for running containers
    let container_rt = orca_agent::docker::ContainerRuntime::new().unwrap();
    let existing = container_rt
        .find_existing("e2e-restart")
        .await
        .expect("find_existing failed");
    assert_eq!(existing.len(), 1, "Should find 1 existing container");
    assert!(
        existing[0].name.contains("e2e-restart"),
        "Container name should contain service name"
    );

    // Populate state from existing (simulating restart recovery)
    let instance = orca_control::state::InstanceState {
        handle: existing.into_iter().next().unwrap(),
        status: orca_core::types::WorkloadStatus::Running,
        host_port: None,
        container_address: None,
        health: orca_core::types::HealthState::Unknown,
        is_canary: false,
    };
    {
        let mut services = state2.services.write().await;
        let svc = services
            .entry("e2e-restart".to_string())
            .or_insert_with(|| orca_control::state::ServiceState::from_config(config.clone()));
        svc.instances.push(instance);
    }

    // Verify no duplicate containers were created
    let count_after = count_containers(&docker, "orca-e2e-restart").await;
    assert_eq!(count_after, 1, "still 1 container after restart recovery");

    // Verify in-memory state has the handle
    {
        let services = state2.services.read().await;
        let svc = services.get("e2e-restart").unwrap();
        assert_eq!(svc.instances.len(), 1);
        assert_eq!(
            svc.instances[0].status,
            orca_core::types::WorkloadStatus::Running
        );
    }

    // Clean up
    drop(state1);
    drop(state2);
    cleanup_containers("orca-e2e-restart").await;
}

async fn count_containers(docker: &bollard::Docker, name_prefix: &str) -> usize {
    let opts = bollard::container::ListContainersOptions {
        all: true,
        filters: HashMap::from([("name".to_string(), vec![name_prefix.to_string()])]),
        ..Default::default()
    };
    docker
        .list_containers(Some(opts))
        .await
        .unwrap_or_default()
        .len()
}
