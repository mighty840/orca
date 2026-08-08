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

/// Days until the certificate expires, parsed from the leaf cert's
/// `NotAfter`. Negative when already expired.
///
/// Never estimated from file metadata: a cert that is copied, restored from
/// backup, or migrated between nodes gets a fresh mtime, so an mtime-based
/// estimate makes months-old certs look freshly issued and renewal fires
/// after the cert has already expired while serving.
pub(crate) fn check_cert_expiry(cert_path: &Path) -> anyhow::Result<i64> {
    let pem = std::fs::read(cert_path)?;
    // The chain starts with the end-entity cert (leaf first, per RFC 8555).
    let cert = rustls_pemfile::certs(&mut pem.as_slice())
        .next()
        .ok_or_else(|| anyhow::anyhow!("no certificate in {}", cert_path.display()))??;
    let (_, parsed) = x509_parser::parse_x509_certificate(&cert)
        .map_err(|e| anyhow::anyhow!("cannot parse certificate {}: {e}", cert_path.display()))?;
    let not_after = parsed.validity().not_after.timestamp();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    Ok((not_after - now).div_euclid(24 * 60 * 60))
}

/// Mint a self-signed cert PEM expiring `days` from now (negative = expired)
/// and write it into `dir`. Test-only helper shared by the acme test modules.
#[cfg(test)]
pub(crate) fn mint_cert_expiring_in(dir: &Path, days: i64) -> std::path::PathBuf {
    let mut params =
        rcgen::CertificateParams::new(vec!["test.example.com".into()]).expect("valid cert params");
    params.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(days);
    let key = rcgen::KeyPair::generate().expect("keygen");
    let cert = params.self_signed(&key).expect("self-signed cert");
    let path = dir.join("cert.pem");
    std::fs::write(&path, cert.pem()).expect("write cert");
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A freshly WRITTEN file whose certificate expires in 10 days must
    /// report ~10 days — not 90. Certs that are copied, restored from
    /// backup, or migrated between nodes get a fresh mtime; expiry must
    /// come from the certificate's NotAfter, or renewal fires months late.
    #[test]
    fn expiry_comes_from_not_after_not_mtime() {
        let tmp = TempDir::new().unwrap();
        let path = mint_cert_expiring_in(tmp.path(), 10);
        let days = check_cert_expiry(&path).unwrap();
        assert!(
            (9..=10).contains(&days),
            "expected ~10 days from NotAfter, got {days} (mtime-based estimate?)"
        );
    }

    /// An already-expired certificate must report non-positive days even
    /// though the file was just written.
    #[test]
    fn expired_cert_reports_nonpositive_days() {
        let tmp = TempDir::new().unwrap();
        let path = mint_cert_expiring_in(tmp.path(), -2);
        let days = check_cert_expiry(&path).unwrap();
        assert!(days <= 0, "expired cert must report <= 0 days, got {days}");
    }

    /// Unparseable cert data must error (callers treat that as needs-renewal).
    #[test]
    fn garbage_cert_data_errors() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("cert.pem");
        std::fs::write(&path, b"not a certificate").unwrap();
        assert!(check_cert_expiry(&path).is_err());
    }
}
