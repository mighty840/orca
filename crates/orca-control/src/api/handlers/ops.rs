use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use tracing::error;

use orca_core::api_types::{LogsQuery, ScaleRequest, ScaleResponse};
use orca_core::types::WorkloadStatus;

use crate::reconciler;
use crate::state::AppState;

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
