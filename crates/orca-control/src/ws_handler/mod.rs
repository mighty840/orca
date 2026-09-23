//! WebSocket handler for agent↔master streaming communication.
//!
//! Agents connect to `GET /api/v1/ws/agent?node_id=<id>&address=<addr>` with an
//! `Authorization: Bearer <cluster_token>` header. Agents older than v0.3 send
//! the token as a `token` query parameter instead, which is still accepted
//! with a deprecation warning so the master can be upgraded first (#182).
//! After the upgrade, messages flow bidirectionally using [`AgentMessage`] and
//! [`MasterMessage`] JSON frames.

mod auth;
mod heartbeat;
mod messages;
mod placeholders;
mod reconcile;

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{ConnectInfo, Query, State, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use orca_core::ws_types::MasterMessage;

use crate::state::AppState;

use auth::{Refusal, authorize_agent};
use messages::handle_agent_message;
use placeholders::{remove_remote_placeholders, upsert_remote_placeholders};
use reconcile::{drain_pending_commands, send_reconcile};

/// Query params for the WS upgrade request.
#[derive(Deserialize)]
pub struct WsQuery {
    /// Deprecated (#182): pre-v0.3 agents send the token here, which puts it
    /// in URLs and logs. Accepted only when no `Authorization` header is sent.
    #[serde(default)]
    token: Option<String>,
    node_id: u64,
    /// Agent's address (e.g. "10.0.0.5:6881") for node registration.
    #[serde(default)]
    address: Option<String>,
}

/// Per-node sender so the master can push messages to a connected agent.
pub type AgentSender = mpsc::Sender<MasterMessage>;

/// Tear down a node's control session immediately (#131): remove it from
/// the session map (so the node stops looking reachable NOW), wake its
/// read loop so the task exits, and drop the node's remote placeholders
/// (so status stops reporting last-known state as current). Used when the
/// session is proven dead — e.g. a deploy ACK timeout. The agent's own
/// read-idle deadline triggers its reconnect; a truly-gone node is
/// stale-pruned 60s later.
pub(crate) async fn kill_agent_session(state: &AppState, node_id: u64, reason: &str) {
    let session = state.ws_agents.write().await.remove(&node_id);
    let Some(session) = session else { return };
    warn!(
        node_id,
        session_id = session.session_id,
        reason,
        "killing agent control session"
    );
    session.shutdown.notify_waiters();
    remove_remote_placeholders(state, node_id).await;
}

/// Handle the WebSocket upgrade request.
///
/// Authenticates via the `token` query param, then upgrades to a
/// bidirectional WebSocket connection.
pub async fn ws_agent_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    Query(query): Query<WsQuery>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Prefer the header. Fall back to the query string only when there is no
    // header at all — never from a rejected header to the query.
    let token = match crate::auth::bearer_token(&headers) {
        Some(token) => token.to_owned(),
        None => {
            if query.token.is_some() {
                warn!(
                    node_id = query.node_id,
                    peer = %peer.ip(),
                    "agent sent its token in the URL query string, which is deprecated \
                     (#182): upgrade the agent so the token travels in a header"
                );
            }
            query.token.clone().unwrap_or_default()
        }
    };

    // Opening the agent channel is admin-only (#201): the master sends this
    // node resolved secrets for every service pinned to it.
    match authorize_agent(&state, &token) {
        Ok(_) => {}
        Err(Refusal::Unauthorized) => {
            return (axum::http::StatusCode::UNAUTHORIZED, "invalid token").into_response();
        }
        Err(Refusal::Forbidden(role)) => {
            warn!(
                node_id = query.node_id,
                peer = %peer.ip(),
                ?role,
                "refusing agent channel: token role may not open it (admin only)"
            );
            return (
                axum::http::StatusCode::FORBIDDEN,
                "token role may not open the agent channel",
            )
                .into_response();
        }
    }

    let node_id = query.node_id;
    let address = query.address;
    info!("WebSocket upgrade accepted for node {node_id}");

    // NOTE: requires the server to be built with
    // `into_make_service_with_connect_info::<SocketAddr>()` — axum rejects
    // the upgrade with a 500 otherwise. All production and test serve
    // paths do; keep it that way when adding harnesses.
    let peer_ip = Some(peer.ip());
    ws.on_upgrade(move |socket| handle_agent_ws(socket, state, node_id, address, peer_ip))
        .into_response()
}

