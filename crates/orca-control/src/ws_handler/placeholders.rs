//! Manage `remote-{node_id}` placeholder `InstanceState` entries that mirror
//! services placed on a connected agent node.

use tracing::info;

use orca_core::runtime::WorkloadHandle;
use orca_core::types::{HealthState, WorkloadStatus};

use crate::state::{AppState, InstanceState};

/// Ensure a `remote-{node_id}` placeholder `InstanceState` exists for every
/// service placed on this node. Called on WS connect so the heartbeat and
/// DeployResult handlers always have a slot to update.
pub(super) async fn upsert_remote_placeholders(state: &AppState, node_id: u64) {
    let node_addr = {
        let nodes = state.registered_nodes.read().await;
        nodes.get(&node_id).map(|n| n.address.clone())
    };
    let Some(node_addr) = node_addr else { return };
    let placeholder_id = format!("remote-{node_id}");
    let mut services = state.services.write().await;
    for svc in services.values_mut() {
        if !svc
            .config
            .placement
            .as_ref()
            .and_then(|p| p.node.as_ref())
            .is_some_and(|t| node_addr.contains(t.as_str()) || t == &node_id.to_string())
        {
            continue;
        }
        if svc
            .instances
            .iter()
            .any(|i| i.handle.runtime_id == placeholder_id)
        {
            continue;
        }
        svc.instances.push(InstanceState {
            handle: WorkloadHandle {
                runtime_id: placeholder_id.clone(),
                name: placeholder_id.clone(),
                metadata: Default::default(),
            },
            status: WorkloadStatus::Stopped,
            host_port: None,
            container_address: None,
            health: HealthState::Unknown,
            is_canary: false,
            started_at: std::time::Instant::now(),
        });
        info!(service = %svc.config.name, node_id, "registered remote placeholder");
    }
}

/// Remove all `remote-{node_id}` placeholder instances on WS disconnect.
pub(super) async fn remove_remote_placeholders(state: &AppState, node_id: u64) {
    let placeholder_id = format!("remote-{node_id}");
    let mut services = state.services.write().await;
    for svc in services.values_mut() {
        svc.instances
            .retain(|i| i.handle.runtime_id != placeholder_id);
    }
}
