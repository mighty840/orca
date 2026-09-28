use chrono::Utc;
use orca_core::api_types::FailureInfo;

use super::ALERT_LOG_LINES;
use crate::context::{ClusterContext, NodeSummary, ServiceSummary};

fn ctx(services: Vec<ServiceSummary>) -> ClusterContext {
    ClusterContext {
        cluster_name: "c".into(),
        nodes: vec![NodeSummary {
            id: "7".into(),
            address: "10.0.0.7".into(),
            status: "healthy".into(),
            heartbeat_age_secs: Some(4),
            ..Default::default()
        }],
        services,
        recent_events: Vec::new(),
        active_alerts: Vec::new(),
    }
}

fn oom_service() -> ServiceSummary {
    ServiceSummary {
        name: "api".into(),
        runtime: "container".into(),
        replicas_desired: 1,
        status: "degraded".into(),
        image: Some("api:1.2".into()),
        node: Some("10.0.0.7".into()),
        memory_limit: Some("512Mi".into()),
        last_failure: Some(FailureInfo {
            reason: "OOMKilled".into(),
            message: "heap grew to 509Mi\nkilled".into(),
            exit_code: Some(137),
            restart_count: 4,
            observed_at: Utc::now(),
        }),
        recent_logs: vec!["DB_PASSWORD=hunter2 connecting".into()],
        ..Default::default()
    }
}

#[test]
fn the_affected_service_carries_its_failure_limit_node_and_logs() {
    let p = ctx(vec![oom_service()]).alert_prompt("api");
    for needle in [
        "## Affected service: api",
        "image: api:1.2",
        "limit 512Mi",
        "reason: OOMKilled",
        "exit code: 137",
        "restart count: 4",
        "    heap grew to 509Mi",
        "node: 7 (10.0.0.7)",
        "last heartbeat 4s ago",
    ] {
        assert!(p.contains(needle), "missing {needle:?} in:\n{p}");
    }
}

#[test]
fn log_lines_are_redacted() {
    let p = ctx(vec![oom_service()]).alert_prompt("api");
    assert!(p.contains("DB_PASSWORD=<redacted> connecting"));
    assert!(!p.contains("hunter2"));
}

#[test]
fn only_the_last_log_lines_are_kept() {
    let mut svc = oom_service();
    svc.recent_logs = (0..100).map(|i| format!("line {i}")).collect();
    let p = ctx(vec![svc]).alert_prompt("api");
    assert!(p.contains("line 99"));
    assert!(!p.contains("line 59\n"));
    assert!(p.contains(&format!("line {}", 100 - ALERT_LOG_LINES)));
}

#[test]
fn an_unknown_service_still_renders() {
    let p = ctx(vec![]).alert_prompt("ghost");
    assert!(p.contains("## Affected service: ghost"));
    assert!(p.contains("not in the cluster state"));
}

#[test]
fn the_command_reference_has_the_real_scale_syntax() {
    let p = ctx(vec![]).alert_prompt("x");
    assert!(p.contains("`orca scale <service> <N>`"));
    assert!(p.contains("`orca start <service>`"));
}
