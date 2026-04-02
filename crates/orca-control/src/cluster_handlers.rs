//! Cluster management API handlers (register, heartbeat, info).

use std::sync::Arc;

use axum::extract::{Path, State};
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
        .route("/api/v1/cluster/nodes/{node_id}/drain", post(drain_node))
        .route(
            "/api/v1/cluster/nodes/{node_id}/undrain",
            post(undrain_node),
        )
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
        drain: false,
    };
    let mut nodes = state.registered_nodes.write().await;
    nodes.insert(req.node_id, node);
    tracing::info!("Node {} registered at {}", req.node_id, req.address);
    (StatusCode::OK, Json(serde_json::json!({"status": "ok"})))
}

/// Status report for a single workload from an agent node.
#[derive(Deserialize)]
pub struct WorkloadStatusReport {
    pub service_name: String,
    pub status: String,
}

#[derive(Deserialize)]
pub struct HeartbeatReq {
    pub node_id: u64,
    #[serde(default)]
    pub workloads: Vec<WorkloadStatusReport>,
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

    // Update service instance statuses from agent-reported workloads
    if !req.workloads.is_empty() {
        let mut services = state.services.write().await;
        for report in &req.workloads {
            if let Some(svc) = services.get_mut(&report.service_name) {
                let reported_status = parse_workload_status(&report.status);
                for instance in &mut svc.instances {
                    instance.status = reported_status;
                }
            }
        }
    }

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

pub async fn drain_node(
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<u64>,
) -> impl IntoResponse {
    set_node_drain(&state, node_id, true).await
}

pub async fn undrain_node(
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<u64>,
) -> impl IntoResponse {
    set_node_drain(&state, node_id, false).await
}

async fn set_node_drain(
    state: &AppState,
    node_id: u64,
    drain: bool,
) -> (StatusCode, Json<serde_json::Value>) {
    let mut nodes = state.registered_nodes.write().await;
    if let Some(node) = nodes.get_mut(&node_id) {
        node.drain = drain;
        let action = if drain { "drained" } else { "undrained" };
        tracing::info!("Node {node_id} {action}");
        (StatusCode::OK, Json(serde_json::json!({"status": action})))
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "node not found"})),
        )
    }
}

/// Parse a status string from agent reports into a `WorkloadStatus`.
fn parse_workload_status(s: &str) -> orca_core::types::WorkloadStatus {
    match s {
        "running" => orca_core::types::WorkloadStatus::Running,
        "stopped" => orca_core::types::WorkloadStatus::Stopped,
        "failed" => orca_core::types::WorkloadStatus::Failed,
        "pending" => orca_core::types::WorkloadStatus::Pending,
        _ => orca_core::types::WorkloadStatus::Stopped,
    }
}
