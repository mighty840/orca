//! Reconciler: ensures actual running containers/wasm instances match desired service config.

use std::time::Duration;

use tracing::{error, info};

use orca_core::config::ServiceConfig;
use orca_core::runtime::Runtime;
use orca_core::types::{DeployKind, Replicas, RuntimeKind, WorkloadSpec, WorkloadStatus};

use crate::instance::create_and_start_instance;
use crate::placement::find_target_node;
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
    let mut changed = Vec::new();

    let ordered = crate::topo_sort::topo_sort(services);
    for svc_config in &ordered {
        match reconcile_service(state, svc_config).await {
            Ok(outcome) => {
                // Record successful deploy in history and clear any prior failure.
                state.deploy_history.write().await.record(svc_config);
                state.last_failures.write().await.remove(&svc_config.name);
                if outcome == ReconcileOutcome::Changed {
                    changed.push(svc_config.name.clone());
                }
                deployed.push(svc_config.name.clone());
            }
            Err(e) => {
                let msg = e.to_string();
                // Capture the failure reason so `orca status` can explain it
                // instead of the operator having to scrape logs (#status).
                state.last_failures.write().await.insert(
                    svc_config.name.clone(),
                    crate::failures::from_deploy_error(&msg),
                );
                errors.push(format!("{}: {msg}", svc_config.name));
            }
        }
    }

    deployed
        .iter()
        .for_each(|name| info!("Deployed service: {name}"));

    // Services whose workloads were replaced leave their dependents holding
    // TCP connections to peers that no longer exist (a removed container
    // never sends RST) — restart those dependents so they reconnect.
    crate::dependents::restart_dependents(state, &changed).await;

    (deployed, errors)
}

