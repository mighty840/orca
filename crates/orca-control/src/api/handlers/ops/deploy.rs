//! Deploy/redeploy/rollback/scale/stop handlers for individual services and projects.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use tracing::error;

use orca_core::api_types::{ScaleRequest, ScaleResponse};

use crate::reconciler;
use crate::state::AppState;

use super::ok_or_500;

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
