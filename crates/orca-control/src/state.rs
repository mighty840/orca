//! Shared application state for the control plane.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use tokio::sync::RwLock;

use orca_core::config::{ClusterConfig, ServiceConfig};
use orca_core::runtime::{Runtime, WorkloadHandle};
use orca_core::types::{HealthState, Replicas, WorkloadStatus};

use crate::stats::ContainerStats;
use crate::webhook::WebhookStore;

pub use orca_proxy::{RouteTarget, SharedWasmTriggers, WasmTrigger};

/// Shared route table type, compatible with [`orca_proxy::run_proxy`].
pub type SharedRouteTable = Arc<RwLock<HashMap<String, Vec<RouteTarget>>>>;

/// Shared state for the control plane, accessible by the API server and reconciler.
pub struct AppState {
    /// Cluster configuration.
    pub cluster_config: ClusterConfig,
    /// Container runtime (Docker).
    pub container_runtime: Arc<dyn Runtime>,
    /// Wasm runtime (wasmtime). Trait object to avoid coupling to concrete type.
    pub wasm_runtime: Option<Arc<dyn Runtime>>,
    /// Current service state, keyed by service name.
    pub services: RwLock<HashMap<String, ServiceState>>,
    /// Routing table for container workloads, shared with the reverse proxy.
    pub route_table: SharedRouteTable,
    /// Wasm HTTP triggers, shared with the reverse proxy.
    pub wasm_triggers: SharedWasmTriggers,
    /// Registered cluster nodes (M2 in-memory, will move to Raft store).
    pub registered_nodes: RwLock<HashMap<u64, RegisteredNode>>,
    /// Webhook configurations for push-triggered deploys.
    pub webhooks: WebhookStore,
    /// API bearer tokens for authentication (empty = allow all).
    pub api_tokens: Vec<String>,
    /// Pending commands for agent nodes, keyed by node_id.
    /// Uses serde_json::Value to avoid circular dependency on orca-agent types.
    pub pending_commands: RwLock<HashMap<u64, Vec<serde_json::Value>>>,
    /// Deploy history for rollback support.
    pub deploy_history: RwLock<crate::deploy_history::DeployHistory>,
    /// ACME manager for hot cert provisioning (None if no TLS).
    pub acme_manager: Option<orca_proxy::acme::AcmeManager>,
    /// Dynamic cert resolver shared with the HTTPS listener.
    pub cert_resolver: Option<orca_proxy::SharedCertResolver>,
    /// Cached container stats, keyed by service name.
    pub container_stats: RwLock<HashMap<String, ContainerStats>>,
    /// Persistent cluster store (redb). None in tests without persistence.
    pub store: Option<Arc<crate::store::ClusterStore>>,
    /// WebSocket senders for connected agent nodes, keyed by node_id.
    pub ws_agents: RwLock<HashMap<u64, crate::ws_handler::AgentSender>>,
    /// Log stream listeners: request_id → (data, done) sender.
    pub log_listeners: RwLock<HashMap<String, tokio::sync::mpsc::Sender<(String, bool)>>>,
    /// Backup status listeners: request_id → report sender. Used by the
    /// `/api/v1/cluster/backups` handler to collect reports dispatched in
    /// parallel to every connected agent.
    pub backup_listeners: RwLock<
        HashMap<String, tokio::sync::mpsc::Sender<orca_core::ws_types::BackupStatusReportData>>,
    >,
    /// Network status listeners: request_id → report sender. Same pattern as
    /// `backup_listeners`, used by the `/api/v1/cluster/networks` handler.
    pub network_listeners: RwLock<
        HashMap<String, tokio::sync::mpsc::Sender<orca_core::ws_types::NetworkStatusReportData>>,
    >,
    /// Adoption-scan listeners: request_id → report sender. Same fan-out
    /// pattern as `network_listeners`, used by the periodic adoption
    /// reconciler to collect each agent's `orca.managed` container list (#95).
    pub adoption_listeners:
        RwLock<HashMap<String, tokio::sync::mpsc::Sender<orca_core::ws_types::AdoptionReportData>>>,
    /// Last completed `BackupResult` per agent node, recorded as the result
    /// arrives over WS. Surfaced alongside the snapshot listing so the
    /// dashboard can show a node's last-failure message without having to
    /// scrape logs. Master has its own field — its backups are subprocess-
    /// driven, not WS-dispatched.
    pub last_backup_results: RwLock<HashMap<u64, LastBackupResult>>,
    /// Last completed master backup result, recorded when `run_master_backup`
    /// finishes. Separate from `last_backup_results` because the master has
    /// no `node_id` and runs its backups via subprocess rather than WS.
    pub master_last_backup_result: RwLock<Option<LastBackupResult>>,
    /// Recent webhook invocations, keyed by `service_name`. Bounded ring
    /// buffer of the last 10 deliveries per webhook so the TUI can render a
    /// history view without scraping logs. Lost on restart by design — this
    /// is operator-visible recent activity, not durable audit.
    pub webhook_invocations: RwLock<
        HashMap<String, std::collections::VecDeque<orca_core::api_types::WebhookInvocation>>,
    >,
    /// Active exec sessions: session_id → output bytes sender (agent → CLI WS).
    pub exec_sessions: RwLock<HashMap<String, tokio::sync::mpsc::Sender<Vec<u8>>>>,
    /// Pending deploy result waiters: service_name → oneshot sender.
    /// Inserted by queue_remote_deploy, resolved by ws_handler on DeployResult.
    pub pending_deploys: RwLock<HashMap<String, tokio::sync::oneshot::Sender<Result<(), String>>>>,
    /// Pending deploy *receipt* waiters: service_name → oneshot sender.
    /// Inserted by queue_remote_deploy, resolved by ws_handler on
    /// DeployReceived. Separate from `pending_deploys` so the master can apply
    /// a short receipt-ack timeout (catch a dead agent fast) independently of
    /// the long completion timeout (cover multi-GB image pulls). See #88/#94.
    pub pending_deploy_acks: RwLock<HashMap<String, tokio::sync::oneshot::Sender<()>>>,
    /// Most recent failure per service — deploy-time errors and agent-reported
    /// container crashes (exit code + log tail). Surfaced in `orca status` so an
    /// operator sees *why* a service is degraded. Inserted on deploy error and
    /// on a non-running heartbeat report; cleared on a successful deploy.
    pub last_failures: RwLock<HashMap<String, orca_core::api_types::FailureInfo>>,
    /// AI conversational-alert engine. `None` when `[ai]` is not configured.
    /// `AiMonitor::run` (spawned in `lib::run_server_with_acme`) feeds it
    /// `open_alert` calls; the HTTP `/api/v1/alerts/...` handlers mutate it
    /// in response to operator actions.
    pub alerts: Option<crate::alerts::SharedAlertEngine>,
    /// Per-service log of restart triggers and instance failures.
    /// Read by the AI alert monitor to populate `restart_count_24h` and
    /// `error_count_1h` in the cluster context. In-memory only; restart
    /// wipes the log (the AI monitor reconstructs alerts as conditions
    /// reappear, so a brief gap after restart is acceptable).
    pub instance_events: RwLock<HashMap<String, InstanceEventLog>>,
}

