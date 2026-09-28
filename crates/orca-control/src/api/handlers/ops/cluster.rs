//! Cluster-spanning ops: streaming logs from remote/local nodes.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use tracing::error;

use orca_core::api_types::LogsQuery;
use orca_core::types::WorkloadStatus;

use crate::service_logs;
use crate::state::AppState;

/// Timeout waiting for log chunks from a remote agent.
const REMOTE_LOG_TIMEOUT: Duration = Duration::from_secs(30);

/// Stream or fetch logs from a service.
pub(crate) async fn logs(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(query): Query<LogsQuery>,
) -> impl IntoResponse {
    let remote = {
        let services = state.services.read().await;
        let Some(svc) = services.get(&name) else {
            return (StatusCode::NOT_FOUND, format!("service '{name}' not found")).into_response();
        };
        crate::service_logs::remote_node(svc).is_some()
    };

    // A remote service's logs are always collected, never followed: the
    // agent answers a LogRequest with one batch.
    if !query.follow || remote {
        return match service_logs::collect(&state, &name, query.tail, REMOTE_LOG_TIMEOUT, false)
            .await
        {
            Ok(text) => text.into_response(),
            Err(e) => (e.status, e.message).into_response(),
        };
    }

    follow_local(&state, &name, query.tail).await
}

/// Stream a local service's logs as they are written.
async fn follow_local(state: &AppState, name: &str, tail: u64) -> axum::response::Response {
    let services = state.services.read().await;
    let Some(svc) = services.get(name) else {
        return (StatusCode::NOT_FOUND, format!("service '{name}' not found")).into_response();
    };
    let Some(instance) = svc
        .instances
        .iter()
        .find(|i| i.status == WorkloadStatus::Running)
    else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("no running instances for '{name}'"),
        )
            .into_response();
    };

    let opts = orca_core::runtime::LogOpts {
        follow: true,
        tail: Some(tail),
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
            let body_stream = tokio_util::io::ReaderStream::new(stream);
            axum::body::Body::from_stream(body_stream).into_response()
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
