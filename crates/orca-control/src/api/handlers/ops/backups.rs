//! Cluster-wide backup dashboard: aggregates per-node snapshot listings and
//! cached last-result status, plus a trigger endpoint to dispatch an immediate
//! backup run to one node or every connected agent.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use tracing::warn;

use orca_core::api_types::{
    ClusterBackupsResponse, LastBackupResult, NodeBackupStatus, NodeRole, TriggerBackupResponse,
};
use orca_core::backup::enumerate_local_backups;
use orca_core::ws_types::{BackupStatusReportData, MasterMessage};

use crate::state::AppState;

/// Per-agent collection timeout. Generous because an agent may be enumerating
/// dozens of snapshots over slow disks; users can refresh manually if a node
/// is genuinely unreachable.
const BACKUP_REPORT_TIMEOUT: Duration = Duration::from_secs(5);

/// GET /api/v1/cluster/backups — aggregate per-node backup status.
pub(crate) async fn cluster_backups(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let master = collect_master_status(&state).await;
    let agents = collect_agent_statuses(&state).await;

    let mut nodes = vec![master];
    nodes.extend(agents);
    Json(ClusterBackupsResponse { nodes })
}

async fn collect_master_status(state: &AppState) -> NodeBackupStatus {
    let snapshots = match dirs_next::home_dir() {
        Some(home) => enumerate_local_backups(&home),
        None => Vec::new(),
    };
    let last_result = state.master_last_backup_result.read().await.clone();
    NodeBackupStatus {
        node_id: None,
        hostname: crate::placement::master_hostname(),
        role: NodeRole::Master,
        snapshots,
        last_result,
        reachable: true,
    }
}

