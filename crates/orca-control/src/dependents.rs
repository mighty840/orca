//! Restart services whose `depends_on` target was just recreated.
//!
//! Regression context (2026-08-10, stepshots login outage): recreating a
//! valkey container gave it a new network identity, but the app that
//! `depends_on` it kept pooled TCP connections to the old address. A removed
//! container never sends RST, so those connections black-hole: every login
//! hung in the post-handler session write until the reverse proxy's read
//! timeout and surfaced as a 502 — while the deploy itself reported success
//! and anonymous traffic stayed healthy. The reconciler now restarts running
//! dependents (transitively) after a dependency's workloads were replaced.

use std::collections::HashSet;

use orca_core::config::ServiceConfig;

use crate::state::AppState;

/// Compute which services to restart, in dependency order, given the names of
/// services whose workloads were just replaced.
///
/// Transitive on purpose: a restarted dependent counts as changed for *its*
/// dependents in turn — its restart replaces the containers its own consumers
/// are connected to. Services already in `changed` are never returned; they
/// were just (re)created with fresh connections.
pub fn restart_set(all: &[ServiceConfig], changed: &[String]) -> Vec<String> {
    let mut changed: HashSet<&str> = changed.iter().map(String::as_str).collect();
    let ordered = crate::topo_sort::topo_sort(all);
    let mut to_restart = Vec::new();
    for svc in &ordered {
        if changed.contains(svc.name.as_str()) {
            continue;
        }
        if svc.depends_on.iter().any(|d| changed.contains(d.as_str())) {
            to_restart.push(svc.name.clone());
            changed.insert(svc.name.as_str());
        }
    }
    to_restart
}

/// Restart every running dependent of the just-changed services via
/// `operations::redeploy`, which already handles both master-local services
/// (rolling replace) and placement-pinned remote ones (WS stop + deploy).
///
/// Failures are logged, never propagated — a dependent that fails to restart
/// must not fail the deploy that triggered it. Stopped/paused services are
/// skipped: they reconnect on their own next start.
pub async fn restart_dependents(state: &AppState, changed: &[String]) {
    if changed.is_empty() {
        return;
    }
    let all: Vec<ServiceConfig> = {
        let services = state.services.read().await;
        services.values().map(|s| s.config.clone()).collect()
    };
    for name in restart_set(&all, changed) {
        let running = {
            let services = state.services.read().await;
            services
                .get(&name)
                .is_some_and(|s| !s.stopped && s.running_count() > 0)
        };
        if !running {
            continue;
        }
        tracing::info!(
            service = %name,
            "dependency was recreated — restarting dependent to refresh its connections"
        );
        if let Err(e) = crate::operations::redeploy(state, &name).await {
            tracing::warn!(service = %name, "dependent restart failed: {e}");
        }
    }
}
