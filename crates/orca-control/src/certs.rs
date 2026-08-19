//! TLS certificate provisioning for reconciled services — BYO cert loading
//! and ACME registration/hot-provisioning.

use orca_core::config::ServiceConfig;

use crate::state::AppState;

/// Load a BYO TLS certificate and key from PEM files.
pub fn load_byo_cert(
    cert_path: &str,
    key_path: &str,
) -> anyhow::Result<rustls::sign::CertifiedKey> {
    let cert_pem = std::fs::read(cert_path)?;
    let key_pem = std::fs::read(key_path)?;
    let certs: Vec<_> =
        rustls_pemfile::certs(&mut cert_pem.as_slice()).collect::<Result<Vec<_>, _>>()?;
    let key = rustls_pemfile::private_key(&mut key_pem.as_slice())?
        .ok_or_else(|| anyhow::anyhow!("no private key in {key_path}"))?;
    let signing_key = rustls::crypto::aws_lc_rs::sign::any_supported_type(&key)?;
    Ok(rustls::sign::CertifiedKey::new(certs, signing_key))
}

/// TLS cert provisioning — one cert per domain (apex + www, dual-TLD, etc.).
/// BYO cert/key, when provided, applies to every listed domain.
///
/// Must run on EVERY local reconcile path — fresh scale-up, rolling/canary
/// update, and the same-spec fast path. A domain change on a running service
/// takes the rolling-update path, and skipping certs there shipped an HTTPS
/// outage: the route existed but no cert was ever provisioned until restart.
/// Failures are logged, not fatal — registration alone is enough for the
/// renewal task's fast-retry loop to pick the domain up.
pub(crate) async fn provision_service_certs(state: &AppState, config: &ServiceConfig) {
    let Some(resolver) = &state.cert_resolver else {
        return;
    };
    for domain in config.all_domains() {
        if let (Some(cert_path), Some(key_path)) = (&config.tls_cert, &config.tls_key) {
            if resolver.has_cert(&domain) {
                continue;
            }
            // BYO cert: load from file
            match load_byo_cert(cert_path, key_path) {
                Ok(key) => {
                    resolver.add_cert(&domain, std::sync::Arc::new(key));
                    tracing::info!(domain, "BYO TLS certificate loaded");
                }
                Err(e) => tracing::error!(domain, "Failed to load BYO cert: {e}"),
            }
        } else if let Some(acme) = &state.acme_manager {
            // Register even when a cert already exists: the renewal task can
            // only renew (and fast-retry) domains it knows about.
            acme.add_domain(&domain).await;
            if resolver.has_cert(&domain) {
                continue;
            }
            // ACME auto-provisioning
            if let Err(e) = acme.ensure_cert_for_resolver(&domain, resolver).await {
                tracing::error!(domain, "Hot cert provisioning failed: {e}");
            }
        }
    }
}
