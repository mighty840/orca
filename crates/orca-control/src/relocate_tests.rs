//! #177: a placement change moves the workload instead of leaving the old
//! copy running unmanaged.

use std::collections::HashMap;
use std::sync::Arc;

use orca_core::config::{ClusterConfig, ServiceConfig};
use orca_core::testing::{MockOpKind, MockRuntime};
use orca_core::ws_types::MasterMessage;
use tokio::sync::{Notify, RwLock, mpsc};

use super::stop_if_moved;
use crate::session::AgentSession;
use crate::state::AppState;

fn state(runtime: Arc<MockRuntime>) -> AppState {
    AppState::new(
        ClusterConfig::default(),
        runtime,
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    )
}

fn config(name: &str) -> ServiceConfig {
    serde_json::from_value(serde_json::json!({
        "name": name,
        "image": "nginx:latest",
        "port": 8080,
    }))
    .unwrap()
}

async fn deploy_locally(state: &AppState, name: &str) {
    crate::reconciler::reconcile_service(state, &config(name))
        .await
        .unwrap();
    assert_eq!(state.services.read().await[name].instances.len(), 1);
}

#[tokio::test]
async fn moving_off_the_master_stops_the_local_container() {
    let runtime = Arc::new(MockRuntime::new());
    let state = state(runtime.clone());
    deploy_locally(&state, "web").await;

    stop_if_moved(&state, "web", Some(7)).await;

    assert_eq!(runtime.count(MockOpKind::Stop).await, 1);
    assert_eq!(runtime.count(MockOpKind::Remove).await, 1);
    assert!(state.services.read().await["web"].instances.is_empty());
}

#[tokio::test]
async fn moving_to_the_master_stops_it_on_the_old_agent() {
    let runtime = Arc::new(MockRuntime::new());
    let state = state(runtime.clone());
    deploy_locally(&state, "web").await;
    state
        .services
        .write()
        .await
        .get_mut("web")
        .unwrap()
        .instances[0]
        .handle
        .runtime_id = "remote-7".into();
    let (tx, mut rx) = mpsc::channel(4);
    state.ws_agents.write().await.insert(
        7,
        AgentSession {
            tx,
            session_id: 1,
            shutdown: Arc::new(Notify::new()),
        },
    );

    stop_if_moved(&state, "web", None).await;

    match rx.try_recv() {
        Ok(MasterMessage::Stop { service_name }) => assert_eq!(service_name, "web"),
        other => panic!("expected Stop for web, got {other:?}"),
    }
    assert_eq!(runtime.count(MockOpKind::Stop).await, 0, "not a local stop");
    assert!(state.services.read().await["web"].instances.is_empty());
}

#[tokio::test]
async fn same_location_is_left_alone() {
    let runtime = Arc::new(MockRuntime::new());
    let state = state(runtime.clone());
    deploy_locally(&state, "web").await;

    stop_if_moved(&state, "web", None).await;

    assert_eq!(runtime.count(MockOpKind::Stop).await, 0);
    assert_eq!(state.services.read().await["web"].instances.len(), 1);
}
