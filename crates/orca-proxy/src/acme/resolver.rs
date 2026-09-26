//! SNI-based dynamic certificate resolver for multi-domain TLS.
//!
//! Allows hot-adding certificates at runtime when new domains are deployed.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use tracing::{debug, warn};

/// Distinct unknown hostnames to warn about before going quiet (scanners
/// send random names).
const WARN_LIMIT: usize = 256;

/// Thread-safe certificate store that resolves certs by SNI hostname.
///
/// New certs can be added at runtime without restarting the TLS listener.
#[derive(Clone, Debug, Default)]
pub struct DynCertResolver {
    certs: Arc<RwLock<HashMap<String, Arc<CertifiedKey>>>>,
    /// Served when the client sends no SNI, an IP address, or a name with no
    /// certificate (#206). Without it the handshake failed below HTTP: the
    /// client saw `ERR_SSL_PROTOCOL_ERROR`, never an error page.
    fallback: Option<Arc<CertifiedKey>>,
    warned: Arc<Mutex<HashSet<String>>>,
}

impl DynCertResolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// A resolver that answers every handshake, using a self-signed
    /// certificate when it has none for the requested name. The client then
    /// gets a certificate warning and, past it, orca's error page, instead
    /// of a dead connection.
    pub fn with_fallback() -> anyhow::Result<Self> {
        let cert = rcgen::generate_simple_self_signed(vec!["orca-fallback.invalid".into()])?;
        let key_der = rustls::pki_types::PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der());
        let signing_key = rustls::crypto::aws_lc_rs::sign::any_supported_type(&key_der.into())?;
        Ok(Self {
            fallback: Some(Arc::new(CertifiedKey::new(
                vec![cert.cert.der().clone()],
                signing_key,
            ))),
            ..Self::default()
        })
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
        let Some(sni) = client_hello.server_name() else {
            // No SNI, or an IP address (which rustls discards): scanners,
            // monitoring probes against the bare IP.
            debug!("TLS handshake without a usable SNI; using the fallback certificate");
            return self.fallback.clone();
        };
        if let Some(key) = self.certs.read().expect("cert store poisoned").get(sni) {
            return Some(key.clone());
        }
        // A hostname with no certificate: often a domain whose certificate
        // never provisioned, so say so (once per name) instead of at debug.
        let mut warned = self.warned.lock().unwrap_or_else(|e| e.into_inner());
        if warned.len() < WARN_LIMIT && warned.insert(sni.to_string()) {
            warn!(
                sni,
                "no certificate for this hostname; serving the fallback certificate"
            );
        }
        self.fallback.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::DynCertResolver;

    /// A TLS client talking to a server that uses `resolver`, with SNI `sni`
    /// (`None`: no SNI at all). Returns whether the handshake completed.
    async fn handshake(resolver: DynCertResolver, sni: Option<&str>) -> bool {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(resolver));
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let _ = acceptor.accept(tcp).await;
        });

        let mut client = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnything))
            .with_no_client_auth();
        client.enable_sni = sni.is_some();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(client));
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let name = rustls::pki_types::ServerName::try_from(sni.unwrap_or("unused.invalid"))
            .unwrap()
            .to_owned();
        connector.connect(name, tcp).await.is_ok()
    }

    /// #206: no SNI, or a name without a certificate, used to kill the
    /// handshake before any HTTP was spoken.
    #[tokio::test]
    async fn the_fallback_answers_unknown_names_and_missing_sni() {
        assert!(!handshake(DynCertResolver::new(), Some("cloud.example.com")).await);
        let fallback = DynCertResolver::with_fallback().unwrap();
        assert!(handshake(fallback.clone(), Some("cloud.example.com")).await);
        assert!(handshake(fallback, None).await);
    }

    #[derive(Debug)]
    struct AcceptAnything;

    impl rustls::client::danger::ServerCertVerifier for AcceptAnything {
        fn verify_server_cert(
            &self,
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &[rustls::pki_types::CertificateDer<'_>],
            _: &rustls::pki_types::ServerName<'_>,
            _: &[u8],
            _: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            rustls::crypto::aws_lc_rs::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }
}