/// Main WebSocket loop for a connected agent.
async fn handle_agent_ws(
    socket: WebSocket,
    state: Arc<AppState>,
    node_id: u64,
    agent_address: Option<String>,
    peer_ip: Option<std::net::IpAddr>,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Channel for master → agent messages (deploy commands, log requests, etc.)
    let (tx, mut rx) = mpsc::channel::<MasterMessage>(64);

    // Register this session. A reconnect from the same node supersedes the
    // old session (#131): wake its read loop so it tears down immediately —
    // its generation-guarded cleanup can't touch our fresh entry.
    let session = crate::state::AgentSession::new(tx.clone());
    let session_id = session.session_id;
    let shutdown = session.shutdown.clone();
    // Read before taking the ws_agents lock so the two never nest.
    let previous_peer = state
        .registered_nodes
        .read()
        .await
        .get(&node_id)
        .and_then(|n| n.peer_ip.clone());
    let new_peer = peer_ip.map(|ip| ip.to_string());
    {
        let mut senders = state.ws_agents.write().await;
        if let Some(old) = senders.insert(node_id, session) {
            if previous_peer.is_some() && previous_peer != new_peer {
                // A live session for this node was replaced from somewhere
                // else. Expected once after deliberately moving an agent;
                // otherwise it means someone holding an admin token is posing
                // as this node (#201).
                warn!(
                    node_id,
                    previous_peer = previous_peer.as_deref().unwrap_or("unknown"),
                    new_peer = new_peer.as_deref().unwrap_or("unknown"),
                    old_session = old.session_id,
                    new_session = session_id,
                    "live agent session taken over from a different address"
                );
            } else {
                info!(
                    node_id,
                    old_session = old.session_id,
                    new_session = session_id,
                    "agent reconnected — superseding previous control session"
                );
            }
            old.shutdown.notify_waiters();
        }
    }

    info!("Agent {node_id} connected via WebSocket (session {session_id})");

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
            });
        node.last_heartbeat = chrono::Utc::now();
        node.address = addr;
        node.peer_ip = peer_ip.map(|ip| ip.to_string());
        info!(
            "Node {node_id} registered at {} (peer {})",
            node.address,
            node.peer_ip.as_deref().unwrap_or("unknown")
        );
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
    // A send failure is proof the socket is dead — wake the read loop so the
    // session tears down now instead of waiting out the idle deadline.
    let send_shutdown = shutdown.clone();
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
                send_shutdown.notify_waiters();
                break;
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

    // Process incoming agent messages under a read-idle deadline (#131).
    // Healthy agents heartbeat every 5s, so IDLE seconds of silence means
    // the socket is half-open (peer gone without FIN/RST) — the exact
    // failure that previously left a zombie session serving stale state
    // for days. Session liveness IS deploy-channel liveness: no traffic,
    // no session.
    let idle = std::time::Duration::from_secs(state.cluster_config.deploy.ws_idle_timeout_secs);
    loop {
        tokio::select! {
            _ = shutdown.notified() => {
                info!(node_id, session_id, "control session shut down (superseded or killed)");
                break;
            }
            next = tokio::time::timeout(idle, ws_rx.next()) => match next {
                Err(_) => {
                    warn!(
                        node_id, session_id, idle_secs = idle.as_secs(),
                        "no traffic from agent within idle deadline — closing half-dead session"
                    );
                    break;
                }
                Ok(None) => break,
                Ok(Some(Err(e))) => {
                    warn!(node_id, session_id, "WebSocket read error: {e}");
                    break;
                }
                Ok(Some(Ok(msg))) => match msg {
                    Message::Text(text) => {
                        if let Err(e) = handle_agent_message(&state, node_id, &text, &tx).await {
                            warn!("Error handling agent message from {node_id}: {e}");
                        }
                    }
                    Message::Close(_) => break,
                    _ => {} // ignore binary, ping, pong (axum handles pong auto)
                },
            }
        }
    }

    // Cleanup on disconnect — generation-guarded (#131): only the session
    // that still owns the map entry may deregister the node and drop its
    // placeholders. A superseded session exiting late must not tear down
    // its replacement's state (the old last-writer-cleanup race).
    send_task.abort();
    ping_task.abort();
    if state.deregister_agent_session(node_id, session_id).await {
        remove_remote_placeholders(&state, node_id).await;
    }
    info!("Agent {node_id} WebSocket disconnected (session {session_id})");
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
