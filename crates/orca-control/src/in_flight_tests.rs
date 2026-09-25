//! #173: while a redeploy has emptied a service's instance list, the watchdog
//! must not start a second container for it.

use std::collections::HashMap;
use std::sync::Arc;

use orca_core::config::{ClusterConfig, ServiceConfig};
use orca_core::testing::{MockOpKind, MockRuntime};
use tokio::sync::RwLock;

use super::*;
use crate::state::ServiceState;

fn state(runtime: Arc<MockRuntime>) -> AppState {
    AppState::new(
        ClusterConfig::default(),
        runtime,
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    )
}

async fn degraded_service(state: &AppState) {
    let config: ServiceConfig = serde_json::from_value(serde_json::json!({
        "name": "web", "image": "nginx:latest", "port": 8080,
    }))
    .unwrap();
    // No instances: mid-redeploy, or crashed.
    state
        .services
        .write()
        .await
        .insert("web".into(), ServiceState::from_config(config));
}

#[tokio::test]
async fn the_watchdog_leaves_a_service_in_flight_alone() {
    let runtime = Arc::new(MockRuntime::new());
    let state = state(runtime.clone());
    degraded_service(&state).await;

    {
        let _redeploy = InFlight::mark(&state, "web");
        crate::watchdog::run_watchdog_cycle(&state).await;
        assert_eq!(
            runtime.count(MockOpKind::Create).await,
            0,
            "no racing create"
        );
    }

    // Once the redeploy is done, a still-degraded service is healed as before.
    crate::watchdog::run_watchdog_cycle(&state).await;
    assert_eq!(runtime.count(MockOpKind::Create).await, 1);
}

#[test]
fn nested_marks_hold_until_the_outermost_is_dropped() {
    let state = state(Arc::new(MockRuntime::new()));
    let outer = InFlight::mark(&state, "web");
    {
        let _inner = InFlight::mark(&state, "web");
        assert!(is_in_flight(&state, "web"));
    }
    assert!(is_in_flight(&state, "web"), "the redeploy still holds it");
    drop(outer);
    assert!(!is_in_flight(&state, "web"));
}
