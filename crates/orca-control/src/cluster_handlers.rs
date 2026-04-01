//! Cluster management API handlers (register, heartbeat, info).

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::state::{AppState, RegisteredNode};

/// Build cluster management routes.
/// Build cluster management routes (call before with_state on parent router).
pub fn cluster_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/cluster/info", get(cluster_info))
        .route("/api/v1/cluster/register", post(register_node))
        .route("/api/v1/cluster/heartbeat", post(heartbeat))
}

pub async fn cluster_info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let nodes = state.registered_nodes.read().await;
    let node_list: Vec<&RegisteredNode> = nodes.values().collect();
    Json(serde_json::json!({
        "cluster_name": state.cluster_config.cluster.name,
        "nodes": node_list,
        "node_count": nodes.len(),
    }))
}

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub node_id: u64,
    pub address: String,
    #[serde(default)]
    pub labels: std::collections::HashMap<String, String>,
}

pub async fn register_node(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RegisterRequest>,
) -> impl IntoResponse {
    let node = RegisteredNode {
        node_id: req.node_id,
        address: req.address.clone(),
        labels: req.labels,
        last_heartbeat: chrono::Utc::now(),
    };
    let mut nodes = state.registered_nodes.write().await;
    nodes.insert(req.node_id, node);
    tracing::info!("Node {} registered at {}", req.node_id, req.address);
    (StatusCode::OK, Json(serde_json::json!({"status": "ok"})))
}

#[derive(Deserialize)]
pub struct HeartbeatReq {
    pub node_id: u64,
}

pub async fn heartbeat(
    State(state): State<Arc<AppState>>,
    Json(req): Json<HeartbeatReq>,
) -> impl IntoResponse {
    let mut nodes = state.registered_nodes.write().await;
    if let Some(node) = nodes.get_mut(&req.node_id) {
        node.last_heartbeat = chrono::Utc::now();
    }
    drop(nodes);

    // Drain any pending commands for this node
    let commands = {
        let mut pending = state.pending_commands.write().await;
        pending.remove(&req.node_id).unwrap_or_default()
    };
    if !commands.is_empty() {
        tracing::info!(
            "Dispatching {} commands to node {}",
            commands.len(),
            req.node_id
        );
    }
    Json(serde_json::json!({"commands": commands}))
}
