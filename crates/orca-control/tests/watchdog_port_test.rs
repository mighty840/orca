//! #286: Docker restarted an OOM-killed container on its own (restart policy),
//! which gave its `127.0.0.1:0` binding a new random host port. The master
//! kept routing to the old port and answered 502 until the next deploy.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use orca_control::reconciler;
use orca_control::state::AppState;
use orca_control::watchdog::run_watchdog_cycle;
use orca_core::config::{ClusterConfig, ServiceConfig};
use orca_core::testing::{MockOpKind, MockRuntime};

fn gitea() -> ServiceConfig {
    serde_json::from_value(serde_json::json!({
        "name": "git-t0001-stage", "image": "gitea/gitea:1.24", "port": 3000,
        "domain": "bp-test.code-stage.breakpilot.com",
    }))
    .unwrap()
}

#[tokio::test]
async fn the_watchdog_reroutes_a_container_that_docker_restarted_on_a_new_port() {
    // Docker reports the port the restarted container has now.
    let runtime = Arc::new(MockRuntime::with_host_port(33802));
    let routes = Arc::new(RwLock::new(HashMap::new()));
    let state = AppState::new(
        ClusterConfig::default(),
        runtime.clone(),
        None,
        routes.clone(),
        Arc::new(RwLock::new(Vec::new())),
    );
    reconciler::reconcile(&state, &[gitea()]).await;

    // orca recorded the port the container had before Docker restarted it,
    // and routes to it.
    state
        .services
        .write()
        .await
        .get_mut("git-t0001-stage")
        .unwrap()
        .instances[0]
        .host_port = Some(32991);
    orca_control::routes::update_container_routes(&state, &gitea()).await;
    let before = routes.read().await["bp-test.code-stage.breakpilot.com"][0]
        .address
        .clone();
    assert_eq!(before, "127.0.0.1:32991");

    run_watchdog_cycle(&state).await;

    let services = state.services.read().await;
    let inst = &services["git-t0001-stage"].instances[0];
    assert_eq!(
        inst.host_port,
        Some(33802),
        "the instance takes the new port"
    );
    let targets = &routes.read().await["bp-test.code-stage.breakpilot.com"];
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert_eq!(
        targets[0].address, "127.0.0.1:33802",
        "and so does its route"
    );
    assert_eq!(
        runtime.count(MockOpKind::Create).await,
        1,
        "re-routed, not recreated"
    );
}