async fn collect_agent_statuses(state: &AppState) -> Vec<NodeBackupStatus> {
    let agent_ids: Vec<u64> = state.ws_agents.read().await.keys().copied().collect();
    if agent_ids.is_empty() {
        return Vec::new();
    }

    // Single listener channel collects reports from every agent we ask. Each
    // agent gets a unique request_id but they all funnel into the same
    // receiver, so the collector loop terminates as soon as we've heard from
    // all of them or the timeout fires.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<BackupStatusReportData>(agent_ids.len() + 1);
    let mut request_ids: Vec<String> = Vec::with_capacity(agent_ids.len());
    {
        let mut listeners = state.backup_listeners.write().await;
        for _ in 0..agent_ids.len() {
            let req = uuid::Uuid::new_v4().to_string();
            listeners.insert(req.clone(), tx.clone());
            request_ids.push(req);
        }
    }

    // Dispatch one BackupStatusRequest to each agent.
    let mut dispatched = HashMap::<u64, String>::new();
    {
        let agents = state.ws_agents.read().await;
        for (node_id, req_id) in agent_ids.iter().zip(request_ids.iter()) {
            if let Some(agent_tx) = agents.get(node_id) {
                let sent = agent_tx
                    .send(MasterMessage::BackupStatusRequest {
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

    // Collect, indexed by node_id so we can attach cached last_result later.
    let mut reports = HashMap::<u64, BackupStatusReportData>::new();
    let deadline = tokio::time::sleep(BACKUP_REPORT_TIMEOUT);
    tokio::pin!(deadline);
    while reports.len() < dispatched.len() {
        tokio::select! {
            biased;
            msg = rx.recv() => {
                match msg {
                    Some(data) => {
                        reports.insert(data.node_id, data);
                    }
                    None => break,
                }
            }
            _ = &mut deadline => {
                warn!(
                    "cluster_backups: timed out after {}s ({} of {} agents responded)",
                    BACKUP_REPORT_TIMEOUT.as_secs(),
                    reports.len(),
                    dispatched.len(),
                );
                break;
            }
        }
    }

    // Cleanup listeners so dropped request_ids don't pile up.
    {
        let mut listeners = state.backup_listeners.write().await;
        for req in &request_ids {
            listeners.remove(req);
        }
    }

    let last_results = state.last_backup_results.read().await;
    agent_ids
        .into_iter()
        .map(|node_id| {
            let last = last_results.get(&node_id).map(|r| LastBackupResult {
                success: r.success,
                message: r.message.clone(),
                recorded_at: r.recorded_at,
            });
            match reports.remove(&node_id) {
                Some(data) => NodeBackupStatus {
                    node_id: Some(node_id),
                    hostname: data.hostname,
                    role: NodeRole::Agent,
                    snapshots: data.snapshots,
                    last_result: last,
                    reachable: true,
                },
                None => NodeBackupStatus {
                    node_id: Some(node_id),
                    hostname: format!("node-{node_id}"),
                    role: NodeRole::Agent,
                    snapshots: Vec::new(),
                    last_result: last,
                    reachable: false,
                },
            }
        })
        .collect()
}

#[derive(Debug, Deserialize)]
pub struct TriggerQuery {
    /// Trigger this specific agent only.
    pub node_id: Option<u64>,
    /// Trigger the master's own local backup. Combine with `node_id` to do
    /// master + a specific agent in one request, or set alone for master
    /// only. With neither field set, the handler fans out to master + every
    /// connected agent.
    #[serde(default)]
    pub master: bool,
}

/// POST /api/v1/cluster/backups/trigger
///
/// Query params:
///   - `node_id=<id>` → trigger only that agent
///   - `master=true`  → trigger only the master
///   - (combined)     → trigger both
///   - (neither)      → fan out to master + every connected agent
///
/// Master backups run as a subprocess (`orca backup all`) spawned by
/// `backup_scheduler::run_master_backup`, which records the result on
/// `state.master_last_backup_result` when the subprocess exits. Agent backups
/// go over WS as `BackupRequest`, same path the scheduler uses, so manual
/// runs produce the same artifacts as scheduled runs.
pub(crate) async fn trigger_cluster_backup(
    State(state): State<Arc<AppState>>,
    Query(q): Query<TriggerQuery>,
) -> impl IntoResponse {
    let Some(config) = state.cluster_config.backup.clone() else {
        return (
            StatusCode::BAD_REQUEST,
            "no [backup] configuration in cluster.toml — cannot trigger".to_string(),
        )
            .into_response();
    };

    // Resolve targets. The "fan out to everything" case fires when neither
    // field is set — useful from curl and from a future "trigger all" UI.
    let trigger_master = q.master || q.node_id.is_none();
    let agent_targets: Vec<u64> = match q.node_id {
        Some(id) => {
            let agents = state.ws_agents.read().await;
            if !agents.contains_key(&id) {
                return (StatusCode::NOT_FOUND, format!("node {id} is not connected"))
                    .into_response();
            }
            vec![id]
        }
        None if q.master => Vec::new(),
        None => state.ws_agents.read().await.keys().copied().collect(),
    };

    let mut master_dispatched = false;
    if trigger_master {
        let state = state.clone();
        let config = config.clone();
        tokio::spawn(async move {
            crate::backup_scheduler::run_master_backup(&state, &config).await;
        });
        master_dispatched = true;
    }

    let dispatched = dispatch_to_agents(&state, &config, &agent_targets).await;
    Json(TriggerBackupResponse {
        dispatched_to: dispatched,
        master_dispatched,
    })
    .into_response()
}

async fn dispatch_to_agents(
    state: &AppState,
    config: &orca_core::backup::BackupConfig,
    targets: &[u64],
) -> Vec<u64> {
    if targets.is_empty() {
        return Vec::new();
    }
    let service_hooks = collect_service_hooks(state).await;
    let agents = state.ws_agents.read().await;
    let mut sent_to = Vec::with_capacity(targets.len());
    for id in targets {
        if let Some(tx) = agents.get(id) {
            let ok = tx
                .send(MasterMessage::BackupRequest {
                    config: config.clone(),
                    service_hooks: service_hooks.clone(),
                })
                .await
                .is_ok();
            if ok {
                sent_to.push(*id);
            }
        }
    }
    sent_to
}

async fn collect_service_hooks(state: &AppState) -> HashMap<String, String> {
    let services = state.services.read().await;
    services
        .values()
        .filter_map(|svc| {
            let hook = svc.config.backup.as_ref()?.pre_hook.as_ref()?;
            Some((svc.config.name.clone(), hook.clone()))
        })
        .collect()
}
