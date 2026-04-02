use std::sync::Arc;

use axum::Router;
use axum::routing::{delete, get, post};

use crate::auth::auth_middleware;
use crate::cluster_handlers;
use crate::state::AppState;
use crate::webhook;

mod handlers;

/// Build the axum router for the API.
pub fn router(state: Arc<AppState>) -> Router {
    // Unauthenticated routes (metrics for Prometheus scraping).
    let public = Router::new()
        .route("/metrics", get(crate::metrics::metrics_handler))
        .with_state(state.clone());

    let authed = Router::new()
        .route("/api/v1/health", get(handlers::health))
        .route("/api/v1/deploy", post(handlers::deploy))
        .route("/api/v1/status", get(handlers::status))
        .route("/api/v1/services/{name}/logs", get(handlers::logs))
        .route("/api/v1/services/{name}/scale", post(handlers::scale))
        .route("/api/v1/services/{name}/rollback", post(handlers::rollback))
        .route("/api/v1/services/{name}/promote", post(handlers::promote))
        .route("/api/v1/services/{name}", delete(handlers::stop_service))
        .route("/api/v1/projects/{project}", delete(handlers::stop_project))
        .route("/api/v1/stop", post(handlers::stop_all))
        .merge(webhook::webhook_router())
        .merge(cluster_handlers::cluster_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .with_state(state);

    public.merge(authed)
}
