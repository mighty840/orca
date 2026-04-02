//! Docker-based stress tests for orca-control.
//!
//! Run with: `cargo test -p orca-control --test stress_docker_test -- --ignored`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use orca_control::api::router;
use orca_control::state::AppState;
use orca_core::config::{ClusterConfig, ClusterMeta};

async fn real_state(port: u16) -> Arc<AppState> {
    let rt = Arc::new(orca_agent::docker::ContainerRuntime::new().expect("Docker required"));
    let cfg = ClusterConfig {
        cluster: ClusterMeta {
            name: "stress".into(),
            api_port: port,
            ..Default::default()
        },
        ..Default::default()
    };
    Arc::new(AppState::new(
        cfg,
        rt,
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ))
}

async fn boot() -> (u16, Arc<AppState>, tokio::task::JoinHandle<()>) {
    let ln = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = ln.local_addr().unwrap().port();
    let st = real_state(port).await;
    let app = router(st.clone());
    let h = tokio::spawn(async move { axum::serve(ln, app).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(100)).await;
    (port, st, h)
}

async fn cleanup(prefix: &str) {
    let dk = bollard::Docker::connect_with_local_defaults().unwrap();
    let opts = bollard::container::ListContainersOptions::<String> {
        all: true,
        filters: HashMap::from([("name".into(), vec![prefix.into()])]),
        ..Default::default()
    };
    if let Ok(cs) = dk.list_containers(Some(opts)).await {
        for c in cs {
            if let Some(id) = c.id {
                let _ = dk
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

fn http() -> reqwest::Client {
    reqwest::Client::new()
}

fn url(port: u16, path: &str) -> String {
    format!("http://127.0.0.1:{port}{path}")
}

fn deploy_json(name: &str) -> serde_json::Value {
    serde_json::json!({
        "services": [{"name": name, "image": "nginx:alpine", "replicas": 1, "port": 80}]
    })
}

#[tokio::test]
#[ignore]
async fn stress_rapid_deploy_undeploy() {
    cleanup("orca-stress-").await;
    let (port, _st, _h) = boot().await;
    let c = http();

    for i in 1..=10 {
        let r = c
            .post(url(port, "/api/v1/deploy"))
            .json(&deploy_json(&format!("stress-{i}")))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "deploy stress-{i} failed");
    }
    tokio::time::sleep(Duration::from_secs(10)).await;

    let body: serde_json::Value = c
        .get(url(port, "/api/v1/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let svcs = body["services"].as_array().unwrap();
    for i in 1..=10 {
        let n = format!("stress-{i}");
        assert!(svcs.iter().any(|s| s["name"] == n), "missing {n}");
    }

    for i in 1..=10 {
        let r = c
            .delete(url(port, &format!("/api/v1/services/stress-{i}")))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "delete stress-{i} failed");
    }
    tokio::time::sleep(Duration::from_secs(3)).await;

    let body: serde_json::Value = c
        .get(url(port, "/api/v1/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for i in 1..=10 {
        let n = format!("stress-{i}");
        if let Some(s) = body["services"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == n)
        {
            assert_eq!(s["running_replicas"], 0, "{n} still running");
        }
    }
    cleanup("orca-stress-").await;
}

#[tokio::test]
#[ignore]
async fn stress_scale_to_10_and_back() {
    cleanup("orca-stress-").await;
    let (port, st, _h) = boot().await;
    let c = http();

    c.post(url(port, "/api/v1/deploy"))
        .json(&deploy_json("stress-scale"))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;

    let r = c
        .post(url(port, "/api/v1/services/stress-scale/scale"))
        .json(&serde_json::json!({ "replicas": 10 }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    tokio::time::sleep(Duration::from_secs(15)).await;
    assert_eq!(st.services.read().await["stress-scale"].instances.len(), 10);

    c.post(url(port, "/api/v1/services/stress-scale/scale"))
        .json(&serde_json::json!({ "replicas": 1 }))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(st.services.read().await["stress-scale"].instances.len(), 1);

    cleanup("orca-stress-").await;
}

#[tokio::test]
#[ignore]
async fn stress_container_churn_watchdog() {
    cleanup("orca-stress-").await;
    let ln = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = ln.local_addr().unwrap().port();
    let st = real_state(port).await;
    let app = router(st.clone());
    tokio::spawn(async move { axum::serve(ln, app).await.unwrap() });
    orca_control::watchdog::spawn_watchdog(st.clone());
    tokio::time::sleep(Duration::from_millis(100)).await;

    let c = http();
    c.post(url(port, "/api/v1/deploy"))
        .json(&deploy_json("stress-churn"))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;

    let first_id = st.services.read().await["stress-churn"].instances[0]
        .handle
        .runtime_id
        .clone();
    let dk = bollard::Docker::connect_with_local_defaults().unwrap();

    for _ in 0..5 {
        let cid = st.services.read().await["stress-churn"].instances[0]
            .handle
            .runtime_id
            .clone();
        let _ = dk.kill_container::<&str>(&cid, None).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        {
            let mut svcs = st.services.write().await;
            svcs.get_mut("stress-churn").unwrap().instances[0].status =
                orca_core::types::WorkloadStatus::Failed;
        }
        orca_control::watchdog::run_watchdog_cycle(&st).await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert_eq!(st.services.read().await["stress-churn"].instances.len(), 1);
    }

    let final_id = st.services.read().await["stress-churn"].instances[0]
        .handle
        .runtime_id
        .clone();
    assert_ne!(first_id, final_id, "container ID should have changed");
    cleanup("orca-stress-").await;
}

#[tokio::test]
#[ignore]
async fn stress_repeated_deploy_same_service() {
    cleanup("orca-stress-").await;
    let (port, st, _h) = boot().await;
    let c = http();
    let body = deploy_json("stress-idem");

    for round in 0..20 {
        let r = c
            .post(url(port, "/api/v1/deploy"))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "round {round} deploy failed");
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(
            st.services.read().await["stress-idem"].instances.len(),
            1,
            "round {round}: expected 1 instance"
        );
    }
    cleanup("orca-stress-").await;
}
