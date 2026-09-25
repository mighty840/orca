//! Bearer token authentication middleware with role-based access control.

use std::sync::Arc;

use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use orca_core::config::Role;
use subtle::ConstantTimeEq;

use crate::state::AppState;

/// Paths that skip bearer token authentication.
const SKIP_AUTH_PATHS: &[&str] = &["/api/v1/health", "/api/v1/webhooks/github"];

/// The action no role but Admin holds.
///
/// [`Role::can`] grants Admin every action and the other roles only the
/// actions they list, so this needs no special case there.
const ADMIN_ONLY: &str = "admin";

/// What each mounted route requires, keyed by method and route template
/// (axum's [`MatchedPath`], e.g. `/api/v1/services/{name}/scale`).
///
/// This table is the single source of truth. A route missing from it is
/// admin-only, so forgetting to classify a new route fails closed rather than
/// silently granting viewer access (#202). `every_mounted_route_is_classified`
/// checks it against the router source.
const ROUTE_POLICY: &[(&str, &str, &str)] = &[
    // --- read-only: viewer and up ---------------------------------------
    ("GET", "/metrics", "status"),
    ("GET", "/api/v1/status", "status"),
    ("GET", "/api/v1/services/{name}/logs", "logs"),
    ("GET", "/api/v1/cluster/info", "cluster_info"),
    ("GET", "/api/v1/cluster/backups", "status"),
    ("GET", "/api/v1/cluster/networks", "status"),
    ("GET", "/api/v1/alerts", "status"),
    ("GET", "/api/v1/alerts/{id}", "status"),
    ("GET", "/api/v1/webhooks", "status"),
    ("GET", "/api/v1/webhooks/{id}/invocations", "status"),
    // Read-only, though each question spends LLM tokens.
    ("POST", "/api/v1/ask", "status"),
    // --- workload changes: deployer and up ------------------------------
    ("POST", "/api/v1/deploy", "deploy"),
    ("POST", "/api/v1/services/{name}/redeploy", "deploy"),
    ("POST", "/api/v1/services/{name}/promote", "deploy"),
    ("POST", "/api/v1/services/{name}/start", "deploy"),
    ("POST", "/api/v1/services/{name}/scale", "scale"),
    ("POST", "/api/v1/services/{name}/rollback", "rollback"),
    ("DELETE", "/api/v1/services/{name}", "stop"),
    ("DELETE", "/api/v1/projects/{project}", "stop"),
    ("POST", "/api/v1/stop", "stop"),
    // --- admin only -------------------------------------------------------
    // Runs an arbitrary command inside any container.
    ("GET", "/api/v1/services/{name}/exec", ADMIN_ONLY),
    // Secret values, and the inventory of which keys exist and who uses them.
    ("GET", "/api/v1/secrets", "secrets"),
    ("GET", "/api/v1/secrets/usage", "secrets"),
    ("POST", "/api/v1/secrets/{key}", "secrets"),
    ("DELETE", "/api/v1/secrets/{key}", "secrets"),
    // A registration decides what an unauthenticated push may redeploy.
    ("POST", "/api/v1/webhooks", ADMIN_ONLY),
    ("DELETE", "/api/v1/webhooks/{id}", ADMIN_ONLY),
    // Cluster membership: a registered node can be scheduled workloads.
    // `Role` documents drain as an admin action.
    ("POST", "/api/v1/cluster/register", ADMIN_ONLY),
    ("POST", "/api/v1/cluster/heartbeat", ADMIN_ONLY),
    ("POST", "/api/v1/cluster/nodes/{node_id}/drain", ADMIN_ONLY),
    (
        "POST",
        "/api/v1/cluster/nodes/{node_id}/undrain",
        ADMIN_ONLY,
    ),
    ("POST", "/api/v1/cluster/backups/trigger", ADMIN_ONLY),
    // Cluster token rotation (#210).
    ("POST", "/api/v1/cluster/token/rotate", ADMIN_ONLY),
    ("GET", "/api/v1/cluster/token/rotation", ADMIN_ONLY),
    ("POST", "/api/v1/cluster/token/rotation/finish", ADMIN_ONLY),
    ("POST", "/api/v1/alerts/{id}/reply", ADMIN_ONLY),
    ("POST", "/api/v1/alerts/{id}/dismiss", ADMIN_ONLY),
    ("POST", "/api/v1/alerts/{id}/resolve", ADMIN_ONLY),
];

/// The action a request needs, from its method and matched route template.
///
/// `template` is `None` when no route matched (the fallback), which is also
/// admin-only: an unknown path must not be a way around the table.
fn required_action(method: &str, template: Option<&str>) -> &'static str {
    template
        .and_then(|t| {
            ROUTE_POLICY
                .iter()
                .find(|(m, p, _)| *m == method && *p == t)
        })
        .map_or(ADMIN_ONLY, |(_, _, action)| action)
}

/// Compare a presented token with a configured one without an early exit on
/// the first differing byte.
///
/// Length is not hidden (lengths are compared first), which is acceptable:
/// a token's length is not secret, only its contents are.
fn ct_eq(presented: &str, configured: &str) -> bool {
    presented.as_bytes().ct_eq(configured.as_bytes()).into()
}

/// The token from an `Authorization: Bearer <token>` header, if present.
///
/// Shared by the HTTP middleware and the agent WebSocket so both read the
/// header the same way.
pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
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
    if state
        .api_tokens
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|t| ct_eq(token, t))
    {
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
    let no_legacy = state
        .api_tokens
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .is_empty();
    let named_tokens = &state.cluster_config.token;

    // If no tokens configured, allow everything (backward compatible)
    if no_legacy && named_tokens.is_empty() {
        return next.run(request).await;
    }

    // Skip auth for exempt paths
    let path = request.uri().path().to_string();
    if SKIP_AUTH_PATHS.contains(&path.as_str()) {
        return next.run(request).await;
    }

    let Some(token) = bearer_token(request.headers()) else {
        return (StatusCode::UNAUTHORIZED, "missing bearer token").into_response();
    };

    let Some(role) = resolve_token(&state, token) else {
        return (StatusCode::UNAUTHORIZED, "invalid bearer token").into_response();
    };

    let template = request
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_owned());
    let action = required_action(request.method().as_str(), template.as_deref());
    if role.can(action) {
        return next.run(request).await;
    }
    (
        StatusCode::FORBIDDEN,
        format!(
            "role '{}' may not perform '{}' on this route",
            serde_json::to_string(&role).unwrap_or_default(),
            action
        ),
    )
        .into_response()
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
