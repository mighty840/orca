//! Background task for automatic ACME certificate renewal.
//!
//! Runs every 24 hours, checks all cached certificates for expiry,
//! and re-provisions via ACME if a cert expires within 30 days.

use std::time::Duration;

use tracing::{error, info, warn};

use super::AcmeManager;
use crate::SharedCertResolver;

/// Spawn a background task that periodically checks and renews expiring certs.
///
/// Runs every 24 hours. For each domain registered with the ACME manager,
/// checks if the certificate needs renewal (expires within 30 days) and
/// re-provisions it via Let's Encrypt if needed.
pub fn spawn_renewal_task(manager: AcmeManager, resolver: SharedCertResolver) {
    tokio::spawn(async move {
        info!("ACME renewal task started (24h interval)");
        loop {
            tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
            check_and_renew(&manager, &resolver).await;
        }
    });
}

/// Check all registered domains and renew expiring certificates.
async fn check_and_renew(manager: &AcmeManager, resolver: &SharedCertResolver) {
    let domains = manager.domains().await;
    if domains.is_empty() {
        return;
    }

    info!(count = domains.len(), "Checking certificates for renewal");

    for domain in &domains {
        if !manager.needs_renewal(domain) {
            continue;
        }
        info!(domain = %domain, "Certificate needs renewal, re-provisioning");
        match manager.ensure_cert_for_resolver(domain, resolver).await {
            Ok(()) => info!(domain = %domain, "Certificate renewed successfully"),
            Err(e) => error!(domain = %domain, error = %e, "Certificate renewal failed"),
        }
    }
}

/// Check all cert files in the cache directory for expiry, including domains
/// that may not be currently registered (e.g., from a previous server run).
pub async fn check_and_renew_from_cache(manager: &AcmeManager, resolver: &SharedCertResolver) {
    // Also scan the cache dir for cert files from previous runs
    let cache_dir = &manager.cache_dir;
    let entries = match std::fs::read_dir(cache_dir) {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, "Cannot read cert cache directory");
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        // Only process .cert.pem files
        let domain = match name.strip_suffix(".cert.pem") {
            Some(d) => d.to_string(),
            None => continue,
        };

        if !manager.needs_renewal(&domain) {
            continue;
        }
        info!(domain = %domain, "Cached certificate needs renewal");
        // Ensure domain is registered so ACME can provision
        manager.add_domain(&domain).await;
        match manager.ensure_cert_for_resolver(&domain, resolver).await {
            Ok(()) => info!(domain = %domain, "Certificate renewed from cache scan"),
            Err(e) => error!(domain = %domain, error = %e, "Renewal from cache failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, SystemTime};
    use tempfile::TempDir;

    #[test]
    fn test_cert_needs_renewal_when_old() {
        let tmp = TempDir::new().unwrap();
        let mgr = AcmeManager::new("test@example.com", tmp.path());

        // Create a cert file with old modification time (91 days ago)
        let cert_path = mgr.cert_path("old.example.com");
        fs::write(&cert_path, b"fake-cert-data").unwrap();
        let old_time = SystemTime::now() - Duration::from_secs(91 * 24 * 3600);
        filetime::set_file_mtime(&cert_path, filetime::FileTime::from_system_time(old_time))
            .unwrap();

        assert!(mgr.needs_renewal("old.example.com"));
    }

    #[test]
    fn test_cert_ok_when_fresh() {
        let tmp = TempDir::new().unwrap();
        let mgr = AcmeManager::new("test@example.com", tmp.path());

        // Create a cert file with recent modification time (1 day ago)
        let cert_path = mgr.cert_path("fresh.example.com");
        fs::write(&cert_path, b"fake-cert-data").unwrap();
        let recent_time = SystemTime::now() - Duration::from_secs(1 * 24 * 3600);
        filetime::set_file_mtime(
            &cert_path,
            filetime::FileTime::from_system_time(recent_time),
        )
        .unwrap();

        assert!(!mgr.needs_renewal("fresh.example.com"));
    }
}
