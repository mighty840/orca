//! TLS cert provisioning must run on every local reconcile path.
//!
//! Regression tests for the production incident where `orca deploy` of a
//! service with a new `domain` added the HTTP route but never registered the
//! domain for ACME or the SNI resolver — HTTPS failed until an agent restart.
//!
//! Tests seed the resolver with self-signed certs so `ensure_cert_for_resolver`
//! short-circuits on `has_cert` and no real ACME order is attempted. The
//! observable is `AcmeManager::domains()`: registration proves the cert
//! provisioning block ran on that reconcile path (and is what the renewal
//! task's sweep + fast-retry loop iterate).

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use orca_control::reconciler::{load_byo_cert, reconcile};
use orca_control::state::AppState;
use orca_core::config::{ClusterConfig, ServiceConfig};
use orca_core::testing::MockRuntime;
use orca_proxy::acme::{AcmeManager, DynCertResolver};

fn svc(name: &str, domain: &str) -> ServiceConfig {
    serde_json::from_value(serde_json::json!({
        "name": name,
        "image": "nginx:latest",
        "replicas": 1,
        "port": 8080,
        "domain": domain,
    }))
    .expect("valid service config")
}

fn state_with_acme(
    certs_dir: &std::path::Path,
) -> (Arc<AppState>, AcmeManager, Arc<DynCertResolver>) {
    let acme = AcmeManager::new("ops@example.com", certs_dir);
    let resolver = Arc::new(DynCertResolver::new());
    let state = AppState::new(
        ClusterConfig::default(),
        Arc::new(MockRuntime::new()),
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    )
    .with_acme(acme.clone(), resolver.clone());
    (Arc::new(state), acme, resolver)
}

/// Add a self-signed cert for `domain` so no real ACME order is attempted.
fn seed_cert(resolver: &DynCertResolver, domain: &str) {
    let dir = tempfile::tempdir().unwrap();
    let c = rcgen::generate_simple_self_signed(vec![domain.to_string()]).unwrap();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, c.cert.pem()).unwrap();
    std::fs::write(&key_path, c.key_pair.serialize_pem()).unwrap();
    let key = load_byo_cert(cert_path.to_str().unwrap(), key_path.to_str().unwrap()).unwrap();
    resolver.add_cert(domain, Arc::new(key));
}

/// Fresh deploy of a service with a domain must register that domain with the
/// ACME manager — otherwise the renewal task never sees it and the cert
/// silently expires at day ~90.
#[tokio::test]
async fn fresh_deploy_registers_domain_for_renewal() {
    let dir = tempfile::tempdir().unwrap();
    let (state, acme, resolver) = state_with_acme(dir.path());
    seed_cert(&resolver, "app.example.com");

    let (deployed, errors) = reconcile(&state, &[svc("web", "app.example.com")]).await;
    assert_eq!(deployed, vec!["web".to_string()]);
    assert!(errors.is_empty(), "deploy errors: {errors:?}");

    assert!(
        acme.domains()
            .await
            .contains(&"app.example.com".to_string()),
        "fresh deploy must register the domain for renewal/retry"
    );
}

/// The incident path: a *running* service gets a new domain. That reconcile
/// takes the rolling-update branch, which returned before the cert
/// provisioning block — route added, no cert, HTTPS down until restart.
#[tokio::test(start_paused = true)]
async fn domain_change_on_running_service_provisions_cert() {
    let dir = tempfile::tempdir().unwrap();
    let (state, acme, resolver) = state_with_acme(dir.path());
    seed_cert(&resolver, "old.example.com");
    seed_cert(&resolver, "new.example.com");

    let (_, errors) = reconcile(&state, &[svc("web", "old.example.com")]).await;
    assert!(errors.is_empty(), "initial deploy errors: {errors:?}");

    // Same service, changed domain → rolling update path.
    let (_, errors) = reconcile(&state, &[svc("web", "new.example.com")]).await;
    assert!(errors.is_empty(), "rolling update errors: {errors:?}");

    assert!(
        acme.domains()
            .await
            .contains(&"new.example.com".to_string()),
        "a domain added to a running service must be registered for ACME"
    );
}

/// The same-spec fast path must also run cert provisioning: if the initial
/// provision failed (e.g. DNS not yet propagated), a redeploy of the identical
/// spec is the operator's natural retry — it must not skip certs.
#[tokio::test]
async fn same_spec_reconcile_registers_domain() {
    let dir = tempfile::tempdir().unwrap();
    let (state, acme, resolver) = state_with_acme(dir.path());
    seed_cert(&resolver, "a.example.com");
    seed_cert(&resolver, "b.example.com");

    let (_, errors) = reconcile(&state, &[svc("web", "a.example.com")]).await;
    assert!(errors.is_empty(), "initial deploy errors: {errors:?}");

    // Force the stored config to match the incoming one so the reconcile
    // below takes the same-spec early-return path, with a domain the ACME
    // manager has not seen yet.
    state
        .services
        .write()
        .await
        .get_mut("web")
        .expect("service exists")
        .config
        .domain = Some("b.example.com".to_string());

    let (_, errors) = reconcile(&state, &[svc("web", "b.example.com")]).await;
    assert!(errors.is_empty(), "same-spec reconcile errors: {errors:?}");

    assert!(
        acme.domains().await.contains(&"b.example.com".to_string()),
        "same-spec reconcile must still register domains for renewal/retry"
    );
}

/// BYO-cert services must load their cert into the resolver but never be
/// registered with the ACME manager — no Let's Encrypt orders for domains
/// the operator provides certs for.
#[tokio::test]
async fn byo_service_not_registered_for_acme() {
    let dir = tempfile::tempdir().unwrap();
    let (state, acme, resolver) = state_with_acme(dir.path());

    let c = rcgen::generate_simple_self_signed(vec!["byo.example.com".into()]).unwrap();
    let cert_path = dir.path().join("byo.cert.pem");
    let key_path = dir.path().join("byo.key.pem");
    std::fs::write(&cert_path, c.cert.pem()).unwrap();
    std::fs::write(&key_path, c.key_pair.serialize_pem()).unwrap();

    let mut config = svc("byo", "byo.example.com");
    config.tls_cert = Some(cert_path.to_str().unwrap().to_string());
    config.tls_key = Some(key_path.to_str().unwrap().to_string());

    let (_, errors) = reconcile(&state, &[config]).await;
    assert!(errors.is_empty(), "deploy errors: {errors:?}");

    assert!(
        resolver.has_cert("byo.example.com"),
        "BYO cert must be loaded into the resolver"
    );
    assert!(
        !acme
            .domains()
            .await
            .contains(&"byo.example.com".to_string()),
        "BYO domains must not be registered for ACME"
    );
}
