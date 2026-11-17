//! `GET /api/v1/services/{name}` and `.../history` (#80, shared with #259
//! and #265). The configuration is masked: see `ServiceConfigView`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use orca_core::api_types::{DeployRecordView, ServiceConfigView, ServiceDetail};

use crate::auth::Principal;
use crate::state::AppState;

/// The masked view of `config` for `caller`.
fn view_for(
    caller: Option<&Principal>,
    config: &orca_core::config::ServiceConfig,
) -> ServiceConfigView {
    let mut view = ServiceConfigView::from(config);
    if !caller.is_some_and(|p| p.role.can("secrets")) {
        view.hide_secret_refs();
    }
    view
}

pub(crate) async fn service_detail(
    State(state): State<Arc<AppState>>,
    caller: Option<Extension<Principal>>,
    Path(name): Path<String>,
) -> Response {
    super::refresh_observed_statuses(&state).await;
    let services = state.services.read().await;
    let Some(svc) = services.get(&name) else {
        return not_found(&name);
    };
    let stats = state.container_stats.read().await;
    let failures = state.last_failures.read().await;
    Json(ServiceDetail {
        status: super::status_view::service_status(svc, &stats, &failures),
        config: view_for(caller.as_deref(), &svc.config),
    })
    .into_response()
}

/// Recorded deploys, newest first. Kept in memory: empty after a restart.
pub(crate) async fn service_history(
    State(state): State<Arc<AppState>>,
    caller: Option<Extension<Principal>>,
    Path(name): Path<String>,
) -> Response {
    if !state.services.read().await.contains_key(&name) {
        return not_found(&name);
    }
    let history = state.deploy_history.read().await;
    let records: Vec<DeployRecordView> = history
        .list(&name)
        .iter()
        .rev()
        .map(|r| DeployRecordView {
            deploy_id: r.deploy_id.clone(),
            timestamp: r.timestamp,
            config: view_for(caller.as_deref(), &r.config),
        })
        .collect();
    Json(records).into_response()
}

fn not_found(name: &str) -> Response {
    (StatusCode::NOT_FOUND, format!("service '{name}' not found")).into_response()
}
