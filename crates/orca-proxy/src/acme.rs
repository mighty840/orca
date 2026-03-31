//! ACME HTTP-01 challenge serving and certificate management.
//!
//! `AcmeManager` serves `/.well-known/acme-challenge/` responses, loads cached
//! TLS certs from disk, and delegates provisioning to `certbot` via subprocess.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
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
}

impl AcmeManager {
    pub fn new(email: impl Into<String>, cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            acme_email: email.into(),
            cache_dir: cache_dir.into(),
            challenges: Arc::new(RwLock::new(HashMap::new())),
            domains: Arc::new(RwLock::new(HashSet::new())),
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
    ) -> Option<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
        let cert_path = self.cert_path(domain);
        let key_path = self.key_path(domain);
        if !cert_path.exists() || !key_path.exists() {
            return None;
        }
        match load_pem_certs(&cert_path, &key_path) {
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
        match check_cert_expiry(&cert_path) {
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
            warn!(
                domain,
                "No cached certs — run `orca certs provision {domain}`"
            );
            return Ok(None);
        };
        if self.needs_renewal(domain) {
            warn!(
                domain,
                "Cert expiring soon — run `orca certs provision {domain}`"
            );
        }
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)?;
        Ok(Some(TlsAcceptor::from(Arc::new(config))))
    }

    /// Provision a certificate via certbot subprocess.
    ///
    /// The proxy must be serving HTTP on port 80 for challenge validation.
    pub async fn provision_with_certbot(&self, domain: &str) -> anyhow::Result<()> {
        info!(domain, "Starting certbot provisioning");
        tokio::fs::create_dir_all("/tmp/orca-acme").await?;

        let output = tokio::process::Command::new("certbot")
            .args([
                "certonly",
                "--webroot",
                "-w",
                "/tmp/orca-acme",
                "--domain",
                domain,
                "--email",
                &self.acme_email,
                "--agree-tos",
                "--non-interactive",
            ])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("certbot failed for {domain}: {stderr}");
        }

        // Copy certs from certbot output to our cache
        let le_dir = PathBuf::from(format!("/etc/letsencrypt/live/{domain}"));
        tokio::fs::create_dir_all(&self.cache_dir).await?;
        tokio::fs::copy(le_dir.join("fullchain.pem"), self.cert_path(domain)).await?;
        tokio::fs::copy(le_dir.join("privkey.pem"), self.key_path(domain)).await?;
        info!(domain, cache_dir = ?self.cache_dir, "Certs provisioned and cached");
        Ok(())
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
}

fn load_pem_certs(
    cert_path: &Path,
    key_path: &Path,
) -> anyhow::Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let cert_data = std::fs::read(cert_path)?;
    let key_data = std::fs::read(key_path)?;
    let certs = rustls_pemfile::certs(&mut cert_data.as_slice()).collect::<Result<Vec<_>, _>>()?;
    let key = rustls_pemfile::private_key(&mut key_data.as_slice())?
        .ok_or_else(|| anyhow::anyhow!("no private key in {}", key_path.display()))?;
    Ok((certs, key))
}

/// Estimate days until cert expires using file mtime + 90-day LE default.
fn check_cert_expiry(cert_path: &Path) -> anyhow::Result<i64> {
    let metadata = std::fs::metadata(cert_path)?;
    let modified = metadata.modified()?;
    let age = modified.elapsed().unwrap_or_default();
    let ninety_days = std::time::Duration::from_secs(90 * 24 * 60 * 60);
    if age > ninety_days {
        Ok(0)
    } else {
        let remaining = ninety_days.saturating_sub(age);
        Ok((remaining.as_secs() / (24 * 60 * 60)) as i64)
    }
}

fn default_orca_dir() -> PathBuf {
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
