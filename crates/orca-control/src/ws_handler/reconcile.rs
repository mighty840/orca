//! Drain queued commands and send Reconcile messages to a freshly connected
//! agent node.

use tokio::sync::mpsc;
use tracing::{info, warn};

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
    let mut configs: Vec<orca_core::config::ServiceConfig> = services
        .values()
        // A paused service is not expected anywhere: including it made every
        // agent reconnect redeploy it, undoing `orca stop` (#227).
        .filter(|svc| !svc.stopped)
        // An adopted record has no env or volumes; recreating from it on
        // rejoin replaced working containers with hollow ones (#313).
        .filter(|svc| {
            if svc.config.adopted {
                warn!(
                    "Reconcile for node {node_id}: leaving out {}: it was adopted from a \
                     running container and has no full config; deploy it from its service.toml",
                    svc.config.name
                );
            }
            !svc.config.adopted
        })
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
        .map(|svc| svc.config.clone())
        .collect();
    // Release both read guards before the channel send below — holding a
    // read across an await lets a queued writer (heartbeats write
    // registered_nodes constantly) block every subsequent reader.
    drop(services);
    drop(nodes);

    // The agent deploys the list in order, one after another. HashMap order
    // started an app before the database it depends on (#311); sort by name
    // first so the order is the same on every reconnect.
    configs.sort_by(|a, b| a.name.cmp(&b.name));
    let expected: Vec<Box<orca_core::types::WorkloadSpec>> = crate::topo_sort::topo_sort(&configs)
        .iter()
        .filter_map(
            |config| match crate::routes::service_config_to_spec(config) {
                Ok(spec) => Some(Box::new(spec)),
                Err(e) => {
                    // Left out of the Reconcile, so the agent keeps whatever it
                    // runs now. Say why instead of skipping it silently.
                    warn!(
                        "Reconcile for node {node_id}: skipping {}: {e:#}",
                        config.name
                    );
                    None
                }
            },
        )
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

#[cfg(test)]
#[path = "reconcile_tests.rs"]
mod tests;