/// Per-service ring of recent failure / restart timestamps. Pruning happens
/// on every append: entries older than [`InstanceEventLog::WINDOW`] are
/// dropped so the queues stay bounded to ~24h of activity regardless of
/// service churn.
#[derive(Debug, Default)]
pub struct InstanceEventLog {
    /// Timestamps of operator/watchdog-triggered restarts.
    pub restarts: VecDeque<DateTime<Utc>>,
    /// Timestamps of observed instance failures — non-zero exits and
    /// individual health-check failures.
    pub failures: VecDeque<DateTime<Utc>>,
}

impl InstanceEventLog {
    /// Longest window any consumer needs; entries older than this are
    /// pruned eagerly on append.
    pub const WINDOW: Duration = Duration::hours(24);

    pub fn record_restart(&mut self, now: DateTime<Utc>) {
        self.restarts.push_back(now);
        prune_older_than(&mut self.restarts, now - Self::WINDOW);
    }

    pub fn record_failure(&mut self, now: DateTime<Utc>) {
        self.failures.push_back(now);
        prune_older_than(&mut self.failures, now - Self::WINDOW);
    }

    /// Number of failure events recorded in the last `window` from `now`.
    pub fn failures_in(&self, now: DateTime<Utc>, window: Duration) -> u64 {
        let cutoff = now - window;
        self.failures.iter().filter(|t| **t >= cutoff).count() as u64
    }

    /// Number of restart events recorded in the last `window` from `now`.
    pub fn restarts_in(&self, now: DateTime<Utc>, window: Duration) -> u32 {
        let cutoff = now - window;
        self.restarts.iter().filter(|t| **t >= cutoff).count() as u32
    }
}

fn prune_older_than(deque: &mut VecDeque<DateTime<Utc>>, cutoff: DateTime<Utc>) {
    while let Some(front) = deque.front() {
        if *front < cutoff {
            deque.pop_front();
        } else {
            break;
        }
    }
}

pub use orca_core::api_types::LastBackupResult;

