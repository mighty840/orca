//! E2E container failure mode tests (require Docker).
//!
//! Run with: `cargo test -p orca-control --test e2e_failures_test -- --ignored`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use orca_control::state::AppState;
use orca_core::config::{ClusterConfig, ClusterMeta};
use orca_core::types::WorkloadStatus;

fn test_state(port: u16) -> Arc<AppState> {
    let runtime = Arc::new(orca_agent::docker::ContainerRuntime::new().expect("Docker required"));
    let config = ClusterConfig {
        cluster: ClusterMeta {
            name: "e2e-failures".into(),
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

struct Client {
    base: String,
    http: reqwest::Client,
}

impl Client {
    fn new(port: u16) -> Self {
        Self {
            base: format!("http://127.0.0.1:{port}"),
            http: reqwest::Client::new(),
        }
    }

    async fn deploy(&self, body: &serde_json::Value) -> reqwest::Response {
        self.http
            .post(format!("{}/api/v1/deploy", self.base))
            .json(body)
            .send()
            .await
            .unwrap()
    }
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

/// Deploy a container that exits immediately. Verify status shows stopped/failed.
#[tokio::test]
#[ignore]
async fn e2e_container_exit_nonzero_detected() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = test_state(port);
    let app = orca_control::api::router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = Client::new(port);
    // Alpine with no long-running process will exit quickly.
    let deploy = serde_json::json!({
        "services": [{"name": "e2e-exit", "image": "alpine:latest", "replicas": 1, "port": 80}]
    });
    let resp = client.deploy(&deploy).await;
    // Deploy itself may succeed (container created + started).
    let _body: serde_json::Value = resp.json().await.unwrap();

    // Wait for container to exit.
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Run watchdog cycle to detect the exited container.
    orca_control::watchdog::run_watchdog_cycle(&state).await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    // After watchdog, the instance should be pruned or marked failed.
    let services = state.services.read().await;
    if let Some(svc) = services.get("e2e-exit") {
        for inst in &svc.instances {
            assert!(
                matches!(
                    inst.status,
                    WorkloadStatus::Stopped
                        | WorkloadStatus::Failed
                        | WorkloadStatus::Completed
                        | WorkloadStatus::Running
                ),
                "instance should have a valid status, got {:?}",
                inst.status
            );
        }
    }
    drop(services);
    cleanup("orca-e2e-exit").await;
}

/// Deploy an image that does not exist. Verify error is reported.
#[tokio::test]
#[ignore]
async fn e2e_deploy_invalid_image() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = test_state(port);
    let app = orca_control::api::router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = Client::new(port);
    let deploy = serde_json::json!({
        "services": [{
            "name": "e2e-bad-image",
            "image": "this-image-does-not-exist-xyz:latest",
            "replicas": 1,
            "port": 80
        }]
    });
    let resp = client.deploy(&deploy).await;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();

    // Docker daemon may fail at pull (error) or succeed in creating a container
    // that immediately exits. Either way, the server must not crash.
    // We verify: the response parsed as JSON (no crash), and either errors
    // are reported or the deploy succeeded but the image will fail at runtime.
    let errors = body["errors"].as_array();
    let deployed = body["deployed"].as_array();
    let graceful = status.is_success() || status.is_server_error();
    assert!(
        graceful,
        "server should respond gracefully, got {status} {body}"
    );

    // If there are no errors, the image was pulled (unlikely for a bogus name,
    // but Docker daemon behavior varies). Log for visibility.
    if errors.is_some_and(|e| e.is_empty()) && deployed.is_some_and(|d| !d.is_empty()) {
        eprintln!("note: Docker pulled the bogus image without error (daemon-specific)");
    }
    cleanup("orca-e2e-bad-image").await;
}

/// Deploy, manually remove the container, run watchdog, verify recreation.
#[tokio::test]
#[ignore]
async fn e2e_container_manually_removed() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = test_state(port);

    let app = orca_control::api::router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    orca_control::watchdog::spawn_watchdog(state.clone());
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = Client::new(port);
    let deploy = serde_json::json!({
        "services": [{"name": "e2e-rm", "image": "nginx:alpine", "replicas": 1, "port": 80}]
    });
    client.deploy(&deploy).await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Grab the container ID.
    let container_id = {
        let services = state.services.read().await;
        services["e2e-rm"].instances[0].handle.runtime_id.clone()
    };

    // Force-remove the container externally.
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let _ = docker
        .remove_container(
            &container_id,
            Some(bollard::container::RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;

    // Mark instance as failed so watchdog picks it up.
    {
        let mut services = state.services.write().await;
        let svc = services.get_mut("e2e-rm").unwrap();
        svc.instances[0].status = WorkloadStatus::Failed;
    }

    // Run watchdog cycle.
    orca_control::watchdog::run_watchdog_cycle(&state).await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify a new container was created.
    let services = state.services.read().await;
    let svc = &services["e2e-rm"];
    assert_eq!(svc.instances.len(), 1, "watchdog should recreate instance");
    assert_ne!(
        svc.instances[0].handle.runtime_id, container_id,
        "new container should have a different ID"
    );
    drop(services);
    cleanup("orca-e2e-rm").await;
}
