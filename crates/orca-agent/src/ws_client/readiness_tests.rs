use std::sync::Arc;
use std::time::Duration;

use orca_core::runtime::{ContainerHealth, Runtime, WorkloadHandle};
use orca_core::testing::MockRuntime;
use orca_core::types::WorkloadStatus;

use super::*;

/// A started mock container, as `deploy_spec` leaves it.
async fn started(runtime: &MockRuntime, name: &str) -> WorkloadHandle {
    let mut spec = orca_core::testing::spec(name);
    spec.port = Some(5432);
    let handle = runtime.create(&spec).await.unwrap();
    runtime.start(&handle).await.unwrap();
    handle
}

/// #300: a dependency with a health check is ready only once it passes.
#[tokio::test]
async fn waits_for_the_health_check_to_pass() {
    let runtime = Arc::new(MockRuntime::new());
    let handle = started(&runtime, "db").await;
    runtime
        .set_health(&handle.runtime_id, ContainerHealth::Starting)
        .await;
    let flip = {
        let runtime = runtime.clone();
        let id = handle.runtime_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1200)).await;
            runtime.set_health(&id, ContainerHealth::Healthy).await;
        })
    };

    let begun = std::time::Instant::now();
    let result = wait_for(runtime.as_ref(), &handle, None, Duration::from_secs(10)).await;

    assert_eq!(result, Readiness::Ready);
    assert!(
        begun.elapsed() >= Duration::from_millis(1200),
        "returned before the flip"
    );
    flip.await.unwrap();
}

/// A failing health check ends the wait at once instead of using the budget.
#[tokio::test]
async fn an_unhealthy_container_ends_the_wait() {
    let runtime = MockRuntime::new();
    let handle = started(&runtime, "db").await;
    runtime
        .set_health(&handle.runtime_id, ContainerHealth::Unhealthy)
        .await;

    let result = wait_for(&runtime, &handle, None, Duration::from_secs(10)).await;

    assert_eq!(result, Readiness::Unhealthy);
}

/// #300: without a health check, ready means the port accepts connections
/// (postgres and ClickHouse open theirs only once they can serve).
#[tokio::test]
async fn without_a_health_check_waits_for_the_port_to_accept() {
    let runtime = MockRuntime::new();
    let handle = started(&runtime, "db").await;
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let address = format!("127.0.0.1:{port}");
    let listen = {
        let address = address.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1200)).await;
            let listener = tokio::net::TcpListener::bind(&address).await.unwrap();
            let _ = listener.accept().await;
        })
    };

    let begun = std::time::Instant::now();
    let result = wait_for(&runtime, &handle, Some(&address), Duration::from_secs(10)).await;

    assert_eq!(result, Readiness::Ready);
    assert!(
        begun.elapsed() >= Duration::from_millis(1200),
        "returned before the port opened"
    );
    listen.await.unwrap();
}

#[tokio::test]
async fn gives_up_after_the_budget() {
    let runtime = MockRuntime::new();
    let handle = started(&runtime, "db").await;
    runtime
        .set_health(&handle.runtime_id, ContainerHealth::Starting)
        .await;

    let result = wait_for(&runtime, &handle, None, Duration::from_secs(1)).await;

    assert_eq!(result, Readiness::TimedOut);
}

/// An app that exited is not going to get ready: stop waiting.
#[tokio::test]
async fn a_container_that_stopped_ends_the_wait() {
    let runtime = MockRuntime::new();
    let handle = started(&runtime, "app").await;
    runtime
        .set_health(&handle.runtime_id, ContainerHealth::Starting)
        .await;
    runtime
        .set_status(&handle.runtime_id, WorkloadStatus::Stopped)
        .await;

    let result = wait_for(&runtime, &handle, None, Duration::from_secs(10)).await;

    assert_eq!(result, Readiness::NotRunning);
}

/// Nothing to wait for: the deploy is reported at once, as before.
#[tokio::test]
async fn without_a_health_check_or_address_there_is_nothing_to_wait_for() {
    let runtime = MockRuntime::new();
    let handle = started(&runtime, "worker").await;

    let result = wait_for(&runtime, &handle, None, Duration::from_secs(10)).await;

    assert_eq!(result, Readiness::NoSignal);
}
