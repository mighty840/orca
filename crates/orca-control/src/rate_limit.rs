//! Per-token limit on mutating API requests (#80), from `[api_rate_limit]`.
//!
//! Keyed by the caller's token name, so it holds for every client (CLI,
//! TUI, CI, MCP), not per IP: in-process callers all share one address.
//! Agent registration and heartbeats, which every agent sends with the
//! cluster token, are not counted.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::auth::Principal;
use crate::state::AppState;

/// One token bucket per principal.
#[derive(Debug, Default)]
pub struct Limiter {
    buckets: Mutex<HashMap<String, (f64, Instant)>>,
}

impl Limiter {
    /// Take one request from `key`'s bucket of `per_minute`, or say how long
    /// until one is available.
    pub fn take(&self, key: &str, per_minute: u32, now: Instant) -> Result<(), Duration> {
        let capacity = f64::from(per_minute.max(1));
        let per_sec = capacity / 60.0;
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        let (tokens, at) = buckets.entry(key.to_string()).or_insert((capacity, now));
        *tokens =
            (*tokens + now.saturating_duration_since(*at).as_secs_f64() * per_sec).min(capacity);
        *at = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - *tokens) / per_sec))
        }
    }
}

/// Middleware, inside `auth_middleware` (it needs the caller) and inside
/// the audit layer (so a 429 is audited).
pub async fn rate_limit_middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let cfg = &state.cluster_config.api_rate_limit;
    if !cfg.enabled {
        return next.run(request).await;
    }
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str());
    let counted = route.is_some_and(|r| crate::audit::is_mutation(request.method().as_str(), r));
    let caller = request
        .extensions()
        .get::<Principal>()
        .map(|p| p.name.clone());
    let (true, Some(caller)) = (counted, caller) else {
        return next.run(request).await;
    };
    match state
        .rate_limiter
        .take(&caller, cfg.mutations_per_minute, Instant::now())
    {
        Ok(()) => next.run(request).await,
        Err(wait) => {
            let secs = wait.as_secs().max(1);
            let mut resp = (
                StatusCode::TOO_MANY_REQUESTS,
                format!(
                    "rate limit: {} mutating requests per minute for token '{caller}'; \
                     retry in {secs}s",
                    cfg.mutations_per_minute
                ),
            )
                .into_response();
            if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
                resp.headers_mut().insert(header::RETRY_AFTER, v);
            }
            resp
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_of_the_limit_then_one_per_interval() {
        let limiter = Limiter::default();
        let t0 = Instant::now();
        for _ in 0..60 {
            assert!(limiter.take("ci", 60, t0).is_ok());
        }
        let wait = limiter.take("ci", 60, t0).unwrap_err();
        assert!(wait <= Duration::from_secs(1), "{wait:?}");
        assert!(limiter.take("other", 60, t0).is_ok(), "per token");

        let t1 = t0 + Duration::from_secs(1);
        assert!(limiter.take("ci", 60, t1).is_ok(), "one back after 1s");
        assert!(limiter.take("ci", 60, t1).is_err());
    }
}
