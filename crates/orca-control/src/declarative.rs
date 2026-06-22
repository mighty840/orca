//! Declarative reconciler: continuously apply a directory of service configs.
//!
//! When `[reconcile].config_dir` is set, the master periodically loads service
//! definitions from it and reconciles the cluster against them:
//! - **new or changed** services are applied (so adding a service is just
//!   dropping its `service.toml` in the dir, no manual `orca deploy`);
//! - services **no longer declared** are pruned (stopped + purged) — guarded so
//!   an empty/garbled load never mass-deletes;
//! - **paused** (`orca stop`) services are left alone: not restarted, not pruned.
//!
//! Unchanged running services are deliberately skipped so we never re-deploy a
//! steady-state workload (which, for remote services, would otherwise re-queue a
//! deploy every tick). Healing of degraded-but-unchanged services stays the
//! watchdog's job.

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use orca_core::config::{ServiceConfig, ServicesConfig};
use orca_core::ws_types::MasterMessage;

use crate::state::AppState;

/// Spawn the declarative reconciler if a config dir is configured. Returns
/// `false` (no-op) when `[reconcile].config_dir` is unset.
pub fn spawn_declarative_reconciler(state: Arc<AppState>) -> bool {
    let Some(dir) = state
        .cluster_config
        .reconcile
        .as_ref()
        .and_then(|r| r.config_dir.clone())
    else {
        return false;
    };
    let interval = Duration::from_secs(
        state
            .cluster_config
            .reconcile
            .as_ref()
            .map(|r| r.interval_secs)
            .unwrap_or(30)
            .max(1),
    );
    tokio::spawn(async move {
        info!(
            "Declarative reconciler started (dir={dir}, interval={}s)",
            interval.as_secs()
        );
        loop {
            tokio::time::sleep(interval).await;
            apply_config_dir(&state, &dir).await;
        }
    });
    true
}

/// Load service configs from `dir` (a directory of `<project>/service.toml`, or
/// a single `services.toml`) and apply the new/changed ones. Exposed for tests.
pub async fn apply_config_dir(state: &AppState, dir: &str) {
    let path = std::path::Path::new(dir);
    let loaded = if path.is_dir() {
        ServicesConfig::load_dir(path)
    } else {
        ServicesConfig::load(path)
    };
    let configs = match loaded {
        Ok(c) => c.service,
        Err(e) => {
            warn!("declarative reconcile: {e}");
            return;
        }
    };

    // Guard: never act on an empty config view — a transient/garbled load must
    // not mass-prune. (`load_dir` already errors on an empty dir; this covers a
    // single `services.toml` that parsed to zero services.)
    if configs.is_empty() {
        warn!("declarative reconcile: {dir} declared no services; skipping (won't prune)");
        return;
    }

    // Validate up front; drop invalid configs with a recorded failure so the
    // operator sees the reason in `orca status` rather than silent inaction.
    let mut valid = Vec::with_capacity(configs.len());
    for cfg in configs {
        if let Err(e) = cfg.validate() {
            warn!(
                "declarative reconcile: invalid config for {}: {e}",
                cfg.name
            );
            state
                .last_failures
                .write()
                .await
                .insert(cfg.name.clone(), crate::failures::from_deploy_error(&e));
            continue;
        }
        valid.push(cfg);
    }
    if valid.is_empty() {
        warn!("declarative reconcile: no valid services in {dir}; skipping (won't prune)");
        return;
    }

    let declared: std::collections::HashSet<String> =
        valid.iter().map(|c| c.name.clone()).collect();
    let stopped: std::collections::HashSet<String> = match &state.store {
        Some(s) => s.get_stopped().unwrap_or_default(),
        None => std::collections::HashSet::new(),
    };

    // Prune services the master knows about that are no longer declared in
    // `service.toml`. Paused (`stopped`) services are kept — pausing is an
    // explicit "keep" signal. Guarded above, so an empty/garbled config view
    // never reaches here.
    let to_prune: Vec<String> = {
        let services = state.services.read().await;
        services
            .keys()
            .filter(|n| !declared.contains(n.as_str()) && !stopped.contains(n.as_str()))
            .cloned()
            .collect()
    };
    for name in &to_prune {
        prune_service(state, name).await;
    }

    // Apply new/changed services. Never auto-start a paused service even if its
    // on-disk spec changed — resuming is an explicit `orca start`.
    let changed: Vec<ServiceConfig> = {
        let services = state.services.read().await;
        valid
            .into_iter()
            .filter(|cfg| !stopped.contains(&cfg.name))
            .filter(|cfg| match services.get(&cfg.name) {
                None => true,
                Some(svc) => !svc.config.spec_matches(cfg),
            })
            .collect()
    };
    if changed.is_empty() {
        return;
    }

    let names: Vec<&str> = changed.iter().map(|c| c.name.as_str()).collect();
    info!(
        "Declaratively applying {} service(s): {}",
        changed.len(),
        names.join(", ")
    );

    let (deployed, errors) = crate::reconciler::reconcile(state, &changed).await;

    // Persist applied configs so they survive a master restart (same path as
    // a CLI deploy).
    if let Some(store) = &state.store {
        for cfg in &changed {
            if deployed.contains(&cfg.name)
                && let Err(e) = store.set_service(&cfg.name, cfg)
            {
                warn!("declarative reconcile: persist {} failed: {e}", cfg.name);
            }
        }
    }
    for e in errors {
        warn!("declarative reconcile error: {e}");
    }
}

/// Fully remove a service that's no longer declared: stop its workloads, drop
/// it from the registry + persisted store, and clear its routes. Unlike
/// `stop` (which pauses), this is a real removal triggered by the service
/// disappearing from `service.toml`.
async fn prune_service(state: &AppState, name: &str) {
    info!("Declaratively pruning '{name}' (removed from service.toml)");

    let is_remote = {
        let services = state.services.read().await;
        services
            .get(name)
            .map(|s| {
                s.config
                    .placement
                    .as_ref()
                    .and_then(|p| p.node.as_ref())
                    .is_some()
            })
            .unwrap_or(false)
    };

    if is_remote {
        // Tell agents to stop the container (idempotent — only the agent that
        // hosts it acts). Stopping it also prevents the adoption reconciler
        // from re-registering it (it only adopts *running* containers).
        let agents = state.ws_agents.read().await;
        for tx in agents.values() {
            let _ = tx
                .send(MasterMessage::Stop {
                    service_name: name.to_string(),
                })
                .await;
        }
    } else {
        // Local: scale to 0 tears down the containers.
        let _ = crate::reconciler::scale(state, name, 0).await;
    }

    state.services.write().await.remove(name);
    if let Some(store) = &state.store {
        let _ = store.remove_service(name);
        let _ = store.unmark_stopped(name);
    }
    {
        let mut routes = state.route_table.write().await;
        routes.retain(|_, targets| {
            targets.retain(|t| t.service_name != name);
            !targets.is_empty()
        });
    }
    {
        let mut triggers = state.wasm_triggers.write().await;
        triggers.retain(|t| t.service_name != name);
    }
}
