//! #227: a paused service must not be in an agent's re-sync, or every
//! reconnect redeploys it and undoes `orca stop`.

use std::collections::HashMap;
use std::sync::Arc;

use orca_core::config::{ClusterConfig, ServiceConfig};
use orca_core::testing::MockRuntime;
use orca_core::ws_types::MasterMessage;
use tokio::sync::{RwLock, mpsc};

use super::send_reconcile;
use crate::state::{AppState, RegisteredNode, ServiceState};

const AGENT: u64 = 42;

fn pinned(name: &str) -> ServiceConfig {
    serde_json::from_value(serde_json::json!({
        "name": name,
        "image": "nginx:latest",
        "port": 8080,
        "placement": { "node": "agent-a" },
    }))
    .unwrap()
}

async fn state_with_agent() -> AppState {
    let state = AppState::new(
        ClusterConfig::default(),
        Arc::new(MockRuntime::new()),
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    );
    state.registered_nodes.write().await.insert(
        AGENT,
        RegisteredNode {
            node_id: AGENT,
            address: "agent-a:6881".into(),
            labels: HashMap::new(),
            peer_ip: None,
            last_heartbeat: chrono::Utc::now(),
            drain: false,
            cpu_percent: 0.0,
            memory_bytes: 0,
            memory_total: 0,
            disk_used: 0,
            disk_total: 0,
            net_rx: 0,
            net_tx: 0,
        },
    );
    state
}

#[tokio::test]
async fn paused_services_are_left_out_of_the_resync() {
    let state = state_with_agent().await;
    {
        let mut services = state.services.write().await;
        services.insert("portal".into(), ServiceState::from_config(pinned("portal")));
        let mut paused = ServiceState::from_config(pinned("portal-acme"));
        paused.stopped = true;
        paused.desired_replicas = 0;
        services.insert("portal-acme".into(), paused);
    }
    let (tx, mut rx) = mpsc::channel(4);

    send_reconcile(&state, AGENT, &tx).await;

    match rx.try_recv() {
        Ok(MasterMessage::Reconcile { expected }) => {
            let names: Vec<&str> = expected.iter().map(|s| s.name.as_str()).collect();
            assert_eq!(names, ["portal"], "the paused service must not be expected");
        }
        other => panic!("expected a Reconcile, got {other:?}"),
    }
}
