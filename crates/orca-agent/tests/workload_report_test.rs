//! Heartbeat reports must reflect the observed container state, not the
//! status cached at deploy time.
//!
//! Regression tests for the incident where `orca status` said `running` for
//! services whose containers Docker showed as `Exited (1)`: the agent's
//! workload map is written once at deploy time and was never re-checked
//! against the runtime, so heartbeats reported stale state forever.

use std::time::Duration;

use orca_agent::grpc::AgentClient;
use orca_core::runtime::Runtime;
use orca_core::testing::MockRuntime;
use orca_core::types::{WorkloadSpec, WorkloadStatus};

fn spec(name: &str) -> WorkloadSpec {
    serde_json::from_value(serde_json::json!({
        "name": name,
        "runtime": "container",
        "image": "nginx:alpine",
        "replicas": 1,
        "routes": [],
        "env": {},
        "aliases": [],
        "mounts": [],
        "triggers": [],
        "internal": false,
    }))
    .expect("valid workload spec")
}

fn agent() -> AgentClient {
    AgentClient::new("http://127.0.0.1:1".into(), 1)
}

/// A container that exits after deploy must be reported with its observed
/// state ("stopped"), not the "running" recorded when it was deployed.
#[tokio::test]
async fn report_reflects_observed_state_not_deploy_time_cache() {
    let runtime = MockRuntime::new();
    let handle = runtime.create(&spec("web")).await.unwrap();
    runtime.start(&handle).await.unwrap();
    // Container dies after deploy — the runtime now observes Stopped.
    runtime.stop(&handle, Duration::from_secs(1)).await.unwrap();

    let agent = agent();
    // Deploy-time cache still says Running.
    agent
        .update_workload_status(&handle.runtime_id, "web", WorkloadStatus::Running)
        .await;

    let reports = agent.collect_workload_reports(&runtime).await;
    assert_eq!(reports.len(), 1);
    assert_eq!(
        reports[0].status, "stopped",
        "heartbeat must report the observed container state"
    );
}

/// A container that no longer exists at all must be reported "failed" —
/// not silently kept at whatever status the map last recorded.
#[tokio::test]
async fn report_marks_vanished_container_failed() {
    let runtime = MockRuntime::new();
    let agent = agent();
    agent
        .update_workload_status("vanished-123", "web", WorkloadStatus::Running)
        .await;

    let reports = agent.collect_workload_reports(&runtime).await;
    assert_eq!(reports.len(), 1);
    assert_eq!(
        reports[0].status, "failed",
        "a vanished container must surface as failed"
    );
}

/// Sanity: a genuinely running container still reports "running".
#[tokio::test]
async fn report_keeps_running_for_running_container() {
    let runtime = MockRuntime::new();
    let handle = runtime.create(&spec("web")).await.unwrap();
    runtime.start(&handle).await.unwrap();

    let agent = agent();
    agent
        .update_workload_status(&handle.runtime_id, "web", WorkloadStatus::Running)
        .await;

    let reports = agent.collect_workload_reports(&runtime).await;
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].status, "running");
}
