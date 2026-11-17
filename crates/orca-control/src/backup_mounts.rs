//! The host bind mounts each node's backup must archive, from the services'
//! configs (#312).
//!
//! The backup found bind mounts by listing containers. `orca stop` removes
//! a paused service's containers, so pausing everything for a cold backup
//! silently left every bind mount out of it.

use std::collections::HashMap;

use orca_core::config::ServiceConfig;

use crate::state::AppState;

/// service → its `mounts` entries with a host path, for services that run
/// on `node` (`None` is the master). Paused services are included.
pub(crate) async fn declared_mounts(
    state: &AppState,
    node: Option<u64>,
) -> HashMap<String, Vec<String>> {
    let nodes = state.registered_nodes.read().await.clone();
    let services = state.services.read().await;
    services
        .values()
        .map(|svc| &svc.config)
        .filter(|c| match node {
            None => !crate::placement::placed_on_agent(c),
            Some(id) => pinned_to(c).is_some_and(|pin| {
                nodes
                    .get(&id)
                    .is_some_and(|n| crate::placement::node_matches_pin(n, pin))
            }),
        })
        .filter_map(|c| {
            let binds: Vec<String> = c
                .mounts
                .iter()
                .filter(|m| m.starts_with('/'))
                .cloned()
                .collect();
            (!binds.is_empty()).then(|| (c.name.clone(), binds))
        })
        .collect()
}

fn pinned_to(c: &ServiceConfig) -> Option<&str> {
    c.placement.as_ref()?.node.as_deref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use orca_core::config::ClusterConfig;
    use orca_core::testing::MockRuntime;
    use tokio::sync::RwLock;

    use crate::state::{RegisteredNode, ServiceState};

    fn svc(name: &str, pin: Option<&str>, mounts: &[&str]) -> ServiceConfig {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "image": "nginx:latest",
            "mounts": mounts,
            "placement": pin.map(|p| serde_json::json!({ "node": p })),
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn each_node_gets_the_bind_mounts_of_its_services_paused_or_not() {
        let state = AppState::new(
            ClusterConfig::default(),
            Arc::new(MockRuntime::new()),
            None,
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(Vec::new())),
        );
        state.registered_nodes.write().await.insert(
            7,
            RegisteredNode {
                node_id: 7,
                address: "prod1:6881".into(),
                labels: HashMap::new(),
                peer_ip: None,
                last_heartbeat: chrono::Utc::now(),
                drain: true, // drained before a migration: still backed up
                cpu_percent: 0.0,
                memory_bytes: 0,
                memory_total: 0,
                disk_used: 0,
                disk_total: 0,
                net_rx: 0,
                net_tx: 0,
            },
        );
        {
            let mut services = state.services.write().await;
            let mut paused =
                ServiceState::from_config(svc("blog", None, &["/srv/blog:/data", "cache:/c"]));
            paused.stopped = true;
            services.insert("blog".into(), paused);
            services.insert(
                "app".into(),
                ServiceState::from_config(svc("app", Some("prod1"), &["/srv/app:/app:ro"])),
            );
            services.insert(
                "plain".into(),
                ServiceState::from_config(svc("plain", None, &[])),
            );
        }

        let master = declared_mounts(&state, None).await;
        let agent = declared_mounts(&state, Some(7)).await;

        assert_eq!(
            master,
            HashMap::from([("blog".into(), vec!["/srv/blog:/data".into()])])
        );
        assert_eq!(
            agent,
            HashMap::from([("app".into(), vec!["/srv/app:/app:ro".into()])])
        );
    }
}
