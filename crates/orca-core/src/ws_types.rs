//! WebSocket message types for agent↔master communication.
//!
//! Both sides use the same enum types so messages are type-safe.
//! All messages are JSON-serialized over the WebSocket text frame.

use serde::{Deserialize, Serialize};

use crate::backup::BackupConfig;
use crate::types::WorkloadSpec;

/// Messages sent from agent to master.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentMessage {
    /// Periodic status report (replaces HTTP heartbeat).
    Heartbeat {
        node_id: u64,
        workloads: Vec<WorkloadReport>,
        stats: HostStats,
    },
    /// A new domain was discovered on this node (container with orca.domain).
    DomainDiscovered {
        service_name: String,
        domain: String,
        host_port: u16,
    },
    /// A container was successfully deployed.
    DeployResult {
        service_name: String,
        success: bool,
        error: Option<String>,
    },
    /// Log chunk from a container (in response to a LogRequest).
    LogChunk {
        request_id: String,
        service_name: String,
        data: String,
        done: bool,
    },
    /// Result of a backup run triggered by master.
    BackupResult {
        node_id: u64,
        success: bool,
        message: String,
    },
}

/// Messages sent from master to agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MasterMessage {
    /// Deploy a workload on this agent.
    Deploy { spec: Box<WorkloadSpec> },
    /// Stop a workload.
    Stop { service_name: String },
    /// Request logs from a container.
    LogRequest {
        request_id: String,
        service_name: String,
        tail: u64,
        follow: bool,
    },
    /// Trigger a backup run on this agent node.
    BackupRequest {
        /// Full backup config with resolved credentials.
        config: BackupConfig,
    },
    /// Acknowledge registration / heartbeat.
    Ack { node_id: u64 },
    /// Report which services should be running on this node
    /// so the agent can reconcile on reconnect.
    #[allow(clippy::vec_box)]
    Reconcile { expected: Vec<Box<WorkloadSpec>> },
}

/// Status of a single workload, reported by agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadReport {
    pub service_name: String,
    pub status: String,
    pub container_id: Option<String>,
    /// Per-container CPU usage percentage.
    #[serde(default)]
    pub cpu_percent: f64,
    /// Per-container memory usage in bytes.
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
    /// Domains currently served by this node's proxy.
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
    fn agent_message_deploy_result_serde() {
        let msg = AgentMessage::DeployResult {
            service_name: "web".into(),
            success: true,
            error: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"deploy_result\""));
    }

    #[test]
    fn all_variants_roundtrip() {
        let agent_msgs: Vec<AgentMessage> = vec![
            AgentMessage::Heartbeat {
                node_id: 1,
                workloads: vec![],
                stats: HostStats::default(),
            },
            AgentMessage::DomainDiscovered {
                service_name: "s".into(),
                domain: "d".into(),
                host_port: 80,
            },
            AgentMessage::DeployResult {
                service_name: "s".into(),
                success: false,
                error: Some("boom".into()),
            },
            AgentMessage::LogChunk {
                request_id: "r".into(),
                service_name: "s".into(),
                data: "log line".into(),
                done: true,
            },
        ];
        for msg in &agent_msgs {
            let json = serde_json::to_string(msg).unwrap();
            let _: AgentMessage = serde_json::from_str(&json).unwrap();
        }

        let master_msgs: Vec<MasterMessage> = vec![
            MasterMessage::Stop {
                service_name: "s".into(),
            },
            MasterMessage::Ack { node_id: 1 },
            MasterMessage::Reconcile { expected: vec![] },
        ];
        for msg in &master_msgs {
            let json = serde_json::to_string(msg).unwrap();
            let _: MasterMessage = serde_json::from_str(&json).unwrap();
        }
    }
}
