//! E2E tests for internal network connectivity.
//!
//! Run with: `cargo test -p orca-control --test e2e_network_test -- --ignored`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use orca_control::state::AppState;
use orca_core::config::{ClusterConfig, ClusterMeta};

fn test_state(port: u16) -> Arc<AppState> {
    let runtime = Arc::new(orca_agent::docker::ContainerRuntime::new().expect("Docker required"));
    let config = ClusterConfig {
        cluster: ClusterMeta {
            name: "e2e-net".into(),
            api_port: port,
            ..Default::default()
        },
        ..Default::default()
    };
    Arc::new(AppState::new(
        config,
        runtime,
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ))
}

async fn start_server() -> (u16, Arc<AppState>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = test_state(port);
    let app = orca_control::api::router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(100)).await;
    (port, state)
}

async fn deploy(port: u16, body: &serde_json::Value) {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{port}/api/v1/deploy"))
        .json(body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "deploy failed");
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

/// Deploy with `internal: true` and verify container is on orca-internal network.
#[tokio::test]
#[ignore]
async fn e2e_internal_network_connects_container() {
    let (port, state) = start_server().await;

    let body = serde_json::json!({
        "services": [{
            "name": "e2e-int-net",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80,
            "internal": true
        }]
    });
    deploy(port, &body).await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let container_id = {
        let services = state.services.read().await;
        services["e2e-int-net"].instances[0]
            .handle
            .runtime_id
            .clone()
    };

    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let info = docker.inspect_container(&container_id, None).await.unwrap();
    let networks = info
        .network_settings
        .as_ref()
        .and_then(|ns| ns.networks.as_ref())
        .expect("container should have network settings");

    assert!(
        networks.contains_key("orca-internal"),
        "container should be on orca-internal, got: {:?}",
        networks.keys().collect::<Vec<_>>()
    );
    // Should also be on its service network
    let has_service_net = networks.keys().any(|k| k.starts_with("orca-"));
    assert!(has_service_net, "container should be on a service network");

    drop(state);
    cleanup("orca-e2e-int-net").await;
}

/// Deploy without `internal` and verify container is NOT on orca-internal.
#[tokio::test]
#[ignore]
async fn e2e_no_internal_network_by_default() {
    let (port, state) = start_server().await;

    let body = serde_json::json!({
        "services": [{
            "name": "e2e-no-int",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80
        }]
    });
    deploy(port, &body).await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let container_id = {
        let services = state.services.read().await;
        services["e2e-no-int"].instances[0]
            .handle
            .runtime_id
            .clone()
    };

    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let info = docker.inspect_container(&container_id, None).await.unwrap();
    let networks = info
        .network_settings
        .as_ref()
        .and_then(|ns| ns.networks.as_ref())
        .expect("container should have network settings");

    assert!(
        !networks.contains_key("orca-internal"),
        "container should NOT be on orca-internal, got: {:?}",
        networks.keys().collect::<Vec<_>>()
    );

    drop(state);
    cleanup("orca-e2e-no-int").await;
}
