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
    // Snapshot the node registry once; placement pins are resolved with the
    // same exact-match resolver the deploy path uses (#124), so what a node
    // is told to run on reconcile always agrees with deploy targeting. A pin
    // matching multiple nodes resolves Ambiguous — expected on no node —
    // mirroring the deploy path's refusal to schedule.
    let nodes = state.registered_nodes.read().await;
    if !nodes.contains_key(&node_id) {
        return;
    }

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
                    crate::placement::resolve_placement(&nodes, target)
                        == crate::placement::PlacementResolution::Node(node_id)
                })
        })
        .filter_map(|svc| {
            crate::routes::service_config_to_spec(&svc.config)
                .ok()
                .map(Box::new)
        })
        .collect();
    // Release both read guards before the channel send below — holding a
    // read across an await lets a queued writer (heartbeats write
    // registered_nodes constantly) block every subsequent reader.
    drop(services);
    drop(nodes);

    if expected.is_empty() {
        return;
    }

    info!(
        "Sending Reconcile to node {node_id} with {} expected services",
        expected.len()
    );
    let _ = tx.send(MasterMessage::Reconcile { expected }).await;
}
