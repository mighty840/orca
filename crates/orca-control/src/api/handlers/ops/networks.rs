//! Cluster networks dashboard: per-node `orca-*` Docker bridge listing plus
//! the public-edge domain routes registered with each node's proxy. Read-only.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use tracing::warn;

use orca_core::api_types::{
    ClusterNetworksResponse, DockerNetwork, DomainRoute, NodeNetworks, NodeRole,
};
use orca_core::ws_types::{MasterMessage, NetworkStatusReportData};

use crate::state::AppState;

/// Per-agent collection timeout. Generous because docker `inspect_container`
/// over many containers can be slow on a loaded box.
const NETWORK_REPORT_TIMEOUT: Duration = Duration::from_secs(5);

/// GET /api/v1/cluster/networks — aggregate Docker network + edge route info
/// across every node in the cluster.
pub(crate) async fn cluster_networks(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let master = collect_master(&state).await;
    let agents = collect_agents(&state).await;

    let mut nodes = vec![master];
    nodes.extend(agents);
    Json(ClusterNetworksResponse { nodes })
}

async fn collect_master(state: &AppState) -> NodeNetworks {
    let networks = enumerate_local_orca_networks().await;
    let domains = collect_domains(state).await;
    NodeNetworks {
        node_id: None,
        hostname: master_hostname(),
        role: NodeRole::Master,
        networks,
        domains,
        reachable: true,
    }
}

async fn collect_agents(state: &AppState) -> Vec<NodeNetworks> {
    let agent_ids: Vec<u64> = state.ws_agents.read().await.keys().copied().collect();
    if agent_ids.is_empty() {
        return Vec::new();
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel::<NetworkStatusReportData>(agent_ids.len() + 1);
    let mut request_ids: Vec<String> = Vec::with_capacity(agent_ids.len());
    {
        let mut listeners = state.network_listeners.write().await;
        for _ in 0..agent_ids.len() {
            let req = uuid::Uuid::new_v4().to_string();
            listeners.insert(req.clone(), tx.clone());
            request_ids.push(req);
        }
    }

    let mut dispatched = HashMap::<u64, String>::new();
    {
        let agents = state.ws_agents.read().await;
        for (node_id, req_id) in agent_ids.iter().zip(request_ids.iter()) {
            if let Some(agent_tx) = agents.get(node_id) {
                let sent = agent_tx
                    .send(MasterMessage::NetworkStatusRequest {
                        request_id: req_id.clone(),
                    })
                    .await
                    .is_ok();
                if sent {
                    dispatched.insert(*node_id, req_id.clone());
                }
            }
        }
    }

    let mut reports = HashMap::<u64, NetworkStatusReportData>::new();
    let deadline = tokio::time::sleep(NETWORK_REPORT_TIMEOUT);
    tokio::pin!(deadline);
    while reports.len() < dispatched.len() {
        tokio::select! {
            biased;
            msg = rx.recv() => {
                match msg {
                    Some(data) => { reports.insert(data.node_id, data); }
                    None => break,
                }
            }
            _ = &mut deadline => {
                warn!(
                    "cluster_networks: timed out after {}s ({} of {} agents responded)",
                    NETWORK_REPORT_TIMEOUT.as_secs(),
                    reports.len(),
                    dispatched.len(),
                );
                break;
            }
        }
    }

    {
        let mut listeners = state.network_listeners.write().await;
        for req in &request_ids {
            listeners.remove(req);
        }
    }

    agent_ids
        .into_iter()
        .map(|node_id| match reports.remove(&node_id) {
            Some(data) => NodeNetworks {
                node_id: Some(node_id),
                hostname: data.hostname,
                role: NodeRole::Agent,
                networks: data.networks,
                // Agent edge routes aren't surfaced yet — the agent's proxy
                // has its own route table but we don't yet ship it over WS.
                // Filed as a follow-up; for now agent rows show docker nets
                // only.
                domains: Vec::new(),
                reachable: true,
            },
            None => NodeNetworks {
                node_id: Some(node_id),
                hostname: format!("node-{node_id}"),
                role: NodeRole::Agent,
                networks: Vec::new(),
                domains: Vec::new(),
                reachable: false,
            },
        })
        .collect()
}

/// Walk the master's own Docker daemon. Delegates to the agent crate's
/// enumerator so master and agent always see the same shape — no risk of
/// the two implementations drifting.
async fn enumerate_local_orca_networks() -> Vec<DockerNetwork> {
    orca_agent::ws_client::network_status::list_local_orca_networks().await
}

async fn collect_domains(state: &AppState) -> Vec<DomainRoute> {
    let table = state.route_table.read().await;
    let mut out: Vec<DomainRoute> = table
        .iter()
        .flat_map(|(domain, targets)| {
            targets.iter().map(|t| DomainRoute {
                domain: domain.clone(),
                service: t.service_name.clone(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.domain.cmp(&b.domain).then(a.service.cmp(&b.service)));
    out
}

fn master_hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "master".to_string())
}
