use std::sync::Arc;
use std::time::Duration;

use orca_core::runtime::ContainerHealth;
use orca_core::testing::{MockOpKind, MockRuntime};
use orca_core::types::WorkloadStatus;
use tokio::sync::mpsc;

use super::*;
use crate::ws_client::service_queue::OpContext;

/// An agent running `(container id, service, health)`, and its queues.
async fn agent_with(
    workloads: &[(&str, &str, Option<ContainerHealth>)],
) -> (Arc<MockRuntime>, Arc<AgentClient>, ServiceQueues) {
    let runtime = Arc::new(MockRuntime::new());
    let agent = Arc::new(AgentClient::new("http://127.0.0.1:9".into(), 1));
    for (id, service, health) in workloads {
        agent
            .update_workload_status(id, service, WorkloadStatus::Running)
            .await;
        runtime.set_status(id, WorkloadStatus::Running).await;
        if let Some(h) = health {
            runtime.set_health(id, *h).await;
        }
    }
    let (out_tx, _) = mpsc::channel(8);
    let (domain_tx, _) = mpsc::channel(8);
    let queues = ServiceQueues::new(OpContext {
        runtime: runtime.clone(),
        agent: agent.clone(),
        domain_tx,
        out_tx,
    });
    (runtime, agent, queues)
}

#[tokio::test]
async fn a_running_container_whose_health_check_fails_is_restarted_in_place() {
    // #294: clamd died, freshclam kept the container running, unhealthy.
    let (runtime, agent, queues) = agent_with(&[
        ("c-1", "clamav", Some(ContainerHealth::Unhealthy)),
        ("w-1", "web", Some(ContainerHealth::Healthy)),
        ("b-1", "booting", Some(ContainerHealth::Starting)),
        ("p-1", "plain", None),
    ])
    .await;

    queue_unhealthy(runtime.as_ref(), &agent, &queues).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let ops = runtime.recorded_ops().await;
    let kinds: Vec<_> = ops.iter().map(|o| (o.kind(), o.workload())).collect();
    assert_eq!(
        kinds,
        [(MockOpKind::Stop, "clamav"), (MockOpKind::Start, "clamav")],
        "stopped and started again, not recreated; the others left alone"
    );
    assert_eq!(
        runtime.health(&handle("c-1", "clamav")).await.unwrap(),
        Some(ContainerHealth::Starting)
    );
}

#[tokio::test]
async fn a_restart_queued_twice_happens_once() {
    // Two passes saw it unhealthy before the first restart ran; the first
    // restart resets the check to starting, so the second one does nothing.
    let (runtime, _agent, _queues) =
        agent_with(&[("c-1", "clamav", Some(ContainerHealth::Unhealthy))]).await;

    restart_if_unhealthy(runtime.as_ref(), "clamav", "c-1").await;
    restart_if_unhealthy(runtime.as_ref(), "clamav", "c-1").await;

    assert_eq!(runtime.count(MockOpKind::Stop).await, 1);
    assert_eq!(runtime.count(MockOpKind::Start).await, 1);
}
