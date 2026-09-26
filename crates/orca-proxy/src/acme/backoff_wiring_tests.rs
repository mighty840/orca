//! #188: a domain in backoff gets no new ACME order from any caller.

use super::*;

#[tokio::test]
async fn a_domain_in_backoff_is_not_ordered_again() {
    let cache = tempfile::tempdir().unwrap();
    let manager = AcmeManager::new("ops@example.com", cache.path());
    let resolver = DynCertResolver::new();
    manager.backoff.lock().unwrap().failed(
        "dead.example.com",
        "Order not ready after challenges: Invalid",
    );

    // No network is involved: the backoff stops it before any ACME call.
    let err = manager
        .ensure_cert_for_resolver("dead.example.com", &resolver)
        .await
        .unwrap_err();

    assert!(err.downcast_ref::<AcmeCooldown>().is_some(), "{err:#}");
}
