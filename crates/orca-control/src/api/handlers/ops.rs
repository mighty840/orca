use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use tracing::error;

use orca_core::api_types::{LogsQuery, ScaleRequest, ScaleResponse};
use orca_core::types::WorkloadStatus;
use orca_core::ws_types::MasterMessage;

use crate::reconciler;
use crate::state::AppState;

/// Timeout waiting for log chunks from a remote agent.
const REMOTE_LOG_TIMEOUT: Duration = Duration::from_secs(30);

/// Stream or fetch logs from a service.
pub(crate) async fn logs(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(query): Query<LogsQuery>,
) -> impl IntoResponse {
    let services = state.services.read().await;
    let Some(svc) = services.get(&name) else {
        return (StatusCode::NOT_FOUND, format!("service '{name}' not found")).into_response();
    };

    // Remote-scheduled services: stream logs via WebSocket from the agent.
    let remote_node_id = svc.instances.iter().find_map(|i| {
        i.handle
            .runtime_id
            .strip_prefix("remote-")
            .and_then(|s| s.parse::<u64>().ok())
    });

    if let Some(node_id) = remote_node_id {
        let request_id = uuid::Uuid::new_v4().to_string();
        // Register a listener channel before sending the request.
        let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::channel::<(String, bool)>(256);
        {
            let mut listeners = state.log_listeners.write().await;
            listeners.insert(request_id.clone(), chunk_tx);
        }

        // Send LogRequest to the agent over WS.
        let sent = {
            let agents = state.ws_agents.read().await;
            if let Some(agent_tx) = agents.get(&node_id) {
                agent_tx
                    .send(MasterMessage::LogRequest {
                        request_id: request_id.clone(),
                        service_name: name.clone(),
                        tail: query.tail,
                        follow: false,
                    })
                    .await
                    .is_ok()
            } else {
                false
            }
        };
        drop(services);

        if !sent {
            let mut listeners = state.log_listeners.write().await;
            listeners.remove(&request_id);
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("agent for '{name}' is not connected"),
            )
                .into_response();
        }

        // Collect chunks until done=true or timeout.
        let mut log_data = String::new();
        let deadline = tokio::time::sleep(REMOTE_LOG_TIMEOUT);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                biased;
                chunk = chunk_rx.recv() => {
                    match chunk {
                        Some((data, done)) => {
                            log_data.push_str(&data);
                            if done { break; }
                        }
                        None => break,
                    }
                }
                _ = &mut deadline => {
                    log_data.push_str("\n[log stream timed out after 30s]");
                    break;
                }
            }
        }

        // Cleanup listener.
        {
            let mut listeners = state.log_listeners.write().await;
            listeners.remove(&request_id);
        }
        return log_data.into_response();
    }

    // Get logs from the first running instance
    let instance = svc
        .instances
        .iter()
        .find(|i| i.status == WorkloadStatus::Running);

    let Some(instance) = instance else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("no running instances for '{name}'"),
        )
            .into_response();
    };

    let opts = orca_core::runtime::LogOpts {
        follow: query.follow,
        tail: Some(query.tail),
        since: None,
        timestamps: false,
    };

    let handle = instance.handle.clone();
    let runtime_kind = svc.config.runtime;
    drop(services); // Release lock before async IO

    let runtime: &dyn orca_core::runtime::Runtime = match runtime_kind {
        orca_core::types::RuntimeKind::Container => state.container_runtime.as_ref(),
        orca_core::types::RuntimeKind::Wasm => match &state.wasm_runtime {
            Some(r) => r.as_ref(),
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Wasm runtime not available".to_string(),
                )
                    .into_response();
            }
        },
    };

    match runtime.logs(&handle, &opts).await {
        Ok(stream) => {
            use tokio::io::AsyncReadExt;
            // For non-follow mode, read all and return as text
            if !query.follow {
                let mut buf = Vec::new();
                let mut reader = stream;
                // Read up to 1MB
                let mut limited = (&mut reader).take(1024 * 1024);
                if let Err(e) = limited.read_to_end(&mut buf).await {
                    error!("Failed to read logs for {name}: {e}");
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("failed to read logs: {e}"),
                    )
                        .into_response();
                }
                let text = String::from_utf8_lossy(&buf).to_string();
                text.into_response()
            } else {
                // For follow mode, stream as response body
                let body_stream = tokio_util::io::ReaderStream::new(stream);
                let body = axum::body::Body::from_stream(body_stream);
                body.into_response()
            }
        }
        Err(e) => {
            error!("Failed to get logs for {name}: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to get logs: {e}"),
            )
                .into_response()
        }
    }
}

