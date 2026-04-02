//! E2E test: health checker marks instance as unhealthy when probe fails.
//!
//! Run with: `cargo test -p orca-control --test e2e_health_unhealthy_test -- --ignored`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use orca_control::health::HealthChecker;
use orca_control::state::AppState;
use orca_core::config::{ClusterConfig, ClusterMeta};
use orca_core::types::HealthState;

fn test_state(port: u16) -> Arc<AppState> {
    let runtime = Arc::new(orca_agent::docker::ContainerRuntime::new().expect("Docker required"));
    let config = ClusterConfig {
        cluster: ClusterMeta {
            name: "e2e-health".into(),
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

/// Deploy a service with a bad health path, run the health checker, and verify
/// the instance is marked unhealthy.
#[tokio::test]
#[ignore]
async fn e2e_health_checker_marks_unhealthy() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = test_state(port);

    let app = orca_control::api::router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Deploy with a health path that will always 404
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "services": [{
            "name": "e2e-unhealthy",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80,
            "health": "/nonexistent-path-xyz"
        }]
    });
    let resp = client
        .post(format!("http://127.0.0.1:{port}/api/v1/deploy"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Run a health check cycle manually
    let checker = HealthChecker::new(state.clone());
    let mut failure_counts = HashMap::new();
    checker.check_all(&mut failure_counts).await;

    // Verify instance is marked unhealthy
    let services = state.services.read().await;
    let svc = &services["e2e-unhealthy"];
    assert_eq!(svc.instances.len(), 1);
    assert_eq!(
        svc.instances[0].health,
        HealthState::Unhealthy,
        "instance should be unhealthy after failed health check"
    );
    drop(services);

    cleanup("orca-e2e-unhealthy").await;
}
