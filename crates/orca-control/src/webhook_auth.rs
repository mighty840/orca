//! Authentication primitives for the push webhook endpoint.
//!
//! `POST /api/v1/webhooks/github` is exempt from bearer-token auth, because
//! Gitea and GitHub cannot present one. The HMAC signature is therefore the
//! *only* thing standing between the internet and a redeploy, and everything
//! here is written to fail closed: a missing, empty or wrong secret rejects.

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::webhook::WebhookConfig;

type HmacSha256 = Hmac<Sha256>;

/// Error for a registration that carries no usable secret.
pub(crate) const SECRET_REQUIRED: &str = "a webhook secret is required: pushes are \
authenticated with HMAC-SHA256 over the body (X-Hub-Signature-256). \
`orca webhooks add` generates one when --secret is omitted";

impl WebhookConfig {
    /// The HMAC secret, if one is actually configured.
    ///
    /// An empty or whitespace-only secret is treated as absent: anyone can
    /// compute an HMAC with an empty key, so it authenticates nothing.
    pub fn effective_secret(&self) -> Option<&str> {
        self.secret.as_deref().filter(|s| !s.trim().is_empty())
    }
}

/// Validate an HMAC-SHA256 signature from the `X-Hub-Signature-256` header.
///
/// The comparison is constant-time (`Mac::verify_slice`).
pub(crate) fn validate_signature(secret: &str, body: &[u8], signature_header: &str) -> bool {
    let Some(hex_sig) = signature_header.strip_prefix("sha256=") else {
        return false;
    };
    let Ok(expected) = hex::decode(hex_sig) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

/// The first eight characters of a commit id, for logs and invocation records.
///
/// `commit_id` comes straight from an unauthenticated request body, so it may
/// hold arbitrary UTF-8. Byte-slicing it (`&id[..8]`) panics whenever byte 8
/// lands inside a multi-byte character — and that happened before any webhook
/// lookup or signature check, so anyone could trigger it. This cuts on a
/// character boundary instead.
pub(crate) fn short_sha(commit_id: &str) -> &str {
    match commit_id.char_indices().nth(8) {
        Some((idx, _)) => &commit_id[..idx],
        None => commit_id,
    }
}

#[cfg(test)]
#[path = "webhook_auth_tests.rs"]
mod tests;