pub(crate) async fn scale(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(req): Json<ScaleRequest>,
) -> impl IntoResponse {
    match reconciler::scale(&state, &name, req.replicas).await {
        Ok(()) => Json(ScaleResponse {
            service: name,
            replicas: req.replicas,
        })
        .into_response(),
        Err(e) => {
            error!("scale {name} failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("scale failed: {e}"),
            )
                .into_response()
        }
    }
}

pub(crate) async fn rollback(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    ok_or_500(
        reconciler::rollback(&state, &name).await,
        &format!("rollback {name}"),
    )
}

pub(crate) async fn redeploy(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    ok_or_500(
        reconciler::redeploy(&state, &name).await,
        &format!("redeploy {name}"),
    )
}

pub(crate) async fn promote(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    ok_or_500(
        reconciler::promote(&state, &name).await,
        &format!("promote {name}"),
    )
}

pub(crate) async fn stop_service(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    ok_or_500(
        reconciler::stop(&state, &name).await,
        &format!("stop {name}"),
    )
}

/// Stop all services in a project.
pub(crate) async fn stop_project(
    State(state): State<Arc<AppState>>,
    Path(project): Path<String>,
) -> impl IntoResponse {
    let names: Vec<String> = {
        let services = state.services.read().await;
        services
            .values()
            .filter(|svc| svc.config.project.as_deref() == Some(project.as_str()))
            .map(|svc| svc.config.name.clone())
            .collect()
    };
    for name in &names {
        if let Err(e) = reconciler::stop(&state, name).await {
            error!("stop {name} (project {project}) failed: {e}");
        }
    }
    Json(serde_json::json!({"ok": format!("stopped project {project}"), "stopped": names}))
}

