//! Reconciler: ensures actual running containers/wasm instances match desired service config.

use std::time::Duration;

use tracing::{error, info};

use orca_core::config::ServiceConfig;
use orca_core::runtime::Runtime;
use orca_core::types::{DeployKind, Replicas, RuntimeKind, WorkloadSpec, WorkloadStatus};

use crate::instance::create_and_start_instance;
use crate::routes::{service_config_to_spec, update_container_routes, update_wasm_triggers};
use crate::state::{AppState, ServiceState};

/// Load a BYO TLS certificate and key from PEM files.
pub fn load_byo_cert(
    cert_path: &str,
    key_path: &str,
) -> anyhow::Result<rustls::sign::CertifiedKey> {
    let cert_pem = std::fs::read(cert_path)?;
    let key_pem = std::fs::read(key_path)?;
    let certs: Vec<_> =
        rustls_pemfile::certs(&mut cert_pem.as_slice()).collect::<Result<Vec<_>, _>>()?;
    let key = rustls_pemfile::private_key(&mut key_pem.as_slice())?
        .ok_or_else(|| anyhow::anyhow!("no private key in {key_path}"))?;
    let signing_key = rustls::crypto::aws_lc_rs::sign::any_supported_type(&key)?;
    Ok(rustls::sign::CertifiedKey::new(certs, signing_key))
}

/// Reconcile all services: make reality match the desired config.
///
/// For each service, creates or removes workloads to match the desired replica count,
/// then updates the routing table (containers) or trigger table (wasm).
pub async fn reconcile(state: &AppState, services: &[ServiceConfig]) -> (Vec<String>, Vec<String>) {
    let mut deployed = Vec::new();
    let mut errors = Vec::new();

    let ordered = crate::topo_sort::topo_sort(services);
    for svc_config in &ordered {
        match reconcile_service(state, svc_config).await {
            Ok(()) => {
                // Record successful deploy in history
                let mut history = state.deploy_history.write().await;
                history.record(svc_config);
                deployed.push(svc_config.name.clone());
            }
            Err(e) => errors.push(format!("{}: {e}", svc_config.name)),
        }
    }

    deployed
        .iter()
        .for_each(|name| info!("Deployed service: {name}"));

    (deployed, errors)
}

/// Get the appropriate runtime for a service config.
pub(crate) fn get_runtime(state: &AppState, kind: RuntimeKind) -> anyhow::Result<&dyn Runtime> {
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
pub(crate) async fn reconcile_service(
    state: &AppState,
    config: &ServiceConfig,
) -> anyhow::Result<()> {
    let desired = match &config.replicas {
        Replicas::Fixed(n) => *n,
        Replicas::Auto => 1,
    };

    let mut spec = service_config_to_spec(config)?;

    // If the service has a build config, build the image from source first.
    if let Some(build_config) = &config.build {
        info!("Building image for {} from source", config.name);
        let builder = orca_agent::builder::DockerBuilder::default_dir()
            .map_err(|e| anyhow::anyhow!("failed to create builder: {e}"))?;
        let image_tag = builder
            .build_service(build_config, &config.name)
            .await
            .map_err(|e| anyhow::anyhow!("build failed for {}: {e}", config.name))?;
        spec.image = image_tag;
    }

    // Check if placement targets a specific remote node
    if let Some(target_node_id) = find_target_node(state, config).await {
        queue_remote_deploy(state, target_node_id, &spec).await?;
        // Record the service + a placeholder instance on the master so
        // `orca status` shows remote-scheduled workloads alongside local
        // ones. Heartbeats from the target node will update the status
        // field; until the first update arrives we optimistically mark
        // the instance Running so the TUI doesn't paint it red during
        // the first few seconds after a deploy.
        let mut services = state.services.write().await;
        let svc_state = services
            .entry(config.name.clone())
            .or_insert_with(|| ServiceState::from_config(config.clone()));
        svc_state.config = config.clone();
        let desired = match &config.replicas {
            Replicas::Fixed(n) => *n,
            Replicas::Auto => 1,
        };
        svc_state.desired_replicas = desired;
        // If the instance list doesn't already have a placeholder for this
        // remote node, add one. Placeholder handles have no runtime_id
        // because the master doesn't own the container.
        if svc_state.instances.is_empty() {
            svc_state.instances.push(crate::state::InstanceState {
                handle: orca_core::runtime::WorkloadHandle {
                    runtime_id: format!("remote-{target_node_id}"),
                    name: format!("orca-{}", config.name),
                    metadata: Default::default(),
                },
                status: WorkloadStatus::Running,
                host_port: None,
                container_address: None,
                health: orca_core::types::HealthState::NoCheck,
                started_at: std::time::Instant::now(),
                is_canary: false,
            });
        }
        info!(
            "Queued deploy of {} to remote node {}",
            config.name, target_node_id
        );
        return Ok(());
    }

    let runtime = get_runtime(state, config.runtime)?;

    let mut services = state.services.write().await;
    let svc_state = services
        .entry(config.name.clone())
        .or_insert_with(|| ServiceState::from_config(config.clone()));

    // Skip scaling if we already have the right number of instances
    // with the same spec — prevents duplicate containers on re-deploy.
    // Compares image, env, cmd, ports, mounts, volume, domain, aliases,
    // extra_ports, strip_prefix, network, internal, and health.
    let same_spec = svc_state.config.spec_matches(config);

    svc_state.config = config.clone();
    svc_state.desired_replicas = desired;

    // Count only Running instances — Failed/Stopped should trigger replacement.
    let current = svc_state
        .instances
        .iter()
        .filter(|i| i.status == WorkloadStatus::Running)
        .count() as u32;
    // Prune dead instances so they don't block replacement.
    svc_state
        .instances
        .retain(|i| i.status == WorkloadStatus::Running);

    if current == desired && same_spec {
        info!(
            "Service {} already at desired state ({} replicas, same image) — skipping",
            config.name, desired
        );
        // Refresh status AND host_port of existing instances — containers
        // may have been restarted externally, changing their host port.
        // If status check errors (container missing), mark Stopped.
        for instance in &mut svc_state.instances {
            match runtime.status(&instance.handle).await {
                Ok(status) => instance.status = status,
                Err(_) => instance.status = WorkloadStatus::Stopped,
            }
            if let Some(p) = config.port
                && let Ok(Some(port)) = runtime.resolve_host_port(&instance.handle, p).await
            {
                instance.host_port = Some(port);
            }
        }
        // Prune any that are now dead
        svc_state
            .instances
            .retain(|i| i.status == WorkloadStatus::Running);
        // If all instances got pruned AND we still want some replicas,
        // fall through to re-create. Without the `desired > 0` guard this
        // would infinitely recurse for services declared with replicas=0.
        if svc_state.instances.is_empty() && desired > 0 {
            drop(services);
            return Box::pin(reconcile_service(state, config)).await;
        }
        drop(services);
        match config.runtime {
            RuntimeKind::Container => update_container_routes(state, config).await,
            RuntimeKind::Wasm => update_wasm_triggers(state, config).await,
        }
        return Ok(());
    }

    // Config changed but replica count is the same — update in place.
    if current == desired && !same_spec {
        let is_canary = config
            .deploy
            .as_ref()
            .is_some_and(|d| d.strategy == DeployKind::Canary);
        let name = &config.name;
        drop(services);
        if is_canary {
            info!("Canary deploy for {name} ({desired} stable + canary)");
            crate::operations::canary_deploy(state, runtime, config, &spec, desired).await?;
        } else {
            info!("Rolling update for {name} ({desired} replicas)");
            crate::operations::rolling_update(state, runtime, config, &spec, desired).await?;
        }
        return Ok(());
    }

    if current < desired {
        let to_create = desired - current;
        info!(
            "Scaling up {} ({:?}): {} -> {} (+{})",
            config.name, config.runtime, current, desired, to_create
        );

        let mut failures = 0u32;
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
                    failures += 1;
                }
            }
        }
        if failures > 0 {
            tracing::warn!("{failures}/{to_create} replicas failed for {}", config.name);
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

    // TLS cert provisioning for domains
    if let Some(domain) = &config.domain
        && let Some(resolver) = &state.cert_resolver
        && !resolver.has_cert(domain)
    {
        if let (Some(cert_path), Some(key_path)) = (&config.tls_cert, &config.tls_key) {
            // BYO cert: load from file
            match load_byo_cert(cert_path, key_path) {
                Ok(key) => {
                    resolver.add_cert(domain, std::sync::Arc::new(key));
                    tracing::info!(domain, "BYO TLS certificate loaded");
                }
                Err(e) => tracing::error!(domain, "Failed to load BYO cert: {e}"),
            }
        } else if let Some(acme) = &state.acme_manager {
            // ACME auto-provisioning
            if let Err(e) = acme.ensure_cert_for_resolver(domain, resolver).await {
                tracing::error!(domain, "Hot cert provisioning failed: {e}");
            }
        }
    }

    Ok(())
}

