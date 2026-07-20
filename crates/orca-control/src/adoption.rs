//! Orphan-adoption reconciler (#95).
//!
//! The master's view of a service can diverge from what an agent is actually
//! running — e.g. a deploy whose completion ACK was missed, or a master
//! restart mid-deploy — leaving a labeled container "running but unregistered"
//! forever. This periodic pass fans an `AdoptionScanRequest` out to every
//! connected agent, and for any `orca.managed=true` container whose
//! `orca.service` is absent from the registry, registers it: an in-memory
//! `ServiceState` with a `remote-<node_id>` placeholder (so `orca status` /
//! `orca logs` / `orca redeploy` see it) plus a persisted `ServiceConfig` (so
//! the adoption survives a master restart).

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use orca_core::config::ServiceConfig;
use orca_core::types::{HealthState, PlacementConstraint, Replicas, RuntimeKind, WorkloadStatus};
use orca_core::ws_types::{AdoptionReportData, ManagedContainer, MasterMessage};

use crate::state::{AppState, InstanceState, ServiceState};

/// Per-agent collection timeout for one scan round.
const ADOPTION_REPORT_TIMEOUT: Duration = Duration::from_secs(5);

/// Spawn the adoption reconciler as a background task. No-op (returns `false`)
/// when adoption is disabled in config.
pub fn spawn_adoption_reconciler(state: Arc<AppState>) -> bool {
    if !state.cluster_config.deploy.adopt_orphans {
        return false;
    }
    let interval = Duration::from_secs(state.cluster_config.deploy.adopt_interval_secs.max(1));
    tokio::spawn(async move {
        info!(
            "Adoption reconciler started (interval: {}s)",
            interval.as_secs()
        );
        loop {
            tokio::time::sleep(interval).await;
            run_adoption_cycle(&state).await;
        }
    });
    true
}

/// Run a single adoption cycle: scan every agent, adopt unknown running
/// containers. Exposed for testing.
pub async fn run_adoption_cycle(state: &AppState) {
    let reports = collect_reports(state).await;
    for report in reports {
        for container in &report.containers {
            // Only adopt *running* orphans missing from the registry. Stopped
            // or dead containers aren't the "running but unregistered" gap this
            // closes, and adopting them risks resurrecting intentionally-removed
            // services.
            if container.status != "running" || container.service_name.is_empty() {
                continue;
            }
            if state
                .services
                .read()
                .await
                .contains_key(&container.service_name)
            {
                continue;
            }
            adopt_container(state, report.node_id, container).await;
        }
    }
}

