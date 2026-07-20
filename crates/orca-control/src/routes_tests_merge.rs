//! Regression tests for the shared-domain route merge: several services can
//! claim one domain with disjoint route patterns, and registration must
//! merge per-service targets — never let the last writer wipe siblings.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::state::{AppState, InstanceState, ServiceState};
use orca_core::config::ServiceConfig;
use orca_core::runtime::WorkloadHandle;
use orca_core::types::{HealthState, WorkloadStatus};

fn svc_config(name: &str, routes: Vec<&str>) -> ServiceConfig {
    serde_json::from_value(serde_json::json!({
        "name": name,
        "image": "nginx:latest",
        "replicas": 1,
        "port": 80,
        "domain": "shop.example.com",
        "routes": routes,
    }))
    .unwrap()
}

fn running_instance(port: u16) -> InstanceState {
    InstanceState {
        handle: WorkloadHandle {
            runtime_id: format!("r-{port}"),
            name: format!("n-{port}"),
            metadata: HashMap::new(),
        },
        status: WorkloadStatus::Running,
        host_port: Some(port),
        container_address: None,
        health: HealthState::NoCheck,
        is_canary: false,
        started_at: std::time::Instant::now(),
    }
}

fn test_state() -> Arc<AppState> {
    Arc::new(AppState::new(
        orca_core::config::ClusterConfig::default(),
        Arc::new(orca_core::testing::MockRuntime::new()),
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ))
}

async fn register(state: &AppState, config: &ServiceConfig, ports: &[u16]) {
    let mut services = state.services.write().await;
    let svc = services
        .entry(config.name.clone())
        .or_insert_with(|| ServiceState::from_config(config.clone()));
    svc.instances = ports.iter().map(|p| running_instance(*p)).collect();
    drop(services);
    super::update_container_routes(state, config).await;
}

/// The bug: `insert()` made the last-registered service replace the whole
/// domain entry, 404ing its siblings' path trees on every deploy or health
/// transition. Registration must merge.
#[tokio::test]
async fn shared_domain_services_merge_instead_of_clobber() {
    let state = test_state();
    let storefront = svc_config("storefront", vec![]);
    let admin = svc_config("admin", vec!["/admin/*"]);

    register(&state, &storefront, &[8001]).await;
    register(&state, &admin, &[8002]).await;

    let routes = state.route_table.read().await;
    let targets = routes.get("shop.example.com").expect("domain registered");
    let names: Vec<&str> = targets.iter().map(|t| t.service_name.as_str()).collect();
    assert!(
        names.contains(&"storefront") && names.contains(&"admin"),
        "both services must coexist under the shared domain, got {names:?}"
    );
}

/// Re-registering one service (deploy, health flip) must refresh only its
/// own targets and leave the sibling untouched — and never duplicate itself.
#[tokio::test]
async fn reregistration_preserves_siblings_without_duplicates() {
    let state = test_state();
    let storefront = svc_config("storefront", vec![]);
    let admin = svc_config("admin", vec!["/admin/*"]);
    register(&state, &storefront, &[8001]).await;
    register(&state, &admin, &[8002]).await;

    // Storefront redeploys onto a new port, twice (idempotency).
    register(&state, &storefront, &[8003]).await;
    register(&state, &storefront, &[8003]).await;

    let routes = state.route_table.read().await;
    let targets = routes.get("shop.example.com").unwrap();
    let admin_count = targets.iter().filter(|t| t.service_name == "admin").count();
    let sf: Vec<&str> = targets
        .iter()
        .filter(|t| t.service_name == "storefront")
        .map(|t| t.address.as_str())
        .collect();
    assert_eq!(admin_count, 1, "sibling must survive re-registration");
    assert_eq!(
        sf,
        vec!["127.0.0.1:8003"],
        "own targets refreshed, not duplicated"
    );
}

/// A service losing all healthy instances drops only its own targets; the
/// domain entry disappears only once NO service holds targets on it.
#[tokio::test]
async fn empty_targets_drop_own_entries_then_domain() {
    let state = test_state();
    let storefront = svc_config("storefront", vec![]);
    let admin = svc_config("admin", vec!["/admin/*"]);
    register(&state, &storefront, &[8001]).await;
    register(&state, &admin, &[8002]).await;

    register(&state, &storefront, &[]).await; // all instances gone
    {
        let routes = state.route_table.read().await;
        let targets = routes.get("shop.example.com").expect("admin still routed");
        assert!(targets.iter().all(|t| t.service_name == "admin"));
    }

    register(&state, &admin, &[]).await;
    assert!(
        !state
            .route_table
            .read()
            .await
            .contains_key("shop.example.com"),
        "domain entry must be removed once no service routes to it"
    );
}
