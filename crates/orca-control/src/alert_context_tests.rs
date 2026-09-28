use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::RwLock;

use orca_ai::monitor::ContextProvider;
use orca_core::api_types::FailureInfo;
use orca_core::config::{ClusterConfig, ClusterMeta, ServiceConfig};
use orca_core::runtime::WorkloadHandle;
use orca_core::testing::MockRuntime;
use orca_core::types::{HealthState, WorkloadStatus};

use super::StateContextProvider;
use crate::state::{AppState, InstanceState, RegisteredNode, ServiceState};

pub(crate) fn state() -> Arc<AppState> {
    Arc::new(AppState::new(
        ClusterConfig {
            cluster: ClusterMeta {
                name: "test".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(MockRuntime::with_host_port(9000)),
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ))
}

pub(crate) fn config(json: serde_json::Value) -> ServiceConfig {
    serde_json::from_value(json).expect("service config")
}

pub(crate) fn instance(runtime_id: &str, status: WorkloadStatus) -> InstanceState {
    InstanceState {
        handle: WorkloadHandle {
            runtime_id: runtime_id.into(),
            name: runtime_id.into(),
            metadata: Default::default(),
        },
        status,
        host_port: None,
        container_address: None,
        health: HealthState::NoCheck,
        is_canary: false,
        started_at: std::time::Instant::now(),
    }
}

fn node(id: u64, address: &str, heartbeat_secs_ago: i64) -> RegisteredNode {
    RegisteredNode {
        node_id: id,
        address: address.into(),
        peer_ip: None,
        labels: HashMap::new(),
        last_heartbeat: Utc::now() - chrono::Duration::seconds(heartbeat_secs_ago),
        drain: false,
        cpu_percent: 12.0,
        memory_bytes: 1,
        memory_total: 4,
        disk_used: 0,
        disk_total: 0,
        net_rx: 0,
        net_tx: 0,
    }
}

#[tokio::test]
async fn the_snapshot_carries_each_services_evidence() {
    let state = state();
    let api = config(serde_json::json!({
        "name": "api",
        "image": "api:1.3",
        "resources": {"memory": "512Mi"},
        "readiness": {"path": "/healthz", "port": 8080},
    }));
    {
        let mut history = state.deploy_history.write().await;
        let mut old = api.clone();
        old.image = Some("api:1.2".into());
        history.record(&old);
        history.record(&api);
    }
    let mut svc = ServiceState::from_config(api);
    svc.instances
        .push(instance("remote-7", WorkloadStatus::Failed));
    state.services.write().await.insert("api".into(), svc);
    state
        .registered_nodes
        .write()
        .await
        .insert(7, node(7, "10.0.0.7:6880", 3));
    state.last_failures.write().await.insert(
        "api".into(),
        FailureInfo {
            reason: "OOMKilled".into(),
            message: "killed".into(),
            exit_code: Some(137),
            restart_count: 2,
            observed_at: Utc::now(),
        },
    );

    let ctx = StateContextProvider::for_state(state)
        .snapshot()
        .await
        .unwrap();
    let s = &ctx.services[0];
    assert_eq!(s.image.as_deref(), Some("api:1.3"));
    assert_eq!(s.previous_image.as_deref(), Some("api:1.2"));
    assert!(s.last_deploy_at.is_some());
    assert_eq!(s.memory_limit.as_deref(), Some("512Mi"));
    assert_eq!(s.node.as_deref(), Some("10.0.0.7:6880"));
    assert_eq!(
        s.health_check.as_deref(),
        Some("readiness GET /healthz on port 8080 every 10s, timeout 3s, fails after 3")
    );
    let f = s.last_failure.as_ref().expect("the recorded failure");
    assert_eq!((f.reason.as_str(), f.exit_code), ("OOMKilled", Some(137)));
    assert!(
        s.recent_logs.is_empty(),
        "logs are fetched per alert, not per snapshot"
    );
}

#[tokio::test]
async fn a_node_without_heartbeats_is_unreachable() {
    let state = state();
    {
        let mut nodes = state.registered_nodes.write().await;
        nodes.insert(1, node(1, "fresh:6880", 5));
        nodes.insert(2, node(2, "stale:6880", 300));
    }
    let ctx = StateContextProvider::for_state(state)
        .snapshot()
        .await
        .unwrap();
    let by_addr = |a: &str| ctx.nodes.iter().find(|n| n.address == a).unwrap();
    assert_eq!(by_addr("fresh:6880").status, "healthy");
    let stale = by_addr("stale:6880");
    assert_eq!(stale.status, "unreachable");
    assert!(stale.heartbeat_age_secs.unwrap() >= 300);
}

#[tokio::test]
async fn the_same_image_twice_is_no_previous_image() {
    let state = state();
    let api = config(serde_json::json!({"name": "api", "image": "api:1.3"}));
    {
        let mut history = state.deploy_history.write().await;
        history.record(&api);
        history.record(&api);
    }
    state
        .services
        .write()
        .await
        .insert("api".into(), ServiceState::from_config(api));
    let ctx = StateContextProvider::for_state(state)
        .snapshot()
        .await
        .unwrap();
    assert_eq!(ctx.services[0].previous_image, None);
}
