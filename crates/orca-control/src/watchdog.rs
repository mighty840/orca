//! Background watchdog that periodically reconciles degraded services.
//!
//! Checks every 30 seconds whether running containers match desired replicas.
//! Removes stopped/failed instances and triggers re-reconciliation, then
//! refreshes the route table.

use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, info, warn};

use orca_core::types::{RuntimeKind, WorkloadStatus};

use crate::reconciler::get_runtime;
use crate::routes::update_container_routes;
use crate::state::AppState;

/// Default watchdog check interval.
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(30);

/// Spawn the watchdog as a background tokio task.
pub fn spawn_watchdog(state: Arc<AppState>) {
    tokio::spawn(async move {
        run_watchdog(&state).await;
    });
}

/// Main watchdog loop. Runs forever, checking services each interval.
async fn run_watchdog(state: &AppState) {
    info!(
        "Watchdog started (interval: {}s)",
        WATCHDOG_INTERVAL.as_secs()
    );
    loop {
        tokio::time::sleep(WATCHDOG_INTERVAL).await;
        check_services(state).await;
    }
}

/// Check all services for degraded instances and re-reconcile as needed.
async fn check_services(state: &AppState) {
    // Collect service names and their runtime kinds under a read lock.
    let service_info: Vec<(String, RuntimeKind)> = {
        let services = state.services.read().await;
        services
            .values()
            .map(|svc| (svc.config.name.clone(), svc.config.runtime))
            .collect()
    };

    for (name, runtime_kind) in &service_info {
        let needs_reconcile = check_and_prune(state, name, *runtime_kind).await;

        if needs_reconcile {
            let config = {
                let services = state.services.read().await;
                services.get(name).map(|svc| svc.config.clone())
            };
            if let Some(config) = config {
                info!(service = %name, "Watchdog triggering reconciliation");
                if let Err(e) = crate::reconciler::reconcile_service(state, &config).await {
                    warn!(service = %name, "Watchdog reconciliation failed: {e}");
                }
            }
        }

        // Refresh routes for container services (Item 5: stale route cleanup).
        if *runtime_kind == RuntimeKind::Container {
            let config = {
                let services = state.services.read().await;
                services.get(name).map(|svc| svc.config.clone())
            };
            if let Some(config) = config {
                update_container_routes(state, &config).await;
            }
        }
    }
}

/// Check instance statuses and remove stopped/failed ones.
///
/// Returns `true` if the service is degraded and needs reconciliation.
async fn check_and_prune(state: &AppState, service_name: &str, runtime_kind: RuntimeKind) -> bool {
    let runtime = match get_runtime(state, runtime_kind) {
        Ok(r) => r,
        Err(_) => return false,
    };

    let mut services = state.services.write().await;
    let Some(svc) = services.get_mut(service_name) else {
        return false;
    };

    let mut removed = 0u32;
    svc.instances.retain_mut(|inst| {
        // We cannot await inside retain, so check status synchronously
        // by looking at the cached status. The health checker and
        // reconciler keep this updated.
        match inst.status {
            WorkloadStatus::Stopped | WorkloadStatus::Failed => {
                removed += 1;
                false
            }
            _ => true,
        }
    });

    if removed > 0 {
        info!(
            service = %service_name,
            removed,
            remaining = svc.instances.len(),
            "Watchdog pruned stopped/failed instances"
        );
    }

    let current = svc.instances.len() as u32;
    let desired = svc.desired_replicas;

    if current < desired {
        debug!(
            service = %service_name,
            current,
            desired,
            "Service degraded, needs reconciliation"
        );
        // Also check live status from runtime for remaining instances
        // (update cached status so next prune catches them).
        for inst in &mut svc.instances {
            if let Ok(status) = runtime.status(&inst.handle).await {
                inst.status = status;
            }
        }
        true
    } else {
        false
    }
}