/// Whether reconciling a service actually touched its workloads.
///
/// [`reconcile`] uses this to know which services' containers were replaced —
/// their dependents (`depends_on`) are then restarted via
/// [`crate::dependents::restart_dependents`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileOutcome {
    /// Spec unchanged, instances already at desired state — nothing touched.
    Unchanged,
    /// Workloads were created, replaced, scaled, or dispatched to an agent.
    Changed,
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
) -> anyhow::Result<ReconcileOutcome> {
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
    if let Some(target_node_id) = find_target_node(state, config).await? {
        // Idempotency (#120): the LOCAL branch below has always skipped
        // when the spec is unchanged and instances are running; the remote
        // branch dispatched unconditionally — so any caller that reconciles
        // the full tree (the infra webhook did) force-recreated every
        // placement-pinned service on the agent while local services were
        // spared. Skip the dispatch when the stored spec matches and the
        // remote placeholder is already Running. Compare BEFORE the config
        // is overwritten below.
        {
            let services = state.services.read().await;
            if let Some(svc_state) = services.get(&config.name) {
                let placeholder_id = format!("remote-{target_node_id}");
                let placeholder_running = svc_state.instances.iter().any(|i| {
                    i.handle.runtime_id == placeholder_id && i.status == WorkloadStatus::Running
                });
                if placeholder_running && svc_state.config.spec_matches(config) {
                    info!(
                        "Service {} already at desired state on node {} (same spec) — skipping",
                        config.name, target_node_id
                    );
                    return Ok(ReconcileOutcome::Unchanged);
                }
            }
        }
        // Record the declared spec + a placeholder instance on the master
        // BEFORE dispatching the deploy. `orca status` then shows the
        // remote-scheduled workload (placeholder optimistically Running so the
        // TUI doesn't paint it red), and — crucially — recording the config
        // here rather than after `queue_remote_deploy` means a deploy whose
        // ack/completion doesn't land cleanly (a flaky agent) can't leave
        // `svc.config` stale. Previously the config update sat after the
        // fallible `?`, so a missed ack made the declarative reconciler see a
        // spec "mismatch" every cycle and redeploy the identical spec forever,
        // churning the container (and breaking its logs/stats).
        {
            let mut services = state.services.write().await;
            let svc_state = services
                .entry(config.name.clone())
                .or_insert_with(|| ServiceState::from_config(config.clone()));
            svc_state.config = config.clone();
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
        }
        queue_remote_deploy(state, target_node_id, &spec).await?;
        info!(
            "Queued deploy of {} to remote node {}",
            config.name, target_node_id
        );
        return Ok(ReconcileOutcome::Changed);
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
        return Ok(ReconcileOutcome::Unchanged);
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
        return Ok(ReconcileOutcome::Changed);
    }

    let outcome = if current < desired {
        let to_create = desired - current;
        info!(
            "Scaling up {} ({:?}): {} -> {} (+{})",
            config.name, config.runtime, current, desired, to_create
        );
        let specs: Vec<_> = (current..desired)
            .map(|i| {
                let mut r = spec.clone();
                if desired > 1 {
                    r.name = format!("{}-{i}", spec.name);
                }
                r
            })
            .collect();
        // Drop the write lock before async I/O so heartbeat processing is not blocked.
        drop(services);

        let mut new_instances = Vec::new();
        let mut failures = 0u32;
        for (idx, replica_spec) in specs.into_iter().enumerate() {
            match create_and_start_instance(runtime, &replica_spec).await {
                Ok(inst) => new_instances.push(inst),
                Err(e) => {
                    error!(
                        "Failed to create instance {}-{}: {e}",
                        config.name,
                        current + idx as u32
                    );
                    failures += 1;
                }
            }
        }
        if failures > 0 {
            tracing::warn!("{failures}/{to_create} replicas failed for {}", config.name);
        }
        // Re-acquire write lock and guard against concurrent deploy overshoot:
        // another reconcile may have created instances while the lock was dropped.
        let (excess_handles, added) = {
            let mut services = state.services.write().await;
            let mut excess = Vec::new();
            let mut added = 0usize;
            if let Some(svc_state) = services.get_mut(&config.name) {
                let already = svc_state
                    .instances
                    .iter()
                    .filter(|i| i.status == WorkloadStatus::Running)
                    .count() as u32;
                let cap = svc_state.desired_replicas.saturating_sub(already) as usize;
                let mut to_add = new_instances;
                if to_add.len() > cap {
                    excess = to_add
                        .split_off(cap)
                        .into_iter()
                        .map(|i| i.handle)
                        .collect();
                }
                added = to_add.len();
                svc_state.instances.extend(to_add);
            }
            (excess, added)
        };
        for handle in excess_handles {
            let _ = runtime.stop(&handle, Duration::from_secs(10)).await;
            let _ = runtime.remove(&handle).await;
        }
        // A scale-up only counts as Changed if at least one instance was
        // actually added. If every create failed (or all were trimmed as
        // concurrent-overshoot), nothing was touched — reporting Changed here
        // would restart healthy dependents for a no-op deploy.
        if added > 0 {
            ReconcileOutcome::Changed
        } else {
            ReconcileOutcome::Unchanged
        }
    } else if current > desired {
        let to_remove = current - desired;
        info!(
            "Scaling down {} ({:?}): {} -> {} (-{})",
            config.name, config.runtime, current, desired, to_remove
        );
        // Sort so failed/stopped instances are removed first, canary last.
        svc_state
            .instances
            .sort_unstable_by_key(|i| match i.status {
                WorkloadStatus::Failed | WorkloadStatus::Stopped => 2u8,
                _ if !i.is_canary => 1,
                _ => 0,
            });
        let mut handles = Vec::new();
        for _ in 0..to_remove {
            if let Some(inst) = svc_state.instances.pop() {
                handles.push(inst.handle);
            }
        }
        // Drop the write lock before async I/O.
        drop(services);
        for handle in handles {
            let _ = runtime.stop(&handle, Duration::from_secs(10)).await;
            let _ = runtime.remove(&handle).await;
        }
        ReconcileOutcome::Changed
    } else {
        drop(services);
        ReconcileOutcome::Unchanged
    };

    // Update routing based on runtime type
    match config.runtime {
        RuntimeKind::Container => update_container_routes(state, config).await,
        RuntimeKind::Wasm => update_wasm_triggers(state, config).await,
    }

    // TLS cert provisioning — one cert per domain (apex + www, dual-TLD, etc.).
    // BYO cert/key, when provided, applies to every listed domain.
    if let Some(resolver) = &state.cert_resolver {
        for domain in config.all_domains() {
            if resolver.has_cert(&domain) {
                continue;
            }
            if let (Some(cert_path), Some(key_path)) = (&config.tls_cert, &config.tls_key) {
                // BYO cert: load from file
                match load_byo_cert(cert_path, key_path) {
                    Ok(key) => {
                        resolver.add_cert(&domain, std::sync::Arc::new(key));
                        tracing::info!(domain, "BYO TLS certificate loaded");
                    }
                    Err(e) => tracing::error!(domain, "Failed to load BYO cert: {e}"),
                }
            } else if let Some(acme) = &state.acme_manager {
                // ACME auto-provisioning
                if let Err(e) = acme.ensure_cert_for_resolver(&domain, resolver).await {
                    tracing::error!(domain, "Hot cert provisioning failed: {e}");
                }
            }
        }
    }

    Ok(outcome)
}

