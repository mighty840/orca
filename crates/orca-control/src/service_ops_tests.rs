//! #279: one operation at a time per service. Whole-tree passes wait for each
//! other; a pass, a dependents restart and the health checker leave a service
//! alone while another operation is on it; and the health checker never
//! restarts a container that is already gone.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use orca_core::config::{ClusterConfig, ServiceConfig};
use orca_core::runtime::WorkloadHandle;
use orca_core::testing::{MockOpKind, MockRuntime};
use orca_core::types::{HealthState, WorkloadStatus};
use tokio::sync::RwLock;

use crate::in_flight::InFlight;
use crate::state::{AppState, InstanceState, ServiceState};

fn state(runtime: Arc<MockRuntime>) -> Arc<AppState> {
    Arc::new(AppState::new(
        ClusterConfig::default(),
        runtime,
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ))
}

fn config(json: serde_json::Value) -> ServiceConfig {
    serde_json::from_value(json).unwrap()
}

/// A service with one Running instance whose container id is `runtime_id`,
/// started long enough ago to be probed.
async fn running(state: &AppState, cfg: ServiceConfig, runtime_id: &str) {
    let mut svc = ServiceState::from_config(cfg.clone());
    svc.instances.push(InstanceState {
        handle: WorkloadHandle {
            runtime_id: runtime_id.into(),
            name: format!("orca-{}", cfg.name),
            metadata: Default::default(),
        },
        status: WorkloadStatus::Running,
        host_port: None,
        container_address: None,
        health: HealthState::Healthy,
        is_canary: false,
        started_at: Instant::now()
            .checked_sub(Duration::from_secs(120))
            .unwrap_or_else(Instant::now),
    });
    state.services.write().await.insert(cfg.name.clone(), svc);
}

#[tokio::test]
async fn a_pass_leaves_a_service_with_an_operation_under_way_alone() {
    let runtime = Arc::new(MockRuntime::new());
    let state = state(runtime.clone());
    let web = config(serde_json::json!({"name": "web", "image": "nginx:1"}));
    {
        let _redeploy = InFlight::mark(&state, "web");
        let (deployed, errors) =
            crate::reconciler::reconcile(&state, std::slice::from_ref(&web)).await;
        assert!(deployed.is_empty() && errors.is_empty());
        assert_eq!(
            runtime.count(MockOpKind::Create).await,
            0,
            "no racing create"
        );
    }
    crate::reconciler::reconcile(&state, &[web]).await;
    assert_eq!(runtime.count(MockOpKind::Create).await, 1);
}

#[tokio::test]
async fn whole_tree_passes_run_one_at_a_time() {
    let runtime = Arc::new(MockRuntime::new());
    let state = state(runtime.clone());
    let web = config(serde_json::json!({"name": "web", "image": "nginx:1"}));

    // Another pass is running (the declarative loop, say).
    let held = state.reconcile_pass.lock().await;
    let s = state.clone();
    let webhook_pass = tokio::spawn(async move { crate::reconciler::reconcile(&s, &[web]).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        runtime.count(MockOpKind::Create).await,
        0,
        "the second pass waits for the first"
    );
    drop(held);
    webhook_pass.await.unwrap();
    assert_eq!(runtime.count(MockOpKind::Create).await, 1);
}

#[tokio::test]
async fn a_dependent_being_deployed_is_not_restarted_as_well() {
    let runtime = Arc::new(MockRuntime::new());
    let state = state(runtime.clone());
    running(
        &state,
        config(serde_json::json!({"name": "db", "image": "pg:1"})),
        "db-1",
    )
    .await;
    running(
        &state,
        config(serde_json::json!({"name": "app", "image": "app:1", "depends_on": ["db"]})),
        "app-1",
    )
    .await;

    {
        // The 2026-10-06 case: the webhook was deploying `app` when the
        // other pass restarted it as a dependent of `db`.
        let _deploy = InFlight::mark(&state, "app");
        crate::dependents::restart_dependents(&state, &["db".to_string()]).await;
        assert_eq!(runtime.count(MockOpKind::Create).await, 0);
    }
    crate::dependents::restart_dependents(&state, &["db".to_string()]).await;
    assert_eq!(
        runtime.count(MockOpKind::Create).await,
        1,
        "an idle dependent is still restarted"
    );
}

#[tokio::test]
async fn the_health_checker_leaves_a_service_in_flight_alone() {
    let runtime = Arc::new(MockRuntime::new());
    let state = state(runtime.clone());
    // Its container is unknown to the runtime, so a probe would fail.
    running(
        &state,
        config(serde_json::json!({"name": "web", "image": "nginx:1"})),
        "old",
    )
    .await;
    let checker = crate::health::HealthChecker::new(state.clone());
    let mut counts = HashMap::new();
    {
        let _deploy = InFlight::mark(&state, "web");
        checker.check_all(&mut counts).await;
        assert!(counts.is_empty(), "not probed while a deploy owns it");
    }
    checker.check_all(&mut counts).await;
    assert_eq!(counts.get("old"), Some(&1));
}

#[tokio::test]
async fn a_container_that_is_gone_is_not_restarted_into_a_new_one() {
    let runtime = Arc::new(MockRuntime::new());
    let state = state(runtime.clone());
    // A deploy replaced container `old`; the instance list still names it.
    running(
        &state,
        config(serde_json::json!({"name": "web", "image": "nginx:1"})),
        "old",
    )
    .await;
    let checker = crate::health::HealthChecker::new(state.clone());
    let mut counts = HashMap::from([("old".to_string(), 2)]);
    checker.check_all(&mut counts).await;
    assert_eq!(
        runtime.count(MockOpKind::Create).await,
        0,
        "no third container for a stale id"
    );
}

#[tokio::test]
async fn failure_counts_of_replaced_containers_are_dropped() {
    let runtime = Arc::new(MockRuntime::new());
    let state = state(runtime.clone());
    let checker = crate::health::HealthChecker::new(state.clone());
    let mut counts = HashMap::from([("replaced-long-ago".to_string(), 2)]);
    checker.check_all(&mut counts).await;
    assert!(counts.is_empty());
}
