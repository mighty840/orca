use std::time::Duration;

use axum::http::StatusCode;
use orca_core::types::WorkloadStatus;
use orca_core::ws_types::MasterMessage;

use super::collect;
use crate::alert_context::tests::{config, instance, state};
use crate::state::{AgentSession, ServiceState};

fn remote_service(node_id: u64) -> ServiceState {
    let mut svc = ServiceState::from_config(config(serde_json::json!({
        "name": "api", "image": "api:1"
    })));
    svc.instances.push(instance(
        &format!("remote-{node_id}"),
        WorkloadStatus::Running,
    ));
    svc
}

#[tokio::test]
async fn a_remote_services_logs_come_from_its_agent() {
    let state = state();
    state
        .services
        .write()
        .await
        .insert("api".into(), remote_service(9));
    let (tx, mut rx) = tokio::sync::mpsc::channel::<MasterMessage>(4);
    state
        .ws_agents
        .write()
        .await
        .insert(9, AgentSession::new(tx));

    let agent_state = state.clone();
    tokio::spawn(async move {
        let Some(MasterMessage::LogRequest {
            request_id, tail, ..
        }) = rx.recv().await
        else {
            panic!("expected a LogRequest");
        };
        assert_eq!(tail, 40);
        let listeners = agent_state.log_listeners.read().await;
        let tx = listeners
            .get(&request_id)
            .expect("listener registered first");
        tx.send(("line 1\n".into(), false)).await.unwrap();
        tx.send(("line 2\n".into(), true)).await.unwrap();
    });

    let text = collect(&state, "api", 40, Duration::from_secs(5), true)
        .await
        .unwrap();
    assert_eq!(text, "line 1\nline 2\n");
    assert!(
        state.log_listeners.read().await.is_empty(),
        "listener removed"
    );
}

#[tokio::test]
async fn a_disconnected_agent_is_503_and_leaves_no_listener() {
    let state = state();
    state
        .services
        .write()
        .await
        .insert("api".into(), remote_service(9));
    let err = collect(&state, "api", 40, Duration::from_secs(5), true)
        .await
        .unwrap_err();
    assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(state.log_listeners.read().await.is_empty());
}

#[tokio::test]
async fn a_silent_agent_times_out_with_what_it_has() {
    let state = state();
    state
        .services
        .write()
        .await
        .insert("api".into(), remote_service(9));
    let (tx, _rx) = tokio::sync::mpsc::channel::<MasterMessage>(4);
    state
        .ws_agents
        .write()
        .await
        .insert(9, AgentSession::new(tx));
    let text = collect(&state, "api", 40, Duration::from_millis(50), true)
        .await
        .unwrap();
    assert!(text.contains("[log stream timed out after 0s]"), "{text}");
    assert!(state.log_listeners.read().await.is_empty());
}

#[tokio::test]
async fn an_unknown_service_is_404() {
    let err = collect(&state(), "nope", 40, Duration::from_secs(1), true)
        .await
        .unwrap_err();
    assert_eq!(err.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_stopped_local_instance_is_read_only_when_asked() {
    let state = state();
    let mut svc = ServiceState::from_config(config(serde_json::json!({
        "name": "api", "image": "api:1"
    })));
    svc.instances
        .push(instance("orca-api", WorkloadStatus::Failed));
    state.services.write().await.insert("api".into(), svc);

    let err = collect(&state, "api", 40, Duration::from_secs(1), false)
        .await
        .unwrap_err();
    assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        collect(&state, "api", 40, Duration::from_secs(1), true)
            .await
            .is_ok(),
        "a crashed container's logs are what a diagnosis needs"
    );
}
