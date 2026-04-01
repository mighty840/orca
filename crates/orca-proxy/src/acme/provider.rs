//! ACME certificate provisioning via `instant-acme`.
//!
//! Handles account creation/caching, HTTP-01 challenges, CSR generation,
//! and certificate download — all in pure Rust, no certbot needed.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier, LetsEncrypt,
    NewAccount, NewOrder, OrderStatus, RetryPolicy,
};
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::default_orca_dir;

/// Pure-Rust ACME provider backed by `instant-acme`.
#[derive(Clone)]
pub struct AcmeProvider {
    email: String,
    cache_dir: PathBuf,
    challenges: Arc<RwLock<HashMap<String, String>>>,
}

impl AcmeProvider {
    pub fn new(
        email: String,
        cache_dir: PathBuf,
        challenges: Arc<RwLock<HashMap<String, String>>>,
    ) -> Self {
        Self {
            email,
            cache_dir,
            challenges,
        }
    }

    /// Provision a TLS certificate for the given domain via ACME HTTP-01.
    ///
    /// The proxy must be serving HTTP on port 80 so Let's Encrypt can reach
    /// `/.well-known/acme-challenge/{token}`.
    ///
    /// Returns `(cert_pem, key_pem)` as byte vectors.
    pub async fn provision_cert(&self, domain: &str) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        info!(domain, "Starting ACME certificate provisioning");

        let account = self.load_or_create_account().await?;
        let identifiers = vec![Identifier::Dns(domain.to_string())];
        let mut order = account.new_order(&NewOrder::new(&identifiers)).await?;
        debug!(domain, "ACME order created");

        // Process authorizations
        self.handle_authorizations(&mut order).await?;

        // Poll until order is ready for finalization.
        // Challenge tokens remain available until LE validates them.
        let status = order.poll_ready(&RetryPolicy::default()).await?;

        // Clean up challenge tokens now that validation is complete
        self.challenges.write().await.clear();

        if status != OrderStatus::Ready {
            anyhow::bail!("Order not ready after challenges: {status:?}");
        }
        info!(domain, "ACME order ready, finalizing");

        // Finalize: instant-acme generates the key + CSR internally
        let key_pem = order.finalize().await?;
        let cert_pem = order.poll_certificate(&RetryPolicy::default()).await?;

        // Save to cache
        self.save_cert(domain, cert_pem.as_bytes(), key_pem.as_bytes())
            .await?;

        info!(domain, "Certificate provisioned and cached");
        Ok((cert_pem.into_bytes(), key_pem.into_bytes()))
    }

    /// Process all authorizations for an order, handling HTTP-01 challenges.
    async fn handle_authorizations(&self, order: &mut instant_acme::Order) -> anyhow::Result<()> {
        let mut authorizations = order.authorizations();
        while let Some(result) = authorizations.next().await {
            let mut authz = result?;

            if authz.status == AuthorizationStatus::Valid {
                debug!("Authorization already valid");
                continue;
            }

            let mut challenge = authz
                .challenge(ChallengeType::Http01)
                .ok_or_else(|| anyhow::anyhow!("No HTTP-01 challenge offered"))?;

            let token = challenge.token.clone();
            let key_auth = challenge.key_authorization().as_str().to_string();

            debug!(token = %token, "Serving HTTP-01 challenge");
            self.challenges
                .write()
                .await
                .insert(token.clone(), key_auth);

            challenge.set_ready().await?;

            // Don't remove the token yet — Let's Encrypt needs to hit our
            // /.well-known/acme-challenge/{token} endpoint. The token stays
            // in memory until poll_ready succeeds on the order, then we
            // clean up all challenge tokens.
        }

        Ok(())
    }

    /// Load ACME account from cache or create a new one.
    async fn load_or_create_account(&self) -> anyhow::Result<Account> {
        let account_path = self.account_cache_path();

        if account_path.exists() {
            debug!("Loading cached ACME account");
            let json = tokio::fs::read_to_string(&account_path).await?;
            let creds: AccountCredentials = serde_json::from_str(&json)?;
            let account = Account::builder()?.from_credentials(creds).await?;
            return Ok(account);
        }

        info!(email = %self.email, "Creating new ACME account");
        let contact = format!("mailto:{}", self.email);
        let (account, credentials) = Account::builder()?
            .create(
                &NewAccount {
                    contact: &[&contact],
                    terms_of_service_agreed: true,
                    only_return_existing: false,
                },
                LetsEncrypt::Production.url().to_owned(),
                None,
            )
            .await?;

        // Cache the account credentials
        if let Some(parent) = account_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let json = serde_json::to_string_pretty(&credentials)?;
        tokio::fs::write(&account_path, json).await?;
        info!("ACME account cached at {}", account_path.display());

        Ok(account)
    }

    /// Save provisioned cert and key to the cache directory.
    async fn save_cert(&self, domain: &str, cert_pem: &[u8], key_pem: &[u8]) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(&self.cache_dir).await?;
        let cert_path = self.cache_dir.join(format!("{domain}.cert.pem"));
        let key_path = self.cache_dir.join(format!("{domain}.key.pem"));
        tokio::fs::write(&cert_path, cert_pem).await?;
        tokio::fs::write(&key_path, key_pem).await?;
        debug!(domain, "Saved cert to {}", cert_path.display());
        Ok(())
    }

    /// Path to the cached ACME account credentials.
    fn account_cache_path(&self) -> PathBuf {
        default_orca_dir().join("acme-account.json")
    }

    /// Ensure a valid cert exists for the domain — load from cache or provision.
    ///
    /// Returns a `TlsAcceptor` ready for use, or an error if provisioning fails.
    pub async fn ensure_cert(&self, domain: &str) -> anyhow::Result<tokio_rustls::TlsAcceptor> {
        // Check cache first
        let cert_path = self.cache_dir.join(format!("{domain}.cert.pem"));
        let key_path = self.cache_dir.join(format!("{domain}.key.pem"));

        if cert_path.exists()
            && key_path.exists()
            && let Ok(days) = super::certs::check_cert_expiry(&cert_path)
        {
            if days >= super::RENEWAL_THRESHOLD_DAYS {
                debug!(domain, days_remaining = days, "Using cached cert");
                return self.build_acceptor(&cert_path, &key_path);
            }
            info!(domain, days_remaining = days, "Cert expiring, renewing");
        }

        // Provision new cert
        let (cert_pem, key_pem) = self.provision_cert(domain).await?;
        self.build_acceptor_from_pem(&cert_pem, &key_pem)
    }

    /// Build a TlsAcceptor from PEM files on disk.
    fn build_acceptor(
        &self,
        cert_path: &std::path::Path,
        key_path: &std::path::Path,
    ) -> anyhow::Result<tokio_rustls::TlsAcceptor> {
        let (certs, key) = super::certs::load_pem_certs(cert_path, key_path)?;
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)?;
        Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
    }

    /// Build a TlsAcceptor from in-memory PEM bytes.
    fn build_acceptor_from_pem(
        &self,
        cert_pem: &[u8],
        key_pem: &[u8],
    ) -> anyhow::Result<tokio_rustls::TlsAcceptor> {
        let certs = rustls_pemfile::certs(&mut &cert_pem[..]).collect::<Result<Vec<_>, _>>()?;
        let key = rustls_pemfile::private_key(&mut &key_pem[..])?
            .ok_or_else(|| anyhow::anyhow!("no private key in PEM data"))?;
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)?;
        Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
    }
}
