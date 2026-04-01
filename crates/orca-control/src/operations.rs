//! Service lifecycle operations: stop, scale, redeploy, rollback.

use std::time::Duration;

use tracing::info;

use orca_core::runtime::Runtime;
use orca_core::types::Replicas;

use crate::reconciler::{get_runtime, reconcile_service};
use crate::state::AppState;

/// Graceful shutdown timeout for container stop operations.
const GRACEFUL_TIMEOUT: Duration = Duration::from_secs(30);

/// Stop a service: scale to 0 and remove from state.
pub async fn stop(state: &AppState, service_name: &str) -> anyhow::Result<()> {
    scale(state, service_name, 0).await?;
    let mut services = state.services.write().await;
    services.remove(service_name);
    let mut routes = state.route_table.write().await;
    routes.retain(|_, targets| {
        targets.retain(|t| t.service_name != service_name);
        !targets.is_empty()
    });
    let mut triggers = state.wasm_triggers.write().await;
    triggers.retain(|t| t.service_name != service_name);
    info!("Stopped and removed service: {service_name}");
    Ok(())
}

/// Stop all services.
pub async fn stop_all(state: &AppState) -> anyhow::Result<()> {
    let names: Vec<String> = {
        let services = state.services.read().await;
        services.keys().cloned().collect()
    };
    for name in &names {
        if let Err(e) = stop(state, name).await {
            tracing::error!("Failed to stop {name}: {e}");
        }
    }
    Ok(())
}

/// Redeploy a service using a rolling update: start new instances before
/// stopping old ones, with a 30-second graceful shutdown timeout.
pub async fn redeploy(state: &AppState, service_name: &str) -> anyhow::Result<()> {
    let config = {
        let services = state.services.read().await;
        let svc = services
            .get(service_name)
            .ok_or_else(|| anyhow::anyhow!("service '{}' not found", service_name))?;
        svc.config.clone()
    };
    let runtime = get_runtime(state, config.runtime)?;

    // Collect old instance handles before creating new ones.
    let old_handles: Vec<_> = {
        let services = state.services.read().await;
        services
            .get(service_name)
            .map(|svc| svc.instances.iter().map(|i| i.handle.clone()).collect())
            .unwrap_or_default()
    };

    // Clear instance list so reconcile creates fresh replicas.
    {
        let mut services = state.services.write().await;
        if let Some(svc) = services.get_mut(service_name) {
            svc.instances.clear();
        }
    }

    // Start new instances (reconcile will create the desired count).
    reconcile_service(state, &config).await?;

    // Gracefully stop old instances with a 30-second timeout.
    for handle in &old_handles {
        let _ = runtime.stop(handle, GRACEFUL_TIMEOUT).await;
        let _ = runtime.remove(handle).await;
    }

    info!("Redeployed service: {service_name}");
    Ok(())
}

/// Rollback a service to its previous deploy configuration.
pub async fn rollback(state: &AppState, service_name: &str) -> anyhow::Result<()> {
    let previous_config = {
        let history = state.deploy_history.read().await;
        history
            .get_previous(service_name)
            .map(|r| r.config.clone())
            .ok_or_else(|| anyhow::anyhow!("no previous deploy for '{service_name}'"))?
    };
    info!("Rolling back {service_name} to previous config");
    let runtime = get_runtime(state, previous_config.runtime)?;
    {
        let mut services = state.services.write().await;
        if let Some(svc) = services.get_mut(service_name) {
            for instance in svc.instances.drain(..) {
                let _ = runtime
                    .stop(&instance.handle, Duration::from_secs(10))
                    .await;
                let _ = runtime.remove(&instance.handle).await;
            }
        }
    }
    reconcile_service(state, &previous_config).await?;
    let mut history = state.deploy_history.write().await;
    history.record(&previous_config);
    info!("Rolled back service: {service_name}");
    Ok(())
}

/// Scale a specific service to the given replica count.
pub async fn scale(state: &AppState, service_name: &str, replicas: u32) -> anyhow::Result<()> {
    let config = {
        let services = state.services.read().await;
        let svc = services
            .get(service_name)
            .ok_or_else(|| anyhow::anyhow!("service '{}' not found", service_name))?;
        let mut config = svc.config.clone();
        config.replicas = Replicas::Fixed(replicas);
        config
    };
    reconcile_service(state, &config).await
}

/// Perform a rolling update: start new instances one at a time, wait for
/// readiness, then stop the corresponding old instance. Minimizes downtime
/// by overlapping old and new containers briefly.
pub(crate) async fn rolling_update(
    state: &AppState,
    runtime: &dyn Runtime,
    config: &orca_core::config::ServiceConfig,
    spec: &orca_core::types::WorkloadSpec,
    desired: u32,
) -> anyhow::Result<()> {
    let old_handles: Vec<_> = {
        let services = state.services.read().await;
        services
            .get(&config.name)
            .map(|svc| svc.instances.iter().map(|i| i.handle.clone()).collect())
            .unwrap_or_default()
    };

    for i in 0..desired {
        let mut replica_spec = spec.clone();
        if desired > 1 {
            replica_spec.name = format!("{}-{i}", spec.name);
        }

        match crate::instance::create_and_start_instance(runtime, &replica_spec).await {
            Ok(new_instance) => {
                if let Some(old_handle) = old_handles.get(i as usize) {
                    let _ = runtime.stop(old_handle, GRACEFUL_TIMEOUT).await;
                    let _ = runtime.remove(old_handle).await;
                }
                let mut services = state.services.write().await;
                if let Some(svc) = services.get_mut(&config.name) {
                    if (i as usize) < svc.instances.len() {
                        svc.instances[i as usize] = new_instance;
                    } else {
                        svc.instances.push(new_instance);
                    }
                }
            }
            Err(e) => {
                tracing::error!("Rolling update failed for {}-{i}: {e}", config.name);
            }
        }
    }

    // Update routing to point to new instances
    match config.runtime {
        orca_core::types::RuntimeKind::Container => {
            crate::routes::update_container_routes(state, config).await;
        }
        orca_core::types::RuntimeKind::Wasm => {
            crate::routes::update_wasm_triggers(state, config).await;
        }
    }

    info!("Rolling update complete for {}", config.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graceful_timeout_is_30_seconds() {
        assert_eq!(GRACEFUL_TIMEOUT, Duration::from_secs(30));
    }
}
