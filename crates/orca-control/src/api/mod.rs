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
    // Unauthenticated routes (metrics, WS agent endpoint).
    // WS does its own token auth via query param.
    let public = Router::new()
        .route("/metrics", get(crate::metrics::metrics_handler))
        .route("/api/v1/ws/agent", get(crate::ws_handler::ws_agent_handler))
        .with_state(state.clone());

    let authed = Router::new()
        .route("/api/v1/health", get(handlers::health))
        .route("/api/v1/deploy", post(handlers::deploy))
        .route("/api/v1/status", get(handlers::status))
        .route("/api/v1/services/{name}/logs", get(handlers::logs))
        .route(
            "/api/v1/services/{name}/exec",
            get(handlers::exec::exec_ws_handler),
        )
        .route("/api/v1/services/{name}/scale", post(handlers::scale))
        .route("/api/v1/services/{name}/rollback", post(handlers::rollback))
        .route("/api/v1/services/{name}/redeploy", post(handlers::redeploy))
        .route("/api/v1/services/{name}/promote", post(handlers::promote))
        .route("/api/v1/services/{name}", delete(handlers::stop_service))
        .route("/api/v1/projects/{project}", delete(handlers::stop_project))
        .route("/api/v1/stop", post(handlers::stop_all))
        .route("/api/v1/cluster/backups", get(handlers::cluster_backups))
        .route(
            "/api/v1/cluster/backups/trigger",
            post(handlers::trigger_cluster_backup),
        )
        .route("/api/v1/cluster/networks", get(handlers::cluster_networks))
        .route("/api/v1/alerts", get(handlers::alerts::list))
        .route("/api/v1/alerts/{id}", get(handlers::alerts::view))
        .route("/api/v1/alerts/{id}/reply", post(handlers::alerts::reply))
        .route(
            "/api/v1/alerts/{id}/dismiss",
            post(handlers::alerts::dismiss),
        )
        .route(
            "/api/v1/alerts/{id}/resolve",
            post(handlers::alerts::resolve),
        )
        .route("/api/v1/secrets", get(handlers::secrets::list_secrets))
        .route(
            "/api/v1/secrets/usage",
            get(handlers::secrets::secrets_usage),
        )
        .route("/api/v1/secrets/{key}", post(handlers::secrets::set_secret))
        .route(
            "/api/v1/secrets/{key}",
            delete(handlers::secrets::remove_secret),
        )
        .merge(webhook::webhook_router())
        .merge(cluster_handlers::cluster_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .with_state(state);

    public.merge(authed)
}
