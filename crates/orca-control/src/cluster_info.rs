//! `GET /api/v1/cluster/info`, built from the typed [`ClusterInfoResponse`]
//! (#80). The wire format is the one the handler produced from hand-built
//! JSON before; `cluster_info_test.rs` guards the node fields.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use orca_core::api_types::{ClusterInfoResponse, ClusterNode};

use crate::state::{AppState, RegisteredNode};

pub async fn cluster_info(State(state): State<Arc<AppState>>) -> Json<ClusterInfoResponse> {
    let nodes = state.registered_nodes.read().await;
    let cluster = &state.cluster_config.cluster;
    Json(ClusterInfoResponse {
        cluster_name: cluster.name.clone(),
        domain: cluster.domain.clone(),
        acme_email: cluster.acme_email.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        commit: option_env!("ORCA_COMMIT").unwrap_or("unknown").to_string(),
        nodes: nodes.values().map(|n| cluster_node(&state, n)).collect(),
        node_count: nodes.len(),
    })
}

/// A registered node plus the GPUs `cluster.toml` declares for it. GPUs are
/// not reported by heartbeats; they are matched to the node by address,
/// exact or by host (declared `ubuntu` matches registered `ubuntu:6881`).
fn cluster_node(state: &AppState, n: &RegisteredNode) -> ClusterNode {
    let host = |addr: &str| {
        addr.rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(addr)
            .to_string()
    };
    let gpus = state
        .cluster_config
        .node
        .iter()
        .find(|d| d.address == n.address || host(&d.address) == host(&n.address))
        .map(|d| d.gpus.clone())
        .unwrap_or_default();
    // Destructured without `..`: a new `RegisteredNode` field fails to
    // compile here instead of silently missing from the response, as
    // disk and network did between #135 and #143.
    let RegisteredNode {
        node_id,
        address,
        peer_ip,
        labels,
        last_heartbeat,
        drain,
        cpu_percent,
        memory_bytes,
        memory_total,
        disk_used,
        disk_total,
        net_rx,
        net_tx,
    } = n.clone();
    ClusterNode {
        node_id,
        address,
        peer_ip,
        labels,
        last_heartbeat,
        drain,
        cpu_percent,
        memory_bytes,
        memory_total,
        disk_used,
        disk_total,
        net_rx,
        net_tx,
        gpus,
    }
}
