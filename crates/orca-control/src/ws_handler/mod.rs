//! WebSocket handler for agent↔master streaming communication.
//!
//! Agents connect to `GET /api/v1/ws/agent?token=<cluster_token>&node_id=<id>`.
//! After the upgrade, messages flow bidirectionally using [`AgentMessage`] and
//! [`MasterMessage`] JSON frames.

mod heartbeat;
mod placeholders;
mod reconcile;

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

use heartbeat::handle_ws_heartbeat;
use placeholders::{remove_remote_placeholders, upsert_remote_placeholders};
use reconcile::{drain_pending_commands, send_reconcile};

/// Query params for the WS upgrade request.
#[derive(Deserialize)]
pub struct WsQuery {
    token: String,
    node_id: u64,
    /// Agent's address (e.g. "10.0.0.5:6881") for node registration.
    #[serde(default)]
    address: Option<String>,
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
    let address = query.address;
    info!("WebSocket upgrade accepted for node {node_id}");

    ws.on_upgrade(move |socket| handle_agent_ws(socket, state, node_id, address))
        .into_response()
}

/// Main WebSocket loop for a connected agent.
async fn handle_agent_ws(
    socket: WebSocket,
    state: Arc<AppState>,
    node_id: u64,
    agent_address: Option<String>,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Channel for master → agent messages (deploy commands, log requests, etc.)
    let (tx, mut rx) = mpsc::channel::<MasterMessage>(64);

    // Register this agent's sender so other parts of the system can push messages.
    {
        let mut senders = state.ws_agents.write().await;
        senders.insert(node_id, tx.clone());
    }

    info!("Agent {node_id} connected via WebSocket");

    // Register/update the node in registered_nodes so the reconciler's
    // find_target_node() can match placement constraints to this agent.
    {
        let addr = agent_address.unwrap_or_else(|| format!("ws-agent-{node_id}"));
        let mut nodes = state.registered_nodes.write().await;
        let node = nodes
            .entry(node_id)
            .or_insert_with(|| crate::state::RegisteredNode {
                node_id,
                address: addr.clone(),
                labels: std::collections::HashMap::new(),
                last_heartbeat: chrono::Utc::now(),
                drain: false,
                cpu_percent: 0.0,
                memory_bytes: 0,
                memory_total: 0,
                disk_used: 0,
                disk_total: 0,
                net_rx: 0,
                net_tx: 0,
            });
        node.last_heartbeat = chrono::Utc::now();
        node.address = addr;
        info!("Node {node_id} registered at {}", node.address);
    }

    // Send initial Ack
    let ack = MasterMessage::Ack { node_id };
    if let Ok(json) = serde_json::to_string(&ack) {
        let _ = ws_tx.send(Message::Text(json.into())).await;
    }

    // Drain any pending commands that were queued before the WS connected.
    drain_pending_commands(&state, node_id, &tx).await;

    // Ensure a placeholder InstanceState exists for every service placed on this
    // node so the heartbeat and DeployResult handlers have something to update,
    // and the watchdog current < desired check never fires for remote services.
    upsert_remote_placeholders(&state, node_id).await;

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

    // Periodic status sync: master pings agent every 30 s for a fresh heartbeat.
    let ping_tx = tx.clone();
    let ping_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.tick().await; // skip first tick (agent just connected and sent initial state)
        loop {
            interval.tick().await;
            if ping_tx.send(MasterMessage::StatusPing).await.is_err() {
                break;
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
    ping_task.abort();
    {
        let mut senders = state.ws_agents.write().await;
        senders.remove(&node_id);
    }
    remove_remote_placeholders(&state, node_id).await;
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
                let mut services = state.services.write().await;
                if let Some(svc) = services.get_mut(&service_name) {
                    let placeholder_id = format!("remote-{node_id}");
                    if let Some(inst) = svc
                        .instances
                        .iter_mut()
                        .find(|i| i.handle.runtime_id == placeholder_id)
                    {
                        inst.status = orca_core::types::WorkloadStatus::Running;
                    }
                }
            } else {
                error!(
                    "Node {node_id}: deploy of {service_name} failed: {}",
                    error.as_deref().unwrap_or("unknown")
                );
            }
            let result = if success {
                Ok(())
            } else {
                Err(error.unwrap_or_else(|| "deploy failed".to_string()))
            };
            if let Some(tx) = state.pending_deploys.write().await.remove(&service_name) {
                let _ = tx.send(result);
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
        AgentMessage::BackupResult {
            node_id,
            success,
            message,
        } => {
            if success {
                info!("Node {node_id}: backup complete — {message}");
            } else {
                error!("Node {node_id}: backup failed — {message}");
            }
            // Cache for the cluster-backups dashboard so it can surface the
            // last-known failure without rescanning logs.
            state.last_backup_results.write().await.insert(
                node_id,
                crate::state::LastBackupResult {
                    success,
                    message,
                    recorded_at: chrono::Utc::now(),
                },
            );
        }
        AgentMessage::BackupStatusReport { request_id, data } => {
            if let Some(tx) = state.backup_listeners.read().await.get(&request_id) {
                let _ = tx.send(data).await;
            }
        }
        AgentMessage::ExecOutput { session_id, data } => {
            use base64::Engine as _;
            let bytes = match base64::engine::general_purpose::STANDARD.decode(&data) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!("exec: bad base64 output for session {session_id}: {e}");
                    return Ok(());
                }
            };
            let sessions = state.exec_sessions.read().await;
            if let Some(tx) = sessions.get(&session_id) {
                let _ = tx.send(bytes).await;
            }
        }
        AgentMessage::ExecDone {
            session_id,
            exit_code,
        } => {
            info!("Node {node_id}: exec session {session_id} done (exit {exit_code})");
            state.exec_sessions.write().await.remove(&session_id);
        }
    }

    Ok(())
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
