//! Service lifecycle operations: stop, scale, redeploy, rollback.

use std::time::Duration;

use tracing::info;

use orca_core::types::Replicas;

use crate::reconciler::{get_runtime, reconcile_service};
use crate::state::AppState;

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

/// Redeploy a service: stop all instances and recreate with fresh image pull.
pub async fn redeploy(state: &AppState, service_name: &str) -> anyhow::Result<()> {
    let config = {
        let services = state.services.read().await;
        let svc = services
            .get(service_name)
            .ok_or_else(|| anyhow::anyhow!("service '{}' not found", service_name))?;
        svc.config.clone()
    };
    let runtime = get_runtime(state, config.runtime)?;
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
    reconcile_service(state, &config).await?;
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