/// Find a registered node matching the service's placement constraint.
/// Returns `None` if no placement node is set or no matching node is found.
async fn find_target_node(state: &AppState, config: &ServiceConfig) -> Option<u64> {
    let placement = config.placement.as_ref()?;
    let target = placement.node.as_ref()?;
    let nodes = state.registered_nodes.read().await;
    for node in nodes.values() {
        if node.drain {
            continue;
        }
        if node.address.contains(target.as_str()) || target == &node.node_id.to_string() {
            return Some(node.node_id);
        }
        // Check hostname label
        if let Some(hostname) = node.labels.get("hostname")
            && hostname == target
        {
            return Some(node.node_id);
        }
    }
    None
}

/// Send a deploy command to a remote agent node and await the result.
///
/// Returns an error if the agent is not connected via WebSocket, if the WS
/// channel is closed, or if the agent reports a deploy failure within 30 s.
async fn queue_remote_deploy(
    state: &AppState,
    node_id: u64,
    spec: &WorkloadSpec,
) -> anyhow::Result<()> {
    let tx = {
        let agents = state.ws_agents.read().await;
        agents
            .get(&node_id)
            .ok_or_else(|| anyhow::anyhow!("agent {node_id} not connected via WebSocket"))?
            .clone()
    };

    let (result_tx, result_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
    state
        .pending_deploys
        .write()
        .await
        .insert(spec.name.clone(), result_tx);

    if let Err(e) = tx
        .send(orca_core::ws_types::MasterMessage::Deploy {
            spec: Box::new(spec.clone()),
        })
        .await
    {
        state.pending_deploys.write().await.remove(&spec.name);
        return Err(anyhow::anyhow!("agent {node_id} channel closed: {e}"));
    }

    info!("Sent deploy via WS to node {node_id}: {}", spec.name);

    match tokio::time::timeout(std::time::Duration::from_secs(30), result_rx).await {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(msg))) => anyhow::bail!("deploy failed on agent {node_id}: {msg}"),
        Ok(Err(_)) => anyhow::bail!("deploy result channel dropped for agent {node_id}"),
        Err(_) => {
            state.pending_deploys.write().await.remove(&spec.name);
            anyhow::bail!("deploy timed out after 30 s waiting for agent {node_id}")
        }
    }
}
pub use crate::operations::{promote, redeploy, rollback, scale, stop, stop_all};