/// A node registered in the cluster.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RegisteredNode {
    /// Node ID.
    pub node_id: u64,
    /// Node address (ip:port).
    pub address: String,
    /// Node labels.
    pub labels: HashMap<String, String>,
    /// Last heartbeat time.
    pub last_heartbeat: chrono::DateTime<chrono::Utc>,
    /// Whether the node is in drain mode (no new workloads scheduled).
    #[serde(default)]
    pub drain: bool,
    /// Latest CPU / memory / disk / network sample reported in a heartbeat.
    /// Populated for both the master (via its local collector) and joined
    /// nodes (via their heartbeat body).
    #[serde(default)]
    pub cpu_percent: f64,
    #[serde(default)]
    pub memory_bytes: u64,
    #[serde(default)]
    pub memory_total: u64,
    #[serde(default)]
    pub disk_used: u64,
    #[serde(default)]
    pub disk_total: u64,
    #[serde(default)]
    pub net_rx: u64,
    #[serde(default)]
    pub net_tx: u64,
}

/// State of a deployed service.
#[derive(Debug)]
pub struct ServiceState {
    /// The service configuration.
    pub config: ServiceConfig,
    /// Desired number of replicas.
    pub desired_replicas: u32,
    /// Running instances.
    pub instances: Vec<InstanceState>,
}

/// State of a single workload instance (one replica).
#[derive(Debug)]
pub struct InstanceState {
    /// Handle to the running workload.
    pub handle: WorkloadHandle,
    /// Current status.
    pub status: WorkloadStatus,
    /// Host port mapped to the container's primary port (containers only).
    pub host_port: Option<u16>,
    /// Container address on Docker network (ip:port) for direct proxy routing.
    pub container_address: Option<String>,
    /// Health check state.
    pub health: HealthState,
    /// Whether this instance is a canary (new version during canary deploy).
    pub is_canary: bool,
    /// When this instance was created (for initial_delay_secs).
    pub started_at: std::time::Instant,
}

impl AppState {
    /// Create with shared route table and Wasm triggers (for sharing with the proxy).
    pub fn new(
        cluster_config: ClusterConfig,
        container_runtime: Arc<dyn Runtime>,
        wasm_runtime: Option<Arc<dyn Runtime>>,
        route_table: SharedRouteTable,
        wasm_triggers: SharedWasmTriggers,
    ) -> Self {
        let api_tokens = cluster_config.api_tokens.clone();
        Self {
            cluster_config,
            container_runtime,
            wasm_runtime,
            services: RwLock::new(HashMap::new()),
            route_table,
            wasm_triggers,
            registered_nodes: RwLock::new(HashMap::new()),
            pending_commands: RwLock::new(HashMap::new()),
            webhooks: crate::webhook::new_store(),
            api_tokens,
            deploy_history: RwLock::new(crate::deploy_history::DeployHistory::new()),
            acme_manager: None,
            cert_resolver: None,
            container_stats: RwLock::new(HashMap::new()),
            store: None,
            ws_agents: RwLock::new(HashMap::new()),
            log_listeners: RwLock::new(HashMap::new()),
            backup_listeners: RwLock::new(HashMap::new()),
            network_listeners: RwLock::new(HashMap::new()),
            adoption_listeners: RwLock::new(HashMap::new()),
            last_backup_results: RwLock::new(HashMap::new()),
            master_last_backup_result: RwLock::new(None),
            webhook_invocations: RwLock::new(HashMap::new()),
            exec_sessions: RwLock::new(HashMap::new()),
            pending_deploys: RwLock::new(HashMap::new()),
            pending_deploy_acks: RwLock::new(HashMap::new()),
            last_failures: RwLock::new(HashMap::new()),
            alerts: None,
            instance_events: RwLock::new(HashMap::new()),
        }
    }

    /// Append a restart event for `service` and prune old entries.
    /// Cheap (one `now()` + a few VecDeque ops); safe to call from hot paths.
    pub async fn record_instance_restart(&self, service: &str) {
        let mut log = self.instance_events.write().await;
        log.entry(service.to_string())
            .or_default()
            .record_restart(Utc::now());
    }

    /// Append a failure event for `service` (non-zero exit or health-check
    /// failure) and prune old entries.
    pub async fn record_instance_failure(&self, service: &str) {
        let mut log = self.instance_events.write().await;
        log.entry(service.to_string())
            .or_default()
            .record_failure(Utc::now());
    }

