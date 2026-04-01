//! ACME certificate management with automatic Let's Encrypt provisioning.
//!
//! Uses `instant-acme` for native Rust ACME (RFC 8555) support — no certbot
//! dependency. Certificates are cached at `~/.orca/certs/`.

pub(crate) mod certs;
mod provider;
mod resolver;

pub use provider::AcmeProvider;
pub use resolver::DynCertResolver;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;
use tokio_rustls::TlsAcceptor;
use tracing::{info, warn};

/// Days before expiry to trigger renewal.
const RENEWAL_THRESHOLD_DAYS: i64 = 30;

/// Manages ACME HTTP-01 challenges and certificate loading.
#[derive(Clone)]
pub struct AcmeManager {
    pub acme_email: String,
    pub cache_dir: PathBuf,
    challenges: Arc<RwLock<HashMap<String, String>>>,
    domains: Arc<RwLock<HashSet<String>>>,
    /// Semaphore ensuring only one ACME order is in-flight at a time.
    provision_lock: Arc<tokio::sync::Semaphore>,
}

impl AcmeManager {
    pub fn new(email: impl Into<String>, cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            acme_email: email.into(),
            cache_dir: cache_dir.into(),
            challenges: Arc::new(RwLock::new(HashMap::new())),
            domains: Arc::new(RwLock::new(HashSet::new())),
            provision_lock: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }

    /// Create a manager with default cache directory (`~/.orca/certs/`).
    pub fn with_default_cache(email: impl Into<String>) -> Self {
        let cache_dir = default_orca_dir().join("certs");
        Self::new(email, cache_dir)
    }

    /// Register a domain for certificate provisioning.
    pub async fn add_domain(&self, domain: impl Into<String>) {
        let domain = domain.into();
        info!(domain = %domain, "Registered domain for ACME");
        self.domains.write().await.insert(domain);
    }

    /// Store a challenge token and its authorization response.
    pub async fn set_challenge(&self, token: String, authorization: String) {
        self.challenges.write().await.insert(token, authorization);
    }

    /// Get the authorization response for an HTTP-01 challenge token.
    pub async fn get_challenge_response(&self, token: &str) -> Option<String> {
        self.challenges.read().await.get(token).cloned()
    }

    /// Remove a completed challenge.
    pub async fn clear_challenge(&self, token: &str) {
        self.challenges.write().await.remove(token);
    }

    /// Load certificates from cache. Returns `None` if missing or unparseable.
    pub fn load_cached_certs(
        &self,
        domain: &str,
    ) -> Option<(
        Vec<rustls::pki_types::CertificateDer<'static>>,
        rustls::pki_types::PrivateKeyDer<'static>,
    )> {
        let cert_path = self.cert_path(domain);
        let key_path = self.key_path(domain);
        if !cert_path.exists() || !key_path.exists() {
            return None;
        }
        match certs::load_pem_certs(&cert_path, &key_path) {
            Ok(pair) => Some(pair),
            Err(e) => {
                warn!(domain, error = %e, "Failed to load cached certs");
                None
            }
        }
    }

    /// Returns `true` if certs are missing or expiring within 30 days.
    pub fn needs_renewal(&self, domain: &str) -> bool {
        let cert_path = self.cert_path(domain);
        if !cert_path.exists() {
            return true;
        }
        match certs::check_cert_expiry(&cert_path) {
            Ok(days) if days >= RENEWAL_THRESHOLD_DAYS => false,
            Ok(days) => {
                info!(domain, days_remaining = days, "Certificate expiring soon");
                true
            }
            Err(e) => {
                warn!(domain, error = %e, "Cannot check cert expiry");
                true
            }
        }
    }

