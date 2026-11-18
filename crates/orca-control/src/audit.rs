//! An audit trail of API calls (#80, shared with #259's activity feed).
//!
//! Every mutating request and every exec is recorded, allowed or denied,
//! with the caller's token name, the route template, the status and the
//! latency. Never the query string or the body: they can carry secrets.
//! Events go to `tracing` (target `orca::audit`). Those with a known caller
//! also go to a bounded in-memory buffer served at `GET /api/v1/audit`;
//! durable storage is #82's job.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::{MatchedPath, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use orca_core::api_types::AuditEvent;

use crate::auth::Principal;
use crate::state::AppState;

/// How many events the buffer keeps.
pub const CAPACITY: usize = 1000;

/// Agents call these every few seconds with the cluster token; recording
/// them would flush every operator action out of the buffer.
const NOT_AUDITED: &[&str] = &["/api/v1/cluster/register", "/api/v1/cluster/heartbeat"];

/// The most recent [`CAPACITY`] events, oldest first.
#[derive(Debug, Default)]
pub struct AuditLog {
    events: Mutex<VecDeque<AuditEvent>>,
}

impl AuditLog {
    pub fn record(&self, event: AuditEvent) {
        tracing::info!(
            target: "orca::audit",
            principal = event.principal.as_deref().unwrap_or("-"),
            method = %event.method,
            route = %event.route,
            target_name = event.target.as_deref().unwrap_or("-"),
            status = event.status,
            latency_ms = event.latency_ms,
            client = event.client.as_deref().unwrap_or("-"),
            "api call"
        );
        // Logged, not buffered: anyone can send requests without a valid
        // token (or to the signed GitHub webhook), and enough of them would
        // push every operator's events out of the buffer.
        if event.principal.is_none() {
            return;
        }
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        if events.len() == CAPACITY {
            events.pop_front();
        }
        events.push_back(event);
    }

    /// The newest `limit` events, newest first.
    pub fn recent(&self, limit: usize) -> Vec<AuditEvent> {
        let events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        events.iter().rev().take(limit).cloned().collect()
    }
}

/// Whether a request changes something, leaving out agent chatter. Also
/// what `rate_limit` counts.
pub(crate) fn is_mutation(method: &str, route: &str) -> bool {
    let mutating = method != "GET" && method != "HEAD" && method != "OPTIONS";
    mutating && !NOT_AUDITED.contains(&route)
}

/// Whether a request to `route` with `method` is recorded: mutations, and
/// exec, which runs a command in a container.
fn audited(method: &str, route: &str) -> bool {
    is_mutation(method, route) || (route.ends_with("/exec") && !NOT_AUDITED.contains(&route))
}

/// The route template and the `{name}`-style segment of a request, if it is
/// audited.
fn describe(request: &Request) -> Option<(String, Option<String>)> {
    let route = request
        .extensions()
        .get::<MatchedPath>()?
        .as_str()
        .to_owned();
    if !audited(request.method().as_str(), &route) {
        return None;
    }
    // The segment where the template has its first `{...}` placeholder.
    let target = route
        .split('/')
        .zip(request.uri().path().split('/'))
        .find(|(t, _)| t.starts_with('{'))
        .map(|(_, v)| v.to_string());
    Some((route, target))
}

/// The parts of a request an event keeps, taken before the request is
/// handed on.
struct Seen {
    method: String,
    route: String,
    target: Option<String>,
    client: Option<String>,
    started: Instant,
}

impl Seen {
    fn of(request: &Request) -> Option<Self> {
        let (route, target) = describe(request)?;
        Some(Self {
            method: request.method().to_string(),
            route,
            target,
            client: request
                .headers()
                .get("x-orca-client")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.chars().take(32).collect()),
            started: Instant::now(),
        })
    }

    fn event(self, principal: Option<String>, status: StatusCode) -> AuditEvent {
        AuditEvent {
            at: chrono::Utc::now(),
            principal,
            method: self.method,
            route: self.route,
            target: self.target,
            status: status.as_u16(),
            latency_ms: self.started.elapsed().as_millis() as u64,
            client: self.client,
        }
    }
}

/// Record a request `auth_middleware` turned away: the inner audit layer
/// never sees it.
pub(crate) fn record_denied(
    state: &AppState,
    request: &Request,
    principal: Option<&str>,
    status: StatusCode,
) {
    if let Some(seen) = Seen::of(request) {
        state
            .audit
            .record(seen.event(principal.map(str::to_owned), status));
    }
}

/// Middleware, inside `auth_middleware`, that records allowed requests.
pub async fn audit_middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let Some(seen) = Seen::of(&request) else {
        return next.run(request).await;
    };
    let principal = request
        .extensions()
        .get::<Principal>()
        .map(|p| p.name.clone());
    let response = next.run(request).await;
    state.audit.record(seen.event(principal, response.status()));
    response
}

#[cfg(test)]
#[path = "audit_tests.rs"]
mod tests;
