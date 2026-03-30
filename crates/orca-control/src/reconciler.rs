//! Reconciler: ensures actual running containers match desired service config.

use std::time::Duration;

use tracing::{error, info};

use orca_core::config::ServiceConfig;
use orca_core::types::{Replicas, WorkloadSpec, WorkloadStatus};

use crate::state::{AppState, InstanceState, ServiceState};
use orca_proxy::RouteTarget;

/// Reconcile all services: make reality match the desired config.
///
/// For each service, creates or removes containers to match the desired replica count,
/// then updates the routing table for services with a domain.
///
/// # Errors
///
/// Returns errors from individual service reconciliations collected as strings.
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

/// Reconcile a single service to match its desired state.
async fn reconcile_service(state: &AppState, config: &ServiceConfig) -> anyhow::Result<()> {
    let desired = match &config.replicas {
        Replicas::Fixed(n) => *n,
        Replicas::Auto => 1,
    };

    let spec = service_config_to_spec(config)?;

    let mut services = state.services.write().await;
    let svc_state = services
        .entry(config.name.clone())
        .or_insert_with(|| ServiceState::from_config(config.clone()));

    // Update config and desired replicas
    svc_state.config = config.clone();
    svc_state.desired_replicas = desired;

    let current = svc_state.instances.len() as u32;

    if current < desired {
        // Scale up: create and start new instances
        let to_create = desired - current;
        info!(
            "Scaling up {}: {} -> {} (+{})",
            config.name, current, desired, to_create
        );

        for i in current..desired {
            let mut replica_spec = spec.clone();
            if desired > 1 {
                replica_spec.name = format!("{}-{i}", spec.name);
            }

            match create_and_start_instance(state, &replica_spec).await {
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
        // Scale down: stop and remove excess instances
        let to_remove = current - desired;
        info!(
            "Scaling down {}: {} -> {} (-{})",
            config.name, current, desired, to_remove
        );

        for _ in 0..to_remove {
            if let Some(instance) = svc_state.instances.pop() {
                let _ = state
                    .runtime
                    .stop(&instance.handle, Duration::from_secs(10))
                    .await;
                let _ = state.runtime.remove(&instance.handle).await;
            }
        }
    }

    // Refresh status of all instances
    for instance in &mut svc_state.instances {
        if let Ok(status) = state.runtime.status(&instance.handle).await {
            instance.status = status;
        }
    }

    // Update routing table
    drop(services); // Release write lock before taking route_table lock
    update_routes(state, config).await;

    Ok(())
}

/// Create and start a single workload instance, returning its state.
async fn create_and_start_instance(
    state: &AppState,
    spec: &WorkloadSpec,
) -> anyhow::Result<InstanceState> {
    let handle = state.runtime.create(spec).await?;
    state.runtime.start(&handle).await?;

    // Resolve the host-accessible port
    let host_port = if let Some(port) = spec.port {
        state
            .runtime
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
    })
}

/// Scale a specific service to the given replica count.
///
/// # Errors
///
/// Returns an error if the service is not found or reconciliation fails.
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

/// Update the routing table for a service based on its current instances.
async fn update_routes(state: &AppState, config: &ServiceConfig) {
    let Some(domain) = &config.domain else {
        return;
    };

    let services = state.services.read().await;
    let Some(svc) = services.get(&config.name) else {
        return;
    };

    let targets: Vec<RouteTarget> = svc
        .instances
        .iter()
        .filter(|i| i.status == WorkloadStatus::Running)
        .filter_map(|i| {
            i.host_port.map(|port| RouteTarget {
                address: format!("127.0.0.1:{port}"),
                service_name: config.name.clone(),
            })
        })
        .collect();

    drop(services);

    let mut route_table = state.route_table.write().await;
    if targets.is_empty() {
        route_table.remove(domain);
    } else {
        route_table.insert(domain.clone(), targets);
    }
}

/// Convert a [`ServiceConfig`] into a [`WorkloadSpec`] for the runtime.
fn service_config_to_spec(config: &ServiceConfig) -> anyhow::Result<WorkloadSpec> {
    let image = config
        .image
        .clone()
        .or_else(|| config.module.clone())
        .ok_or_else(|| anyhow::anyhow!("service '{}' has no image or module", config.name))?;

    Ok(WorkloadSpec {
        name: config.name.clone(),
        runtime: config.runtime,
        image,
        replicas: config.replicas.clone(),
        port: config.port,
        domain: config.domain.clone(),
        health: config.health.clone(),
        env: config.env.clone(),
        resources: config.resources.clone(),
        volume: config.volume.clone(),
        deploy: config.deploy.clone(),
        placement: config.placement.clone(),
        triggers: config
            .triggers
            .iter()
            .filter_map(|t| t.clone().try_into().ok())
            .collect(),
    })
}
