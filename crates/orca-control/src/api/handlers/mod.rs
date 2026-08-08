use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;

use orca_core::api_types::{DeployRequest, DeployResponse, ServiceStatus, StatusResponse};

use crate::reconciler;
use crate::state::AppState;

pub(crate) mod alerts;
pub(crate) mod ask;
pub(crate) mod exec;
mod ops;
pub(crate) mod secrets;

pub(crate) use ops::{
    cluster_backups, cluster_networks, logs, promote, redeploy, rollback, scale, start_service,
    stop_all, stop_project, stop_service, trigger_cluster_backup,
};

/// Health check endpoint.
pub(crate) async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

/// Deploy services from the request body.
pub(crate) async fn deploy(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeployRequest>,
) -> impl IntoResponse {
    // Validate every config up front so an invalid one (e.g. both `domain`
    // and `domains` set) fails clearly instead of half-deploying.
    let validation_errors: Vec<String> = req
        .services
        .iter()
        .filter_map(|s| s.validate().err())
        .collect();
    if !validation_errors.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(DeployResponse {
                deployed: Vec::new(),
                errors: validation_errors,
            }),
        );
    }

    let (deployed, errors) = reconciler::reconcile(&state, &req.services).await;

    // Persist deployed services to store
    if let Some(store) = &state.store {
        for config in &req.services {
            if deployed.contains(&config.name)
                && let Err(e) = store.set_service(&config.name, config)
            {
                tracing::warn!("Failed to persist {}: {e}", config.name);
            }
        }
    }

    let status_code = if errors.is_empty() {
        StatusCode::OK
    } else if deployed.is_empty() {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::PARTIAL_CONTENT
    };

    (status_code, Json(DeployResponse { deployed, errors }))
}

/// Query params for status filtering.
#[derive(Debug, Deserialize, Default)]
pub(crate) struct StatusQuery {
    #[serde(default)]
    project: Option<String>,
}

/// Refresh local instance statuses from the runtime so `status` reports the
/// OBSERVED container state, not what the deploy path recorded. Instances are
/// marked Running optimistically on create and only corrected by the 30s
/// watchdog cycle — inside that window a failed deploy reported `running`
/// while Docker showed `Exited (1)`. Status is what an operator (or an AI
/// agent) trusts to decide whether a deploy succeeded, so it must ask the
/// runtime. Remote placeholders (`remote-*`) are owned by the heartbeat
/// handler and are skipped here.
async fn refresh_observed_statuses(state: &AppState) {
    use orca_core::types::WorkloadStatus;

    // Snapshot handles under the read lock, then query without holding it.
    let handles: Vec<(String, usize, orca_core::runtime::WorkloadHandle, _)> = {
        let services = state.services.read().await;
        services
            .values()
            .flat_map(|svc| {
                let name = svc.config.name.clone();
                let kind = svc.config.runtime;
                svc.instances
                    .iter()
                    .enumerate()
                    .filter(|(_, i)| !i.handle.runtime_id.starts_with("remote-"))
                    .map(move |(idx, i)| (name.clone(), idx, i.handle.clone(), kind))
                    .collect::<Vec<_>>()
            })
            .collect()
    };

    let mut observed = Vec::with_capacity(handles.len());
    for (name, idx, handle, kind) in handles {
        let Ok(runtime) = crate::reconciler::get_runtime(state, kind) else {
            continue;
        };
        let status = runtime
            .status(&handle)
            .await
            .unwrap_or(WorkloadStatus::Failed);
        observed.push((name, idx, status));
    }

    let mut services = state.services.write().await;
    for (name, idx, status) in observed {
        if let Some(inst) = services
            .get_mut(&name)
            .and_then(|svc| svc.instances.get_mut(idx))
        {
            inst.status = status;
        }
    }
}

/// Get cluster and service status, optionally filtered by project.
pub(crate) async fn status(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<StatusQuery>,
) -> impl IntoResponse {
    refresh_observed_statuses(&state).await;

    let services = state.services.read().await;
    let stats_cache = state.container_stats.read().await;
    let failures = state.last_failures.read().await;

    let service_statuses: Vec<ServiceStatus> = services
        .values()
        .filter(|svc| {
            query
                .project
                .as_ref()
                .is_none_or(|p| svc.config.project.as_deref() == Some(p.as_str()))
        })
        .map(|svc| {
            let running = svc.running_count();
            let overall_status = if svc.stopped {
                // Operator-paused (`orca stop`): defined + startable, distinct
                // from a service that failed to come up.
                "paused"
            } else if running == 0 && svc.desired_replicas > 0 {
                "stopped"
            } else if running < svc.desired_replicas {
                "degraded"
            } else {
                "running"
            };

            // Look up cached stats for this service.
            let cached = stats_cache.get(&svc.config.name);

            ServiceStatus {
                name: svc.config.name.clone(),
                image: svc
                    .config
                    .image
                    .clone()
                    .or_else(|| svc.config.module.clone())
                    .unwrap_or_default(),
                runtime: svc.config.runtime,
                desired_replicas: svc.desired_replicas,
                running_replicas: running,
                status: overall_status.to_string(),
                domain: svc.config.primary_domain(),
                domains: svc.config.all_domains(),
                project: svc.config.project.clone(),
                memory_usage: cached.map(|s| s.memory_usage.clone()),
                cpu_percent: cached.map(|s| s.cpu_percent),
                node: svc.config.placement.as_ref().and_then(|p| p.node.clone()),
                memory_limit_bytes: svc
                    .config
                    .resources
                    .as_ref()
                    .and_then(|r| r.memory.as_deref())
                    .and_then(parse_memory_limit),
                // Only surface a failure reason while the service is actually
                // degraded/stopped — a healthy service shouldn't show stale crashes.
                last_failure: if overall_status == "running" {
                    None
                } else {
                    failures.get(&svc.config.name).cloned()
                },
            }
        })
        .collect();

    Json(StatusResponse {
        cluster_name: state.cluster_config.cluster.name.clone(),
        services: service_statuses,
    })
}

/// Parse a human-readable memory limit (`512Mi`, `2Gi`, `1024`) into bytes.
/// Mirrors the parser in the docker config builder so a valid memory limit
/// in service.toml always surfaces in the status response.
fn parse_memory_limit(s: &str) -> Option<u64> {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    let (num, mult) = if let Some(n) = lower.strip_suffix("gi") {
        (n, 1024u64 * 1024 * 1024)
    } else if let Some(n) = lower.strip_suffix("mi") {
        (n, 1024u64 * 1024)
    } else if let Some(n) = lower.strip_suffix("ki") {
        (n, 1024u64)
    } else if let Some(n) = lower.strip_suffix("g") {
        (n, 1_000_000_000u64)
    } else if let Some(n) = lower.strip_suffix("m") {
        (n, 1_000_000u64)
    } else {
        (lower.as_str(), 1u64)
    };
    num.trim().parse::<u64>().ok().map(|v| v * mult)
}
