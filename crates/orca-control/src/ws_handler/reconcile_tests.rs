//! The re-sync an agent gets on (re)connect: #227 (a paused service must not
//! be in it, or every reconnect undoes `orca stop`) and #311 (dependencies
//! first, the agent deploys the list in order).

use std::collections::HashMap;
use std::sync::Arc;

use orca_core::config::{ClusterConfig, ServiceConfig};
use orca_core::testing::MockRuntime;
use orca_core::ws_types::MasterMessage;
use tokio::sync::{RwLock, mpsc};

use super::send_reconcile;
use crate::state::{AppState, RegisteredNode, ServiceState};

const AGENT: u64 = 42;

fn pinned(name: &str) -> ServiceConfig {
    serde_json::from_value(serde_json::json!({
        "name": name,
        "image": "nginx:latest",
        "port": 8080,
        "placement": { "node": "agent-a" },
    }))
    .unwrap()
}

async fn state_with_agent() -> AppState {
    let state = AppState::new(
        ClusterConfig::default(),
        Arc::new(MockRuntime::new()),
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    );
    state.registered_nodes.write().await.insert(
        AGENT,
        RegisteredNode {
            node_id: AGENT,
            address: "agent-a:6881".into(),
            labels: HashMap::new(),
            peer_ip: None,
            last_heartbeat: chrono::Utc::now(),
            drain: false,
            cpu_percent: 0.0,
            memory_bytes: 0,
            memory_total: 0,
            disk_used: 0,
            disk_total: 0,
            net_rx: 0,
            net_tx: 0,
        },
    );
    state
}

#[tokio::test]
async fn paused_services_are_left_out_of_the_resync() {
    let state = state_with_agent().await;
    {
        let mut services = state.services.write().await;
        services.insert("portal".into(), ServiceState::from_config(pinned("portal")));
        let mut paused = ServiceState::from_config(pinned("portal-acme"));
        paused.stopped = true;
        paused.desired_replicas = 0;
        services.insert("portal-acme".into(), paused);
    }
    let (tx, mut rx) = mpsc::channel(4);

    send_reconcile(&state, AGENT, &tx).await;

    match rx.try_recv() {
        Ok(MasterMessage::Reconcile { expected }) => {
            let names: Vec<&str> = expected.iter().map(|s| s.name.as_str()).collect();
            assert_eq!(names, ["portal"], "the paused service must not be expected");
        }
        other => panic!("expected a Reconcile, got {other:?}"),
    }
}

/// Placeholders attach by exact pin resolution, like the re-sync above. A
/// substring match on the address gave an agent at `ubuntu-16gb-fsn1-1` the
/// services of a master pinned as `ubuntu`.
#[tokio::test]
async fn placeholders_do_not_match_a_pin_that_is_a_prefix_of_the_agent() {
    let state = state_with_agent().await;
    state
        .registered_nodes
        .write()
        .await
        .get_mut(&AGENT)
        .unwrap()
        .address = "ubuntu-16gb-fsn1-1:6881".into();
    {
        let mut services = state.services.write().await;
        let mut on_master = pinned("on-master");
        on_master.placement.as_mut().unwrap().node = Some("ubuntu".into());
        let mut on_agent = pinned("on-agent");
        on_agent.placement.as_mut().unwrap().node = Some("ubuntu-16gb-fsn1-1".into());
        services.insert("on-master".into(), ServiceState::from_config(on_master));
        services.insert("on-agent".into(), ServiceState::from_config(on_agent));
    }

    crate::ws_handler::placeholders::upsert_remote_placeholders(&state, AGENT).await;

    let services = state.services.read().await;
    assert!(
        services["on-master"].instances.is_empty(),
        "not the agent's"
    );
    assert_eq!(services["on-agent"].instances.len(), 1);
}

#[tokio::test]
async fn the_resync_lists_dependencies_before_their_dependents() {
    // #311: `app-prod` started 3s before `app-pg-prod` existed and exited
    // on ENOTFOUND. Names chosen so name order alone would be wrong too.
    let state = state_with_agent().await;
    {
        let mut services = state.services.write().await;
        let mut app = pinned("a-app");
        app.depends_on = vec!["z-db".into(), "m-cache".into()];
        let mut db = pinned("z-db");
        db.depends_on = vec!["on-master".into()]; // not on this node
        let mut cache = pinned("m-cache");
        cache.depends_on = vec!["z-db".into()];
        for c in [app, db, cache] {
            services.insert(c.name.clone(), ServiceState::from_config(c));
        }
    }
    let (tx, mut rx) = mpsc::channel(4);

    send_reconcile(&state, AGENT, &tx).await;

    match rx.try_recv() {
        Ok(MasterMessage::Reconcile { expected }) => {
            let names: Vec<&str> = expected.iter().map(|s| s.name.as_str()).collect();
            assert_eq!(names, ["z-db", "m-cache", "a-app"]);
        }
        other => panic!("expected a Reconcile, got {other:?}"),
    }
}
