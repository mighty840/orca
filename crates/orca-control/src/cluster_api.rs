//! API endpoints for multi-node cluster management.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::cluster_state::ClusterState;

/// Shared state for cluster API endpoints.
pub type ClusterApiState = Arc<ClusterState>;

/// Build the cluster management router.
pub fn cluster_router(state: ClusterApiState) -> Router {
    Router::new()
        .route("/api/v1/cluster/info", get(cluster_info))
        .route("/api/v1/cluster/register", post(register_node))
        .route("/api/v1/cluster/heartbeat", post(heartbeat))
        .with_state(state)
}

/// Cluster info response.
#[derive(Serialize)]
struct ClusterInfo {
    nodes: Vec<NodeInfo>,
    services: usize,
    assignments: usize,
}

#[derive(Serialize)]
struct NodeInfo {
    node_id: u64,
    address: String,
    status: String,
}

async fn cluster_info(State(state): State<ClusterApiState>) -> impl IntoResponse {
    let nodes = state.get_nodes().unwrap_or_default();
    let services = state.get_services().unwrap_or_default();
    let assignments = state.get_all_assignments().unwrap_or_default();

    let node_list: Vec<NodeInfo> = nodes
        .values()
        .map(|n| NodeInfo {
            node_id: n.node_id,
            address: n.address.clone(),
            status: format!("{:?}", n.status).to_lowercase(),
        })
        .collect();

    Json(ClusterInfo {
        nodes: node_list,
        services: services.len(),
        assignments: assignments.len(),
    })
}

/// Register node request.
#[derive(Deserialize)]
struct RegisterRequest {
    node_id: u64,
    address: String,
    #[serde(default)]
    labels: HashMap<String, String>,
}

async fn register_node(
    State(state): State<ClusterApiState>,
    Json(req): Json<RegisterRequest>,
) -> impl IntoResponse {
    match state
        .register_node(req.node_id, req.address.clone(), req.labels)
        .await
    {
        Ok(()) => {
            info!("Node {} registered at {}", req.node_id, req.address);
            (StatusCode::OK, Json(serde_json::json!({"status": "ok"}))).into_response()
        }
        Err(e) => {
            tracing::error!("Node registration failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response()
        }
    }
}

/// Heartbeat request from an agent.
#[derive(Deserialize)]
struct HeartbeatRequest {
    node_id: u64,
    // workloads field accepted but not processed in M2
    #[serde(default)]
    _workloads: Vec<serde_json::Value>,
}

/// Heartbeat response with commands for the agent.
#[derive(Serialize)]
struct HeartbeatResponse {
    commands: Vec<serde_json::Value>,
}

async fn heartbeat(
    State(_state): State<ClusterApiState>,
    Json(req): Json<HeartbeatRequest>,
) -> impl IntoResponse {
    // For M2, heartbeats are acknowledged but don't trigger commands yet.
    // The scheduler assigns workloads proactively, not reactively via heartbeat.
    tracing::debug!("Heartbeat from node {}", req.node_id);

    Json(HeartbeatResponse {
        commands: Vec::new(),
    })
}
