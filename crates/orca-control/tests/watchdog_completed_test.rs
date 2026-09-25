//! #175: a long-running service whose container exits 0 must be healed. It
//! was recorded as `Completed`, kept in the instance list, and counted as a
//! live replica, so the watchdog saw 1/1 and never acted (the Gitea outages).

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use orca_control::reconciler;
use orca_control::state::AppState;
use orca_control::watchdog::run_watchdog_cycle;
use orca_core::config::{ClusterConfig, ServiceConfig};
use orca_core::testing::{MockOpKind, MockRuntime};
use orca_core::types::WorkloadStatus;

fn gitea() -> ServiceConfig {
    serde_json::from_value(serde_json::json!({
        "name": "gitea", "image": "gitea/gitea:1.24", "port": 3000,
    }))
    .unwrap()
}

#[tokio::test]
async fn a_service_whose_container_exited_zero_is_recreated() {
    let runtime = Arc::new(MockRuntime::with_host_port(9000));
    let state = AppState::new(
        ClusterConfig::default(),
        runtime.clone(),
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    );
    reconciler::reconcile(&state, &[gitea()]).await;
    let old = state.services.read().await["gitea"].instances[0]
        .handle
        .runtime_id
        .clone();

    // s6 exits 0 after the OOM killer took Gitea.
    runtime.set_status(&old, WorkloadStatus::Completed).await;
    run_watchdog_cycle(&state).await;

    let services = state.services.read().await;
    let instances = &services["gitea"].instances;
    assert_eq!(instances.len(), 1, "{instances:?}");
    assert_ne!(
        instances[0].handle.runtime_id, old,
        "must be a new container"
    );
    assert_eq!(instances[0].status, WorkloadStatus::Running);
    assert_eq!(runtime.count(MockOpKind::Create).await, 2);
}
