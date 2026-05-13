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
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn agent_message_heartbeat_serde() {
        let msg = AgentMessage::Heartbeat {
            node_id: 123,
            workloads: vec![WorkloadReport {
                service_name: "web".into(),
                status: "running".into(),
                container_id: Some("abc123".into()),
                cpu_percent: 15.5,
                memory_bytes: 128 * 1024 * 1024,
            }],
            stats: HostStats {
                cpu_percent: 42.0,
                memory_bytes: 1024,
                memory_total: 4096,
                ..Default::default()
            },
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"heartbeat\""));
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            parsed,
            AgentMessage::Heartbeat { node_id: 123, .. }
        ));
    }

    #[test]
    fn agent_message_domain_discovered_serde() {
        let msg = AgentMessage::DomainDiscovered {
            service_name: "dashboard".into(),
            domain: "yt.example.com".into(),
            host_port: 35476,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"domain_discovered\""));
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, AgentMessage::DomainDiscovered { .. }));
    }

    #[test]
    fn master_message_deploy_serde() {
        let spec = WorkloadSpec {
            name: "web".into(),
            runtime: crate::types::RuntimeKind::Container,
            image: "nginx:latest".into(),
            replicas: crate::types::Replicas::Fixed(1),
            port: Some(80),
            host_port: None,
            domain: Some("example.com".into()),
            routes: vec![],
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
            triggers: vec![],
            build: None,
            tls_cert: None,
            tls_key: None,
            internal: false,
            cmd: vec![],
            extra_ports: vec![],
            strip_prefix: None,
            pull_policy: Default::default(),
        };
        let msg = MasterMessage::Deploy {
            spec: Box::new(spec),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"deploy\""));
        let parsed: MasterMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, MasterMessage::Deploy { .. }));
    }

    #[test]
    fn master_message_log_request_serde() {
        let msg = MasterMessage::LogRequest {
            request_id: "req-1".into(),
            service_name: "web".into(),
            tail: 100,
            follow: true,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"log_request\""));
    }

    #[test]
    fn exec_message_roundtrip() {
        let start = MasterMessage::ExecStart {
            session_id: "s1".into(),
            service_name: "web".into(),
            cmd: vec!["sh".into()],
            cols: 80,
            rows: 24,
        };
        let json = serde_json::to_string(&start).unwrap();
        assert!(json.contains("\"type\":\"exec_start\""));

        let input = MasterMessage::ExecInput {
            session_id: "s1".into(),
            data: "bHM=".into(),
        };
        let json2 = serde_json::to_string(&input).unwrap();
        let back: MasterMessage = serde_json::from_str(&json2).unwrap();
        assert!(matches!(back, MasterMessage::ExecInput { .. }));

        let output = AgentMessage::ExecOutput {
            session_id: "s1".into(),
            data: "aGVsbG8=".into(),
        };
        let json3 = serde_json::to_string(&output).unwrap();
        let back3: AgentMessage = serde_json::from_str(&json3).unwrap();
        assert!(matches!(back3, AgentMessage::ExecOutput { .. }));

        let done = AgentMessage::ExecDone {
            session_id: "s1".into(),
            exit_code: 0,
        };
        let json4 = serde_json::to_string(&done).unwrap();
        assert!(json4.contains("\"type\":\"exec_done\""));
    }

    #[test]
    fn backup_status_request_roundtrip() {
        let msg = MasterMessage::BackupStatusRequest {
            request_id: "req-42".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"backup_status_request\""));
        let back: MasterMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, MasterMessage::BackupStatusRequest { .. }));
    }

    #[test]
    fn backup_status_report_roundtrip() {
        use crate::backup::{BackupFileEntry, BackupSnapshotSummary};
        let msg = AgentMessage::BackupStatusReport {
            request_id: "req-42".into(),
            data: BackupStatusReportData {
                node_id: 7,
                hostname: "agent-1".into(),
                snapshots: vec![BackupSnapshotSummary {
                    epoch_secs: 1_700_000_000,
                    total_size_bytes: 1024,
                    files: vec![BackupFileEntry {
                        name: "vol.tar.gz".into(),
                        size_bytes: 1024,
                    }],
                }],
            },
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"backup_status_report\""));
        // Verify the flattened representation: data fields appear at the
        // top level of the JSON object alongside `type` and `request_id`,
        // matching how every other AgentMessage variant looks on the wire.
        assert!(json.contains("\"node_id\":7"));
        let back: AgentMessage = serde_json::from_str(&json).unwrap();
        match back {
            AgentMessage::BackupStatusReport { data, .. } => {
                assert_eq!(data.node_id, 7);
                assert_eq!(data.hostname, "agent-1");
                assert_eq!(data.snapshots.len(), 1);
                assert_eq!(data.snapshots[0].epoch_secs, 1_700_000_000);
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn backup_request_service_hooks_default_empty() {
        let msg = MasterMessage::BackupRequest {
            config: BackupConfig {
                schedule: None,
                retention_days: 30,
                targets: vec![],
            },
            service_hooks: HashMap::new(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: MasterMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, MasterMessage::BackupRequest { .. }));
    }
}
