//! Declarative reconciler: continuously apply a directory of service configs.
//!
//! When `[reconcile].config_dir` is set, the master periodically loads service
//! definitions from it and applies any that are **new or changed** — so adding
//! a service is just dropping its `service.toml` in the dir, with no manual
//! `orca deploy`. Unchanged services are deliberately skipped so we never
//! re-deploy a steady-state workload (which, for remote services, would
//! otherwise re-queue a deploy every tick). Healing of degraded-but-unchanged
//! services stays the watchdog's job; pruning of removed services is left to
//! the operator.

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use orca_core::config::{ServiceConfig, ServicesConfig};

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

    // Apply only services that are new or whose spec changed. Snapshot the
    // registry once under a read lock.
    let changed: Vec<ServiceConfig> = {
        let services = state.services.read().await;
        valid
            .into_iter()
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
