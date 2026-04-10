//! WebSocket handler for agent↔master streaming communication.
//!
//! Agents connect to `GET /api/v1/ws/agent?token=<cluster_token>&node_id=<id>`.
//! After the upgrade, messages flow bidirectionally using [`AgentMessage`] and
//! [`MasterMessage`] JSON frames.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use orca_core::ws_types::{AgentMessage, MasterMessage};

use crate::state::AppState;

/// Query params for the WS upgrade request.
#[derive(Deserialize)]
pub struct WsQuery {
    token: String,
    node_id: u64,
}

/// Per-node sender so the master can push messages to a connected agent.
pub type AgentSender = mpsc::Sender<MasterMessage>;

/// Handle the WebSocket upgrade request.
///
/// Authenticates via the `token` query param, then upgrades to a
/// bidirectional WebSocket connection.
pub async fn ws_agent_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(query): Query<WsQuery>,
) -> impl IntoResponse {
    // Authenticate: check token against cluster tokens
    let valid = state.api_tokens.iter().any(|t| t == &query.token)
        || state
            .cluster_config
            .token
            .iter()
            .any(|t| t.value == query.token);

    if !valid {
        return (axum::http::StatusCode::UNAUTHORIZED, "invalid token").into_response();
    }

    let node_id = query.node_id;
    info!("WebSocket upgrade accepted for node {node_id}");

    ws.on_upgrade(move |socket| handle_agent_ws(socket, state, node_id))
        .into_response()
}

/// Main WebSocket loop for a connected agent.
async fn handle_agent_ws(socket: WebSocket, state: Arc<AppState>, node_id: u64) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Channel for master → agent messages (deploy commands, log requests, etc.)
    let (tx, mut rx) = mpsc::channel::<MasterMessage>(64);

    // Register this agent's sender so other parts of the system can push messages.
    {
        let mut senders = state.ws_agents.write().await;
        senders.insert(node_id, tx.clone());
    }

    info!("Agent {node_id} connected via WebSocket");

    // Send initial Ack
    let ack = MasterMessage::Ack { node_id };
    if let Ok(json) = serde_json::to_string(&ack) {
        let _ = ws_tx.send(Message::Text(json.into())).await;
    }

    // Drain any pending commands that were queued before the WS connected.
    drain_pending_commands(&state, node_id, &tx).await;

    // Send Reconcile with all services expected on this node so the agent
    // can self-heal after a restart (fixes #21: stale remote state).
    send_reconcile(&state, node_id, &tx).await;

    // Spawn task to forward master→agent messages from the channel to the WS.
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let json = match serde_json::to_string(&msg) {
                Ok(j) => j,
                Err(e) => {
                    error!("Failed to serialize MasterMessage: {e}");
                    continue;
                }
            };
            if ws_tx.send(Message::Text(json.into())).await.is_err() {
                break; // connection closed
            }
        }
    });

    // Process incoming agent messages.
    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Text(text) => {
                if let Err(e) = handle_agent_message(&state, node_id, &text, &tx).await {
                    warn!("Error handling agent message from {node_id}: {e}");
                }
            }
            Message::Close(_) => break,
            _ => {} // ignore binary, ping, pong (axum handles pong auto)
        }
    }

    // Cleanup on disconnect
    send_task.abort();
    {
        let mut senders = state.ws_agents.write().await;
        senders.remove(&node_id);
    }
    info!("Agent {node_id} WebSocket disconnected");
}

/// Process a single message from the agent.
async fn handle_agent_message(
    state: &AppState,
    node_id: u64,
    text: &str,
    _tx: &mpsc::Sender<MasterMessage>,
) -> anyhow::Result<()> {
    let msg: AgentMessage = serde_json::from_str(text)?;

    match msg {
        AgentMessage::Heartbeat {
            node_id: reported_id,
            workloads,
            stats,
        } => {
            handle_ws_heartbeat(state, reported_id, &workloads, &stats).await;
        }
        AgentMessage::DomainDiscovered {
            service_name,
            domain,
            host_port,
        } => {
            info!(
                "Node {node_id} discovered domain {domain} for {service_name} (port {host_port})"
            );
            // Update the service's domain tracking on the master so the
            // TUI and status API reflect the domain correctly.
            let mut services = state.services.write().await;
            if let Some(svc) = services.get_mut(&service_name) {
                svc.config.domain = Some(domain);
            }
        }
        AgentMessage::DeployResult {
            service_name,
            success,
            error,
        } => {
            if success {
                info!("Node {node_id}: deploy of {service_name} succeeded");
            } else {
                error!(
                    "Node {node_id}: deploy of {service_name} failed: {}",
                    error.as_deref().unwrap_or("unknown")
                );
            }
        }
        AgentMessage::LogChunk {
            request_id,
            service_name: _,
            data,
            done,
        } => {
            // Forward to any pending log stream listener.
            let listeners = state.log_listeners.read().await;
            if let Some(listener_tx) = listeners.get(&request_id) {
                let _ = listener_tx.send((data, done)).await;
            }
        }
    }

    Ok(())
}

/// Process a heartbeat received over WebSocket (same logic as HTTP handler).
async fn handle_ws_heartbeat(
    state: &AppState,
    node_id: u64,
    workloads: &[orca_core::ws_types::WorkloadReport],
    stats: &orca_core::ws_types::HostStats,
) {
    let mut nodes = state.registered_nodes.write().await;
    if let Some(node) = nodes.get_mut(&node_id) {
        node.last_heartbeat = chrono::Utc::now();
        node.cpu_percent = stats.cpu_percent;
        node.memory_bytes = stats.memory_bytes;
        node.memory_total = stats.memory_total;
        node.disk_used = stats.disk_used;
        node.disk_total = stats.disk_total;
        node.net_rx = stats.net_rx;
        node.net_tx = stats.net_tx;
    }
    drop(nodes);

    // Update service statuses
    if !workloads.is_empty() {
        let mut services = state.services.write().await;
        for report in workloads {
            if let Some(svc) = services.get_mut(&report.service_name) {
                let status = match report.status.as_str() {
                    "running" => orca_core::types::WorkloadStatus::Running,
                    "stopped" => orca_core::types::WorkloadStatus::Stopped,
                    "failed" => orca_core::types::WorkloadStatus::Failed,
                    _ => orca_core::types::WorkloadStatus::Stopped,
                };
                for instance in &mut svc.instances {
                    instance.status = status;
                }
            }
        }
    }
}

/// Drain pending commands from the HTTP queue and send them over WS.
async fn drain_pending_commands(state: &AppState, node_id: u64, tx: &mpsc::Sender<MasterMessage>) {
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
async fn send_reconcile(state: &AppState, node_id: u64, tx: &mpsc::Sender<MasterMessage>) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_query_deserializes() {
        let q: WsQuery = serde_json::from_str(r#"{"token":"abc123","node_id":42}"#).unwrap();
        assert_eq!(q.token, "abc123");
        assert_eq!(q.node_id, 42);
    }
}
