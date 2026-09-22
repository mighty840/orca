//! Tests for the mock runtime's recording and failure-injection behavior.
//!
//! These guard the harness itself: the regression tests for deploy ordering
//! and failure handling are only meaningful if the mock records and fails the
//! way they assume.

use std::time::Duration;

use super::*;
use crate::runtime::Runtime;
use crate::testing::spec;

#[tokio::test]
async fn stop_records_the_grace_period_it_was_given() {
    let rt = MockRuntime::new();
    let handle = rt.create(&spec("web")).await.unwrap();

    rt.stop(&handle, Duration::from_secs(30)).await.unwrap();

    let ops = rt.ops_for("web").await;
    let stop = ops.iter().find(|o| o.kind() == MockOpKind::Stop).unwrap();
    assert_eq!(stop.stop_timeout(), Some(Duration::from_secs(30)));
}

#[tokio::test]
async fn ops_for_correlates_create_and_stop_of_one_workload() {
    let rt = MockRuntime::new();
    let web = rt.create(&spec("web")).await.unwrap();
    let _db = rt.create(&spec("db")).await.unwrap();
    rt.stop(&web, Duration::from_secs(10)).await.unwrap();
    rt.remove(&web).await.unwrap();

    let kinds: Vec<_> = rt.ops_for("web").await.iter().map(|o| o.kind()).collect();
    assert_eq!(
        kinds,
        vec![MockOpKind::Create, MockOpKind::Stop, MockOpKind::Remove]
    );
    // The db record must not leak into web's history.
    assert_eq!(rt.ops_for("db").await.len(), 1);
}

#[tokio::test]
async fn fail_next_fires_once_then_clears() {
    let rt = MockRuntime::new();
    rt.fail_next(MockOpKind::Create).await;

    assert!(rt.create(&spec("web")).await.is_err());
    assert!(rt.create(&spec("web")).await.is_ok());
}

#[tokio::test]
async fn fail_next_n_fires_exactly_n_times() {
    let rt = MockRuntime::new();
    rt.fail_next_n(MockOpKind::Create, 2).await;

    assert!(rt.create(&spec("web")).await.is_err());
    assert!(rt.create(&spec("web")).await.is_err());
    assert!(rt.create(&spec("web")).await.is_ok());
}

#[tokio::test]
async fn fail_always_persists_until_cleared() {
    let rt = MockRuntime::new();
    rt.fail_always(MockOpKind::Create).await;

    assert!(rt.create(&spec("web")).await.is_err());
    assert!(rt.create(&spec("web")).await.is_err());

    rt.clear_failures().await;
    assert!(rt.create(&spec("web")).await.is_ok());
}

#[tokio::test]
async fn a_failed_operation_is_not_recorded() {
    // The op log means "what actually happened", so a deploy that could not
    // create its replacement must leave no trace of one.
    let rt = MockRuntime::new();
    rt.fail_next(MockOpKind::Create).await;

    assert!(rt.create(&spec("web")).await.is_err());
    assert_eq!(rt.recorded_ops().await, vec![]);
    assert_eq!(rt.count(MockOpKind::Create).await, 0);
}

#[tokio::test]
async fn failure_injection_is_per_operation_kind() {
    let rt = MockRuntime::new();
    let handle = rt.create(&spec("web")).await.unwrap();
    rt.fail_always(MockOpKind::Remove).await;

    // Stop still succeeds; only remove is broken.
    assert!(rt.stop(&handle, Duration::from_secs(5)).await.is_ok());
    assert!(rt.remove(&handle).await.is_err());
}

#[tokio::test]
async fn set_status_stages_a_state_the_mock_would_not_reach() {
    // A container that exits 0 reports Completed while orca still believes it
    // is running. The mock cannot produce that on its own.
    let rt = MockRuntime::new();
    let handle = rt.create(&spec("gitea")).await.unwrap();
    rt.start(&handle).await.unwrap();
    assert_eq!(rt.status(&handle).await.unwrap(), WorkloadStatus::Running);

    rt.set_status(&handle.runtime_id, WorkloadStatus::Completed)
        .await;
    assert_eq!(rt.status(&handle).await.unwrap(), WorkloadStatus::Completed);
}

#[tokio::test]
async fn clear_ops_drops_records_but_keeps_status_and_failures() {
    let rt = MockRuntime::new();
    let handle = rt.create(&spec("web")).await.unwrap();
    rt.start(&handle).await.unwrap();
    rt.fail_always(MockOpKind::Remove).await;

    rt.clear_ops().await;

    assert_eq!(rt.recorded_ops().await, vec![]);
    assert_eq!(rt.status(&handle).await.unwrap(), WorkloadStatus::Running);
    assert!(rt.remove(&handle).await.is_err());
}

#[tokio::test]
async fn injected_error_names_the_operation_and_target() {
    let rt = MockRuntime::new();
    rt.fail_next(MockOpKind::Create).await;

    let err = rt.create(&spec("web")).await.unwrap_err().to_string();
    assert!(err.contains("create"), "{err}");
    assert!(err.contains("web"), "{err}");
}
