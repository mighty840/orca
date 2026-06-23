//! Baseline security response headers injected by the proxy.
//!
//! The proxy is the TLS terminator and the single ingress for every service, so
//! it's the right place to add safe defaults (HSTS, `nosniff`, `Referrer-Policy`)
//! once rather than per app. Headers are added **only if the backend didn't set
//! them**, so an app's own `Content-Security-Policy` / `X-Frame-Options` /
//! `Strict-Transport-Security` always wins — this is the pass-through-plus-
//! defaults model used by Traefik/Caddy.
//!
//! The policy is process-global, installed once at startup from `cluster.toml`
//! via [`init`]. If `init` is never called (e.g. library/test callers), [`apply`]
//! is a no-op, so it never changes behavior for those paths.

use std::sync::OnceLock;

use hyper::Response;
use hyper::header::{HeaderMap, HeaderName, HeaderValue};

use crate::body::ProxyBody;
use orca_core::config::SecurityHeadersConfig;

static POLICY: OnceLock<Policy> = OnceLock::new();

/// Resolved header set, precomputed once from config.
struct Policy {
    /// Added to every response (e.g. `nosniff`, `Referrer-Policy`).
    always: Vec<(HeaderName, HeaderValue)>,
    /// Added only to HTTPS responses — HSTS is invalid/ignored over plain HTTP.
    https_only: Vec<(HeaderName, HeaderValue)>,
}

/// Install the proxy's security-header policy from config. Call once at startup
/// before serving. `None` installs the safe default set (on); a disabled config
/// installs an empty (no-op) policy.
pub fn init(config: Option<SecurityHeadersConfig>) {
    let _ = POLICY.set(build(config.unwrap_or_default()));
}

fn build(cfg: SecurityHeadersConfig) -> Policy {
    if !cfg.enabled {
        return Policy {
            always: Vec::new(),
            https_only: Vec::new(),
        };
    }
    let mut always = Vec::new();
    let mut https_only = Vec::new();

    push(
        &mut always,
        "x-content-type-options",
        &cfg.content_type_options,
    );
    push(&mut always, "referrer-policy", &cfg.referrer_policy);
    // Off by default (empty) — opt-in, since these break embedded / most apps.
    push(&mut always, "x-frame-options", &cfg.frame_options);
    push(&mut always, "content-security-policy", &cfg.csp);
    // HSTS only makes sense over TLS.
    push(&mut https_only, "strict-transport-security", &cfg.hsts);

    // Arbitrary operator-supplied extras (add-if-absent like the rest).
    for (k, v) in &cfg.extra {
        if let (Ok(name), Ok(val)) = (
            HeaderName::try_from(k.as_str()),
            HeaderValue::try_from(v.as_str()),
        ) {
            always.push((name, val));
        }
    }

    Policy { always, https_only }
}

/// Push a `(name, value)` pair, skipping empty values (= "disable this header")
/// and any value that isn't a valid header value.
fn push(out: &mut Vec<(HeaderName, HeaderValue)>, name: &'static str, value: &str) {
    if value.is_empty() {
        return;
    }
    if let Ok(val) = HeaderValue::try_from(value) {
        out.push((HeaderName::from_static(name), val));
    }
}

/// Apply the configured security headers to a response — **add-if-absent**, so a
/// header the backend already set is never overwritten. HSTS is added only when
/// `is_tls`. No-op if [`init`] was never called.
pub(crate) fn apply(resp: &mut Response<ProxyBody>, is_tls: bool) {
    let Some(policy) = POLICY.get() else {
        return;
    };
    let headers = resp.headers_mut();
    add_if_absent(headers, &policy.always);
    if is_tls {
        add_if_absent(headers, &policy.https_only);
    }
}

fn add_if_absent(map: &mut HeaderMap, entries: &[(HeaderName, HeaderValue)]) {
    for (name, value) in entries {
        if !map.contains_key(name) {
            map.insert(name.clone(), value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::full_body;

    fn resp() -> Response<ProxyBody> {
        Response::new(full_body(hyper::body::Bytes::new()))
    }

    fn policy_from(cfg: SecurityHeadersConfig) -> Policy {
        build(cfg)
    }

    fn apply_with(policy: &Policy, resp: &mut Response<ProxyBody>, is_tls: bool) {
        let headers = resp.headers_mut();
        add_if_absent(headers, &policy.always);
        if is_tls {
            add_if_absent(headers, &policy.https_only);
        }
    }

    #[test]
    fn defaults_add_safe_set_on_https() {
        let p = policy_from(SecurityHeadersConfig::default());
        let mut r = resp();
        apply_with(&p, &mut r, true);
        let h = r.headers();
        assert_eq!(h.get("x-content-type-options").unwrap(), "nosniff");
        assert_eq!(
            h.get("referrer-policy").unwrap(),
            "strict-origin-when-cross-origin"
        );
        assert_eq!(
            h.get("strict-transport-security").unwrap(),
            "max-age=31536000"
        );
        // Off by default.
        assert!(h.get("x-frame-options").is_none());
        assert!(h.get("content-security-policy").is_none());
    }

    #[test]
    fn hsts_only_on_tls() {
        let p = policy_from(SecurityHeadersConfig::default());
        let mut r = resp();
        apply_with(&p, &mut r, false);
        assert!(r.headers().get("strict-transport-security").is_none());
        assert!(r.headers().get("x-content-type-options").is_some());
    }

    #[test]
    fn never_clobbers_backend_header() {
        let p = policy_from(SecurityHeadersConfig::default());
        let mut r = resp();
        r.headers_mut()
            .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
        apply_with(&p, &mut r, true);
        // Backend's value is preserved (pass-through), not overwritten.
        assert_eq!(r.headers().get("referrer-policy").unwrap(), "no-referrer");
    }

    #[test]
    fn disabled_adds_nothing() {
        let p = policy_from(SecurityHeadersConfig {
            enabled: false,
            ..Default::default()
        });
        let mut r = resp();
        apply_with(&p, &mut r, true);
        assert!(r.headers().is_empty());
    }

    #[test]
    fn opt_in_frame_options_and_extra() {
        let mut extra = std::collections::HashMap::new();
        extra.insert(
            "permissions-policy".to_string(),
            "geolocation=()".to_string(),
        );
        let p = policy_from(SecurityHeadersConfig {
            frame_options: "SAMEORIGIN".to_string(),
            extra,
            ..Default::default()
        });
        let mut r = resp();
        apply_with(&p, &mut r, true);
        assert_eq!(r.headers().get("x-frame-options").unwrap(), "SAMEORIGIN");
        assert_eq!(
            r.headers().get("permissions-policy").unwrap(),
            "geolocation=()"
        );
    }
}