    /// Build a `TlsAcceptor` from cached certs for the given domain.
    pub fn tls_acceptor_for(&self, domain: &str) -> anyhow::Result<Option<TlsAcceptor>> {
        let Some((certs, key)) = self.load_cached_certs(domain) else {
            return Ok(None);
        };
        if self.needs_renewal(domain) {
            warn!(domain, "Cert expiring soon — will auto-renew");
        }
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)?;
        Ok(Some(TlsAcceptor::from(Arc::new(config))))
    }

    /// Provision a cert for a domain and add it to the dynamic resolver.
    ///
    /// If a valid cached cert exists, it's loaded instead of re-provisioning.
    /// This is the hot-provisioning entry point called during `orca deploy`.
    pub async fn ensure_cert_for_resolver(
        &self,
        domain: &str,
        resolver: &DynCertResolver,
    ) -> anyhow::Result<()> {
        if resolver.has_cert(domain) && !self.needs_renewal(domain) {
            return Ok(());
        }

        // Acquire the provision lock to serialize ACME orders.
        // Concurrent orders to the same ACME provider can fail.
        let _permit = self
            .provision_lock
            .acquire()
            .await
            .map_err(|e| anyhow::anyhow!("ACME provision lock closed: {e}"))?;

        // Re-check after acquiring lock (another task may have provisioned it)
        if resolver.has_cert(domain) && !self.needs_renewal(domain) {
            return Ok(());
        }

        let provider = self.provider();
        let cert_path = self.cert_path(domain);
        let key_path = self.key_path(domain);

        // Try cache first
        let (cert_pem, key_pem) =
            if cert_path.exists() && key_path.exists() && !self.needs_renewal(domain) {
                info!(domain, "Loading cached cert for hot provisioning");
                (std::fs::read(&cert_path)?, std::fs::read(&key_path)?)
            } else {
                info!(domain, "Hot-provisioning TLS certificate");
                provider.provision_cert(domain).await?
            };

        let certified_key = Self::build_certified_key(&cert_pem, &key_pem)?;
        resolver.add_cert(domain, Arc::new(certified_key));
        info!(domain, "Certificate ready (hot-provisioned)");
        Ok(())
    }

    /// Build a `CertifiedKey` from PEM bytes.
    fn build_certified_key(
        cert_pem: &[u8],
        key_pem: &[u8],
    ) -> anyhow::Result<rustls::sign::CertifiedKey> {
        let certs: Vec<_> =
            rustls_pemfile::certs(&mut &cert_pem[..]).collect::<Result<Vec<_>, _>>()?;
        let key = rustls_pemfile::private_key(&mut &key_pem[..])?
            .ok_or_else(|| anyhow::anyhow!("no private key in PEM data"))?;
        let signing_key = rustls::crypto::aws_lc_rs::sign::any_supported_type(&key)?;
        Ok(rustls::sign::CertifiedKey::new(certs, signing_key))
    }

    pub fn cert_path(&self, domain: &str) -> PathBuf {
        self.cache_dir.join(format!("{domain}.cert.pem"))
    }

    pub fn key_path(&self, domain: &str) -> PathBuf {
        self.cache_dir.join(format!("{domain}.key.pem"))
    }

    pub async fn domains(&self) -> Vec<String> {
        self.domains.read().await.iter().cloned().collect()
    }

    /// Build an `AcmeProvider` from this manager for cert provisioning.
    pub fn provider(&self) -> AcmeProvider {
        AcmeProvider::new(
            self.acme_email.clone(),
            self.cache_dir.clone(),
            self.challenges.clone(),
        )
    }
}

pub(crate) fn default_orca_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".orca")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_challenge_lifecycle() {
        let mgr = AcmeManager::new("test@example.com", "/tmp/orca-test-certs");
        assert!(mgr.get_challenge_response("tok1").await.is_none());
        mgr.set_challenge("tok1".into(), "auth1".into()).await;
        assert_eq!(mgr.get_challenge_response("tok1").await.unwrap(), "auth1");
        mgr.clear_challenge("tok1").await;
        assert!(mgr.get_challenge_response("tok1").await.is_none());
    }

    #[tokio::test]
    async fn test_domain_registration() {
        let mgr = AcmeManager::new("test@example.com", "/tmp/orca-test-certs");
        mgr.add_domain("example.com").await;
        assert!(mgr.domains().await.contains(&"example.com".to_string()));
    }

    #[test]
    fn test_cert_paths() {
        let mgr = AcmeManager::new("test@example.com", "/tmp/certs");
        assert_eq!(
            mgr.cert_path("example.com"),
            PathBuf::from("/tmp/certs/example.com.cert.pem")
        );
        assert_eq!(
            mgr.key_path("example.com"),
            PathBuf::from("/tmp/certs/example.com.key.pem")
        );
    }

    #[test]
    fn test_missing_certs_needs_renewal() {
        let mgr = AcmeManager::new("test@example.com", "/tmp/nonexistent-certs");
        assert!(mgr.needs_renewal("example.com"));
    }
}
