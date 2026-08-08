//! `GET /api/v1/status` must report observed container state, not the state
//! recorded at deploy time.
//!
//! Regression test for the incident where the API returned `status=running`
//! for services whose containers Docker showed as `Exited (1)`: the handler
//! read only the in-memory instance list, which is written optimistically on
//! deploy and refreshed no faster than the 30s watchdog cycle.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use orca_control::api::router;
use orca_control::reconciler::reconcile;
use orca_control::state::AppState;
use orca_core::api_types::StatusResponse;
use orca_core::config::{ClusterConfig, ServiceConfig};
use orca_core::runtime::Runtime;
use orca_core::testing::MockRuntime;

fn svc(name: &str) -> ServiceConfig {
    serde_json::from_value(serde_json::json!({
        "name": name,
        "image": "nginx:alpine",
        "replicas": 1,
    }))
    .expect("valid service config")
}

async fn serve(state: Arc<AppState>) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = router(state);
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    port
}

/// Deploy a service, then stop its container behind the control plane's back
/// (as a crash would). `status` must report what the runtime observes.
#[tokio::test]
async fn status_reports_observed_state_after_container_dies() {
    let mock = Arc::new(MockRuntime::new());
    let state = Arc::new(AppState::new(
        ClusterConfig::default(),
        mock.clone(),
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ));

    let (deployed, errors) = reconcile(&state, &[svc("web")]).await;
    assert_eq!(deployed, vec!["web".to_string()]);
    assert!(errors.is_empty(), "deploy errors: {errors:?}");

    // The container exits behind the control plane's back.
    let handle = {
        let services = state.services.read().await;
        services.get("web").unwrap().instances[0].handle.clone()
    };
    mock.stop(&handle, Duration::from_secs(1)).await.unwrap();

    let port = serve(state).await;
    let resp: StatusResponse = reqwest::get(format!("http://127.0.0.1:{port}/api/v1/status"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let web = resp.services.iter().find(|s| s.name == "web").unwrap();
    assert_eq!(
        web.running_replicas, 0,
        "a dead container must not count as a running replica"
    );
    assert_eq!(
        web.status, "stopped",
        "status must reflect the observed container state, got {:?}",
        web.status
    );
}

/// Sanity: a genuinely running service still reports running.
#[tokio::test]
async fn status_reports_running_for_live_container() {
    let mock = Arc::new(MockRuntime::new());
    let state = Arc::new(AppState::new(
        ClusterConfig::default(),
        mock.clone(),
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ));

    let (_, errors) = reconcile(&state, &[svc("web")]).await;
    assert!(errors.is_empty(), "deploy errors: {errors:?}");

    let port = serve(state).await;
    let resp: StatusResponse = reqwest::get(format!("http://127.0.0.1:{port}/api/v1/status"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let web = resp.services.iter().find(|s| s.name == "web").unwrap();
    assert_eq!(web.running_replicas, 1);
    assert_eq!(web.status, "running");
}
