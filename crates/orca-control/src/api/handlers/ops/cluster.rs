//! Cluster-spanning ops: streaming logs from remote/local nodes.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use tracing::error;

use orca_core::api_types::LogsQuery;
use orca_core::types::WorkloadStatus;
use orca_core::ws_types::MasterMessage;

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
