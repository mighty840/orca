//! E2E test: rolling update with multiple replicas.
//!
//! Run with: `cargo test -p orca-control --test e2e_rolling_multi_test -- --ignored`

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
            name: "e2e-roll-multi".into(),
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

/// Deploy with 2 replicas, redeploy with new image, verify both updated.
#[tokio::test]
#[ignore]
async fn e2e_rolling_update_multi_replica() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = test_state(port);

    let app = orca_control::api::router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = reqwest::Client::new();

    // Deploy v1 with 2 replicas
    let v1 = serde_json::json!({
        "services": [{
            "name": "e2e-rmulti",
            "image": "nginx:alpine",
            "replicas": 2,
            "port": 80
        }]
    });
    let resp = client
        .post(format!("http://127.0.0.1:{port}/api/v1/deploy"))
        .json(&v1)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    tokio::time::sleep(Duration::from_secs(4)).await;

    // Verify 2 replicas running
    let old_ids: Vec<String> = {
        let services = state.services.read().await;
        let svc = &services["e2e-rmulti"];
        assert_eq!(svc.instances.len(), 2, "should have 2 replicas");
        svc.instances
            .iter()
            .map(|i| i.handle.runtime_id.clone())
            .collect()
    };

    // Deploy v2 with different image (triggers rolling update)
    let v2 = serde_json::json!({
        "services": [{
            "name": "e2e-rmulti",
            "image": "httpd:alpine",
            "replicas": 2,
            "port": 80
        }]
    });
    let resp = client
        .post(format!("http://127.0.0.1:{port}/api/v1/deploy"))
        .json(&v2)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    tokio::time::sleep(Duration::from_secs(8)).await;

    // Verify 2 replicas running with new IDs
    let services = state.services.read().await;
    let svc = &services["e2e-rmulti"];
    assert_eq!(svc.instances.len(), 2, "should still have 2 replicas");
    for inst in &svc.instances {
        assert_eq!(inst.status, WorkloadStatus::Running);
        assert!(
            !old_ids.contains(&inst.handle.runtime_id),
            "instance should have a new container ID after rolling update"
        );
    }
    drop(services);

    cleanup("orca-e2e-rmulti").await;
}
