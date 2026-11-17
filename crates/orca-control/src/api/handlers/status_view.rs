//! One service's entry in `GET /api/v1/status`, shared with
//! `GET /api/v1/services/{name}`.

use std::collections::HashMap;

use orca_core::api_types::{FailureInfo, ServiceStatus};

use crate::state::ServiceState;
use crate::stats::ContainerStats;

pub(crate) fn service_status(
    svc: &ServiceState,
    stats_cache: &HashMap<String, ContainerStats>,
    failures: &HashMap<String, FailureInfo>,
) -> ServiceStatus {
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
        depends_on: svc.config.depends_on.clone(),
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
