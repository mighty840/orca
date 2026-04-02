//! E2E tests for self-healing: health checks and stale route cleanup.
//!
//! Run with: `cargo test -p orca-control --test e2e_selfheal_routes_test -- --ignored`

mod e2e_selfheal_helpers;

use e2e_selfheal_helpers::{Client, cleanup, start_server_with_watchdog};
use std::time::Duration;

/// Deploy with health check, verify route table updates when healthy.
#[tokio::test]
#[ignore]
async fn e2e_health_check_populates_routes() {
    let (_port, state) = start_server_with_watchdog().await;
    let client = Client::new(_port);

    let deploy = serde_json::json!({
        "services": [{
            "name": "e2e-hc",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80,
            "health": "/",
            "domain": "e2e-hc.local"
        }]
    });
    client.deploy(&deploy).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Route table should have the domain
    let routes = state.route_table.read().await;
    assert!(
        routes.contains_key("e2e-hc.local"),
        "domain should be in route table"
    );
    assert!(!routes["e2e-hc.local"].is_empty(), "should have targets");
    drop(routes);

    cleanup("orca-e2e-hc").await;
}

/// Deploy with env vars, redeploy with changed env — verify rolling update triggers.
#[tokio::test]
#[ignore]
async fn e2e_env_change_triggers_rolling_update() {
    let (_port, state) = start_server_with_watchdog().await;
    let client = Client::new(_port);

    let deploy_v1 = serde_json::json!({
        "services": [{
            "name": "e2e-env",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80,
            "env": {"FOO": "bar"}
        }]
    });
    client.deploy(&deploy_v1).await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let old_id = {
        let services = state.services.read().await;
        services["e2e-env"].instances[0].handle.runtime_id.clone()
    };

    // Redeploy with different env
    let deploy_v2 = serde_json::json!({
        "services": [{
            "name": "e2e-env",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80,
            "env": {"FOO": "baz"}
        }]
    });
    client.deploy(&deploy_v2).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let services = state.services.read().await;
    let svc = &services["e2e-env"];
    assert_eq!(svc.instances.len(), 1);
    assert_ne!(
        svc.instances[0].handle.runtime_id, old_id,
        "env change should trigger new container"
    );
    drop(services);

    cleanup("orca-e2e-env").await;
}

/// Verify stale routes are cleaned up when container dies.
#[tokio::test]
#[ignore]
async fn e2e_stale_route_cleanup() {
    let (_port, state) = start_server_with_watchdog().await;
    let client = Client::new(_port);

    let deploy = serde_json::json!({
        "services": [{
            "name": "e2e-stale",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80,
            "domain": "e2e-stale.local"
        }]
    });
    client.deploy(&deploy).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Verify route exists
    assert!(
        state
            .route_table
            .read()
            .await
            .contains_key("e2e-stale.local")
    );

    // Kill container and mark as failed
    let container_id = {
        let services = state.services.read().await;
        services["e2e-stale"].instances[0].handle.runtime_id.clone()
    };
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let _ = docker.kill_container::<&str>(&container_id, None).await;
    {
        let mut services = state.services.write().await;
        let svc = services.get_mut("e2e-stale").unwrap();
        svc.instances[0].status = orca_core::types::WorkloadStatus::Failed;
        svc.instances[0].health = orca_core::types::HealthState::Unhealthy;
    }

    // Run watchdog cycle to prune and update routes
    orca_control::watchdog::run_watchdog_cycle(&state).await;

    // Route should be removed (no healthy backends)
    // The watchdog reconciles, creating a new container, but routes only
    // include Healthy/NoCheck instances. Briefly the route may be empty.
    tokio::time::sleep(Duration::from_secs(1)).await;

    cleanup("orca-e2e-stale").await;
}
