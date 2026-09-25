//! #151: on startup, a service pinned to the master's own hostname was filed
//! as a remote placeholder. Its running container was never re-attached, so
//! orca showed it 0/1, stopped health-checking and routing it, and alerted.

use std::collections::HashMap;
use std::sync::Arc;

use orca_core::config::{ClusterConfig, ServiceConfig};
use orca_core::testing::MockRuntime;
use tokio::sync::RwLock;

use super::restore_or_reconcile;
use crate::state::AppState;

fn state() -> AppState {
    AppState::new(
        ClusterConfig::default(),
        Arc::new(MockRuntime::new()),
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    )
}

fn pinned(name: &str, node: &str) -> ServiceConfig {
    serde_json::from_value(serde_json::json!({
        "name": name,
        "image": "nginx:latest",
        "port": 8080,
        "placement": { "node": node },
    }))
    .unwrap()
}

async fn instance_ids(state: &AppState, name: &str) -> Vec<String> {
    state.services.read().await[name]
        .instances
        .iter()
        .map(|i| i.handle.runtime_id.clone())
        .collect()
}

#[tokio::test]
async fn pin_to_the_masters_own_hostname_restores_locally() {
    let state = state();
    let config = pinned("kontakt-relay", &crate::placement::master_hostname());

    restore_or_reconcile(&state, &config).await.unwrap();

    let ids = instance_ids(&state, "kontakt-relay").await;
    assert_eq!(ids.len(), 1, "the local workload must be tracked: {ids:?}");
    assert!(
        !ids[0].starts_with("remote-"),
        "a self-pinned service must not become a remote placeholder: {ids:?}"
    );
}

#[tokio::test]
async fn pin_to_another_node_still_registers_a_placeholder() {
    let state = state();
    let config = pinned("nextcloud", "some-agent-that-is-not-connected-yet");

    restore_or_reconcile(&state, &config).await.unwrap();

    let services = state.services.read().await;
    let svc = &services["nextcloud"];
    assert!(svc.instances.is_empty(), "the agent owns the container");
    assert_eq!(svc.desired_replicas, 1);
}
