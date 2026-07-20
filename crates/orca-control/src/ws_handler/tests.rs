//! Unit tests for the agent WS handler.

use super::*;

/// #120: a discovered domain must never mutate a DECLARED service's
/// domain set — that diverges `spec_matches` from the on-disk config
/// and makes the declarative loop redeploy it every pass.
#[tokio::test]
async fn domain_discovery_never_mutates_declared_services() {
    let state = crate::state::AppState::new(
        orca_core::config::ClusterConfig::default(),
        std::sync::Arc::new(orca_core::testing::MockRuntime::new()),
        None,
        std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
    );
    let declared: orca_core::config::ServiceConfig = serde_json::from_value(serde_json::json!({
        "name": "web", "image": "nginx", "replicas": 1, "port": 80,
        "domain": "declared.example.com",
    }))
    .unwrap();
    let undeclared: orca_core::config::ServiceConfig = serde_json::from_value(
        serde_json::json!({ "name": "adopted", "image": "nginx", "replicas": 1, "port": 80 }),
    )
    .unwrap();
    {
        let mut services = state.services.write().await;
        services.insert(
            "web".into(),
            crate::state::ServiceState::from_config(declared.clone()),
        );
        services.insert(
            "adopted".into(),
            crate::state::ServiceState::from_config(undeclared.clone()),
        );
    }
    let (tx, _rx) = mpsc::channel(4);

    let msg = serde_json::to_string(&orca_core::ws_types::AgentMessage::DomainDiscovered {
        service_name: "web".into(),
        domain: "other.example.com".into(),
        host_port: 8080,
    })
    .unwrap();
    handle_agent_message(&state, 1, &msg, &tx).await.unwrap();
    let services = state.services.read().await;
    assert!(
        services.get("web").unwrap().config.spec_matches(&declared),
        "declared service's config must stay byte-faithful to disk"
    );
    drop(services);

    let msg = serde_json::to_string(&orca_core::ws_types::AgentMessage::DomainDiscovered {
        service_name: "adopted".into(),
        domain: "found.example.com".into(),
        host_port: 8080,
    })
    .unwrap();
    handle_agent_message(&state, 1, &msg, &tx).await.unwrap();
    let services = state.services.read().await;
    assert_eq!(
        services.get("adopted").unwrap().config.all_domains(),
        vec!["found.example.com".to_string()],
        "undeclared services still receive discovered domains"
    );
}

#[test]
fn ws_query_deserializes() {
    let q: WsQuery = serde_json::from_str(r#"{"token":"abc123","node_id":42}"#).unwrap();
    assert_eq!(q.token, "abc123");
    assert_eq!(q.node_id, 42);
}
