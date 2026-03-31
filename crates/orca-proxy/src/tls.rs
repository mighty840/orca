//! TLS configuration for the reverse proxy.
//!
//! Supports: self-signed certs (auto-generated), user-provided certs,
//! and ACME/Let's Encrypt (future).

use std::sync::Arc;

use rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;
use tracing::info;

/// TLS mode for the proxy.
#[derive(Debug, Clone)]
pub enum TlsMode {
    /// No TLS (HTTP only).
    None,
    /// Auto-generated self-signed certificate.
    SelfSigned,
    /// User-provided certificate and key files.
    Custom { cert_path: String, key_path: String },
}

/// Create a TLS acceptor based on the configured mode.
pub fn create_tls_acceptor(mode: &TlsMode) -> anyhow::Result<Option<TlsAcceptor>> {
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
    }
}