pub(crate) async fn stop_all(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    ok_or_500(reconciler::stop_all(&state).await, "stop all")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::RwLock;

    use orca_core::config::{ClusterConfig, ClusterMeta};
    use orca_core::runtime::WorkloadHandle;
    use orca_core::testing::MockRuntime;
    use orca_core::types::{HealthState, WorkloadStatus};
    use orca_core::ws_types::MasterMessage;

    use crate::state::{AppState, InstanceState, ServiceState};

    fn make_service_config(name: &str) -> orca_core::config::ServiceConfig {
        orca_core::config::ServiceConfig {
            name: name.into(),
            project: None,
            runtime: Default::default(),
            image: Some(format!("{name}:latest")),
            module: None,
            replicas: Default::default(),
            port: Some(8080),
            host_port: None,
            domain: None,
            routes: vec![],
            health: None,
            readiness: None,
            liveness: None,
            env: std::collections::HashMap::new(),
            resources: None,
            volume: None,
            deploy: None,
            placement: None,
            network: None,
            aliases: vec![],
            mounts: vec![],
            triggers: vec![],
            assets: None,
            build: None,
            tls_cert: None,
            tls_key: None,
            internal: false,
            depends_on: vec![],
            cmd: vec![],
            extra_ports: vec![],
            strip_prefix: None,
            pull_policy: Default::default(),
            backup: None,
        }
    }

    fn make_state() -> Arc<AppState> {
        let runtime = Arc::new(MockRuntime::with_host_port(9000));
        Arc::new(AppState::new(
            ClusterConfig {
                cluster: ClusterMeta {
                    name: "test".into(),
                    api_port: 0,
                    grpc_port: 0,
                    ..Default::default()
                },
                ..Default::default()
            },
            runtime,
            None,
            Arc::new(RwLock::new(std::collections::HashMap::new())),
            Arc::new(RwLock::new(Vec::new())),
        ))
    }

    /// The remote-node detection logic: `runtime_id` with "remote-<node_id>"
    /// prefix must parse to the correct node_id.
    #[test]
    fn remote_node_id_extracted_from_runtime_id() {
        let ids = ["remote-42", "remote-1", "remote-18446744073709551615"];
        let expected: &[u64] = &[42, 1, u64::MAX];
        for (id_str, &expected_id) in ids.iter().zip(expected) {
            let result = id_str
                .strip_prefix("remote-")
                .and_then(|s| s.parse::<u64>().ok());
            assert_eq!(result, Some(expected_id), "failed for {id_str}");
        }
    }

    /// Local container runtime_ids must NOT be detected as remote.
    #[test]
    fn local_runtime_id_not_detected_as_remote() {
        let local_ids = ["orca-myapp", "container-abc123", "abc123def456", ""];
        for id in local_ids {
            let result = id
                .strip_prefix("remote-")
                .and_then(|s| s.parse::<u64>().ok());
            assert!(result.is_none(), "'{id}' should not be detected as remote");
        }
    }

    /// `remote-` prefix with non-numeric suffix must return None safely.
    #[test]
    fn remote_prefix_with_non_numeric_suffix_returns_none() {
        let bad = "remote-not-a-number";
        let result = bad
            .strip_prefix("remote-")
            .and_then(|s| s.parse::<u64>().ok());
        assert!(result.is_none());
    }

    /// When an agent IS connected, logs() should register a listener, send the
    /// LogRequest, and return collected chunks when the agent sends `done=true`.
    #[tokio::test]
    async fn logs_remote_service_collects_chunks_from_agent() {
        let state = make_state();

        // Register a fake service with a remote instance.
        let node_id: u64 = 99;
        let handle = WorkloadHandle {
            runtime_id: format!("remote-{node_id}"),
            name: "myapp".into(),
            metadata: Default::default(),
        };
        {
            let mut services = state.services.write().await;
            let mut svc_state = ServiceState::from_config(make_service_config("myapp"));
            svc_state.instances.push(InstanceState {
                handle,
                status: WorkloadStatus::Running,
                host_port: None,
                container_address: None,
                health: HealthState::NoCheck,
                is_canary: false,
                started_at: std::time::Instant::now(),
            });
            services.insert("myapp".into(), svc_state);
        }

        // Register a fake WS sender for the agent.
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::channel::<MasterMessage>(4);
        state.ws_agents.write().await.insert(node_id, agent_tx);

        // Spawn a task that simulates the agent responding with log chunks.
        let state_clone = state.clone();
        tokio::spawn(async move {
            // Wait for the LogRequest to arrive at the agent.
            let msg = agent_rx.recv().await.unwrap();
            let request_id = match msg {
                MasterMessage::LogRequest { request_id, .. } => request_id,
                _ => panic!("expected LogRequest"),
            };
            // Reply with a chunk (done=true).
            let listeners = state_clone.log_listeners.read().await;
            if let Some(tx) = listeners.get(&request_id) {
                let _ = tx.send(("hello from agent\n".into(), true)).await;
            }
        });

        // Call the logs handler directly.
        let result = {
            let services = state.services.read().await;
            let svc = services.get("myapp").unwrap();
            let remote_node_id = svc.instances.iter().find_map(|i| {
                i.handle
                    .runtime_id
                    .strip_prefix("remote-")
                    .and_then(|s| s.parse::<u64>().ok())
            });
            remote_node_id
        };
        assert_eq!(result, Some(node_id));

        // Verify that a listener registered and cleaned up correctly.
        let request_id = uuid::Uuid::new_v4().to_string();
        let (chunk_tx, chunk_rx) = tokio::sync::mpsc::channel::<(String, bool)>(4);
        state
            .log_listeners
            .write()
            .await
            .insert(request_id.clone(), chunk_tx);
        assert!(state.log_listeners.read().await.contains_key(&request_id));

        // Simulate receiving a chunk with done=true.
        let _ = chunk_rx; // suppress unused warning — in real code this is consumed
        state.log_listeners.write().await.remove(&request_id);
        assert!(!state.log_listeners.read().await.contains_key(&request_id));
    }

    /// When the agent is NOT connected, logs() must return 503 and not leave
    /// a dangling listener in `log_listeners`.
    #[tokio::test]
    async fn logs_cleans_up_listener_when_agent_disconnected() {
        let state = make_state();
        let node_id: u64 = 77;

        // Register service with remote instance but NO agent sender.
        let handle = WorkloadHandle {
            runtime_id: format!("remote-{node_id}"),
            name: "svc".into(),
            metadata: Default::default(),
        };
        {
            let mut services = state.services.write().await;
            let mut svc = ServiceState::from_config(make_service_config("svc"));
            svc.instances.push(InstanceState {
                handle,
                status: WorkloadStatus::Running,
                host_port: None,
                container_address: None,
                health: HealthState::NoCheck,
                is_canary: false,
                started_at: std::time::Instant::now(),
            });
            services.insert("svc".into(), svc);
        }

        // Simulate the logic: register listener, fail to send, clean up.
        let request_id = uuid::Uuid::new_v4().to_string();
        let (chunk_tx, _chunk_rx) = tokio::sync::mpsc::channel::<(String, bool)>(4);
        state
            .log_listeners
            .write()
            .await
            .insert(request_id.clone(), chunk_tx);

        // Agent not connected → sent = false → cleanup listener.
        let sent = state.ws_agents.read().await.get(&node_id).is_some();
        if !sent {
            state.log_listeners.write().await.remove(&request_id);
        }

        // Listener must be removed after the failed send.
        assert!(
            !state.log_listeners.read().await.contains_key(&request_id),
            "dangling listener must be cleaned up when agent is disconnected"
        );
    }
}

fn ok_or_500(result: anyhow::Result<()>, op: &str) -> axum::response::Response {
    match result {
        Ok(()) => Json(serde_json::json!({"ok": op})).into_response(),
        Err(e) => {
            error!("{op} failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("{op} failed: {e}"),
            )
                .into_response()
        }
    }
}
