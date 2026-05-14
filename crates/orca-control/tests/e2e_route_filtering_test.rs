//! E2E test: route table drops unhealthy replicas while siblings keep serving.
//!
//! Deploy a 3-replica service with a passing HTTP liveness probe. Once all
//! three are healthy and the route table is populated, externally `docker
//! stop` one container, run a single health-check cycle, and assert:
//! 1. the killed instance's `health` is `Unhealthy`
//! 2. the proxy route table for the service shrinks from 3 entries to 2
//!
//! Regression coverage for `routes::update_container_routes` (the health
//! filter on lines 63-64 — `WorkloadStatus::Running` AND health in
//! {Healthy, NoCheck}). A change that broke either filter would either
//! ship traffic to a dead replica (sticky targets) or pull all replicas
//! out of rotation when one fails (regression cliff).
//!
//! Run with: `cargo test -p orca-control --test e2e_route_filtering_test -- --ignored`

mod e2e_helpers;

use std::collections::HashMap;
use std::time::Duration;

use e2e_helpers::{TestClient, cleanup_containers, start_server};
use orca_control::health::HealthChecker;
use orca_core::types::HealthState;

const SVC: &str = "e2e-routefilter";

#[tokio::test]
#[ignore]
async fn e2e_route_table_drops_killed_replica_keeps_healthy() {
    let (port, state, _handle) = start_server().await;
    let client = TestClient::new(port);

    // Deploy 3 replicas with a liveness probe pointing at `/` (always 200
    // on nginx). `initial_delay_secs: 0` so the health checker probes
    // immediately rather than waiting out the default 5s window.
    let deploy = serde_json::json!({
        "services": [{
            "name": SVC,
            "image": "nginx:alpine",
            "replicas": 3,
            "port": 80,
            "domain": "routefilter.test",
            "liveness": {
                "path": "/",
                "initial_delay_secs": 0,
                "interval_secs": 1,
                // High threshold so the unhealthy replica is left in the
                // service map for assertion — at threshold the watchdog
                // tears the container down and replaces it, which races
                // the test.
                "failure_threshold": 99
            }
        }]
    });
    assert_eq!(
        client.post_json("/api/v1/deploy", &deploy).await.status(),
        200,
    );

    // Wait for containers to come up, then drive a check cycle so the
    // health checker marks them Healthy and rebuilds the route table.
    tokio::time::sleep(Duration::from_secs(4)).await;
    let checker = HealthChecker::new(state.clone());
    let mut counts = HashMap::new();
    checker.check_all(&mut counts).await;

    // Route table should now have 3 entries for the service's domain.
    let routes = state.route_table.read().await;
    let initial = routes.get("routefilter.test").map(|v| v.len()).unwrap_or(0);
    drop(routes);
    assert_eq!(
        initial, 3,
        "expected 3 healthy backends in the route table, got {initial}"
    );

    // Pick one of the 3 instances, find its container, and stop it.
    let services_guard = state.services.read().await;
    let svc = services_guard
        .get(SVC)
        .expect("service should be registered");
    let target_runtime_id = svc.instances[0].handle.runtime_id.clone();
    drop(services_guard);

    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    docker
        .stop_container(&target_runtime_id, None)
        .await
        .expect("docker stop");

    // Run another health-check cycle. With failure_threshold=1 the killed
    // instance flips to Unhealthy on this single failed probe, and the
    // route refresh inside `check_all` drops it from the route table.
    counts.clear();
    checker.check_all(&mut counts).await;

    // The killed instance must now be Unhealthy.
    let services_guard = state.services.read().await;
    let svc = services_guard.get(SVC).unwrap();
    let killed = svc
        .instances
        .iter()
        .find(|i| i.handle.runtime_id == target_runtime_id)
        .expect("killed instance still in service state");
    assert_eq!(
        killed.health,
        HealthState::Unhealthy,
        "killed replica should be marked Unhealthy"
    );
    drop(services_guard);

    // The route table must have shed exactly one entry — sibling replicas
    // continue serving.
    let routes = state.route_table.read().await;
    let after = routes.get("routefilter.test").map(|v| v.len()).unwrap_or(0);
    drop(routes);
    assert_eq!(
        after, 2,
        "expected 2 healthy backends after one replica killed, got {after}"
    );

    drop(state);
    cleanup_containers("orca-e2e-").await;
}
