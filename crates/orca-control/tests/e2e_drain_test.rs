//! E2E test: drain mode prevents dispatch to a remote node.
//!
//! This test uses a MockRuntime (no Docker required) to verify that
//! draining a node prevents services with placement targeting that
//! node from being queued for remote deploy.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use orca_control::state::{AppState, RegisteredNode};
use orca_core::config::ClusterConfig;
use orca_core::testing::MockRuntime;
use orca_core::ws_types::MasterMessage;

fn mock_state() -> Arc<AppState> {
    let runtime = Arc::new(MockRuntime::new());
    Arc::new(AppState::new(
        ClusterConfig::default(),
        runtime,
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ))
}

async fn register_node(state: &AppState, node_id: u64) {
    let node = RegisteredNode {
        node_id,
        address: format!("10.0.0.{node_id}:6880"),
        labels: HashMap::new(),
        last_heartbeat: chrono::Utc::now(),
        drain: false,
        cpu_percent: 0.0,
        memory_bytes: 0,
        memory_total: 0,
        disk_used: 0,
        disk_total: 0,
        net_rx: 0,
        net_tx: 0,
    };
    state.registered_nodes.write().await.insert(node_id, node);
}

fn services_json(name: &str, node: u64) -> Vec<orca_core::config::ServiceConfig> {
    serde_json::from_value(serde_json::json!([{
        "name": name,
        "image": "nginx:latest",
        "replicas": 1,
        "port": 80,
        "placement": { "node": node.to_string() }
    }]))
    .unwrap()
}

#[tokio::test]
async fn e2e_drain_prevents_remote_dispatch() {
    let state = mock_state();
    register_node(&state, 7).await;

    // Wire up a WS sender for node 7 so placement can succeed, then drain it.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<MasterMessage>(8);
    state
        .ws_agents
        .write()
        .await
        .insert(7, orca_control::state::AgentSession::new(tx));
    state
        .registered_nodes
        .write()
        .await
        .get_mut(&7)
        .unwrap()
        .drain = true;

    let services = services_json("e2e-drain-svc", 7);
    orca_control::reconciler::reconcile(&state, &services).await;

    // Drained node should receive no Deploy message.
    assert!(
        rx.try_recv().is_err(),
        "no deploy should be dispatched to drained node 7"
    );
}

#[tokio::test]
async fn e2e_undrained_node_receives_dispatch() {
    let state = mock_state();
    register_node(&state, 8).await;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<MasterMessage>(8);
    state
        .ws_agents
        .write()
        .await
        .insert(8, orca_control::state::AgentSession::new(tx));

    let services = services_json("e2e-active-svc", 8);
    orca_control::reconciler::reconcile(&state, &services).await;

    match rx.try_recv() {
        Ok(MasterMessage::Deploy { spec }) => {
            assert_eq!(spec.name, "e2e-active-svc");
        }
        other => panic!("expected Deploy message, got {other:?}"),
    }
}

#[tokio::test]
async fn e2e_drain_then_undrain_allows_dispatch() {
    let state = mock_state();
    register_node(&state, 9).await;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<MasterMessage>(8);
    state
        .ws_agents
        .write()
        .await
        .insert(9, orca_control::state::AgentSession::new(tx));

    // Drain — deploy should be skipped.
    state
        .registered_nodes
        .write()
        .await
        .get_mut(&9)
        .unwrap()
        .drain = true;
    let services = services_json("e2e-toggle-svc", 9);
    orca_control::reconciler::reconcile(&state, &services).await;
    assert!(
        rx.try_recv().is_err(),
        "drained node should have no commands"
    );

    // Undrain and clear service state so reconcile re-deploys.
    state
        .registered_nodes
        .write()
        .await
        .get_mut(&9)
        .unwrap()
        .drain = false;
    state.services.write().await.remove("e2e-toggle-svc");

    orca_control::reconciler::reconcile(&state, &services).await;

    match rx.try_recv() {
        Ok(MasterMessage::Deploy { spec }) => {
            assert_eq!(
                spec.name, "e2e-toggle-svc",
                "undrained node should receive deploy"
            );
        }
        other => panic!("expected Deploy after undrain, got {other:?}"),
    }
}
