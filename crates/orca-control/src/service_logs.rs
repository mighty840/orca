//! Fetch a service's recent logs as text, from the local runtime or from the
//! agent that runs it. Shared by `GET /services/{name}/logs` and the alert
//! monitor, which puts the tail into a diagnosis prompt.

use std::time::Duration;

use axum::http::StatusCode;
use tokio::io::AsyncReadExt;
use tracing::error;

use orca_core::types::{RuntimeKind, WorkloadStatus};
use orca_core::ws_types::MasterMessage;

use crate::state::AppState;

/// Upper bound on log text read from a local runtime.
const LOCAL_READ_MAX: u64 = 1024 * 1024;

/// Why logs couldn't be fetched, with the HTTP status the API answers with.
#[derive(Debug)]
pub(crate) struct LogsError {
    pub status: StatusCode,
    pub message: String,
}

impl LogsError {
    fn new(status: StatusCode, message: String) -> Self {
        Self { status, message }
    }
}

/// Which node runs a service remotely, if an agent does.
pub(crate) fn remote_node(svc: &crate::state::ServiceState) -> Option<u64> {
    svc.instances.iter().find_map(|i| {
        i.handle
            .runtime_id
            .strip_prefix("remote-")
            .and_then(|s| s.parse::<u64>().ok())
    })
}

/// The last `tail` lines of `name`'s logs.
///
/// A remote service's logs come from its agent; the agent gets
/// `remote_timeout` to send them all. A local service's come from its
/// running instance, or, with `include_stopped`, from a stopped one when
/// none runs: a crashed container's last lines are what a diagnosis needs.
pub(crate) async fn collect(
    state: &AppState,
    name: &str,
    tail: u64,
    remote_timeout: Duration,
    include_stopped: bool,
) -> Result<String, LogsError> {
    let services = state.services.read().await;
    let Some(svc) = services.get(name) else {
        return Err(LogsError::new(
            StatusCode::NOT_FOUND,
            format!("service '{name}' not found"),
        ));
    };

    if let Some(node_id) = remote_node(svc) {
        drop(services);
        return collect_remote(state, name, node_id, tail, remote_timeout).await;
    }

    let instance = svc
        .instances
        .iter()
        .find(|i| i.status == WorkloadStatus::Running)
        .or_else(|| include_stopped.then(|| svc.instances.last()).flatten());
    let Some(instance) = instance else {
        return Err(LogsError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("no running instances for '{name}'"),
        ));
    };
    let handle = instance.handle.clone();
    let runtime_kind = svc.config.runtime;
    drop(services); // Release the lock before async IO.

    let runtime: &dyn orca_core::runtime::Runtime = match runtime_kind {
        RuntimeKind::Container => state.container_runtime.as_ref(),
        RuntimeKind::Wasm => match &state.wasm_runtime {
            Some(r) => r.as_ref(),
            None => {
                return Err(LogsError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Wasm runtime not available".to_string(),
                ));
            }
        },
    };
    let opts = orca_core::runtime::LogOpts {
        follow: false,
        tail: Some(tail),
        since: None,
        timestamps: false,
    };
    let mut stream = runtime.logs(&handle, &opts).await.map_err(|e| {
        error!("Failed to get logs for {name}: {e}");
        LogsError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to get logs: {e}"),
        )
    })?;
    let mut buf = Vec::new();
    (&mut stream)
        .take(LOCAL_READ_MAX)
        .read_to_end(&mut buf)
        .await
        .map_err(|e| {
            error!("Failed to read logs for {name}: {e}");
            LogsError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to read logs: {e}"),
            )
        })?;
    Ok(String::from_utf8_lossy(&buf).to_string())
}

async fn collect_remote(
    state: &AppState,
    name: &str,
    node_id: u64,
    tail: u64,
    timeout: Duration,
) -> Result<String, LogsError> {
    let request_id = uuid::Uuid::new_v4().to_string();
    // Register the listener before sending the request.
    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::channel::<(String, bool)>(256);
    state
        .log_listeners
        .write()
        .await
        .insert(request_id.clone(), chunk_tx);

    let sent = {
        let agents = state.ws_agents.read().await;
        match agents.get(&node_id) {
            Some(agent_tx) => agent_tx
                .send(MasterMessage::LogRequest {
                    request_id: request_id.clone(),
                    service_name: name.to_string(),
                    tail,
                    follow: false,
                })
                .await
                .is_ok(),
            None => false,
        }
    };
    if !sent {
        state.log_listeners.write().await.remove(&request_id);
        return Err(LogsError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("agent for '{name}' is not connected"),
        ));
    }

    // Collect chunks until done=true or the deadline.
    let mut log_data = String::new();
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            biased;
            chunk = chunk_rx.recv() => match chunk {
                Some((data, done)) => {
                    log_data.push_str(&data);
                    if done { break; }
                }
                None => break,
            },
            _ = &mut deadline => {
                log_data.push_str(&format!(
                    "\n[log stream timed out after {}s]",
                    timeout.as_secs()
                ));
                break;
            }
        }
    }
    state.log_listeners.write().await.remove(&request_id);
    Ok(log_data)
}

#[cfg(test)]
#[path = "service_logs_tests.rs"]
mod tests;
