//! Bearer token authentication middleware with role-based access control.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use orca_core::config::Role;
use subtle::ConstantTimeEq;

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
        ("POST", p) if p.contains("/redeploy") => "deploy",
        ("POST", p) if p.contains("/drain") => "deploy",
        ("POST", p) if p.contains("/undrain") => "deploy",
        ("POST", p) if p.contains("/register") => "deploy",
        ("POST", p) if p.contains("/heartbeat") => "deploy",
        ("GET", p) if p.contains("/logs") => "logs",
        ("GET", "/api/v1/status") => "status",
        ("GET", "/api/v1/cluster/info") => "cluster_info",
        // Secrets are admin-only — they read and write encrypted material
        // and viewer/deployer roles must not see the key list either.
        ("GET", "/api/v1/secrets") => "secrets",
        ("POST", p) if p.starts_with("/api/v1/secrets/") => "secrets",
        ("DELETE", p) if p.starts_with("/api/v1/secrets/") => "secrets",
        _ => "status", // default to viewer-level for unknown GETs
    }
}

/// Compare a presented token with a configured one without an early exit on
/// the first differing byte.
///
/// Length is not hidden (lengths are compared first), which is acceptable:
/// a token's length is not secret, only its contents are.
fn ct_eq(presented: &str, configured: &str) -> bool {
    presented.as_bytes().ct_eq(configured.as_bytes()).into()
}

/// The role a presented token grants, or `None` if it grants nothing.
///
/// Legacy `api_tokens` grant admin; `[[token]]` entries grant their named
/// role. Shared by the HTTP middleware and the agent WebSocket so both answer
/// "who is this?" identically. An empty token never authenticates, even if an
/// empty string was configured by mistake.
pub(crate) fn resolve_token(state: &AppState, token: &str) -> Option<Role> {
    if token.is_empty() {
        return None;
    }
    if state.api_tokens.iter().any(|t| ct_eq(token, t)) {
        return Some(Role::Admin);
    }
    state
        .cluster_config
        .token
        .iter()
        .find(|t| ct_eq(token, &t.value))
        .map(|t| t.role)
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

    let Some(role) = resolve_token(&state, token) else {
        return (StatusCode::UNAUTHORIZED, "invalid bearer token").into_response();
    };

    let method = request.method().as_str().to_string();
    let action = required_action(&path, &method);
    if role.can(action) {
        return next.run(request).await;
    }
    (
        StatusCode::FORBIDDEN,
        format!(
            "role '{}' cannot perform '{}' (requires admin or deployer)",
            serde_json::to_string(&role).unwrap_or_default(),
            action
        ),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deploy_requires_deploy_action() {
        assert_eq!(required_action("/api/v1/deploy", "POST"), "deploy");
    }

    #[test]
    fn redeploy_requires_deploy_action() {
        assert_eq!(
            required_action("/api/v1/services/nginx/redeploy", "POST"),
            "deploy"
        );
    }

    #[test]
    fn rollback_requires_rollback_action() {
        assert_eq!(
            required_action("/api/v1/services/nginx/rollback", "POST"),
            "rollback"
        );
    }

    #[test]
    fn scale_requires_scale_action() {
        assert_eq!(
            required_action("/api/v1/services/nginx/scale", "POST"),
            "scale"
        );
    }

    #[test]
    fn stop_service_requires_stop_action() {
        assert_eq!(required_action("/api/v1/services/nginx", "DELETE"), "stop");
    }

    #[test]
    fn status_requires_status_action() {
        assert_eq!(required_action("/api/v1/status", "GET"), "status");
    }

    #[test]
    fn secrets_requires_secrets_action() {
        assert_eq!(required_action("/api/v1/secrets", "GET"), "secrets");
        assert_eq!(required_action("/api/v1/secrets/MY_KEY", "POST"), "secrets");
    }

    #[test]
    fn unknown_path_defaults_to_status() {
        assert_eq!(required_action("/unknown/path", "GET"), "status");
    }
}
