//! Drain queued commands and send Reconcile messages to a freshly connected
//! agent node.

use tokio::sync::mpsc;
use tracing::info;

use orca_core::ws_types::MasterMessage;

use crate::state::AppState;

/// Drain pending commands from the HTTP queue and send them over WS.
pub(super) async fn drain_pending_commands(
    state: &AppState,
    node_id: u64,
    tx: &mpsc::Sender<MasterMessage>,
) {
    let commands = {
        let mut pending = state.pending_commands.write().await;
        pending.remove(&node_id).unwrap_or_default()
    };
    for cmd in commands {
        if let Some(action) = cmd.get("action").and_then(|a| a.as_str()) {
            match action {
                "deploy" => {
                    if let Some(spec) = cmd.get("spec")
                        && let Ok(spec) = serde_json::from_value(spec.clone())
                    {
                        let _ = tx
                            .send(MasterMessage::Deploy {
                                spec: Box::new(spec),
                            })
                            .await;
                    }
                }
                "stop" => {
                    if let Some(name) = cmd.get("service_name").and_then(|n| n.as_str()) {
                        let _ = tx
                            .send(MasterMessage::Stop {
                                service_name: name.to_string(),
                            })
                            .await;
                    }
                }
                _ => {}
            }
        }
    }
}

/// Send the list of services expected on this agent node so it can
/// reconcile (redeploy missing containers, stop unexpected ones).
pub(super) async fn send_reconcile(
    state: &AppState,
    node_id: u64,
    tx: &mpsc::Sender<MasterMessage>,
) {
    // Find the node's address/hostname for placement matching
    let node_address = {
        let nodes = state.registered_nodes.read().await;
        nodes.get(&node_id).map(|n| n.address.clone())
    };
    let Some(node_addr) = node_address else {
        return;
    };

    // Collect all services whose placement targets this node
    let services = state.services.read().await;
    let expected: Vec<Box<orca_core::types::WorkloadSpec>> = services
        .values()
        .filter(|svc| {
            svc.config
                .placement
                .as_ref()
                .and_then(|p| p.node.as_ref())
                .is_some_and(|target| {
                    node_addr.contains(target.as_str()) || target == &node_id.to_string() || {
                        let nodes_guard =
                            futures_util::FutureExt::now_or_never(state.registered_nodes.read());
                        nodes_guard
                            .and_then(|nodes| {
                                nodes
                                    .get(&node_id)
                                    .and_then(|n| n.labels.get("hostname").map(|h| h == target))
                            })
                            .unwrap_or(false)
                    }
                })
        })
        .filter_map(|svc| {
            crate::routes::service_config_to_spec(&svc.config)
                .ok()
                .map(Box::new)
        })
        .collect();

    if expected.is_empty() {
        return;
    }

    info!(
        "Sending Reconcile to node {node_id} with {} expected services",
        expected.len()
    );
    let _ = tx.send(MasterMessage::Reconcile { expected }).await;
}
