//! WebSocket message types for agent↔master communication.
//!
//! Both sides use the same enum types so messages are type-safe.
//! All messages are JSON-serialized over the WebSocket text frame.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::backup::{BackupConfig, BackupSnapshotSummary};
use crate::types::WorkloadSpec;

/// Messages sent from agent to master.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentMessage {
    Heartbeat {
        node_id: u64,
        workloads: Vec<WorkloadReport>,
        stats: HostStats,
    },
    DomainDiscovered {
        service_name: String,
        domain: String,
        host_port: u16,
    },
    /// Agent acknowledges receipt of a `Deploy` command and is starting work
    /// (image pull + container create). Sent *before* the potentially
    /// long-running deploy begins so the master can distinguish a slow pull
    /// from an unreachable agent and apply separate ACK/completion timeouts.
    DeployReceived { service_name: String },
    DeployResult {
        service_name: String,
        success: bool,
        error: Option<String>,
    },
    LogChunk {
        request_id: String,
        service_name: String,
        data: String,
        done: bool,
    },
    BackupResult {
        node_id: u64,
        success: bool,
        message: String,
    },
    /// Reply to `MasterMessage::BackupStatusRequest`. Carries the agent's
    /// local snapshot index for the cluster dashboard.
    BackupStatusReport {
        request_id: String,
        #[serde(flatten)]
        data: BackupStatusReportData,
    },
    /// Reply to `MasterMessage::NetworkStatusRequest`. Carries the agent's
    /// `orca-*` Docker network listing for the cluster networks view.
    NetworkStatusReport {
        request_id: String,
        #[serde(flatten)]
        data: NetworkStatusReportData,
    },
    /// Reply to `MasterMessage::AdoptionScanRequest`. Lists every
    /// `orca.managed=true` container on the agent so the master can adopt
    /// orphans missing from its registry (#95).
    AdoptionReport {
        request_id: String,
        #[serde(flatten)]
        data: AdoptionReportData,
    },
    /// PTY output chunk from a container exec session (base64-encoded bytes).
    ExecOutput { session_id: String, data: String },
    /// Exec session has ended.
    ExecDone { session_id: String, exit_code: i64 },
}

/// Messages sent from master to agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MasterMessage {
    Deploy {
        spec: Box<WorkloadSpec>,
    },
    Stop {
        service_name: String,
    },
    LogRequest {
        request_id: String,
        service_name: String,
        tail: u64,
        follow: bool,
    },
    BackupRequest {
        config: BackupConfig,
        /// service_name → pre_hook shell command, populated from ServiceConfig.backup.
        #[serde(default)]
        service_hooks: HashMap<String, String>,
    },
    /// Ask an agent to enumerate its local backup snapshots and respond with
    /// `AgentMessage::BackupStatusReport`. Request/response correlation via
    /// the `request_id` field, same pattern as `LogRequest`.
    BackupStatusRequest {
        request_id: String,
    },
    /// Ask an agent to enumerate its `orca-*` Docker networks and respond
    /// with `AgentMessage::NetworkStatusReport`. Mirrors the backup-status
    /// fan-out pattern.
    NetworkStatusRequest {
        request_id: String,
    },
    /// Ask an agent to enumerate all its `orca.managed=true` containers and
    /// respond with `AgentMessage::AdoptionReport` so the master can adopt
    /// orphans into its registry (#95). Mirrors the network-status fan-out.
    AdoptionScanRequest {
        request_id: String,
    },
    Ack {
        node_id: u64,
    },
    #[allow(clippy::vec_box)]
    Reconcile {
        expected: Vec<Box<WorkloadSpec>>,
    },
    /// Start an interactive PTY exec session on a container.
    ExecStart {
        session_id: String,
        service_name: String,
        cmd: Vec<String>,
        cols: u16,
        rows: u16,
    },
    /// Stdin bytes for an active exec session (base64-encoded).
    ExecInput {
        session_id: String,
        data: String,
    },
    /// Terminal resize event for an active exec session.
    ExecResize {
        session_id: String,
        cols: u16,
        rows: u16,
    },
    /// Signal the agent to terminate an exec session cleanly.
    ExecClose {
        session_id: String,
    },
    /// Request a fresh status report from the agent. Agent responds with Heartbeat.
    StatusPing,
    /// Signal the agent to run `docker system prune -f`.
    PruneSystem,
}

/// Payload of `AgentMessage::BackupStatusReport` — split into its own type so
/// the master's listener channel can carry the data without the `request_id`
/// (which is already used as the listener map key).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupStatusReportData {
    pub node_id: u64,
    pub hostname: String,
    pub snapshots: Vec<BackupSnapshotSummary>,
}

/// Payload of `AgentMessage::NetworkStatusReport`. Split out for the same
/// reason as `BackupStatusReportData` — the listener-channel value doesn't
/// need to re-carry the `request_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkStatusReportData {
    pub node_id: u64,
    pub hostname: String,
    pub networks: Vec<crate::api_types::DockerNetwork>,
}

/// Payload of `AgentMessage::AdoptionReport`. Lists the agent's
/// `orca.managed=true` containers so the master's adoption reconciler can
/// register any it doesn't already know about (#95).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdoptionReportData {
    pub node_id: u64,
    pub hostname: String,
    pub containers: Vec<ManagedContainer>,
}

/// A single `orca.managed=true` container as seen by the agent, with the
/// metadata needed to reconstruct a `ServiceConfig` for adoption — derived
/// from the `orca.*` labels plus the container's image and state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedContainer {
    /// Value of the `orca.service` label (the service name).
    pub service_name: String,
    /// Container image (from the Docker container summary).
    pub image: String,
    /// Docker container state, e.g. "running", "exited".
    pub status: String,
    /// Docker container id.
    pub container_id: String,
    /// `orca.port` label — the in-container port the service listens on.
    #[serde(default)]
    pub port: Option<u16>,
    /// Public hostname(s) — from the `orca.domains` label (or single
    /// `orca.domain`). Empty when the container has no domain.
    #[serde(default)]
    pub domains: Vec<String>,
    /// `orca.network` label — the bridge network the container joined.
    #[serde(default)]
    pub network: Option<String>,
    /// `orca.routes` label — path-prefix routes, if any.
    #[serde(default)]
    pub routes: Vec<String>,
    /// `orca.strip_prefix` label.
    #[serde(default)]
    pub strip_prefix: Option<String>,
}

/// Status of a single workload, reported by agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadReport {
    pub service_name: String,
    pub status: String,
    pub container_id: Option<String>,
    #[serde(default)]
    pub cpu_percent: f64,
    #[serde(default)]
    pub memory_bytes: u64,
    /// Exit code of the last run, for a non-running container (crash detail).
    #[serde(default)]
    pub exit_code: Option<i64>,
    /// Restart count, for crash-loop detection.
    #[serde(default)]
    pub restart_count: u32,
    /// Tail of the container logs, captured when the container is not running
    /// so the master can explain *why* it failed without a separate fetch.
    #[serde(default)]
    pub last_logs: Option<String>,
}

/// Host-level stats reported by agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostStats {
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub memory_total: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub net_rx: u64,
    pub net_tx: u64,
    #[serde(default)]
    pub domains: Vec<String>,
}

#[cfg(test)]
#[path = "ws_types_tests.rs"]
mod tests;
