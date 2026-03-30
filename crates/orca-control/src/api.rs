//! REST API server for the orca control plane.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use tracing::error;

use orca_core::api_types::{
    DeployRequest, DeployResponse, LogsQuery, ScaleRequest, ScaleResponse, ServiceStatus,
    StatusResponse,
};
use orca_core::types::WorkloadStatus;

use crate::reconciler;
use crate::state::AppState;

/// Build the axum router for the API.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/deploy", post(deploy))
        .route("/api/v1/status", get(status))
        .route("/api/v1/services/{name}/logs", get(logs))
        .route("/api/v1/services/{name}/scale", post(scale))
        .with_state(state)
}

/// Health check endpoint.
async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

/// Deploy services from the request body.
async fn deploy(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeployRequest>,
) -> impl IntoResponse {
    let (deployed, errors) = reconciler::reconcile(&state, &req.services).await;

    let status_code = if errors.is_empty() {
        StatusCode::OK
    } else if deployed.is_empty() {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::PARTIAL_CONTENT
    };

    (status_code, Json(DeployResponse { deployed, errors }))
}

/// Get cluster and service status.
async fn status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let services = state.services.read().await;

    let service_statuses: Vec<ServiceStatus> = services
        .values()
        .map(|svc| {
            let running = svc.running_count();
            let overall_status = if running == 0 && svc.desired_replicas > 0 {
                "stopped"
            } else if running < svc.desired_replicas {
                "degraded"
            } else {
                "running"
            };

            ServiceStatus {
                name: svc.config.name.clone(),
                image: svc
                    .config
                    .image
                    .clone()
                    .or_else(|| svc.config.module.clone())
                    .unwrap_or_default(),
                runtime: svc.config.runtime,
                desired_replicas: svc.desired_replicas,
                running_replicas: running,
                status: overall_status.to_string(),
                domain: svc.config.domain.clone(),
            }
        })
        .collect();

    Json(StatusResponse {
        cluster_name: state.cluster_config.cluster.name.clone(),
        services: service_statuses,
    })
}

/// Stream or fetch logs from a service.
async fn logs(
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

/// Scale a service to the requested replica count.
async fn scale(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(req): Json<ScaleRequest>,
) -> impl IntoResponse {
    match reconciler::scale(&state, &name, req.replicas).await {
        Ok(()) => (
            StatusCode::OK,
            Json(ScaleResponse {
                service: name,
                replicas: req.replicas,
            }),
        )
            .into_response(),
        Err(e) => {
            error!("Failed to scale {name}: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("scale failed: {e}"),
            )
                .into_response()
        }
    }
}
