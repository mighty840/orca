//! Deciding which alerts a monitor cycle opens (#181). Pure: no model call,
//! no delivery, no lock held beyond the caller's brief read.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use tracing::info;

use crate::context::ClusterContext;
use orca_core::types::AlertSeverity;

/// One alert to open this cycle.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AlertRequest {
    pub service: String,
    pub severity: AlertSeverity,
    pub trigger: String,
}

/// Evaluate every rule against `ctx`. `active` holds the services that
/// already have an open alert; at most one alert per service is planned,
/// and the first matching rule wins (the rules' order is unchanged from before).
pub(crate) fn plan_alerts(
    ctx: &ClusterContext,
    now: Instant,
    down_grace: Duration,
    down_since: &mut HashMap<String, Instant>,
    active: &HashSet<String>,
) -> Vec<AlertRequest> {
    let mut planned: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    let mut open = |service: &str, severity, trigger: String| {
        if !active.contains(service) && planned.insert(service.to_string()) {
            out.push(AlertRequest {
                service: service.to_string(),
                severity,
                trigger,
            });
        }
    };

    for svc in &ctx.services {
        // Service down, but debounce deploy blips: page only once the outage
        // has outlasted `down_grace`.
        if svc.replicas_running == 0 && svc.replicas_desired > 0 {
            let since = *down_since.entry(svc.name.clone()).or_insert(now);
            let down_for = now.saturating_duration_since(since);
            if down_for < down_grace {
                info!(
                    "Service '{}' has 0/{} replicas but only down {}s (< {}s grace) — deferring alert (likely a deploy)",
                    svc.name,
                    svc.replicas_desired,
                    down_for.as_secs(),
                    down_grace.as_secs()
                );
            } else {
                open(
                    &svc.name,
                    AlertSeverity::Critical,
                    format!(
                        "Service '{}' has 0/{} replicas running. Restarts in 24h: {}. Recent errors: {}",
                        svc.name, svc.replicas_desired, svc.restart_count_24h, svc.error_count_1h
                    ),
                );
            }
        } else {
            down_since.remove(&svc.name);
        }

        if svc.restart_count_24h > 10 && svc.replicas_running > 0 {
            open(
                &svc.name,
                AlertSeverity::Warning,
                format!(
                    "Service '{}' has restarted {} times in the last 24 hours. \
                     Currently {}/{} replicas are running.",
                    svc.name, svc.restart_count_24h, svc.replicas_running, svc.replicas_desired
                ),
            );
        }

        if svc.error_count_1h > 100 {
            open(
                &svc.name,
                AlertSeverity::Warning,
                format!(
                    "Service '{}' has {} errors in the last hour. Recent log lines:\n{}",
                    svc.name,
                    svc.error_count_1h,
                    svc.recent_logs
                        .iter()
                        .take(5)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            );
        }
    }

    // Drop grace timers for services no longer in the cluster.
    let live: HashSet<&str> = ctx.services.iter().map(|s| s.name.as_str()).collect();
    down_since.retain(|name, _| live.contains(name.as_str()));

    for node in &ctx.nodes {
        for gpu in &node.gpu_summary {
            if let Some(temp) = gpu.temperature
                && temp > 90.0
            {
                open(
                    &format!("node-{}-gpu-{}", node.id, gpu.index),
                    AlertSeverity::Warning,
                    format!(
                        "GPU {} on node {} ({}) temperature is {:.0}C (>90C threshold). \
                         Utilization: {:.0}%, VRAM: {}/{}MB",
                        gpu.index,
                        node.id,
                        gpu.model,
                        temp,
                        gpu.utilization,
                        gpu.vram_used_mb,
                        gpu.vram_total_mb
                    ),
                );
            }
            if gpu.vram_total_mb > 0 {
                let usage_pct = (gpu.vram_used_mb as f64 / gpu.vram_total_mb as f64) * 100.0;
                if usage_pct > 95.0 {
                    open(
                        &format!("node-{}-gpu-{}-vram", node.id, gpu.index),
                        AlertSeverity::Warning,
                        format!(
                            "GPU {} on node {} VRAM is {:.0}% full ({}/{}MB). Workloads may OOM.",
                            gpu.index, node.id, usage_pct, gpu.vram_used_mb, gpu.vram_total_mb
                        ),
                    );
                }
            }
        }
    }
    out
}
