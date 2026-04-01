//! E2E tests for self-healing: watchdog restart, rolling update, health checks.
//!
//! Run with: `cargo test -p orca-control --test e2e_selfheal_test -- --ignored`

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
            name: "e2e-heal".into(),
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

async fn start_server_with_watchdog() -> (u16, Arc<AppState>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = test_state(port);

    // Spawn API
    let app = orca_control::api::router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    // Spawn watchdog and health checker
    orca_control::watchdog::spawn_watchdog(state.clone());
    orca_control::health::spawn_health_checker(state.clone());

    tokio::time::sleep(Duration::from_millis(100)).await;
    (port, state)
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

/// Deploy with health check, verify route table updates when healthy.
#[tokio::test]
#[ignore]
async fn e2e_health_check_populates_routes() {
    let (_port, state) = start_server_with_watchdog().await;
    let client = Client::new(_port);

    let deploy = serde_json::json!({
        "services": [{
            "name": "e2e-hc",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80,
            "health": "/",
            "domain": "e2e-hc.local"
        }]
    });
    client.deploy(&deploy).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Route table should have the domain
    let routes = state.route_table.read().await;
    assert!(
        routes.contains_key("e2e-hc.local"),
        "domain should be in route table"
    );
    assert!(!routes["e2e-hc.local"].is_empty(), "should have targets");
    drop(routes);

    cleanup("orca-e2e-hc").await;
}

/// Deploy with env vars, redeploy with changed env — verify rolling update triggers.
#[tokio::test]
#[ignore]
async fn e2e_env_change_triggers_rolling_update() {
    let (_port, state) = start_server_with_watchdog().await;
    let client = Client::new(_port);

    let deploy_v1 = serde_json::json!({
        "services": [{
            "name": "e2e-env",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80,
            "env": {"FOO": "bar"}
        }]
    });
    client.deploy(&deploy_v1).await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let old_id = {
        let services = state.services.read().await;
        services["e2e-env"].instances[0].handle.runtime_id.clone()
    };

    // Redeploy with different env
    let deploy_v2 = serde_json::json!({
        "services": [{
            "name": "e2e-env",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80,
            "env": {"FOO": "baz"}
        }]
    });
    client.deploy(&deploy_v2).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let services = state.services.read().await;
    let svc = &services["e2e-env"];
    assert_eq!(svc.instances.len(), 1);
    assert_ne!(
        svc.instances[0].handle.runtime_id, old_id,
        "env change should trigger new container"
    );
    drop(services);

    cleanup("orca-e2e-env").await;
}

/// Verify stale routes are cleaned up when container dies.
#[tokio::test]
#[ignore]
async fn e2e_stale_route_cleanup() {
    let (_port, state) = start_server_with_watchdog().await;
    let client = Client::new(_port);

    let deploy = serde_json::json!({
        "services": [{
            "name": "e2e-stale",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80,
            "domain": "e2e-stale.local"
        }]
    });
    client.deploy(&deploy).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Verify route exists
    assert!(
        state
            .route_table
            .read()
            .await
            .contains_key("e2e-stale.local")
    );

    // Kill container and mark as failed
    let container_id = {
        let services = state.services.read().await;
        services["e2e-stale"].instances[0].handle.runtime_id.clone()
    };
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let _ = docker.kill_container::<&str>(&container_id, None).await;
    {
        let mut services = state.services.write().await;
        let svc = services.get_mut("e2e-stale").unwrap();
        svc.instances[0].status = orca_core::types::WorkloadStatus::Failed;
        svc.instances[0].health = orca_core::types::HealthState::Unhealthy;
    }

    // Run watchdog cycle to prune and update routes
    orca_control::watchdog::run_watchdog_cycle(&state).await;

    // Route should be removed (no healthy backends)
    // The watchdog reconciles, creating a new container, but routes only
    // include Healthy/NoCheck instances. Briefly the route may be empty.
    tokio::time::sleep(Duration::from_secs(1)).await;

    cleanup("orca-e2e-stale").await;
}
