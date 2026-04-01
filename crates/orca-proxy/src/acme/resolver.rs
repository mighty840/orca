//! SNI-based dynamic certificate resolver for multi-domain TLS.
//!
//! Allows hot-adding certificates at runtime when new domains are deployed.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use tracing::debug;

/// Thread-safe certificate store that resolves certs by SNI hostname.
///
/// New certs can be added at runtime without restarting the TLS listener.
#[derive(Clone, Debug, Default)]
pub struct DynCertResolver {
    certs: Arc<RwLock<HashMap<String, Arc<CertifiedKey>>>>,
}

impl DynCertResolver {
    pub fn new() -> Self {
        Self {
            certs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add or replace a certificate for a domain.
    pub fn add_cert(&self, domain: &str, key: Arc<CertifiedKey>) {
        self.certs
            .write()
            .expect("cert store poisoned")
            .insert(domain.to_string(), key);
    }

    /// Check if a cert exists for the given domain.
    pub fn has_cert(&self, domain: &str) -> bool {
        self.certs
            .read()
            .expect("cert store poisoned")
            .contains_key(domain)
    }
}

impl ResolvesServerCert for DynCertResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let sni = client_hello.server_name()?;
        let certs = self.certs.read().expect("cert store poisoned");
        let key = certs.get(sni).cloned();
        if key.is_none() {
            debug!(sni, "No cert for SNI hostname");
        }
        key
    }
}
