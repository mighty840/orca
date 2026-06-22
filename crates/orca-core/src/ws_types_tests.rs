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
            exit_code: None,
            restart_count: 0,
            last_logs: None,
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
fn agent_message_deploy_received_serde() {
    let msg = AgentMessage::DeployReceived {
        service_name: "web".into(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"type\":\"deploy_received\""));
    let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
    assert!(matches!(parsed, AgentMessage::DeployReceived { .. }));
}

#[test]
fn adoption_report_roundtrip() {
    let msg = AgentMessage::AdoptionReport {
        request_id: "req-9".into(),
        data: AdoptionReportData {
            node_id: 5,
            hostname: "agent-1".into(),
            containers: vec![ManagedContainer {
                service_name: "web".into(),
                image: "nginx:latest".into(),
                status: "running".into(),
                container_id: "abc123".into(),
                port: Some(80),
                domains: vec!["web.example.com".into()],
                network: Some("orca-app".into()),
                routes: vec!["/api".into()],
                strip_prefix: None,
            }],
        },
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"type\":\"adoption_report\""));
    // Flattened payload: node_id appears at the top level alongside `type`.
    assert!(json.contains("\"node_id\":5"));
    let back: AgentMessage = serde_json::from_str(&json).unwrap();
    match back {
        AgentMessage::AdoptionReport { request_id, data } => {
            assert_eq!(request_id, "req-9");
            assert_eq!(data.node_id, 5);
            assert_eq!(data.containers.len(), 1);
            assert_eq!(data.containers[0].service_name, "web");
            assert_eq!(data.containers[0].port, Some(80));
        }
        _ => panic!("unexpected variant"),
    }
}

#[test]
fn adoption_scan_request_roundtrip() {
    let msg = MasterMessage::AdoptionScanRequest {
        request_id: "req-9".into(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"type\":\"adoption_scan_request\""));
    let back: MasterMessage = serde_json::from_str(&json).unwrap();
    assert!(matches!(back, MasterMessage::AdoptionScanRequest { .. }));
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
        domains: vec![],
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
