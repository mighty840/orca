//! #177: edits that leave the container as-is (placement, probes, replicas)
//! must still be applied and persisted by the declarative loop.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use orca_control::declarative::apply_config_dir;
use orca_control::state::AppState;
use orca_control::store::ClusterStore;
use orca_core::config::ClusterConfig;
use orca_core::testing::{MockOpKind, MockRuntime};

fn state_with_store(db_path: &std::path::Path, runtime: Arc<MockRuntime>) -> Arc<AppState> {
    Arc::new(
        AppState::new(
            ClusterConfig::default(),
            runtime,
            None,
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(Vec::new())),
        )
        .with_store(Arc::new(ClusterStore::open(db_path).unwrap())),
    )
}

fn write_service(dir: &std::path::Path, body: &str) {
    let sub = dir.join("relay");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("service.toml"), body).unwrap();
}

const RELAY: &str = r#"
    [[service]]
    name = "relay"
    image = "relay:latest"
    port = 8080
"#;

/// The production case: a pin to the master itself was removed from
/// `service.toml`, but the pinned config stayed in the store, so every master
/// restart restored the old placement.
#[tokio::test]
async fn removing_a_pin_is_persisted_without_recreating_the_container() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = Arc::new(MockRuntime::with_host_port(9000));
    let state = state_with_store(&tmp.path().join("c.db"), runtime.clone());
    let dir = tmp.path().join("services");
    write_service(
        &dir,
        &format!("{RELAY}\n    [service.placement]\n    node = \"localhost\"\n"),
    );
    apply_config_dir(&state, dir.to_str().unwrap()).await;
    let store = state.store.as_ref().unwrap();
    let stored = store.get_service("relay").unwrap().unwrap();
    assert!(stored.placement.is_some(), "setup: pinned config persisted");

    write_service(&dir, RELAY);
    apply_config_dir(&state, dir.to_str().unwrap()).await;

    let stored = store.get_service("relay").unwrap().unwrap();
    assert!(
        stored.placement.is_none(),
        "the unpinned config must replace the stored one"
    );
    assert!(
        state.services.read().await["relay"]
            .config
            .placement
            .is_none()
    );
    assert_eq!(
        runtime.count(MockOpKind::Create).await,
        1,
        "same node, same spec: the container stays"
    );
}

#[tokio::test]
async fn a_probe_edit_is_applied_without_recreating_the_container() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = Arc::new(MockRuntime::with_host_port(9000));
    let state = state_with_store(&tmp.path().join("c.db"), runtime.clone());
    let dir = tmp.path().join("services");
    write_service(&dir, RELAY);
    apply_config_dir(&state, dir.to_str().unwrap()).await;

    write_service(
        &dir,
        &format!("{RELAY}\n    [service.liveness]\n    path = \"/gesund\"\n"),
    );
    apply_config_dir(&state, dir.to_str().unwrap()).await;

    let live = state.services.read().await["relay"].config.liveness.clone();
    assert_eq!(live.map(|p| p.path).as_deref(), Some("/gesund"));
    let stored = state.store.as_ref().unwrap().get_service("relay").unwrap();
    assert!(stored.unwrap().liveness.is_some(), "persisted");
    assert_eq!(runtime.count(MockOpKind::Create).await, 1);
}

#[tokio::test]
async fn a_manual_scale_survives_but_a_replicas_edit_is_applied() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = Arc::new(MockRuntime::with_host_port(9000));
    let state = state_with_store(&tmp.path().join("c.db"), runtime.clone());
    let dir = tmp.path().join("services");
    write_service(&dir, RELAY);
    apply_config_dir(&state, dir.to_str().unwrap()).await;

    orca_control::reconciler::scale(&state, "relay", 2)
        .await
        .unwrap();
    apply_config_dir(&state, dir.to_str().unwrap()).await;
    assert_eq!(
        state.services.read().await["relay"].desired_replicas,
        2,
        "an unchanged file must not revert a manual scale"
    );

    write_service(&dir, &format!("{RELAY}    replicas = 3\n"));
    apply_config_dir(&state, dir.to_str().unwrap()).await;
    assert_eq!(state.services.read().await["relay"].desired_replicas, 3);
}

/// #176: pruning a service pinned to the master itself must stop its local
/// container. It used to broadcast Stop to the agents (none of which host
/// it) and forget the service, leaving the container running unmanaged
/// with its ports bound.
#[tokio::test]
async fn pruning_a_master_self_pinned_service_stops_its_local_container() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = Arc::new(MockRuntime::with_host_port(9000));
    let state = state_with_store(&tmp.path().join("c.db"), runtime.clone());
    let dir = tmp.path().join("services");
    write_service(
        &dir,
        &format!("{RELAY}\n    [service.placement]\n    node = \"localhost\"\n"),
    );
    let other = dir.join("keep");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(
        other.join("service.toml"),
        "[[service]]\nname = \"keep\"\nimage = \"nginx:latest\"\nport = 8081\n",
    )
    .unwrap();
    apply_config_dir(&state, dir.to_str().unwrap()).await;
    assert_eq!(runtime.count(MockOpKind::Create).await, 2);

    std::fs::remove_dir_all(dir.join("relay")).unwrap();
    apply_config_dir(&state, dir.to_str().unwrap()).await;

    assert!(!state.services.read().await.contains_key("relay"));
    assert_eq!(
        runtime.count(MockOpKind::Stop).await,
        1,
        "the local container must be stopped"
    );
    assert_eq!(runtime.count(MockOpKind::Remove).await, 1);
}