/// Fan an `AdoptionScanRequest` out to every connected agent and collect the
/// reports, bounded by [`ADOPTION_REPORT_TIMEOUT`]. Mirrors the cluster
/// networks/backups fan-out pattern.
async fn collect_reports(state: &AppState) -> Vec<AdoptionReportData> {
    let agent_ids: Vec<u64> = state.ws_agents.read().await.keys().copied().collect();
    if agent_ids.is_empty() {
        return Vec::new();
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel::<AdoptionReportData>(agent_ids.len() + 1);
    let mut request_ids: Vec<String> = Vec::with_capacity(agent_ids.len());
    {
        let mut listeners = state.adoption_listeners.write().await;
        for _ in 0..agent_ids.len() {
            let req = uuid::Uuid::new_v4().to_string();
            listeners.insert(req.clone(), tx.clone());
            request_ids.push(req);
        }
    }

    let mut dispatched = 0usize;
    {
        let agents = state.ws_agents.read().await;
        for (node_id, req_id) in agent_ids.iter().zip(request_ids.iter()) {
            if let Some(agent_tx) = agents.get(node_id)
                && agent_tx
                    .send(MasterMessage::AdoptionScanRequest {
                        request_id: req_id.clone(),
                    })
                    .await
                    .is_ok()
            {
                dispatched += 1;
            }
        }
    }

    let mut reports = Vec::new();
    let deadline = tokio::time::sleep(ADOPTION_REPORT_TIMEOUT);
    tokio::pin!(deadline);
    while reports.len() < dispatched {
        tokio::select! {
            biased;
            msg = rx.recv() => match msg {
                Some(data) => reports.push(data),
                None => break,
            },
            _ = &mut deadline => break,
        }
    }

    {
        let mut listeners = state.adoption_listeners.write().await;
        for req in &request_ids {
            listeners.remove(req);
        }
    }
    reports
}

/// Register a single orphan container into the registry: in-memory state +
/// persisted config. Caller has already confirmed the service is unknown, but
/// we re-check under the write lock so two cycles can't double-insert.
pub(crate) async fn adopt_container(state: &AppState, node_id: u64, c: &ManagedContainer) {
    let config = config_from_managed(node_id, c);
    {
        let mut services = state.services.write().await;
        if services.contains_key(&c.service_name) {
            return;
        }
        let mut svc_state = ServiceState::from_config(config.clone());
        svc_state.instances.push(InstanceState {
            handle: orca_core::runtime::WorkloadHandle {
                runtime_id: format!("remote-{node_id}"),
                name: format!("remote-{node_id}"),
                metadata: Default::default(),
            },
            status: WorkloadStatus::Running,
            host_port: None,
            container_address: None,
            health: HealthState::NoCheck,
            is_canary: false,
            started_at: std::time::Instant::now(),
        });
        services.insert(c.service_name.clone(), svc_state);
    }

    // Persist so the adoption survives a master restart — restore_or_reconcile
    // re-registers it as a remote placeholder on next boot.
    if let Some(store) = &state.store
        && let Err(e) = store.set_service(&c.service_name, &config)
    {
        warn!(service = %c.service_name, "adopted service persist failed: {e}");
    }

    info!(
        service = %c.service_name,
        node_id,
        image = %c.image,
        "adopted orphan container into registry"
    );
}

/// Reconstruct a `ServiceConfig` from a labeled container's metadata. The
/// `placement.node` is pinned to the reporting agent so restore + reconcile
/// treat it as a remote service.
pub(crate) fn config_from_managed(node_id: u64, c: &ManagedContainer) -> ServiceConfig {
    ServiceConfig {
        restart_policy: None,
        name: c.service_name.clone(),
        project: None,
        runtime: RuntimeKind::Container,
        image: Some(c.image.clone()),
        module: None,
        replicas: Replicas::Fixed(1),
        port: c.port,
        host_port: None,
        domain: None,
        domains: c.domains.clone(),
        routes: c.routes.clone(),
        health: None,
        readiness: None,
        liveness: None,
        env: std::collections::HashMap::new(),
        resources: None,
        volume: None,
        deploy: None,
        placement: Some(PlacementConstraint {
            labels: None,
            node: Some(node_id.to_string()),
            requires_gpu: None,
        }),
        network: c.network.clone(),
        aliases: vec![],
        mounts: vec![],
        triggers: vec![],
        assets: None,
        build: None,
        tls_cert: None,
        tls_key: None,
        internal: false,
        depends_on: vec![],
        cmd: vec![],
        extra_ports: vec![],
        strip_prefix: c.strip_prefix.clone(),
        pull_policy: Default::default(),
        backup: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    use orca_core::config::ClusterConfig;
    use orca_core::testing::MockRuntime;
    use tokio::sync::RwLock;

    fn state() -> Arc<AppState> {
        Arc::new(AppState::new(
            ClusterConfig::default(),
            Arc::new(MockRuntime::new()),
            None,
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(Vec::new())),
        ))
    }

    fn managed(name: &str) -> ManagedContainer {
        ManagedContainer {
            service_name: name.into(),
            image: "nginx:latest".into(),
            status: "running".into(),
            container_id: "abc123".into(),
            port: Some(8080),
            domains: vec!["svc.example.com".into()],
            network: Some("app".into()),
            routes: vec![],
            strip_prefix: None,
        }
    }

    #[test]
    fn config_from_managed_pins_placement_and_metadata() {
        let cfg = config_from_managed(7, &managed("web"));
        assert_eq!(cfg.name, "web");
        assert_eq!(cfg.image.as_deref(), Some("nginx:latest"));
        assert_eq!(cfg.port, Some(8080));
        assert_eq!(cfg.all_domains(), vec!["svc.example.com".to_string()]);
        assert_eq!(cfg.network.as_deref(), Some("app"));
        assert_eq!(
            cfg.placement.and_then(|p| p.node).as_deref(),
            Some("7"),
            "placement must pin to the reporting node"
        );
    }

    #[tokio::test]
    async fn adopt_inserts_service_with_remote_placeholder() {
        let state = state();
        adopt_container(&state, 7, &managed("web")).await;

        let services = state.services.read().await;
        let svc = services.get("web").expect("service adopted");
        assert_eq!(svc.instances.len(), 1);
        assert_eq!(svc.instances[0].handle.runtime_id, "remote-7");
        assert_eq!(svc.instances[0].status, WorkloadStatus::Running);
    }

    #[tokio::test]
    async fn adopt_does_not_overwrite_known_service() {
        let state = state();
        // Pre-seed a service of the same name with a sentinel image.
        {
            let mut existing = config_from_managed(7, &managed("web"));
            existing.image = Some("original:tag".into());
            let mut services = state.services.write().await;
            services.insert("web".into(), ServiceState::from_config(existing));
        }
        adopt_container(&state, 9, &managed("web")).await;

        let services = state.services.read().await;
        let svc = services.get("web").unwrap();
        assert_eq!(
            svc.config.image.as_deref(),
            Some("original:tag"),
            "adoption must never clobber a service the master already knows"
        );
        assert!(
            svc.instances.is_empty(),
            "no placeholder should be added to a pre-existing service"
        );
    }
}
