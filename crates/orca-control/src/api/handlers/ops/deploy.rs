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
    let result = reconciler::rollback(&state, &name).await;
    if result.is_ok() {
        // Rollback recreates the container (new network identity); its
        // dependents must reconnect. reconcile() does this for the declarative
        // path — the manual path goes straight through operations::redeploy,
        // so trigger it here. restart_dependents computes the full transitive
        // set and never re-enters this handler, so no recursion.
        crate::dependents::restart_dependents(&state, std::slice::from_ref(&name)).await;
    }
    ok_or_500(result, &format!("rollback {name}"))
}

pub(crate) async fn redeploy(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let result = reconciler::redeploy(&state, &name).await;
    if result.is_ok() {
        // See rollback: a manual redeploy replaces the container, so its
        // dependents are restarted to drop black-holed connections.
        crate::dependents::restart_dependents(&state, std::slice::from_ref(&name)).await;
    }
    ok_or_500(result, &format!("redeploy {name}"))
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

/// Resume a paused service (`orca start`): clears the stopped mark and deploys
/// it back to its configured replica count.
pub(crate) async fn start_service(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    ok_or_500(
        reconciler::start(&state, &name).await,
        &format!("start {name}"),
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
