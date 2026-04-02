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

async fn register_node_with_labels(
    state: &AppState,
    node_id: u64,
    labels: HashMap<String, String>,
) {
    let node = RegisteredNode {
        node_id,
        address: format!("10.0.0.{node_id}:6880"),
        labels,
        last_heartbeat: chrono::Utc::now(),
        drain: false,
    };
    let mut nodes = state.registered_nodes.write().await;
    nodes.insert(node_id, node);
}

#[tokio::test]
async fn e2e_drain_prevents_remote_dispatch() {
    let state = mock_state();

    // Register a node
    register_node_with_labels(&state, 7, HashMap::new()).await;

    // Drain the node via the state directly (simulating API call)
    {
        let mut nodes = state.registered_nodes.write().await;
        nodes.get_mut(&7).unwrap().drain = true;
    }

    // Deploy a service targeting the drained node by node_id
    let services: Vec<orca_core::config::ServiceConfig> =
        serde_json::from_value(serde_json::json!([{
            "name": "e2e-drain-svc",
            "image": "nginx:latest",
            "replicas": 1,
            "port": 80,
            "placement": { "node": "7" }
        }]))
        .unwrap();

    let (_deployed, _errors) = orca_control::reconciler::reconcile(&state, &services).await;

    // Verify: no commands were queued for the drained node
    let pending = state.pending_commands.read().await;
    assert!(
        !pending.contains_key(&7),
        "no commands should be queued for drained node 7"
    );
}

#[tokio::test]
async fn e2e_undrained_node_receives_dispatch() {
    let state = mock_state();

    // Register a node (not drained)
    register_node_with_labels(&state, 8, HashMap::new()).await;

    // Deploy a service targeting the healthy node
    let services: Vec<orca_core::config::ServiceConfig> =
        serde_json::from_value(serde_json::json!([{
            "name": "e2e-active-svc",
            "image": "nginx:latest",
            "replicas": 1,
            "port": 80,
            "placement": { "node": "8" }
        }]))
        .unwrap();

    let (_deployed, _errors) = orca_control::reconciler::reconcile(&state, &services).await;

    // Verify: commands WERE queued for the healthy node
    let pending = state.pending_commands.read().await;
    let cmds = pending.get(&8).cloned().unwrap_or_default();
    assert_eq!(
        cmds.len(),
        1,
        "exactly one deploy command should be queued for node 8"
    );
    assert_eq!(cmds[0]["action"], "deploy");
}

#[tokio::test]
async fn e2e_drain_then_undrain_allows_dispatch() {
    let state = mock_state();

    register_node_with_labels(&state, 9, HashMap::new()).await;

    // Drain the node
    {
        let mut nodes = state.registered_nodes.write().await;
        nodes.get_mut(&9).unwrap().drain = true;
    }

    // Deploy while drained — should NOT queue
    let services: Vec<orca_core::config::ServiceConfig> =
        serde_json::from_value(serde_json::json!([{
            "name": "e2e-toggle-svc",
            "image": "nginx:latest",
            "replicas": 1,
            "port": 80,
            "placement": { "node": "9" }
        }]))
        .unwrap();

    orca_control::reconciler::reconcile(&state, &services).await;

    {
        let pending = state.pending_commands.read().await;
        assert!(
            !pending.contains_key(&9),
            "drained node should have no commands"
        );
    }

    // Undrain
    {
        let mut nodes = state.registered_nodes.write().await;
        nodes.get_mut(&9).unwrap().drain = false;
    }

    // Clear the service state so reconcile doesn't skip as "already deployed"
    {
        let mut svcs = state.services.write().await;
        svcs.remove("e2e-toggle-svc");
    }

    // Deploy again — should queue this time
    orca_control::reconciler::reconcile(&state, &services).await;

    let pending = state.pending_commands.read().await;
    let cmds = pending.get(&9).cloned().unwrap_or_default();
    assert_eq!(
        cmds.len(),
        1,
        "undrained node should receive deploy command"
    );
}
