//! Bearer token authentication middleware with role-based access control.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::state::AppState;

/// Paths that skip bearer token authentication.
const SKIP_AUTH_PATHS: &[&str] = &["/api/v1/health", "/api/v1/webhooks/github"];

/// Map an API path + method to a required action for RBAC.
fn required_action(path: &str, method: &str) -> &'static str {
    match (method, path) {
        ("POST", "/api/v1/deploy") => "deploy",
        ("DELETE", p) if p.starts_with("/api/v1/services/") => "stop",
        ("DELETE", p) if p.starts_with("/api/v1/projects/") => "stop",
        ("POST", "/api/v1/stop") => "stop",
        ("POST", p) if p.contains("/scale") => "scale",
        ("POST", p) if p.contains("/rollback") => "rollback",
        ("POST", p) if p.contains("/drain") => "deploy",
        ("POST", p) if p.contains("/undrain") => "deploy",
        ("POST", p) if p.contains("/register") => "deploy",
        ("POST", p) if p.contains("/heartbeat") => "deploy",
        ("GET", p) if p.contains("/logs") => "logs",
        ("GET", "/api/v1/status") => "status",
        ("GET", "/api/v1/cluster/info") => "cluster_info",
        _ => "status", // default to viewer-level for unknown GETs
    }
}

/// Axum middleware that validates bearer tokens and checks RBAC roles.
///
/// Supports both legacy `api_tokens` (flat list, all admin) and new
/// `[[token]]` entries with named roles.
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let legacy_tokens = &state.api_tokens;
    let named_tokens = &state.cluster_config.token;

    // If no tokens configured, allow everything (backward compatible)
    if legacy_tokens.is_empty() && named_tokens.is_empty() {
        return next.run(request).await;
    }

    // Skip auth for exempt paths
    let path = request.uri().path().to_string();
    if SKIP_AUTH_PATHS.contains(&path.as_str()) {
        return next.run(request).await;
    }

    // Extract bearer token
    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    let token = match auth_header {
        Some(header) if header.starts_with("Bearer ") => &header[7..],
        _ => return (StatusCode::UNAUTHORIZED, "missing bearer token").into_response(),
    };

    // Check legacy tokens first (all treated as admin)
    if legacy_tokens.iter().any(|t| t == token) {
        return next.run(request).await;
    }

    // Check named tokens with RBAC
    let method = request.method().as_str().to_string();
    if let Some(api_token) = named_tokens.iter().find(|t| t.value == token) {
        let action = required_action(&path, &method);
        if api_token.role.can(action) {
            return next.run(request).await;
        }
        return (
            StatusCode::FORBIDDEN,
            format!(
                "role '{}' cannot perform '{}' (requires admin or deployer)",
                serde_json::to_string(&api_token.role).unwrap_or_default(),
                action
            ),
        )
            .into_response();
    }

    (StatusCode::UNAUTHORIZED, "invalid bearer token").into_response()
}
