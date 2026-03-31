//! Certificate loading and expiry checking utilities.

use std::path::Path;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};

/// Load PEM-encoded certificate chain and private key from disk.
pub(crate) fn load_pem_certs(
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
pub(crate) fn check_cert_expiry(cert_path: &Path) -> anyhow::Result<i64> {
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
