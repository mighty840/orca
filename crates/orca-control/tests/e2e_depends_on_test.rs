//! E2E test: depends_on startup ordering with real Docker containers.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use orca_control::state::AppState;
use orca_core::config::{ClusterConfig, ClusterMeta};

fn test_state(port: u16) -> Arc<AppState> {
    let runtime = Arc::new(orca_agent::docker::ContainerRuntime::new().expect("Docker required"));
    Arc::new(AppState::new(
        ClusterConfig {
            cluster: ClusterMeta {
                name: "e2e-deps".into(),
                api_port: port,
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

/// Deploy services with depends_on chain: app depends on db.
/// Verify both deploy successfully and db starts before app.
#[tokio::test]
#[ignore]
async fn e2e_depends_on_db_before_app() {
    let state = test_state(0);

    let services = vec![
        orca_core::config::ServiceConfig {
            name: "e2e-dep-app".into(),
            project: None,
            runtime: Default::default(),
            image: Some("nginx:alpine".into()),
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
            triggers: vec![],
            assets: None,
            build: None,
            tls_cert: None,
            tls_key: None,
            internal: false,
            depends_on: vec!["e2e-dep-db".into()],
        },
        orca_core::config::ServiceConfig {
            name: "e2e-dep-db".into(),
            project: None,
            runtime: Default::default(),
            image: Some("redis:7-alpine".into()),
            module: None,
            replicas: orca_core::types::Replicas::Fixed(1),
            port: Some(6379),
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
            triggers: vec![],
            assets: None,
            build: None,
            tls_cert: None,
            tls_key: None,
            internal: false,
            depends_on: vec![],
        },
    ];

    // Deploy — depends_on should sort db before app
    let (deployed, errors) = orca_control::reconciler::reconcile(&state, &services).await;
    assert!(errors.is_empty(), "errors: {errors:?}");
    assert_eq!(deployed.len(), 2);
    // db should be first in deployed list (reconciled first)
    assert_eq!(deployed[0], "e2e-dep-db");
    assert_eq!(deployed[1], "e2e-dep-app");

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Both should be running
    let svcs = state.services.read().await;
    assert!(svcs.contains_key("e2e-dep-db"));
    assert!(svcs.contains_key("e2e-dep-app"));
    drop(svcs);

    cleanup("orca-e2e-dep").await;
}
