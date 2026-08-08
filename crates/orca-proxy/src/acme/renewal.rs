//! Background task for automatic ACME certificate renewal.
//!
//! A single loop ticks every 60 seconds. Each tick fast-retries registered
//! domains that have no certificate in the resolver (a failed or
//! never-attempted provision) on a short backoff — one minute, then five,
//! then fifteen. Every 24 hours a full sweep re-provisions certificates
//! expiring within 30 days.

use std::collections::HashMap;
use std::time::Duration;

use tokio::time::Instant;
use tracing::{error, info, warn};

use super::AcmeManager;
use crate::SharedCertResolver;

/// Interval between fast-retry ticks (also the resolution of the 24h sweep).
const RETRY_TICK: Duration = Duration::from_secs(60);
/// Interval between full renewal sweeps.
const SWEEP_INTERVAL: Duration = Duration::from_secs(24 * 3600);

/// Per-domain retry bookkeeping: consecutive failures and next attempt time.
struct RetryState {
    failures: u32,
    next_attempt: Instant,
}

/// Backoff before retrying a failed provision: 1 min, then 5, then 15 (cap).
///
/// 15 min steady-state stays under Let's Encrypt's failed-validation rate
/// limit (5 per hostname per hour) while turning a bad cutover — domain
/// registered, initial order failed because DNS hadn't propagated — into a
/// self-healing delay instead of a wait-for-tomorrow outage.
fn retry_delay(failures: u32) -> Duration {
    match failures {
        0 | 1 => Duration::from_secs(60),
        2 => Duration::from_secs(5 * 60),
        _ => Duration::from_secs(15 * 60),
    }
}

/// Spawn the background renewal task: fast retry for missing certs every
/// minute (with per-domain backoff) and a full expiry sweep every 24 hours.
pub fn spawn_renewal_task(manager: AcmeManager, resolver: SharedCertResolver) {
    tokio::spawn(async move {
        info!("ACME renewal task started (24h sweep + fast retry for missing certs)");
        let mut retries: HashMap<String, RetryState> = HashMap::new();
        let mut last_sweep = Instant::now();
        loop {
            tokio::time::sleep(RETRY_TICK).await;
            retry_missing_certs(&manager, &resolver, &mut retries).await;
            if last_sweep.elapsed() >= SWEEP_INTERVAL {
                last_sweep = Instant::now();
                check_and_renew(&manager, &resolver).await;
            }
        }
    });
}

/// Provision certs for registered domains that have none in the resolver.
async fn retry_missing_certs(
    manager: &AcmeManager,
    resolver: &SharedCertResolver,
    retries: &mut HashMap<String, RetryState>,
) {
    let now = Instant::now();
    for domain in manager.domains().await {
        if resolver.has_cert(&domain) {
            retries.remove(&domain);
            continue;
        }
        if retries.get(&domain).is_some_and(|r| now < r.next_attempt) {
            continue;
        }
        info!(domain = %domain, "Provisioning missing certificate");
        match manager.ensure_cert_for_resolver(&domain, resolver).await {
            Ok(()) => {
                retries.remove(&domain);
                info!(domain = %domain, "Certificate provisioned");
            }
            Err(e) => {
                let failures = retries.get(&domain).map_or(1, |r| r.failures + 1);
                let delay = retry_delay(failures);
                warn!(
                    domain = %domain,
                    error = %e,
                    retry_in_secs = delay.as_secs(),
                    "Certificate provisioning failed, will retry"
                );
                retries.insert(
                    domain,
                    RetryState {
                        failures,
                        next_attempt: now + delay,
                    },
                );
            }
        }
    }
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
    fn test_retry_backoff_is_one_five_fifteen_capped() {
        assert_eq!(retry_delay(1), Duration::from_secs(60));
        assert_eq!(retry_delay(2), Duration::from_secs(300));
        assert_eq!(retry_delay(3), Duration::from_secs(900));
        assert_eq!(retry_delay(100), Duration::from_secs(900));
    }

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
        let recent_time = SystemTime::now() - Duration::from_secs(24 * 3600);
        filetime::set_file_mtime(
            &cert_path,
            filetime::FileTime::from_system_time(recent_time),
        )
        .unwrap();

        assert!(!mgr.needs_renewal("fresh.example.com"));
    }
}
