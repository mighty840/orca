//! Reconciler: ensures actual running containers/wasm instances match desired service config.

use std::time::Duration;

use tracing::{error, info};

use orca_core::config::ServiceConfig;
use orca_core::runtime::Runtime;
use orca_core::types::{Replicas, RuntimeKind, WorkloadSpec, WorkloadStatus};

use crate::routes::{service_config_to_spec, update_container_routes, update_wasm_triggers};
use crate::state::{AppState, InstanceState, ServiceState};

/// Reconcile all services: make reality match the desired config.
///
/// For each service, creates or removes workloads to match the desired replica count,
/// then updates the routing table (containers) or trigger table (wasm).
pub async fn reconcile(state: &AppState, services: &[ServiceConfig]) -> (Vec<String>, Vec<String>) {
    let mut deployed = Vec::new();
    let mut errors = Vec::new();

    for svc_config in services {
        match reconcile_service(state, svc_config).await {
            Ok(()) => deployed.push(svc_config.name.clone()),
            Err(e) => errors.push(format!("{}: {e}", svc_config.name)),
        }
    }

    deployed
        .iter()
        .for_each(|name| info!("Deployed service: {name}"));

    (deployed, errors)
}

/// Get the appropriate runtime for a service config.
fn get_runtime(state: &AppState, kind: RuntimeKind) -> anyhow::Result<&dyn Runtime> {
    match kind {
        RuntimeKind::Container => Ok(state.container_runtime.as_ref()),
        RuntimeKind::Wasm => state
            .wasm_runtime
            .as_ref()
            .map(|r| r.as_ref() as &dyn Runtime)
            .ok_or_else(|| anyhow::anyhow!("Wasm runtime not available")),
    }
}

/// Reconcile a single service to match its desired state.
async fn reconcile_service(state: &AppState, config: &ServiceConfig) -> anyhow::Result<()> {
    let desired = match &config.replicas {
        Replicas::Fixed(n) => *n,
        Replicas::Auto => 1,
    };

    let spec = service_config_to_spec(config)?;
    let runtime = get_runtime(state, config.runtime)?;

    let mut services = state.services.write().await;
    let svc_state = services
        .entry(config.name.clone())
        .or_insert_with(|| ServiceState::from_config(config.clone()));

    svc_state.config = config.clone();
    svc_state.desired_replicas = desired;

    let current = svc_state.instances.len() as u32;

    if current < desired {
        let to_create = desired - current;
        info!(
            "Scaling up {} ({:?}): {} -> {} (+{})",
            config.name, config.runtime, current, desired, to_create
        );

        for i in current..desired {
            let mut replica_spec = spec.clone();
            if desired > 1 {
                replica_spec.name = format!("{}-{i}", spec.name);
            }

            match create_and_start_instance(runtime, &replica_spec).await {
                Ok(instance) => {
                    svc_state.instances.push(instance);
                }
                Err(e) => {
                    error!("Failed to create instance {}-{i}: {e}", config.name);
                    return Err(e);
                }
            }
        }
    } else if current > desired {
        let to_remove = current - desired;
        info!(
            "Scaling down {} ({:?}): {} -> {} (-{})",
            config.name, config.runtime, current, desired, to_remove
        );

        for _ in 0..to_remove {
            if let Some(instance) = svc_state.instances.pop() {
                let _ = runtime
                    .stop(&instance.handle, Duration::from_secs(10))
                    .await;
                let _ = runtime.remove(&instance.handle).await;
            }
        }
    }

    // Refresh status of all instances
    for instance in &mut svc_state.instances {
        if let Ok(status) = runtime.status(&instance.handle).await {
            instance.status = status;
        }
    }

    drop(services);

    // Update routing based on runtime type
    match config.runtime {
        RuntimeKind::Container => update_container_routes(state, config).await,
        RuntimeKind::Wasm => update_wasm_triggers(state, config).await,
    }

    Ok(())
}

/// Create and start a single workload instance.
async fn create_and_start_instance(
    runtime: &dyn Runtime,
    spec: &WorkloadSpec,
) -> anyhow::Result<InstanceState> {
    let handle = runtime.create(spec).await?;
    runtime.start(&handle).await?;

    let host_port = if let Some(port) = spec.port {
        runtime
            .resolve_host_port(&handle, port)
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    Ok(InstanceState {
        handle,
        status: WorkloadStatus::Running,
        host_port,
        health: orca_core::types::HealthState::Unknown,
    })
}

/// Stop a service: scale to 0 and remove from state.
pub async fn stop(state: &AppState, service_name: &str) -> anyhow::Result<()> {
    // Scale to 0 first (stops all containers)
    scale(state, service_name, 0).await?;
    // Remove from services map
    let mut services = state.services.write().await;
    services.remove(service_name);
    // Remove from route table
    let mut routes = state.route_table.write().await;
    routes.retain(|_, targets| {
        targets.retain(|t| t.service_name != service_name);
        !targets.is_empty()
    });
    // Remove wasm triggers
    let mut triggers = state.wasm_triggers.write().await;
    triggers.retain(|t| t.service_name != service_name);
    tracing::info!("Stopped and removed service: {service_name}");
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

/// Redeploy a service: stop all instances and recreate them (forces fresh image pull).
pub async fn redeploy(state: &AppState, service_name: &str) -> anyhow::Result<()> {
    let config = {
        let services = state.services.read().await;
        let svc = services
            .get(service_name)
            .ok_or_else(|| anyhow::anyhow!("service '{}' not found", service_name))?;
        svc.config.clone()
    };

    let runtime = get_runtime(state, config.runtime)?;

    // Stop and remove all existing instances
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

    // Reconcile again to recreate instances with fresh image
    reconcile_service(state, &config).await?;
    info!("Redeployed service: {service_name}");
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