/// Send a deploy command to a remote agent node and await the result.
///
/// The wait is split into two phases so a slow image pull is never reported as
/// an unreachable agent (#88/#94):
///
/// 1. **Receipt ACK** (`deploy.ack_timeout_secs`, default 10s): the agent
///    confirms it received the command and started work. A miss here means the
///    WS session is dead / the agent is unreachable — fail fast with a clear
///    message.
/// 2. **Completion** (`deploy.completion_timeout_secs`, default 600s): the
///    agent reports the deploy finished. Long enough to cover multi-GB
///    first-time pulls. On a real failure the agent's pull error surfaces
///    verbatim; on timeout the message says the pull may still be running.
///
/// Returns an error if the agent is not connected via WebSocket, the WS channel
/// is closed, either phase times out, or the agent reports a deploy failure.
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

    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel::<()>();
    let (result_tx, result_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
    state
        .pending_deploy_acks
        .write()
        .await
        .insert(spec.name.clone(), ack_tx);
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
        clear_pending_deploy(state, &spec.name).await;
        return Err(anyhow::anyhow!("agent {node_id} channel closed: {e}"));
    }

    info!("Sent deploy via WS to node {node_id}: {}", spec.name);

    let ack_timeout = Duration::from_secs(state.cluster_config.deploy.ack_timeout_secs);
    let completion_timeout =
        Duration::from_secs(state.cluster_config.deploy.completion_timeout_secs);

    // Phase 1: receipt ACK — distinguishes a dead/unreachable agent from a
    // slow pull. This is the failure mode that used to masquerade as the
    // misleading "is `orca server` running?" timeout.
    match tokio::time::timeout(ack_timeout, ack_rx).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => {
            clear_pending_deploy(state, &spec.name).await;
            anyhow::bail!("deploy receipt channel dropped for agent {node_id}");
        }
        Err(_) => {
            clear_pending_deploy(state, &spec.name).await;
            // A missed receipt-ACK is proof the control session is dead
            // (#131): the agent ACKs instantly when the channel works. Tear
            // the session down so the node stops looking reachable and
            // subsequent deploys fail fast with the real story — instead of
            // every deploy re-timing-out against the same zombie tx for
            // days. The agent's own idle deadline triggers its reconnect;
            // a truly-gone node is stale-pruned 60s later.
            crate::ws_handler::kill_agent_session(state, node_id, "deploy ACK timeout").await;
            anyhow::bail!(
                "agent {node_id} did not acknowledge deploy of {} within {}s — control                  session closed; node is unreachable until it rejoins",
                spec.name,
                ack_timeout.as_secs()
            );
        }
    }

    // Phase 2: completion — long enough for multi-GB first-time pulls. The
    // agent's real pull error surfaces here instead of a bare timeout.
    match tokio::time::timeout(completion_timeout, result_rx).await {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(msg))) => anyhow::bail!("deploy failed on agent {node_id}: {msg}"),
        Ok(Err(_)) => anyhow::bail!("deploy result channel dropped for agent {node_id}"),
        Err(_) => {
            state.pending_deploys.write().await.remove(&spec.name);
            anyhow::bail!(
                "deploy of {} did not complete within {}s on agent {node_id} (image pull may still be running — raise deploy.completion_timeout_secs if the image is large)",
                spec.name,
                completion_timeout.as_secs()
            )
        }
    }
}

/// Remove both pending-deploy waiter entries for a service. Used on send
/// failure and on receipt-ACK timeout so neither map leaks an orphan entry.
async fn clear_pending_deploy(state: &AppState, name: &str) {
    state.pending_deploy_acks.write().await.remove(name);
    state.pending_deploys.write().await.remove(name);
}
pub use crate::operations::{promote, redeploy, rollback, scale, start, stop, stop_all};
