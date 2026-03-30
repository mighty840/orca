//! Cluster management API handlers (register, heartbeat, info).

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;

use crate::state::{AppState, RegisteredNode};

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
    Json(serde_json::json!({"commands": []}))
}