    /// Set persistent store for service state.
    pub fn with_store(mut self, store: Arc<crate::store::ClusterStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// Attach the AI alert engine. Builder-style so startup code can wire it
    /// only when `[ai]` is configured.
    pub fn with_alerts(mut self, engine: crate::alerts::SharedAlertEngine) -> Self {
        self.alerts = Some(engine);
        self
    }

    /// Set ACME manager and cert resolver for hot cert provisioning.
    pub fn with_acme(
        mut self,
        manager: orca_proxy::acme::AcmeManager,
        resolver: orca_proxy::SharedCertResolver,
    ) -> Self {
        self.acme_manager = Some(manager);
        self.cert_resolver = Some(resolver);
        self
    }
}

impl ServiceState {
    /// Create from a service config.
    pub fn from_config(config: ServiceConfig) -> Self {
        let desired_replicas = match &config.replicas {
            Replicas::Fixed(n) => *n,
            Replicas::Auto => 1,
        };
        Self {
            config,
            desired_replicas,
            instances: Vec::new(),
        }
    }

    /// Count how many instances are currently running.
    pub fn running_count(&self) -> u32 {
        self.instances
            .iter()
            .filter(|i| i.status == WorkloadStatus::Running)
            .count() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use orca_core::config::ServiceConfig;
    use orca_core::runtime::WorkloadHandle;
    use orca_core::types::{Replicas, WorkloadStatus};

    fn minimal_config(replicas: Replicas) -> ServiceConfig {
        ServiceConfig {
            name: "test-svc".to_string(),
            project: None,
            runtime: Default::default(),
            image: Some("nginx:latest".to_string()),
            module: None,
            replicas,
            port: Some(8080),
            domain: None,
            domains: vec![],
            health: None,
            readiness: None,
            liveness: None,
            env: HashMap::new(),
            resources: None,
            volume: None,
            deploy: None,
            placement: None,
            network: None,
            aliases: vec![],
            mounts: vec![],
            routes: vec![],
            host_port: None,
            triggers: Vec::new(),
            assets: None,
            build: None,
            tls_cert: None,
            tls_key: None,
            internal: false,
            depends_on: vec![],
            cmd: vec![],
            extra_ports: vec![],
            strip_prefix: None,
            pull_policy: Default::default(),
            backup: None,
        }
    }

    fn make_instance(status: WorkloadStatus) -> InstanceState {
        InstanceState {
            handle: WorkloadHandle {
                runtime_id: "test-id".to_string(),
                name: "test-instance".to_string(),
                metadata: HashMap::new(),
            },
            status,
            host_port: None,
            container_address: None,
            health: HealthState::Unknown,
            is_canary: false,
            started_at: std::time::Instant::now(),
        }
    }

    #[test]
    fn from_config_fixed_sets_desired_replicas() {
        let state = ServiceState::from_config(minimal_config(Replicas::Fixed(3)));
        assert_eq!(state.desired_replicas, 3);
    }

    #[test]
    fn from_config_auto_defaults_to_one() {
        let state = ServiceState::from_config(minimal_config(Replicas::Auto));
        assert_eq!(state.desired_replicas, 1);
    }

    #[test]
    fn running_count_with_mixed_statuses() {
        let mut state = ServiceState::from_config(minimal_config(Replicas::Fixed(4)));
        state.instances = vec![
            make_instance(WorkloadStatus::Running),
            make_instance(WorkloadStatus::Stopped),
            make_instance(WorkloadStatus::Running),
            make_instance(WorkloadStatus::Failed),
        ];
        assert_eq!(state.running_count(), 2);
    }

    #[test]
    fn instance_event_log_counts_inside_window() {
        // Inject test data directly so we can simulate timestamps across the
        // window boundary. Real callers go through `record_*` which always
        // stamps with `Utc::now()` (monotonically increasing).
        let now = Utc::now();
        let mut log = InstanceEventLog::default();
        log.failures.push_back(now - Duration::minutes(30));
        log.failures.push_back(now - Duration::minutes(45));
        log.failures.push_back(now - Duration::hours(2));
        log.restarts.push_back(now - Duration::hours(48));
        log.restarts.push_back(now - Duration::hours(20));
        log.restarts.push_back(now - Duration::minutes(10));

        assert_eq!(log.failures_in(now, Duration::hours(1)), 2);
        assert_eq!(log.failures_in(now, Duration::hours(24)), 3);
        assert_eq!(log.restarts_in(now, Duration::hours(1)), 1);
        assert_eq!(log.restarts_in(now, Duration::hours(24)), 2);
    }

    #[test]
    fn instance_event_log_prunes_on_append() {
        // A real append at `now` should drop anything earlier than 24h.
        let now = Utc::now();
        let mut log = InstanceEventLog::default();
        log.restarts.push_back(now - Duration::hours(48)); // pre-existing stale
        log.restarts.push_back(now - Duration::hours(20));
        log.record_restart(now);
        assert_eq!(
            log.restarts.len(),
            2,
            "the 48h-old entry must be pruned when a fresh `now` arrives"
        );
    }
}
