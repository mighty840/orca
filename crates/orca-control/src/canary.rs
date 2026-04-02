//! Canary deployment operations: deploy canary instances and promote them.

use std::time::Duration;

use tracing::info;

use orca_core::runtime::Runtime;

use crate::state::AppState;

/// Graceful shutdown timeout for canary operations.
const GRACEFUL_TIMEOUT: Duration = Duration::from_secs(5);

/// Deploy canary instances alongside existing stable instances.
///
/// Keeps old instances running and starts new instances marked as canary.
/// The route table assigns weights according to `deploy.canary_weight`.
pub(crate) async fn canary_deploy(
    state: &AppState,
    runtime: &dyn Runtime,
    config: &orca_core::config::ServiceConfig,
    spec: &orca_core::types::WorkloadSpec,
    desired: u32,
) -> anyhow::Result<()> {
    for i in 0..desired {
        let mut replica_spec = spec.clone();
        // Use "-canary-N" suffix to avoid name conflicts with stable instances.
        replica_spec.name = format!("{}-canary-{i}", spec.name);

        match crate::instance::create_and_start_instance(runtime, &replica_spec).await {
            Ok(mut instance) => {
                instance.is_canary = true;
                let mut services = state.services.write().await;
                if let Some(svc) = services.get_mut(&config.name) {
                    svc.instances.push(instance);
                }
            }
            Err(e) => {
                tracing::error!("Canary instance {}-canary-{i} failed: {e}", config.name);
            }
        }
    }

    // Update routing with canary weights
    match config.runtime {
        orca_core::types::RuntimeKind::Container => {
            crate::routes::update_container_routes(state, config).await;
        }
        orca_core::types::RuntimeKind::Wasm => {
            crate::routes::update_wasm_triggers(state, config).await;
        }
    }

    info!("Canary deploy complete for {}", config.name);
    Ok(())
}

/// Promote canary instances to stable: remove old instances, mark canary
/// instances as stable, and set all weights to 100.
pub async fn promote(state: &AppState, service_name: &str) -> anyhow::Result<()> {
    let runtime_kind = {
        let services = state.services.read().await;
        let svc = services
            .get(service_name)
            .ok_or_else(|| anyhow::anyhow!("service '{}' not found", service_name))?;
        svc.config.runtime
    };
    let runtime = crate::reconciler::get_runtime(state, runtime_kind)?;

    let mut services = state.services.write().await;
    let svc = services
        .get_mut(service_name)
        .ok_or_else(|| anyhow::anyhow!("service '{}' not found", service_name))?;

    // Separate stable (old) and canary (new) instances
    let (canary, stable): (Vec<_>, Vec<_>) = svc.instances.drain(..).partition(|i| i.is_canary);

    if canary.is_empty() {
        // Put instances back and bail
        svc.instances = stable;
        anyhow::bail!("no canary instances to promote for '{service_name}'");
    }

    // Stop and remove old stable instances
    for instance in &stable {
        let _ = runtime.stop(&instance.handle, GRACEFUL_TIMEOUT).await;
        let _ = runtime.remove(&instance.handle).await;
    }

    // Mark canary instances as stable
    for mut instance in canary {
        instance.is_canary = false;
        svc.instances.push(instance);
    }

    let config = svc.config.clone();
    drop(services);

    // Refresh routes with weight=100 for all
    crate::routes::update_container_routes(state, &config).await;

    info!("Promoted canary to stable for {service_name}");
    Ok(())
}
