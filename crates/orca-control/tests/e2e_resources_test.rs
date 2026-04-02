//! E2E test: deploy with resource limits and verify via Docker inspect.
//!
//! Requires Docker running locally. Run with: `cargo test -- --ignored e2e`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use orca_control::state::AppState;
use orca_core::config::{ClusterConfig, ClusterMeta};

async fn real_app_state(api_port: u16) -> Arc<AppState> {
    let runtime = Arc::new(
        orca_agent::docker::ContainerRuntime::new().expect("Docker must be running for E2E tests"),
    );
    let config = ClusterConfig {
        cluster: ClusterMeta {
            name: "e2e-resources".to_string(),
            api_port,
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

async fn cleanup_containers(prefix: &str) {
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let opts = bollard::container::ListContainersOptions {
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

#[tokio::test]
#[ignore] // requires Docker
async fn e2e_deploy_with_resource_limits() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = real_app_state(port).await;
    let app = orca_control::api::router(state.clone());

    let _handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // Deploy with resource limits: 64Mi memory, 0.5 CPU
    let deploy = serde_json::json!({
        "services": [{
            "name": "e2e-limits",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80,
            "resources": {
                "memory": "64Mi",
                "cpu": 0.5
            }
        }]
    });

    let resp = client
        .post(format!("{base}/api/v1/deploy"))
        .json(&deploy)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Inspect the container via bollard
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let info = docker
        .inspect_container("orca-e2e-limits", None)
        .await
        .unwrap();

    let host_config = info.host_config.unwrap();

    // 64Mi = 64 * 1024 * 1024 = 67108864
    assert_eq!(host_config.memory, Some(67108864));

    // 0.5 CPU = 500_000_000 nano_cpus
    assert_eq!(host_config.nano_cpus, Some(500_000_000));

    drop(state);
    cleanup_containers("orca-e2e-limits").await;
}
