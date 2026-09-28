//! The cluster snapshot the alert monitor diagnoses from.
//!
//! Counts alone ("0/1 replicas") only support a generic diagnosis. Each
//! service also carries the failure the reconciler or heartbeat recorded,
//! its image and the one before, memory against its limit, its health
//! checks and its node. Logs are fetched separately, only for a service
//! that opens an alert.

use std::sync::Arc;
use std::time::Duration as StdDuration;

use async_trait::async_trait;
use chrono::{Duration, Utc};

use orca_ai::context::{ClusterContext, NodeSummary, ServiceSummary};
use orca_ai::monitor::ContextProvider;
use orca_core::config::{ProbeConfig, ServiceConfig};
use orca_core::types::{RuntimeKind, WorkloadStatus};

use crate::state::{AppState, RegisteredNode};

/// A node without a heartbeat for this long is reported as unreachable.
const HEARTBEAT_STALE_SECS: i64 = 60;

/// How long an agent gets to send a service's logs for a diagnosis.
const ALERT_LOG_TIMEOUT: StdDuration = StdDuration::from_secs(4);

/// Reads `AppState` snapshots into a `ClusterContext` for the AI monitor.
pub struct StateContextProvider {
    state: Arc<AppState>,
}

impl StateContextProvider {
    pub fn for_state(state: Arc<AppState>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl ContextProvider for StateContextProvider {
    async fn snapshot(&self) -> anyhow::Result<ClusterContext> {
        let state = &self.state;
        let cluster_name = state.cluster_config.cluster.name.clone();
        let now = Utc::now();

        let nodes = state.registered_nodes.read().await;
        let master_id = crate::master_node::master_node_id();
        let services = state.services.read().await;
        let events = state.instance_events.read().await;
        let failures = state.last_failures.read().await;
        let stats = state.container_stats.read().await;
        let history = state.deploy_history.read().await;

        let services: Vec<ServiceSummary> = services
            .values()
            .map(|svc| {
                let name = &svc.config.name;
                let running = svc
                    .instances
                    .iter()
                    .filter(|i| matches!(i.status, WorkloadStatus::Running))
                    .count() as u32;
                let status = if svc.instances.is_empty() {
                    "stopped".into()
                } else if running == svc.desired_replicas {
                    "healthy".into()
                } else {
                    "degraded".into()
                };
                let (errors_1h, restarts_24h) = events
                    .get(name)
                    .map(|log| {
                        (
                            log.failures_in(now, Duration::hours(1)),
                            log.restarts_in(now, Duration::hours(24)),
                        )
                    })
                    .unwrap_or((0, 0));
                let image = svc
                    .config
                    .image
                    .clone()
                    .or_else(|| svc.config.module.clone());
                let deploys = history.list(name);
                let previous_image = history
                    .get_previous(name)
                    .and_then(|r| r.image.clone())
                    .filter(|prev| Some(prev) != image.as_ref());
                let node_id = crate::service_logs::remote_node(svc).unwrap_or(master_id);
                let node = nodes
                    .get(&node_id)
                    .map(|n| n.address.clone())
                    .or_else(|| svc.config.placement.as_ref().and_then(|p| p.node.clone()));
                let cached = stats.get(name);
                ServiceSummary {
                    name: name.clone(),
                    runtime: match svc.config.runtime {
                        RuntimeKind::Container => "container".into(),
                        RuntimeKind::Wasm => "wasm".into(),
                    },
                    replicas_running: running,
                    replicas_desired: svc.desired_replicas,
                    status,
                    uses_gpu: svc
                        .config
                        .resources
                        .as_ref()
                        .is_some_and(|r| r.gpu.is_some()),
                    recent_logs: Vec::new(),
                    error_count_1h: errors_1h,
                    restart_count_24h: restarts_24h,
                    image,
                    previous_image,
                    last_deploy_at: deploys.last().map(|r| r.timestamp),
                    node,
                    memory_usage: cached.map(|s| s.memory_usage.clone()),
                    memory_limit: svc.config.resources.as_ref().and_then(|r| r.memory.clone()),
                    cpu_percent: cached.map(|s| s.cpu_percent),
                    health_check: health_summary(&svc.config),
                    last_failure: failures.get(name).cloned(),
                }
            })
            .collect();

        let nodes: Vec<NodeSummary> = nodes.values().map(|n| node_summary(n, now)).collect();

        Ok(ClusterContext {
            cluster_name,
            nodes,
            services,
            recent_events: Vec::new(),
            active_alerts: Vec::new(),
        })
    }

    async fn recent_logs(&self, service: &str, tail: usize) -> Vec<String> {
        match crate::service_logs::collect(
            &self.state,
            service,
            tail as u64,
            ALERT_LOG_TIMEOUT,
            true,
        )
        .await
        {
            Ok(text) => text.lines().map(str::to_string).collect(),
            Err(e) => {
                tracing::debug!("No logs of {service} for its diagnosis: {}", e.message);
                Vec::new()
            }
        }
    }
}

fn node_summary(n: &RegisteredNode, now: chrono::DateTime<Utc>) -> NodeSummary {
    let age = (now - n.last_heartbeat).num_seconds().max(0);
    let status = if age > HEARTBEAT_STALE_SECS {
        "unreachable"
    } else if n.drain {
        "draining"
    } else {
        "healthy"
    };
    NodeSummary {
        id: n.node_id.to_string(),
        address: n.address.clone(),
        status: status.into(),
        cpu_percent: n.cpu_percent,
        memory_percent: if n.memory_total > 0 {
            (n.memory_bytes as f64 / n.memory_total as f64) * 100.0
        } else {
            0.0
        },
        gpu_summary: Vec::new(),
        heartbeat_age_secs: Some(age),
    }
}

/// One line describing a service's health checks, if it has any.
fn health_summary(config: &ServiceConfig) -> Option<String> {
    let probe = |kind: &str, p: &ProbeConfig| {
        let port = p.port.map(|p| format!(" on port {p}")).unwrap_or_default();
        format!(
            "{kind} GET {}{port} every {}s, timeout {}s, fails after {}",
            p.path, p.interval_secs, p.timeout_secs, p.failure_threshold
        )
    };
    let mut parts = Vec::new();
    if let Some(r) = &config.readiness {
        parts.push(probe("readiness", r));
    }
    if let Some(l) = &config.liveness {
        parts.push(probe("liveness", l));
    }
    if parts.is_empty()
        && let Some(path) = &config.health
    {
        parts.push(format!("GET {path}"));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

#[cfg(test)]
#[path = "alert_context_tests.rs"]
pub(crate) mod tests;
