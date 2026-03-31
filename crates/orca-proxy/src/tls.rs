//! TLS configuration for the reverse proxy.
//!
//! Supports: self-signed certs (auto-generated), user-provided certs,
//! and ACME/Let's Encrypt via `instant-acme` (zero-config auto-TLS).

use std::path::PathBuf;
use std::sync::Arc;

use rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;
use tracing::info;

use crate::acme::AcmeManager;

/// TLS mode for the proxy.
#[derive(Debug, Clone)]
pub enum TlsMode {
    /// No TLS (HTTP only).
    None,
    /// Auto-generated self-signed certificate.
    SelfSigned,
    /// User-provided certificate and key files.
    Custom { cert_path: String, key_path: String },
    /// ACME/Let's Encrypt via `instant-acme` — fully automatic.
    ///
    /// The proxy serves HTTP-01 challenges on port 80 and provisions certs
    /// automatically when a domain is configured.
    Acme {
        /// Email for the Let's Encrypt account registration.
        email: String,
        /// Directory to cache provisioned certificates.
        /// Defaults to `~/.orca/certs/` if not specified.
        cache_dir: Option<PathBuf>,
    },
}

/// Create a TLS acceptor based on the configured mode.
///
/// For `TlsMode::Acme`, pass the primary `domain` to load certs for.
/// Returns `None` if no certs are cached yet (they will be provisioned
/// automatically when the proxy starts).
pub fn create_tls_acceptor(mode: &TlsMode) -> anyhow::Result<Option<TlsAcceptor>> {
    create_tls_acceptor_for_domain(mode, None)
}

/// Create a TLS acceptor, optionally for a specific ACME domain.
pub fn create_tls_acceptor_for_domain(
    mode: &TlsMode,
    domain: Option<&str>,
) -> anyhow::Result<Option<TlsAcceptor>> {
    match mode {
        TlsMode::None => Ok(None),
        TlsMode::SelfSigned => {
            info!("Generating self-signed TLS certificate");
            let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()])?;
            let cert_der = cert.cert.der().clone();
            let key_der = cert.key_pair.serialize_der();

            let certs = vec![cert_der];
            let key = rustls::pki_types::PrivatePkcs8KeyDer::from(key_der).into();

            let config = ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certs, key)?;

            Ok(Some(TlsAcceptor::from(Arc::new(config))))
        }
        TlsMode::Custom {
            cert_path,
            key_path,
        } => {
            info!("Loading TLS certificate from {cert_path}");
            let cert_file = std::fs::read(cert_path)?;
            let key_file = std::fs::read(key_path)?;

            let certs =
                rustls_pemfile::certs(&mut cert_file.as_slice()).collect::<Result<Vec<_>, _>>()?;
            let key = rustls_pemfile::private_key(&mut key_file.as_slice())?
                .ok_or_else(|| anyhow::anyhow!("no private key found in {key_path}"))?;

            let config = ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certs, key)?;

            Ok(Some(TlsAcceptor::from(Arc::new(config))))
        }
        TlsMode::Acme { email, cache_dir } => {
            let cache = cache_dir.clone().unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".orca/certs")
            });
            let manager = AcmeManager::new(email.clone(), cache);

            let Some(domain) = domain else {
                // No domain specified — ACME will auto-provision when domains
                // are registered and the proxy starts.
                info!("ACME mode: certs will be auto-provisioned on startup");
                return Ok(None);
            };

            match manager.tls_acceptor_for(domain)? {
                Some(acceptor) => {
                    info!(domain, "Loaded cached ACME certificate");
                    Ok(Some(acceptor))
                }
                None => {
                    info!(
                        domain,
                        "No cached ACME cert — will auto-provision on startup"
                    );
                    Ok(None)
                }
            }
        }
    }
}
